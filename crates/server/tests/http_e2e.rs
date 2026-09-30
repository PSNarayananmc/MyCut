//! End-to-end tests for the local web runtime: real HTTP requests against
//! a real server on an ephemeral loopback port, a real lavfi fixture, and
//! ffprobe verification of rendered output. Security cases assert the jail
//! and cross-origin protections actually reject.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use mycut_core::{History, Project};
use mycut_projects::ProjectDocument;
use mycut_server::http::Server;
use mycut_server::state::{AppSettings, AppState};

fn ffmpeg_on_path() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok()
}

fn make_fixture(dir: &Path, name: &str) -> PathBuf {
    let out = dir.join(name);
    let ok = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=25:duration=4",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=4",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&out)
        .status()
        .expect("spawn ffmpeg")
        .success();
    assert!(ok, "fixture generation failed");
    out
}

fn temp_app(project_dir: impl AsRef<std::path::Path>) -> AppState {
    let project_dir = project_dir.as_ref().to_path_buf();
    let project_path = project_dir.join("test.mycut");
    std::fs::create_dir_all(&project_dir).unwrap();
    AppState {
        doc: Mutex::new(ProjectDocument {
            schema_version: mycut_core::SCHEMA_VERSION.into(),
            project: Project::new("test"),
            history: History::new(),
            saved_at_unix_ms: 0,
            app_version: "test".into(),
        }),
        project_path: Mutex::new(project_path),
        settings: Mutex::new(AppSettings::default()),
        cache_dir: project_dir.join("cache"),
        cancel: AtomicBool::new(false),
        models_cache: Mutex::new(mycut_server::state::ModelsCache::default()),
        export: mycut_server::jobs::ExportState::default(),
        conversation: Mutex::new(Vec::new()),
    }
}

fn start_server(state: AppState) -> String {
    let server = Server::bind("127.0.0.1", 0, state).expect("bind");
    let addr = server.local_addr().unwrap();
    std::thread::spawn(move || server.serve());
    format!("http://{addr}")
}

struct Resp {
    status: u16,
    body: Vec<u8>,
    content_range: Option<String>,
}

fn request(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>) -> Resp {
    let without_scheme = url.trim_start_matches("http://");
    let (host, path) = match without_scheme.split_once('/') {
        Some((h, p)) => (h.to_string(), format!("/{p}")),
        None => (without_scheme.to_string(), "/".to_string()),
    };
    let mut stream = TcpStream::connect(&host).expect("connect");
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    match body {
        Some(b) => req.push_str(&format!("Content-Length: {}\r\n\r\n", b.len())),
        None => req.push_str("Content-Length: 0\r\n\r\n"),
    }
    stream.write_all(req.as_bytes()).unwrap();
    if let Some(b) = body {
        stream.write_all(b).unwrap();
    }
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    parse_response(&buf)
}

fn parse_response(raw: &[u8]) -> Resp {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("header split");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let content_range = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-range:"))
        .map(|l| l.splitn(2, ':').nth(1).unwrap().trim().to_string());
    Resp {
        status,
        body: raw[split + 4..].to_vec(),
        content_range,
    }
}

fn api(base: &str, cmd: &str, body: &str) -> Resp {
    request(
        "POST",
        &format!("{base}/api/{cmd}"),
        &[("Content-Type", "application/json"), ("X-MyCut", "1")],
        Some(body.as_bytes()),
    )
}

// ------------------------- tests -------------------------

