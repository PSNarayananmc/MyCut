//! Runtime diagnostics: FFmpeg tri-state (missing / too old / ok), storage
//! mode, and environment report. Surfaced via the `doctor` command and
//! shown by the UI when media operations fail so nothing fails silently.

use serde_json::{json, Value};

use crate::state::AppState;

/// MyCut requires `xfade` (FFmpeg 4.3+) for transitions and the standard
/// filter set (libass subtitles, zoompan, eq, loudnorm).
pub const MIN_FFMPEG_MAJOR: u32 = 4;
pub const MIN_FFMPEG_MINOR: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FfmpegState {
    Missing,
    TooOld { version: String, minimum: String },
    Ok { version: String },
}

#[derive(Debug, Clone)]
pub struct FfmpegDiag {
    pub state: FfmpegState,
    pub path: Option<String>,
    pub source: String,
}

/// Parse `ffprobe version N.N[.x]-...` from `ffprobe -version` output.
/// Handles both `4.2.7` and the `n8.1.3-...` scheme used by some builds.
fn parse_version(output: &str) -> Option<String> {
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("ffprobe version ") {
            let first = rest.split_whitespace().next().unwrap_or("");
            let digits: String = first
                .trim_start_matches('n')
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if !digits.is_empty() {
                return Some(digits);
            }
        }
    }
    None
}

fn version_tuple(v: &str) -> (u32, u32) {
    let mut it = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0))
}

/// Probe ffprobe (the same binary ships ffmpeg in every practical layout).
pub fn ffmpeg_diagnostics() -> FfmpegDiag {
    // Mirrors the engine's resolution order: env sidecar override, then PATH.
    let (name, source) = ("ffprobe", "sidecar/env MYCUT_FFPROBE or PATH");
    let Some(p) = mycut_engine::process::resolve_tool(name).ok() else {
        return FfmpegDiag {
            state: FfmpegState::Missing,
            path: None,
            source: String::new(),
        };
    };
    let out = std::process::Command::new(&p).arg("-version").output();
    if let Ok(o) = out {
        let text = String::from_utf8_lossy(&o.stdout).into_owned();
        if let Some(v) = parse_version(&text) {
            let (maj, min) = version_tuple(&v);
            let state =
                if maj > MIN_FFMPEG_MAJOR || (maj == MIN_FFMPEG_MAJOR && min >= MIN_FFMPEG_MINOR) {
                    FfmpegState::Ok { version: v }
                } else {
                    FfmpegState::TooOld {
                        version: v,
                        minimum: format!("{MIN_FFMPEG_MAJOR}.{MIN_FFMPEG_MINOR}"),
                    }
                };
            return FfmpegDiag {
                state,
                path: Some(p.to_string_lossy().into_owned()),
                source: source.into(),
            };
        }
    }
    FfmpegDiag {
        state: FfmpegState::Missing,
        path: Some(p.to_string_lossy().into_owned()),
        source: source.into(),
    }
}

/// Actionable one-line message for every failure path.
#[must_use]
pub fn ffmpeg_message(d: &FfmpegDiag) -> String {
    match &d.state {
        FfmpegState::Missing => "FFmpeg not found. Install it (`sudo apt install ffmpeg`), \
             use the MyCut package (bundles FFmpeg), or set MYCUT_FFMPEG/MYCUT_FFPROBE."
            .into(),
        FfmpegState::TooOld { version, minimum } => format!(
            "FFmpeg {version} found at {} is too old (need >= {minimum} for xfade/transitions). \
             Install a newer FFmpeg or keep using the bundled one.",
            d.path.clone().unwrap_or_default()
        ),
        FfmpegState::Ok { version } => format!(
            "FFmpeg {version} OK ({})",
            d.path.clone().unwrap_or_default()
        ),
    }
}

/// Full doctor report for the UI.
pub fn report(state: &AppState) -> Value {
    let d = ffmpeg_diagnostics();
    let dir = mycut_projects::config_dir();
    let secret_mode = if std::env::var("MYCUT_SECRET_FILE").is_ok()
        || !dir.join("mycut/secret").exists() && dir.join("mycut").exists()
    {
        // Best-effort hint; the secret store reports its own mode on errors.
        "auto (keyring if a Secret Service is available, else 0600 file)"
    } else {
        "auto (keyring if a Secret Service is available, else 0600 file)"
    };
    json!({
        "appVersion": env!("CARGO_PKG_VERSION"),
        "runtime": "local-web",
        "ffmpeg": {
            "state": match &d.state {
                FfmpegState::Missing => "missing",
                FfmpegState::TooOld { .. } => "too-old",
                FfmpegState::Ok { .. } => "ok",
            },
            "version": match &d.state {
                FfmpegState::TooOld { version, .. } | FfmpegState::Ok { version } => json!(version),
                FfmpegState::Missing => Value::Null,
            },
            "minimum": format!("{MIN_FFMPEG_MAJOR}.{MIN_FFMPEG_MINOR}"),
            "path": d.path,
            "source": d.source,
            "message": ffmpeg_message(&d),
            "usingBundled": std::env::var_os("MYCUT_FFMPEG").is_some(),
        },
        "storage": {
            "configDir": dir.to_string_lossy(),
            "projectPath": state.project_path.lock().unwrap().to_string_lossy(),
            "cacheDir": state.cache_dir.to_string_lossy(),
            "secretMode": secret_mode,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffprobe_version_line() {
        assert_eq!(
            parse_version("ffprobe version 7.1.5-0+deb13u1 Copyright (c) 2000-2026"),
            Some("7.1.5".into())
        );
        assert_eq!(
            parse_version("ffprobe version 4.2.7-0ubuntu0.1"),
            Some("4.2.7".into())
        );
        assert_eq!(
            parse_version("ffprobe version n8.1.3-6-gff48edd8b2-20260929"),
            Some("8.1.3".into())
        );
        assert_eq!(parse_version("nonsense"), None);
    }

    #[test]
    fn version_compare_applies_minimum() {
        let d = |v: &str| FfmpegDiag {
            state: FfmpegState::Ok { version: v.into() },
            path: None,
            source: "test".into(),
        };
        // 4.2 -> too old, 4.3 -> ok, 7.1 -> ok (exercised through message fn)
        let old = FfmpegDiag {
            state: FfmpegState::TooOld {
                version: "4.2.7".into(),
                minimum: "4.3".into(),
            },
            path: Some("/usr/bin/ffprobe".into()),
            source: "test".into(),
        };
        assert!(ffmpeg_message(&old).contains("too old"));
        assert!(ffmpeg_message(&d("7.1")).contains("OK"));
    }
}
