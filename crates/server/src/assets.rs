//! Embedded UI assets (generated at build time — see build.rs).

include!(concat!(env!("OUT_DIR"), "/ui_assets.rs"));

/// Content-Type for an embedded asset path.
#[must_use]
pub fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Fallback page when the UI has not been built yet (honest, actionable).
pub const FALLBACK_INDEX: &[u8] = br#"<!doctype html>
<html><head><meta charset="utf-8"><title>MyCut</title>
<style>body{background:#101216;color:#e8eaf0;font:16px/1.6 system-ui,sans-serif;display:grid;place-items:center;height:100vh;margin:0}
code{background:#1b2027;padding:2px 8px;border-radius:6px}</style></head>
<body><div><h1>MyCut backend is running</h1>
<p>The UI bundle is not embedded in this binary. Build it with:</p>
<p><code>npm --prefix ui install &amp;&amp; npm --prefix ui run build</code></p>
<p>then rebuild <code>mycut</code>. The command API is served at
<code>/api/&lt;command&gt;</code>.</p></div></body></html>
"#;