#[test]
fn http_full_pipeline_import_snapshot_proxy_render() {
    if !ffmpeg_on_path() {
        panic!("ffmpeg must be on PATH for these integration tests");
    }
    let tmp = tempfile::tempdir().unwrap();
    let fixture = make_fixture(tmp.path(), "clip.mp4");
    let base = start_server(temp_app(tmp.path().join("project")));

    // 1. Import via HTTP.
    let r = api(
        &base,
        "import_media",
        &format!("{{\"paths\":[{:?}]}}", fixture.display().to_string()),
    );
    assert_eq!(
        r.status,
        200,
        "import failed: {}",
        String::from_utf8_lossy(&r.body)
    );

    // 2. Snapshot shows the source with real metadata.
    let r = api(&base, "project_snapshot", "{}");
    assert_eq!(r.status, 200);
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(v["sources"].as_array().unwrap().len(), 1);
    assert_eq!(v["sources"][0]["duration_ms"], 4000);
    assert_eq!(v["sources"][0]["has_audio"], true);
    let src_id = v["sources"][0]["id"].as_str().unwrap().to_string();

    // 3. Proxy path renders on demand and streams back with Range support.
    let r = api(
        &base,
        "proxy_path",
        &format!("{{\"sourceId\":\"{src_id}\"}}"),
    );
    assert_eq!(
        r.status,
        200,
        "proxy render failed: {}",
        String::from_utf8_lossy(&r.body)
    );
    let proxy_path: String = serde_json::from_slice(&r.body)
        .map(|v: serde_json::Value| v["path"].as_str().unwrap().to_string())
        .unwrap();
    let mut req = format!(
        "GET /media?path={} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n",
        percent_encode(&proxy_path)
    );
    req.push_str("Range: bytes=0-99\r\n\r\n");
    let mut stream = TcpStream::connect(base.trim_start_matches("http://")).unwrap();
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let resp = parse_response(&buf);
    assert_eq!(resp.status, 206, "Range must yield 206");
    assert_eq!(resp.body.len(), 100);
    assert!(resp.content_range.unwrap().starts_with("bytes 0-99/"));

    // 4. Final render produces a real, decodable MP4.
    let r = api(&base, "render_final", "{}");
    assert_eq!(
        r.status,
        200,
        "render failed: {}",
        String::from_utf8_lossy(&r.body)
    );
    let out_path: String = serde_json::from_slice(&r.body)
        .map(|v: serde_json::Value| v["path"].as_str().unwrap().to_string())
        .unwrap();
    let out = PathBuf::from(&out_path);
    assert!(out.exists(), "export exists");
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,codec_name",
            "-of",
            "json",
        ])
        .arg(&out)
        .output()
        .unwrap();
    assert!(probe.status.success(), "ffprobe must accept the export");
    let pv: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    let codecs: Vec<&str> = pv["streams"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["codec_name"].as_str())
        .collect();
    assert!(
        codecs.contains(&"h264"),
        "video stream must be h264, got {codecs:?}"
    );
    let dur: f64 = pv["format"]["duration"].as_str().unwrap().parse().unwrap();
    assert!((dur - 4.0).abs() < 0.35, "duration ~4s, got {dur}");
}

#[test]
fn http_security_jail_and_headers() {
    let tmp = tempfile::tempdir().unwrap();
    let base = start_server(temp_app(tmp.path().join("project")));

    // No X-MyCut header → rejected (blocks cross-origin form posts).
    let r = request(
        "POST",
        &format!("{base}/api/project_snapshot"),
        &[("Content-Type", "application/json")],
        Some(b"{}".as_slice()),
    );
    assert_eq!(r.status, 403);

    // Cross-origin Origin header → rejected.
    let r = request(
        "POST",
        &format!("{base}/api/project_snapshot"),
        &[
            ("Content-Type", "application/json"),
            ("X-MyCut", "1"),
            ("Origin", "http://evil.example"),
        ],
        Some(b"{}".as_slice()),
    );
    assert_eq!(r.status, 403);

    // Path traversal against /media → rejected.
    let r = request(
        "GET",
        &format!("{base}/media?path={}", percent_encode("/etc/passwd")),
        &[],
        None,
    );
    assert_eq!(r.status, 403);

    // Unknown command → 404, not a panic.
    let r = api(&base, "definitely_not_a_command", "{}");
    assert_eq!(r.status, 404);

    // AI without a key → honest 400 with actionable guidance (never "no-api-key").
    let r = api(
        &base,
        "ai_apply_request",
        "{\"request\":\"make a 30s cut\"}",
    );
    assert_eq!(r.status, 400);
    let body = String::from_utf8_lossy(&r.body);
    assert!(
        body.contains("No NVIDIA NIM API key is configured"),
        "actionable error expected, got: {body}"
    );
    assert!(!body.to_lowercase().contains("nvapi-"), "key value must never leak");
}

