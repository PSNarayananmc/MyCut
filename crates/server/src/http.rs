//! Minimal HTTP/1.1 server on `std::net` — no web framework, no TLS, no
//! async runtime. Thread-per-connection with a small concurrency cap.
//!
//! Routes:
//! - `GET /`            → embedded UI (or honest fallback page)
//! - `GET /assets/…`    → embedded hashed Vite assets
//! - `GET /health`      → liveness JSON
//! - `GET /media?path=` → jailed media streaming (Range-capable for `<video>`)
//! - `POST /api/<cmd>`  → typed command API (same surface as the Tauri shell)
//!
//! API hardening: `X-MyCut: 1` header required (browsers cannot attach it
//! cross-origin without a preflight, which we never approve), and a
//! present `Origin` must match our own loopback origin.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::Value;

use crate::assets;
use crate::state::{self, AppState, CommandError};

const MAX_HEADER_BYTES: usize = 32 * 1024;
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 24;
const READ_TIMEOUT_SECS: u64 = 600; // long renders stream no bytes; be generous

pub struct Server {
    listener: TcpListener,
    state: Arc<AppState>,
}

impl Server {
    /// Bind on `host:port`; port 0 picks an ephemeral port (used by tests).
    ///
    /// # Errors
    /// IO errors from binding.
    pub fn bind(host: &str, port: u16, state: AppState) -> std::io::Result<Self> {
        let listener = TcpListener::bind((host, port))?;
        Ok(Self {
            listener,
            state: Arc::new(state),
        })
    }

    /// The bound address (useful with port 0).
    pub fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.listener.local_addr()
    }

    /// Access to the shared application state (startup imports in `main`).
    pub fn state(&self) -> Arc<AppState> {
        Arc::clone(&self.state)
    }

    /// Accept loop. Runs forever; each connection is served on its own
    /// bounded thread.
    pub fn serve(&self) -> ! {
        let live = Arc::new(AtomicUsize::new(0));
        loop {
            let Ok((stream, _addr)) = self.listener.accept() else {
                continue;
            };
            if live.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                let _ = respond_simple(
                    &stream,
                    503,
                    "application/json",
                    b"{\"error\":\"busy\"}".to_vec(),
                );
                continue;
            }
            let state = Arc::clone(&self.state);
            let live2 = Arc::clone(&live);
            live.fetch_add(1, Ordering::Relaxed);
            std::thread::spawn(move || {
                let _ = handle_connection(stream, &state);
                live2.fetch_sub(1, Ordering::Relaxed);
            });
        }
    }
}

struct Request {
    method: String,
    path: String,  // decoded path without query
    query: String, // raw query string
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn handle_connection(stream: TcpStream, state: &AppState) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(READ_TIMEOUT_SECS)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(READ_TIMEOUT_SECS)))?;
    let mut reader = BufReader::new(stream.try_clone()?);

    // Support keep-alive for UI assets, close for long media streams.
    while let Some(req) = read_request(&mut reader)? {
        let keep_alive = req
            .header("connection")
            .map(|c| !c.eq_ignore_ascii_case("close"))
            .unwrap_or(true);
        let is_media = req.path == "/media";
        let mut stream = stream.try_clone()?;
        let outcome = route(&req, state, &mut stream);
        if let Err(e) = outcome {
            let _ = respond_simple(
                &stream,
                500,
                "application/json",
                format!("{{\"error\":\"{}\"}}", escape_json(&e.to_string())).into_bytes(),
            );
        }
        if is_media || !keep_alive {
            break; // connection consumed by media stream or client asked close
        }
    }
    Ok(())
}

fn read_request(reader: &mut BufReader<TcpStream>) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Ok(None);
    };
    let mut headers = Vec::new();
    let mut total = line.len();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 {
            return Ok(None);
        }
        total += h.len();
        if total > MAX_HEADER_BYTES {
            return Ok(None);
        }
        let t = h.trim_end();
        if t.is_empty() {
            break;
        }
        if let Some((k, v)) = t.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    if len > MAX_BODY_BYTES {
        return Ok(None);
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut body)?;
    }
    let (path_raw, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.to_string(), String::new()),
    };
    Ok(Some(Request {
        method: method.to_string(),
        path: url_decode(&path_raw),
        query,
        headers,
        body,
    }))
}

