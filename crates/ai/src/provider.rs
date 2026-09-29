//! AI provider abstraction + NVIDIA NIM implementation (OpenAI-compatible
//! chat completions). Blocking HTTP on worker threads; retries with
//! exponential backoff + jitter honoring Retry-After; cancellable.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// What a provider/model can do (reported, never guessed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    pub text: bool,
    pub vision: bool,
    pub structured_output: bool,
    pub max_context: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// Optional downscaled frame (data URL) — only when vision + consent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}

/// Connection states, each with a distinct human-readable message.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum AiError {
    #[error("Connected.")]
    Connected,
    #[error("Invalid API key. Check the key in Settings and try again.")]
    InvalidApiKey,
    #[error("Network error: could not reach the AI provider. Check your connection.")]
    Network,
    #[error("Rate limited by the provider. Try again in a moment.")]
    RateLimited,
    #[error("Model unavailable. Pick another model in Settings.")]
    ModelUnavailable,
    #[error("This model lacks a required capability (structured JSON output).")]
    MissingCapability,
    #[error("The request timed out.")]
    Timeout,
    #[error("The model returned invalid output after repairs: {0}")]
    BadOutput(String),
    #[error("Provider error: {0}")]
    Provider(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRequest {
    pub system_prompt: String,
    pub messages: Vec<ChatMessage>,
    /// JSON Schema for constrained decoding when supported.
    pub json_schema: Option<serde_json::Value>,
    pub max_tokens: u32,
    pub temperature: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanResponse {
    /// The raw assistant text (expected to be JSON).
    pub text: String,
    pub model: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// The provider seam. One implementation ships (NIM); others are one file.
pub trait AIProvider: Send + Sync {
    fn capabilities(&self) -> Caps;
    /// Test the connection; returns a human-readable state.
    fn test_connection(&self) -> Result<(), AiError>;
    /// Send a plan request (JSON expected back).
    ///
    /// # Errors
    /// [`AiError`] states surfaced verbatim to the UI.
    fn complete(&self, req: &PlanRequest, cancel: &std::sync::atomic::AtomicBool) -> Result<PlanResponse, AiError>;
}

#[derive(Debug, Clone)]
pub struct NimConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub max_retries: u32,
    pub backoff_base_ms: u64,
    pub timeout_secs: u64,
}

impl Default for NimConfig {
    fn default() -> Self {
        // Defaults live in config data (docs + settings UI), not logic —
        // this struct carries user-editable values.
        Self {
            api_key: String::new(),
            base_url: "https://integrate.api.nvidia.com/v1".into(),
            model: "meta/llama-3.1-8b-instruct".into(),
            max_retries: 3,
            backoff_base_ms: 500,
            timeout_secs: 60,
        }
    }
}

/// NVIDIA NIM provider: OpenAI-compatible `/chat/completions`.
pub struct NvidiaNimProvider {
    cfg: NimConfig,
    caps: Caps,
}

impl NvidiaNimProvider {
    #[must_use]
    pub fn new(cfg: NimConfig) -> Self {
        Self {
            cfg,
            // Conservative default caps; `refresh_capabilities` can upgrade
            // after a probe. Structured output support is requested and
            // gracefully degraded (validation+repair) when absent.
            caps: Caps { text: true, vision: false, structured_output: true, max_context: 8192 },
        }
    }

    #[must_use]
    pub fn with_caps(mut self, caps: Caps) -> Self {
        self.caps = caps;
        self
    }

    fn request_once(&self, req: &PlanRequest) -> Result<PlanResponse, (AiError, Option<u64>)> {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(self.cfg.timeout_secs))
            .build();
        let url = format!("{}/chat/completions", self.cfg.base_url.trim_end_matches('/'));
        let mut body = serde_json::json!({
            "model": self.cfg.model,
            "messages": req.messages.iter().map(|m| {
                let mut o = serde_json::json!({"role": m.role, "content": m.content});
                o
            }).collect::<Vec<_>>(),
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
        });
        if self.caps.structured_output {
            if let Some(schema) = &req.json_schema {
                body["response_format"] = serde_json::json!({
                    "type": "json_schema",
                    "json_schema": { "name": "edit_plan", "strict": true, "schema": schema }
                });
            }
        }
        let resp = agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.cfg.api_key))
            .set("Accept", "application/json")
            .send_json(body);
        match resp {
            Ok(r) => {
                let parsed: serde_json::Value = r.into_json().map_err(|e| (AiError::Provider(e.to_string()), None))?;
                let text = parsed["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let model = parsed["model"].as_str().unwrap_or("").to_string();
                Ok(PlanResponse {
                    text,
                    model,
                    prompt_tokens: parsed["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32,
                    completion_tokens: parsed["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32,
                })
            }
            Err(ureq::Error::Status(code, r)) => {
                let retry_after = r
                    .header("Retry-After")
                    .and_then(|v| v.parse::<u64>().ok());
                let err = match code {
                    401 | 403 => AiError::InvalidApiKey,
                    404 => AiError::ModelUnavailable,
                    429 => AiError::RateLimited,
                    500..=599 => AiError::Provider(format!("server error {code}")),
                    _ => AiError::Provider(format!("http {code}")),
                };
                Err((err, retry_after))
            }
            Err(ureq::Error::Transport(t)) => {
                let msg = t.to_string();
                if msg.contains("timed out") || msg.contains("timeout") {
                    Err((AiError::Timeout, None))
                } else {
                    Err((AiError::Network, None))
                }
            }
        }
    }
}

impl AIProvider for NvidiaNimProvider {
    fn capabilities(&self) -> Caps {
        self.caps
    }

    fn test_connection(&self) -> Result<(), AiError> {
        if self.cfg.api_key.trim().is_empty() {
            return Err(AiError::InvalidApiKey);
        }
        let req = PlanRequest {
            system_prompt: "You are a health check. Reply with the single word: ok".into(),
            messages: vec![ChatMessage { role: "user".into(), content: "ping".into(), image_url: None }],
            json_schema: None,
            max_tokens: 5,
            temperature: 0.0,
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        self.complete(&req, &cancel).map(|_| ())
    }

    /// # Errors
    /// [`AiError`] after retries are exhausted.
    fn complete(&self, req: &PlanRequest, cancel: &std::sync::atomic::AtomicBool) -> Result<PlanResponse, AiError> {
        let mut attempt = 0u32;
        loop {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(AiError::Provider("cancelled".into()));
            }
            match self.request_once(req) {
                Ok(r) => return Ok(r),
                Err((AiError::RateLimited, retry_after)) | Err((AiError::Provider(_), retry_after @ Some(_))) => {
                    if attempt >= self.cfg.max_retries {
                        return Err(AiError::RateLimited);
                    }
                    let jitter = (std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.subsec_millis())
                        .unwrap_or(0)) as u64
                        % 100;
                    let backoff = retry_after
                        .map(|s| Duration::from_secs(s))
                        .unwrap_or_else(|| Duration::from_millis(self.cfg.backoff_base_ms * (1u64 << attempt) + jitter));
                    std::thread::sleep(backoff.min(Duration::from_secs(30)));
                    attempt += 1;
                }
                Err((AiError::Provider(_), None)) => {
                    if attempt >= self.cfg.max_retries {
                        return Err(AiError::Provider("server error".into()));
                    }
                    let backoff = self.cfg.backoff_base_ms * (1u64 << attempt);
                    std::thread::sleep(Duration::from_millis(backoff.min(8_000)));
                    attempt += 1;
                }
                Err((other, _)) => return Err(other),
            }
        }
    }
}