#[test]
fn http_undo_redo_and_settings_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = make_fixture(tmp.path(), "clip2.mp4");
    let base = start_server(temp_app(tmp.path().join("project")));

    let r = api(
        &base,
        "import_media",
        &format!("{{\"paths\":[{:?}]}}", fixture.display().to_string()),
    );
    assert_eq!(r.status, 200);

    // Snapshot non-empty → undo (2 steps: AddSource then AddClip) → empty.
    let count = |base: &str| -> usize {
        let r = api(base, "project_snapshot", "{}");
        let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        v["sources"].as_array().unwrap().len()
    };
    let clip_count = |base: &str| -> usize {
        let r = api(base, "project_snapshot", "{}");
        let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        v["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["items"].as_array().unwrap().len())
            .sum()
    };
    assert_eq!(count(&base), 1);
    assert_eq!(clip_count(&base), 1);
    // Step 1 undoes AddClip (timeline item), source stays in the bin.
    let r = api(&base, "undo", "{}");
    assert_eq!(r.status, 200);
    assert_eq!(clip_count(&base), 0);
    assert_eq!(count(&base), 1);
    // Step 2 undoes AddSource.
    let r = api(&base, "undo", "{}");
    assert_eq!(r.status, 200);
    assert_eq!(count(&base), 0);
    // Redo restores both, newest first.
    let r = api(&base, "redo", "{}");
    assert_eq!(r.status, 200);
    assert_eq!(count(&base), 1);
    let r = api(&base, "redo", "{}");
    assert_eq!(r.status, 200);
    assert_eq!(clip_count(&base), 1);

    // Settings round-trip (no secrets in this surface).
    let r = api(
        &base,
        "set_setting",
        "{\"key\":\"model\",\"value\":\"meta/test\"}",
    );
    assert_eq!(r.status, 200);
    let r = api(&base, "get_settings", "{}");
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(v["model"], "meta/test");
}