fn route(req: &Request, state: &AppState, stream: &mut TcpStream) -> std::io::Result<()> {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => serve_asset(stream, "index.html"),
        ("GET", p) if p.starts_with("/assets/") => serve_asset(stream, p.trim_start_matches('/')),
        ("GET", "/health") => respond_simple(
            stream,
            200,
            "application/json",
            br#"{"ok":true,"app":"mycut"}"#.to_vec(),
        ),
        ("GET", "/media") => media_route(req, state, stream),
        ("POST", p) if p.starts_with("/api/") => api_route(req, state, stream, p),
        ("GET", "/favicon.ico") => respond_simple(stream, 204, "text/plain", Vec::new()),
        _ => respond_simple(
            stream,
            404,
            "application/json",
            b"{\"error\":\"not found\"}".to_vec(),
        ),
    }
}

fn serve_asset(stream: &mut TcpStream, path: &str) -> std::io::Result<()> {
    if let Some((_, bytes)) = assets::ASSETS.iter().find(|(p, _)| *p == path) {
        let mut resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n",
            assets::content_type(path),
            bytes.len()
        ).into_bytes();
        resp.extend_from_slice(bytes);
        return stream.write_all(&resp);
    }
    if path == "index.html" {
        return respond_simple(
            stream,
            200,
            "text/html; charset=utf-8",
            assets::FALLBACK_INDEX.to_vec(),
        );
    }
    respond_simple(stream, 404, "text/plain", b"not found".to_vec())
}

// ------------------------- API -------------------------

fn api_origin_ok(req: &Request) -> bool {
    if req.header("x-mycut") != Some("1") {
        return false;
    }
    if let Some(origin) = req.header("origin") {
        let Some(host) = req.header("host") else {
            return false;
        };
        let expected = format!("http://{host}");
        if origin != expected && origin != format!("https://{host}") {
            return false;
        }
    }
    true
}

fn api_route(
    req: &Request,
    state: &AppState,
    stream: &mut TcpStream,
    path: &str,
) -> std::io::Result<()> {
    if !api_origin_ok(req) {
        return respond_simple(
            stream,
            403,
            "application/json",
            b"{\"error\":\"forbidden (missing X-MyCut header or cross-origin request)\"}".to_vec(),
        );
    }
    let cmd = path.trim_start_matches("/api/");
    let body: Value = if req.body.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(e) => {
                return respond_simple(
                    stream,
                    400,
                    "application/json",
                    format!(
                        "{{\"error\":\"bad JSON: {}\"}}",
                        escape_json(&e.to_string())
                    )
                    .into_bytes(),
                )
            }
        }
    };

    let result = dispatch(state, cmd, &body);
    match result {
        Ok(v) => respond_simple(
            stream,
            200,
            "application/json",
            serde_json::to_vec(&v).unwrap_or_else(|_| b"{}".to_vec()),
        ),
        Err(e) => respond_simple(
            stream,
            e.status,
            "application/json",
            format!("{{\"error\":\"{}\"}}", escape_json(&e.message)).into_bytes(),
        ),
    }
}

/// Map `camelCase` JS arg names (Tauri convention) onto the command calls.
fn dispatch(state: &AppState, cmd: &str, body: &Value) -> Result<Value, CommandError> {
    let s = |k: &str| body.get(k).and_then(Value::as_str).map(str::to_string);
    let sn = |k1: &str, k2: &str| s(k1).or_else(|| s(k2));
    match cmd {
        "project_snapshot" => state::project_snapshot(state),
        "import_media" => {
            let paths = body
                .get("paths")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            state::import_media(state, &paths)
        }
        "ai_apply_request" => {
            let request = s("request").unwrap_or_default();
            state::ai_apply_request(state, &request)
        }
        "set_api_key" => {
            let key = s("key").unwrap_or_default();
            state::set_api_key(&key)
        }
        "ai_test_connection" => state::ai_test_connection(state),
        "render_final" => state::render_final(state),
        "render_preview_range" => state::render_preview_range(state),
        "undo" => state::undo(state),
        "redo" => state::redo(state),
        "save" => state::save(state),
        "get_settings" => state::get_settings(state),
        "set_setting" => {
            let key = s("key").unwrap_or_default();
            let value = s("value").unwrap_or_default();
            state::set_setting(state, &key, &value)
        }
        "clear_cache" => state::clear_cache(state),
        "cache_usage" => state::cache_usage(state),
        "proxy_path" => {
            let source_id = sn("sourceId", "source_id").unwrap_or_default();
            state::proxy_path(state, &source_id)
        }
        "relink_source" => {
            let source_id = sn("sourceId", "source_id").unwrap_or_default();
            state::relink_source(state, &source_id)
        }
        "doctor" => state::doctor_json(state),
        other => Err(CommandError::new(404, format!("unknown command: {other}"))),
    }
}

// ------------------------- media -------------------------

