//! ffprobe runner and JSON parsing (media metadata).

use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use crate::error::EngineError;

#[derive(Debug, Clone, Deserialize)]
pub struct ProbeFormat {
    pub duration: Option<String>,
    #[serde(default)]
    pub format_name: String,
    pub size: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProbeStream {
    pub codec_type: Option<String>,
    pub codec_name: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub r_frame_rate: Option<String>,
    pub avg_frame_rate: Option<String>,
    pub duration: Option<String>,
    pub nb_frames: Option<String>,
    pub sample_rate: Option<String>,
    pub channels: Option<u32>,
    pub bit_rate: Option<String>,
    pub pix_fmt: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProbeResult {
    pub streams: Vec<ProbeStream>,
    pub format: ProbeFormat,
}

impl ProbeResult {
    #[must_use]
    pub fn duration_ms(&self) -> i64 {
        self.format
            .duration
            .as_deref()
            .and_then(|d| d.parse::<f64>().ok())
            .map(|s| (s * 1000.0).round() as i64)
            .or_else(|| {
                self.streams
                    .iter()
                    .filter_map(|s| s.duration.as_deref())
                    .filter_map(|d| d.parse::<f64>().ok())
                    .fold(None::<f64>, |acc, v| Some(acc.map_or(v, |a: f64| a.max(v))))
                    .map(|s| (s * 1000.0).round() as i64)
            })
            .unwrap_or(0)
    }

    #[must_use]
    pub fn fps(&self) -> (u32, u32) {
        self.video_stream()
            .and_then(|v| {
                parse_rational(v.avg_frame_rate.as_deref().or(v.r_frame_rate.as_deref()))
            })
            .unwrap_or((30, 1))
    }

    #[must_use]
    pub fn video_stream(&self) -> Option<&ProbeStream> {
        self.streams.iter().find(|s| s.codec_type.as_deref() == Some("video"))
    }

    #[must_use]
    pub fn audio_stream(&self) -> Option<&ProbeStream> {
        self.streams.iter().find(|s| s.codec_type.as_deref() == Some("audio"))
    }

    #[must_use]
    pub fn has_audio(&self) -> bool {
        self.audio_stream().is_some()
    }

    #[must_use]
    pub fn resolution(&self) -> (u32, u32) {
        self.video_stream().map(|v| (v.width.unwrap_or(0), v.height.unwrap_or(0))).unwrap_or((0, 0))
    }
}

/// Parse "30000/1001" style rationals.
#[must_use]
pub fn parse_rational(s: Option<&str>) -> Option<(u32, u32)> {
    s.map(str::trim).filter(|v| !v.is_empty() && *v != "0/0").and_then(|v| {
        let mut parts = v.splitn(2, '/');
        let num: u32 = parts.next()?.trim().parse().ok()?;
        let den: u32 = parts.next().unwrap_or("1").trim().parse().ok()?;
        (den > 0).then_some((num, den))
    })
}

/// Run ffprobe with a strict argument array and parse JSON output.
///
/// # Errors
/// [`EngineError`] on spawn failure, non-zero exit, or JSON parse failure.
pub fn probe(media: &Path) -> Result<ProbeResult, EngineError> {
    let ffprobe = crate::process::resolve_tool("ffprobe")?;
    let out = Command::new(ffprobe)
        .args([
            "-v", "error",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(media)
        .output()
        .map_err(EngineError::Spawn)?;
    if !out.status.success() {
        return Err(EngineError::Tool(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let parsed: ProbeResult = serde_json::from_slice(&out.stdout)?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rationals() {
        assert_eq!(parse_rational(Some("30000/1001")), Some((30000, 1001)));
        assert_eq!(parse_rational(Some("30/1")), Some((30, 1)));
        assert_eq!(parse_rational(Some("0/0")), None);
        assert_eq!(parse_rational(Some("")), None);
        assert_eq!(parse_rational(None), None);
    }
}
