//! Background export job: one render at a time, FFmpeg-driven, cancellable,
//! with real progress parsed from the encoder's own `time=` output.

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;

/// Observable export state (one job at a time — the UI polls it). The job
/// slot and cancel flag are Arc-shared so the worker thread owns neither
/// `AppState` nor any borrowed data.
#[derive(Default)]
pub struct ExportState {
    pub slot: Arc<Mutex<Option<Job>>>,
    pub cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

/// JSON-visible job state.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Job {
    pub running: bool,
    pub done: bool,
    /// 0.0..=1.0, derived from FFmpeg's own progress output.
    pub progress: f64,
    pub error: Option<String>,
    pub path: Option<String>,
    pub cancelled: bool,
}

impl Job {
    fn starting() -> Self {
        Self {
            running: true,
            done: false,
            progress: 0.0,
            error: None,
            path: None,
            cancelled: false,
        }
    }
}

/// Kick off a render in a background thread. Fails when a job is already
/// running (the UI surfaces it; no queue needed on a low-end machine).
///
/// # Errors
/// Returns a message when another export is in flight.
pub fn start(
    st: &ExportState,
    program: String,
    args: Vec<String>,
    out_path: PathBuf,
    total_seconds: f64,
) -> Result<(), String> {
    {
        let mut slot = st.slot.lock().unwrap();
        if slot.as_ref().is_some_and(|j| j.running) {
            return Err("an export is already running — cancel it first".into());
        }
        *slot = Some(Job::starting());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *st.cancel.lock().unwrap() = Some(Arc::clone(&cancel));

    let slot_arc = Arc::clone(&st.slot);
    std::thread::spawn(move || {
        let result = run_with_progress(&slot_arc, &program, &args, &cancel, total_seconds);
        let mut slot = slot_arc.lock().unwrap();
        if let Some(job) = slot.as_mut() {
            job.running = false;
            job.done = true;
            match result {
                Ok(()) => job.path = Some(out_path.to_string_lossy().into_owned()),
                Err(e) if e == "cancelled" => {
                    job.cancelled = true;
                    job.error = Some("Export cancelled.".into());
                }
                Err(e) => job.error = Some(e),
            }
        }
    });
    Ok(())
}

/// Best-effort cancel: flips the flag; the worker notices within ~250 ms.
pub fn cancel_request(st: &ExportState) {
    if let Some(c) = st.cancel.lock().unwrap().as_ref() {
        c.store(true, Ordering::Relaxed);
    }
    if let Some(job) = st.slot.lock().unwrap().as_mut() {
        if job.running {
            job.cancelled = true;
        }
    }
}

pub fn status(st: &ExportState) -> serde_json::Value {
    let guard = st.slot.lock().unwrap();
    match guard.as_ref() {
        Some(job) => serde_json::to_value(job).unwrap_or_else(|_| json!({"running": false})),
        None => json!({"running": false, "done": false, "progress": 0.0}),
    }
}

/// Run the encoder, streaming stderr for live `time=` progress. Arg-array
/// only (no shell), bounded memory, cancellable. Progress snapshots are
/// published into `slot` while running.
fn run_with_progress(
    slot_arc: &Arc<Mutex<Option<Job>>>,
    program: &str,
    args: &[String],
    cancel: &AtomicBool,
    total_seconds: f64,
) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start FFmpeg: {e}"))?;
    let stderr = child.stderr.take();
    let last_time = Arc::new(AtomicU64::new(0));
    let progress_reader = {
        let last_time = Arc::clone(&last_time);
        std::thread::spawn(move || {
            if let Some(stream) = stderr {
                let mut reader = std::io::BufReader::new(stream);
                let mut line = Vec::with_capacity(256);
                loop {
                    match reader.read_until(b'\r', &mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            if let Ok(text) = std::str::from_utf8(&line) {
                                if let Some(pos) = text.rfind("time=") {
                                    let rest = text[pos + 5..]
                                        .split_whitespace()
                                        .next()
                                        .unwrap_or("");
                                    if let Some(t) = parse_hhmmss(rest) {
                                        last_time.store(
                                            (t * 1000.0) as u64,
                                            Ordering::Relaxed,
                                        );
                                    }
                                }
                            }
                            line.clear();
                        }
                    }
                }
            }
        })
    };
    // Poll completion + cancellation; publish progress snapshots as we go.
    let total_ms = (total_seconds * 1000.0).max(1.0);
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = progress_reader.join();
            return Err("cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = progress_reader.join();
                if status.success() {
                    if let Some(job) = slot_arc.lock().unwrap().as_mut() {
                        job.progress = 1.0;
                    }
                    return Ok(());
                }
                return Err(format!(
                    "FFmpeg exited with {status}. The timeline, codecs or output \
                     path may be invalid — see Settings → Diagnostics."
                ));
            }
            Ok(None) => {
                let done_ms = last_time.load(Ordering::Relaxed);
                let frac = (done_ms as f64 / total_ms).clamp(0.0, 0.999);
                if let Some(job) = slot_arc.lock().unwrap().as_mut() {
                    job.progress = frac;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(e) => return Err(format!("FFmpeg wait failed: {e}")),
        }
    }
}

fn parse_hhmmss(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    match parts.as_slice() {
        [h, m, sec] => {
            let h: f64 = h.parse().ok()?;
            let m: f64 = m.parse().ok()?;
            let sec: f64 = sec.parse().ok()?;
            Some(h * 3600.0 + m * 60.0 + sec)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_timestamps() {
        assert_eq!(parse_hhmmss("00:00:03.24"), Some(3.24));
        assert_eq!(parse_hhmmss("01:02:03"), Some(3723.0));
        assert_eq!(parse_hhmmss("n/a"), None);
    }

    #[test]
    fn start_rejects_concurrent_jobs() {
        let st = ExportState::default();
        st.slot.lock().unwrap().replace(Job {
            running: true,
            done: false,
            progress: 0.1,
            error: None,
            path: None,
            cancelled: false,
        });
        assert!(start(&st, "true".into(), vec![], PathBuf::from("/tmp/x"), 1.0).is_err());
    }

    #[test]
    fn cancel_before_start_is_noop() {
        let st = ExportState::default();
        cancel_request(&st); // must not panic
        assert_eq!(status(&st)["running"], false);
    }
}
