//! App settings persisted as JSON under the user config dir.
//! Defaults live here as data (documented), not scattered logic.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// On-disk settings (snake_case JSON for stability across versions).
/// The Tauri command `get_settings` returns a separate DTO (`SettingsDto`)
/// that adds runtime-only fields like `key_stored` / `key_backend` and
/// uses camelCase field names matching the TypeScript `AppSettingsInfo`
/// interface in `ui/src/lib/bridge.ts`. The two MUST stay in sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct AppSettings {
    pub base_url: String,
    pub model: String,
    pub frames_enabled: bool,
    pub profile: String,
    pub cache_limit_bytes: u64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            base_url: "https://integrate.api.nvidia.com/v1".into(),
            model: "meta/llama-3.1-8b-instruct".into(),
            frames_enabled: false,
            // Low profile is the default for the 8 GB iGPU target machine.
            profile: "low".into(),
            cache_limit_bytes: 10 * 1024 * 1024 * 1024 / 2, // 5 GB
        }
    }
}

/// Runtime DTO shipped to the webview. Adds the key-state fields the UI needs
/// to render "✓ Key stored (OS keyring)" vs "No key stored" without ever
/// exposing the key itself.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    pub base_url: String,
    pub model: String,
    pub frames_enabled: bool,
    pub profile: String,
    pub cache_limit_bytes: u64,
    pub key_stored: bool,
    pub key_backend: String,
}

impl SettingsDto {
    /// Build the DTO from on-disk settings + a live probe of the secret store.
    #[must_use]
    pub fn from_settings(s: &AppSettings, key_stored: bool, key_backend: &str) -> Self {
        Self {
            base_url: s.base_url.clone(),
            model: s.model.clone(),
            frames_enabled: s.frames_enabled,
            profile: s.profile.clone(),
            cache_limit_bytes: s.cache_limit_bytes,
            key_stored,
            key_backend: key_backend.to_string(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".config")).unwrap_or_else(|_| PathBuf::from(".")))
}

impl AppSettings {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("mycut/settings.json");
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let dir = config_dir().join("mycut");
        std::fs::create_dir_all(&dir)?;
        let bytes = serde_json::to_vec_pretty(self)?;
        std::fs::write(dir.join("settings.json"), bytes)
    }

    /// Apply one key/value from the settings UI.
    pub fn apply(&mut self, key: &str, value: &str) {
        match key {
            "base_url" | "baseUrl" => self.base_url = value.to_string(),
            "model" => self.model = value.to_string(),
            "profile" => self.profile = value.to_string(),
            "frames_enabled" | "framesEnabled" => {
                self.frames_enabled = matches!(value.to_lowercase().as_str(), "true" | "1" | "yes" | "on");
            }
            "cache_limit_bytes" | "cacheLimitBytes" => {
                if let Ok(v) = value.parse() {
                    self.cache_limit_bytes = v;
                }
            }
            _ => {}
        }
    }
}
