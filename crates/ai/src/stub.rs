//! Stub HTTP server for provider tests (tests only — never in production
//! paths). Minimal HTTP/1.1 over std TcpListener.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub struct StubResponse {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
    /// Sleep before responding (timeout tests).
    pub delay_ms: u64,
}

pub type Handler = Arc<dyn Fn(u32, &str, &str) -> StubResponse + Send + Sync>;

/// Spawn a stub server; returns base URL and a request counter handle.
pub fn spawn(handler: Handler) -> (String, Arc<AtomicU32>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let handler = handler.clone();
            let counter = counter_clone.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    return;
                }
                let mut headers: HashMap<String, String> = HashMap::new();
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).is_err() {
                        return;
                    }
                    let h = h.trim_end().to_string();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        headers.insert(k.trim().to_lowercase(), v.trim().to_string());
                    }
                }
                let len: usize = headers
                    .get("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0u8; len];
                if len > 0 {
                    let _ = reader.read_exact(&mut body);
                }
                let body = String::from_utf8_lossy(&body).into_owned();
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let resp = handler(n, &line, &body);
                if resp.delay_ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(resp.delay_ms));
                }
                let mut out = stream;
                let status_text = match resp.status {
                    200 => "OK",
                    401 => "Unauthorized",
                    404 => "Not Found",
                    429 => "Too Many Requests",
                    500 => "Internal Server Error",
                    _ => "OK",
                };
                let mut response = format!("HTTP/1.1 {} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", resp.status, resp.body.len());
                for (k, v) in &resp.headers {
                    response.push_str(&format!("{k}: {v}\r\n"));
                }
                response.push_str("\r\n");
                let _ = out.write_all(response.as_bytes());
                let _ = out.write_all(resp.body.as_bytes());
                let _ = out.flush();
            });
        }
    });
    (format!("http://127.0.0.1:{port}"), counter)
}

pub const VALID_PLAN_JSON: &str = r#"{
  "schema_version": "1.0",
  "intent_summary": "30s vertical gaming Short, captions, vibrant color",
  "assumptions": ["Category inferred: gameplay"],
  "operations": [
    { "op": "cut_ranges", "strategy": "remove_low_activity",
      "ranges": [{ "start": 12.0, "end": 16.0 }], "reason": "no speech, low motion" },
    { "op": "set_color", "params": { "saturation": 1.15 } },
    { "op": "add_effect", "effect": "zoom_punch", "start": 5.0, "end": 6.0,
      "params": { "strength": 1.18 }, "reason": "audio peak" },
    { "op": "set_aspect", "ratio": "9:16", "reframe": "subject_follow" },
    { "op": "set_export_preset", "preset": "youtube_shorts" }
  ]
}"#;
