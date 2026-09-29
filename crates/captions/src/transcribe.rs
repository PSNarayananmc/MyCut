//! Transcriber abstraction. Default: local whisper.cpp (optional download,
//! never bundled). Word-level timestamps required for karaoke captions.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::TimedWord;

#[derive(Debug, Error)]
pub enum TranscribeError {
    #[error("transcriber not configured. Install whisper.cpp (Settings > Transcription) to enable captions.")]
    NotConfigured,
    #[error("transcriber failed: {0}")]
    Failed(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Modular transcriber trait (additional providers are additive).
pub trait Transcriber: Send + Sync {
    /// Transcribe `media` (source seconds); returns word timings.
    ///
    /// # Errors
    /// [`TranscribeError`] surfaced to the UI verbatim.
    fn transcribe(&self, media: &std::path::Path, cancel: &std::sync::atomic::AtomicBool) -> Result<Vec<TimedWord>, TranscribeError>;
    fn name(&self) -> &'static str;
}

/// whisper.cpp `main` binary integration: spawns the user-provided binary
/// (argument arrays, no shell) with full JSON output.
pub struct WhisperCppTranscriber {
    /// Path to the whisper.cpp `main` (or `whisper-cli`) binary.
    pub binary: std::path::PathBuf,
    /// Path to a ggml model file the user downloaded (shown size+license).
    pub model: std::path::PathBuf,
    pub language: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WhisperFullJson {
    #[serde(default)]
    transcription: Vec<WhisperSegment>,
}

#[derive(Debug, Deserialize)]
struct WhisperSegment {
    #[serde(default)]
    tokens: Vec<WhisperToken>,
}

#[derive(Debug, Deserialize)]
struct WhisperToken {
    text: String,
    #[serde(default)]
    offsets: WhisperOffsets,
}

#[derive(Debug, Deserialize, Default)]
struct WhisperOffsets {
    from: Option<i64>,
    to: Option<i64>,
}

impl Transcriber for WhisperCppTranscriber {
    fn transcribe(&self, media: &std::path::Path, cancel: &std::sync::atomic::AtomicBool) -> Result<Vec<TimedWord>, TranscribeError> {
        if !self.binary.exists() || !self.model.exists() {
            return Err(TranscribeError::NotConfigured);
        }
        // whisper.cpp needs 16 kHz WAV; produce a temp wav first (throwaway).
        let tmp = tempfile::tempdir()?;
        let wav = tmp.path().join("in.wav");
        let ffmpeg = which_ffmpeg().ok_or_else(|| TranscribeError::Failed("ffmpeg not found".into()))?;
        let mut cmd = std::process::Command::new(ffmpeg);
        cmd.args(["-hide_banner", "-y", "-i"]).arg(media)
            .args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"])
            .arg(&wav)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let st = cmd.status()?;
        if !st.success() {
            return Err(TranscribeError::Failed("audio extraction failed".into()));
        }
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(TranscribeError::Failed("cancelled".into()));
        }
        let out_json = tmp.path().join("out.json");
        let mut args: Vec<std::ffi::OsString> = vec![
            "-m".into(), self.model.clone().into(),
            "-f".into(), wav.clone().into_os_string(),
            "--output-json-full".into(),
            "--print-progress".into(), "false".into(),
        ];
        if let Some(lang) = &self.language {
            args.push("--language".into());
            args.push(lang.clone().into());
        }
        args.push(out_json.clone().into_os_string());
        let st = std::process::Command::new(&self.binary)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .status()?;
        if !st.success() {
            return Err(TranscribeError::Failed("whisper.cpp exited with an error".into()));
        }
        let bytes = std::fs::read(&out_json)?;
        let parsed: WhisperFullJson = serde_json::from_slice(&bytes)
            .map_err(|e| TranscribeError::Failed(format!("whisper JSON: {e}")))?;
        Ok(whisper_json_to_words(&parsed))
    }

    fn name(&self) -> &'static str {
        "whisper.cpp"
    }
}

fn which_ffmpeg() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("MYCUT_FFMPEG") {
        let pb = std::path::PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("ffmpeg")).find(|c| c.is_file())
}

/// Pure conversion: whisper.cpp full JSON -> TimedWords (drop special tokens).
#[must_use]
pub fn whisper_json_to_words(parsed: &WhisperFullJson) -> Vec<TimedWord> {
    let mut words = Vec::new();
    for seg in &parsed.transcription {
        for tok in &seg.tokens {
            let text = decode_whisper_text(&tok.text);
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            if tok.text.starts_with("[_") || tok.text.starts_with("<|") {
                continue; // special tokens
            }
            words.push(TimedWord {
                text,
                start_ms: tok.offsets.from.unwrap_or(0),
                end_ms: tok.offsets.to.unwrap_or(0),
            });
        }
    }
    words
}

/// whisper.cpp escapes special bytes as \xNN; keep printable content.
fn decode_whisper_text(t: &str) -> String {
    t.replace("\\x", "")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriberConfig {
    pub binary_path: Option<String>,
    pub model_path: Option<String>,
    pub language: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whisper_fixture_words() {
        let fixture = r#"{
          "transcription": [
            { "tokens": [
                { "text": "[_START_]", "offsets": { "from": 0, "to": 0 } },
                { "text": " Hello", "offsets": { "from": 0, "to": 420 } },
                { "text": " gamers", "offsets": { "from": 420, "to": 900 } },
                { "text": "!", "offsets": { "from": 900, "to": 950 } }
            ] }
          ]
        }"#;
        let parsed: WhisperFullJson = serde_json::from_str(fixture).unwrap();
        let words = whisper_json_to_words(&parsed);
        assert_eq!(words.len(), 3, "special tokens dropped");
        assert_eq!(words[0].text, "Hello");
        assert_eq!(words[0].start_ms, 0);
        assert_eq!(words[2].end_ms, 950);
    }

    #[test]
    fn not_configured_is_explicit() {
        let t = WhisperCppTranscriber {
            binary: "/nonexistent/whisper".into(),
            model: "/nonexistent/model.bin".into(),
            language: None,
        };
        let r = t.transcribe(std::path::Path::new("/dev/null"), &std::sync::atomic::AtomicBool::new(false));
        assert!(matches!(r, Err(TranscribeError::NotConfigured)));
    }
}