/// Roots a `/media` request may read from: the project dir, cache dir,
/// export dir, plus (read-only, mirroring `engine::pathing::resolve_media`)
/// the parent dirs of user-imported external media — nothing the AI could
/// introduce, only files the user imported or wrote.
pub fn media_roots(state: &AppState) -> Vec<PathBuf> {
    let mut roots = vec![
        state
            .project_path
            .lock()
            .unwrap()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
        state.cache_dir.clone(),
        state::export_dir(),
    ];
    let doc = state.doc.lock().unwrap();
    let project_dir = roots[0].clone();
    for s in &doc.project.sources {
        if let Some(p) = mycut_projects::resolve_source_path(&s.rel_path, &project_dir) {
            if let Some(parent) = p.parent() {
                roots.push(parent.to_path_buf());
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Canonicalize + jail: the path must exist and resolve under one root.
pub fn jail_path(state: &AppState, raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() || raw.contains('\0') {
        return Err("bad path".into());
    }
    let p = PathBuf::from(raw);
    let canon = p.canonicalize().map_err(|_| "path not found")?;
    for root in media_roots(state) {
        let rc = root.canonicalize().unwrap_or(root);
        if canon.starts_with(&rc) {
            return Ok(canon);
        }
    }
    Err("path outside allowed roots".into())
}

fn mime_for(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        "wav" => "audio/wav",
        "ass" | "srt" | "vtt" => "text/plain; charset=utf-8",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

fn media_route(req: &Request, state: &AppState, stream: &mut TcpStream) -> std::io::Result<()> {
    let raw = query_param(&req.query, "path").unwrap_or_default();
    let path = match jail_path(state, &raw) {
        Ok(p) => p,
        Err(e) => {
            return respond_simple(
                stream,
                403,
                "application/json",
                format!("{{\"error\":\"{}\"}}", escape_json(&e)).into_bytes(),
            );
        }
    };
    let Ok(file) = std::fs::File::open(&path) else {
        return respond_simple(
            stream,
            404,
            "application/json",
            b"{\"error\":\"not found\"}".to_vec(),
        );
    };
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);

    // Range support: browsers send `Range: bytes=a-` (or a-b) for <video>.
    let (status, start, end) = match req.header("range") {
        Some(r) if r.starts_with("bytes=") => {
            let spec = &r[6..];
            let mut it = spec.splitn(2, '-');
            let a: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let b: Option<u64> = it.next().and_then(|v| v.parse().ok());
            let e = b
                .map(|x| x.min(total.saturating_sub(1)))
                .unwrap_or(total.saturating_sub(1));
            if a >= total {
                return respond_simple(
                    stream,
                    416,
                    "application/json",
                    format!("{{\"error\":\"range out of bounds (len {total})\"}}").into_bytes(),
                );
            }
            (206, a, e)
        }
        _ => (200, 0, total.saturating_sub(1)),
    };
    let len = if total == 0 { 0 } else { end - start + 1 };
    let mut headers = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {}\r\nContent-Length: {len}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n",
        if status == 206 { "Partial Content" } else { "OK" },
        mime_for(&path),
    );
    if status == 206 {
        headers.push_str(&format!("Content-Range: bytes {start}-{end}/{total}\r\n"));
    }
    headers.push_str("\r\n");
    stream.write_all(headers.as_bytes())?;

    if len == 0 {
        return Ok(());
    }
    let mut file = file;
    use std::io::Seek;
    file.seek(std::io::SeekFrom::Start(start))?;
    let mut remaining = len;
    let mut buf = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let n = file.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        stream.write_all(&buf[..n])?;
        remaining -= n as u64;
    }
    Ok(())
}

// ------------------------- plumbing -------------------------

fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == key {
            return Some(url_decode(v));
        }
    }
    None
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = |b: u8| -> Option<u8> {
                    match b {
                        b'0'..=b'9' => Some(b - b'0'),
                        b'a'..=b'f' => Some(b - b'a' + 10),
                        b'A'..=b'F' => Some(b - b'A' + 10),
                        _ => None,
                    }
                };
                if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    out.push(h * 16 + l);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn respond_simple(
    stream: &TcpStream,
    status: u16,
    ctype: &str,
    body: Vec<u8>,
) -> std::io::Result<()> {
    let mut stream = stream.try_clone()?;
    let text = match status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {text}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&body)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_decode_roundtrip() {
        assert_eq!(url_decode("%2Fhome%2Fu%2Fa+b.mp4"), "/home/u/a b.mp4");
        assert_eq!(url_decode("plain"), "plain");
        assert_eq!(url_decode("100%25.mp4"), "100%.mp4");
    }

    #[test]
    fn escape_json_handles_control_chars() {
        assert_eq!(escape_json("a\"b\nc"), "a\\\"b\\nc");
    }
}