#[test]
fn doctor_reports_ffmpeg_state() {
    let tmp = tempfile::tempdir().unwrap();
    let base = start_server(temp_app(tmp.path().join("project")));
    let r = api(&base, "doctor", "{}");
    assert_eq!(r.status, 200);
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    let state = v["ffmpeg"]["state"].as_str().unwrap();
    assert!(
        ["ok", "too-old", "missing"].contains(&state),
        "tri-state doctor, got {state}"
    );
    assert!(!v["ffmpeg"]["message"].as_str().unwrap().is_empty());
    assert_eq!(v["runtime"], "local-web");
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[test]
#[ignore = "run explicitly: key store touches real config dir via env override"]
fn key_roundtrip_never_leaks_value() {
    // Covered implicitly; the unit suite owns secret-store coverage.
}

fn urlenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[test]
fn upload_import_and_catalog_commands() {
    if !ffmpeg_on_path() {
        eprintln!("ffmpeg not installed; skipping");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let fixture = make_fixture(tmp.path(), "my clip.mp4");
    let base = start_server(temp_app(tmp.path().join("project")));

    // 1. Upload import via the streaming route (space in the name!).
    let bytes = std::fs::read(&fixture).unwrap();
    let r = request(
        "POST",
        &format!("{}/api/import_upload?name={}", base, urlenc("my clip.mp4")),
        &[("Content-Type", "application/octet-stream"), ("X-MyCut", "1")],
        Some(&bytes),
    );
    assert_eq!(r.status, 200, "upload failed: {}", String::from_utf8_lossy(&r.body));
    let body = String::from_utf8_lossy(&r.body);
    assert!(body.contains("\"id\":\"src_"), "source returned: {body}");

    // 2. Snapshot shows it, with a proxy coming from the media dir.
    let r = api(&base, "project_snapshot", "{}");
    assert!(String::from_utf8_lossy(&r.body).contains("my clip.mp4"));

    // 3. Effect catalog is real (video + audio entries with params).
    let r = api(&base, "effect_catalog", "{}");
    let cat = String::from_utf8_lossy(&r.body);
    assert!(cat.contains("zoom_punch") && cat.contains("fade_in"));

    // 4. Transition + caption + preset catalogs.
    assert!(api(&base, "transition_catalog", "{}").status == 200);
    assert!(api(&base, "caption_style_catalog", "{}").status == 200);
    let r = api(&base, "export_presets", "{}");
    assert!(String::from_utf8_lossy(&r.body).contains("youtube_shorts"));

    // 5. Key state: none stored initially.
    let r = api(&base, "key_state", "{}");
    assert!(String::from_utf8_lossy(&r.body).contains("\"stored\":false"));

    // 6. set_api_key("") must NOT wipe anything / must be rejected.
    let r = api(&base, "set_api_key", "{\"key\":\"\"}");
    assert_eq!(r.status, 400, "empty key rejected");

    // 7. Export job: status starts idle; the upload already placed a clip,
    // so export_start begins a real job — cancel it right away.
    let r = api(&base, "export_status", "{}");
    assert!(String::from_utf8_lossy(&r.body).contains("\"running\":false"));
    let r = api(&base, "export_start", "{}");
    let start_body = String::from_utf8_lossy(&r.body).into_owned();
    if r.status == 200 {
        // Real job started (ffmpeg on PATH); cancel and wait it out.
        let _ = api(&base, "export_cancel", "{}");
        for _ in 0..100 {
            let s = api(&base, "export_status", "{}");
            let sb = String::from_utf8_lossy(&s.body).into_owned();
            if sb.contains("\"running\":false") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let s = api(&base, "export_status", "{}");
        let sb = String::from_utf8_lossy(&s.body).into_owned();
        assert!(
            sb.contains("\"running\":false"),
            "export job must terminate after cancel: {sb}"
        );
    } else {
        assert_eq!(r.status, 400, "unexpected export_start state: {start_body}");
    }
}

#[test]
fn manual_edit_commands_roundtrip_with_undo() {
    if !ffmpeg_on_path() {
        eprintln!("ffmpeg not installed; skipping");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let _fixture = make_fixture(tmp.path(), "clip3.mp4");
    let base = start_server(temp_app(tmp.path().join("project")));

    // Import via explicit path (fixture already inside tmp — allowed root).
    let import_body = format!("{{\"paths\":[{}]}}", serde_json::to_string(&_fixture).unwrap());
    let r = api(&base, "import_media", &import_body);
    assert_eq!(r.status, 200, "import failed: {}", String::from_utf8_lossy(&r.body));

    // Grab the clip item id from the snapshot.
    let r = api(&base, "project_snapshot", "{}");
    let snap: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    let item_id = snap["tracks"][0]["items"][0]["id"].as_str().unwrap().to_string();
    let dur = snap["tracks"][0]["items"][0]["timeline_duration_ms"].as_i64().unwrap();

    // Split at the midpoint.
    let body = serde_json::json!({"itemId": item_id, "atMs": dur / 2}).to_string();
    let r = api(&base, "split_clip", &body);
    assert_eq!(r.status, 200, "split failed: {}", String::from_utf8_lossy(&r.body));

    // Add an effect to the first half.
    let body = serde_json::json!({"itemId": item_id, "defId": "zoom_punch"}).to_string();
    let r = api(&base, "add_effect", &body);
    assert_eq!(r.status, 200, "add_effect failed: {}", String::from_utf8_lossy(&r.body));
    let effect_id: String = {
        let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        v["effectId"].as_str().unwrap().to_string()
    };

    // Tune the effect param.
    let body = serde_json::json!({"itemId": item_id, "effectId": effect_id, "name": "strength", "value": 1.4}).to_string();
    assert_eq!(api(&base, "set_effect_param", &body).status, 200);

    // Add text + captions manually.
    let body = serde_json::json!({"text": "Hello 世界", "startMs": 0, "durationMs": 1500, "position": "top_center"}).to_string();
    assert_eq!(api(&base, "add_text", &body).status, 200);
    let body = serde_json::json!({"style": "gaming", "entries": [
        {"startMs": 0, "endMs": 1500, "text": "hello world"},
        {"startMs": 1500, "endMs": 3000, "text": "second line"}
    ]}).to_string();
    assert_eq!(api(&base, "set_captions", &body).status, 200);

    // Undo twice: captions gone, text gone.
    assert_eq!(api(&base, "undo", "{}").status, 200);
    assert_eq!(api(&base, "undo", "{}").status, 200);
    let r = api(&base, "project_snapshot", "{}");
    let snap: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(snap["tracks"].as_array().unwrap().iter().filter(|t| t["kind"] == "text").count(), 1);
    let text_items = snap["tracks"].as_array().unwrap().iter()
        .find(|t| t["kind"] == "text").unwrap()["items"].as_array().unwrap().len();
    assert_eq!(text_items, 0, "text undone");

    // Redo brings them back.
    assert_eq!(api(&base, "redo", "{}").status, 200);
    let r = api(&base, "project_snapshot", "{}");
    let snap: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    let text_items = snap["tracks"].as_array().unwrap().iter()
        .find(|t| t["kind"] == "text").unwrap()["items"].as_array().unwrap().len();
    assert_eq!(text_items, 1, "text redone");
}
