//! Child-process execution: argument arrays ONLY, no shell, cancellable,
//! bounded stderr capture, and tool resolution (PATH + optional bundled).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::EngineError;

/// Optional override for the ffmpeg binary (bundled AppImage sets this).
pub const ENV_FFMPEG: &str = "MYCUT_FFMPEG";
pub const ENV_FFPROBE: &str = "MYCUT_FFPROBE";

/// Resolve a tool binary: env override first, then PATH.
///
/// # Errors
/// [`EngineError::ToolNotFound`] when unavailable.
pub fn resolve_tool(name: &str) -> Result<PathBuf, EngineError> {
    let override_path = if name == "ffmpeg" {
        std::env::var_os(ENV_FFMPEG)
    } else if name == "ffprobe" {
        std::env::var_os(ENV_FFPROBE)
    } else {
        None
    };
    if let Some(p) = override_path {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Ok(pb);
        }
    }
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Ok(cand);
        }
    }
    Err(EngineError::ToolNotFound(name.to_string()))
}

/// Shared cancellation flag for long jobs.
pub type CancelFlag = Arc<AtomicBool>;

pub fn new_cancel() -> CancelFlag {
    Arc::new(AtomicBool::new(false))
}

const MAX_STDERR_BYTES: usize = 256 * 1024;

/// A running child whose stderr is collected (bounded) and which can be
/// cancelled through a shared flag.
pub struct RunningJob {
    child: Child,
    stderr_handle: Option<std::thread::JoinHandle<String>>,
    cancel: CancelFlag,
    pub program: String,
}

impl RunningJob {
    /// Spawn with an argument array (never a shell string).
    ///
    /// # Errors
    /// [`EngineError`] on spawn failure.
    pub fn spawn(program: &str, args: &[String], cancel: CancelFlag) -> Result<Self, EngineError> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(EngineError::Spawn)?;
        let stderr = child.stderr.take();
        let stderr_handle = stderr.map(|mut stream| {
            std::thread::spawn(move || {
                use std::io::Read;
                let mut buf = Vec::with_capacity(8 * 1024);
                let mut chunk = [0u8; 8192];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if buf.len() < MAX_STDERR_BYTES {
                                buf.extend_from_slice(&chunk[..n]);
                            }
                            // Keep reading (drain) even past cap so ffmpeg
                            // never blocks on a full pipe.
                        }
                    }
                }
                String::from_utf8_lossy(&buf).into_owned()
            })
        });
        Ok(Self {
            child,
            stderr_handle,
            cancel,
            program: program.to_string(),
        })
    }

    /// Wait for completion; cancels if the shared flag flips.
    ///
    /// # Errors
    /// [`EngineError::Cancelled`] or [`EngineError::Tool`] with stderr tail.
    pub fn wait(mut self) -> Result<String, EngineError> {
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(EngineError::Cancelled);
            }
            match self.child.try_wait()? {
                Some(status) => {
                    let stderr = self
                        .stderr_handle
                        .and_then(|h| h.join().ok())
                        .unwrap_or_default();
                    if status.success() {
                        return Ok(stderr);
                    }
                    return Err(EngineError::Tool(stderr_tail(&stderr)));
                }
                None => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        }
    }
}

/// Run a tool to completion with cancellation support.
///
/// # Errors
/// [`EngineError`] on failure; includes bounded stderr tail.
pub fn run_tool(program: &str, args: &[String], cancel: CancelFlag) -> Result<String, EngineError> {
    let job = RunningJob::spawn(program, args, cancel)?;
    job.wait()
}

/// Synchronous small-output run used for probing encoder availability.
pub(crate) fn run_tool_blocking(program: &str, args: &[String]) -> Result<String, EngineError> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(EngineError::Spawn)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(EngineError::Tool(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }
}

#[must_use]
pub fn stderr_tail(s: &str) -> String {
    let tail: String = s
        .lines()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    tail
}

/// Detect available H.264 encoders by listing + a tiny real test encode.
/// Returns map encoder-name -> usable. CPU libx264 is always last-resort.
#[must_use]
pub fn detect_hw_encoders() -> HashMap<String, bool> {
    let mut result = HashMap::new();
    for name in ["h264_nvenc", "h264_vaapi", "h264_qsv"] {
        result.insert(name.to_string(), tiny_test_encode(name));
    }
    result
}

fn tiny_test_encode(encoder: &str) -> bool {
    let Ok(ffmpeg) = resolve_tool("ffmpeg") else {
        return false;
    };
    // VAAPI additionally needs a render device; absence just makes it false.
    let mut args: Vec<String> = [
        "-hide_banner",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=64x64:d=0.2",
        "-frames:v",
        "3",
        "-c:v",
        encoder,
        "-f",
        "null",
        "-",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    if encoder == "h264_vaapi" {
        // Try common render node; vaapi requires format conversion too.
        args = [
            "-hide_banner",
            "-v",
            "error",
            "-vaapi_device",
            "/dev/dri/renderD128",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=64x64:d=0.2",
            "-frames:v",
            "3",
            "-vf",
            "format=nv12,hwupload",
            "-c:v",
            encoder,
            "-f",
            "null",
            "-",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    }
    run_tool_blocking(&ffmpeg.to_string_lossy(), &args).is_ok()
}

/// Parse `ffmpeg -encoders` output for a codec name.
#[must_use]
pub fn encoders_listing(ffmpeg: &str) -> Vec<String> {
    run_tool_blocking(ffmpeg, &["-hide_banner".into(), "-encoders".into()])
        .map(|out| {
            out.lines()
                .filter(|l| {
                    l.trim_start().starts_with('V') || l.contains("H264") || l.contains("h264")
                })
                .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Ensure no code path in this crate ever builds a shell string: a canary
/// assertion used by tests. argv[0] is the program; args are passed as-is.
#[allow(dead_code)]
fn debug_assert_no_shell(args: &[String]) {
    for a in args {
        debug_assert!(!a.contains("sh -c"), "shell usage detected: {a}");
    }
}

// Global cache of hw detection to avoid repeated test encodes.
static HW_CACHE: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);

/// Cached hardware encoder detection.
#[must_use]
pub fn hw_encoders_cached() -> HashMap<String, bool> {
    if let Ok(mut guard) = HW_CACHE.lock() {
        if let Some(map) = guard.as_ref() {
            return map.clone();
        }
        let map = detect_hw_encoders();
        *guard = Some(map.clone());
        map
    } else {
        HashMap::new()
    }
}
