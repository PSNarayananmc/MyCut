//! `mycut` — MyCut Local Web Runtime launcher.
//!
//! Starts the loopback HTTP server (UI + command API), optionally imports
//! media given on the command line, and opens the user's browser. This is
//! the Ubuntu 20.04 / GLIBC 2.31-compatible frontend (docs/DECISIONS.md
//! D15); the Tauri 2 desktop shell remains available for newer distros.
//!
//! Usage:
//!   mycut [PROJECT.mycut] [--import PATH…] [--no-open] [--port N] [--host H]
//!   mycut --version | --help
//!
//! Environment:
//!   MYCUT_NO_OPEN=1        do not launch a browser
//!   MYCUT_BROWSER=CMD      use CMD to open the URL
//!   MYCUT_FFMPEG/FFPROBE   sidecar overrides (respected by the engine)
//!   MYCUT_USE_SYSTEM_FFMPEG=1  ignore bundled sidecars set by the launcher
//!   MYCUT_EXPORT_DIR=DIR   where final exports are written (default ~/Videos)
//!   MYCUT_HOST / MYCUT_PORT  alternate bind settings

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use mycut_core::{History, Project};
use mycut_projects::ProjectDocument;
use mycut_server::state::{AppSettings, AppState};

const HELP: &str = "\
MyCut — chat-driven Linux video editor (local web runtime)

USAGE:
  mycut [PROJECT.mycut] [OPTIONS]

OPTIONS:
  --import <PATH>…   Import media files at startup (repeatable / comma lists)
  --no-open          Do not open a browser automatically
  --port <N>         TCP port (default: auto)
  --host <ADDR>      Bind address (default: 127.0.0.1 — keep it local!)
  --version          Print version
  --help             This help

The UI opens in your browser; all editing, AI planning and rendering run in
this local process. No data leaves your machine except requests YOU send to
the configured NVIDIA NIM endpoint.
";

struct Args {
    project: PathBuf,
    imports: Vec<String>,
    open: bool,
    port: u16,
    host: String,
    version: bool,
}

fn parse_args() -> Args {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut a = Args {
        project: PathBuf::new(),
        imports: Vec::new(),
        open: std::env::var("MYCUT_NO_OPEN").map_or(true, |v| v != "1" && v != "true"),
        port: 0,
        host: std::env::var("MYCUT_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
        version: false,
    };
    if let Ok(p) = std::env::var("MYCUT_PORT") {
        a.port = p.parse().unwrap_or(0);
    }
    let mut i = 0;
    let mut positional: Option<String> = None;
    while i < argv.len() {
        match argv[i].as_str() {
            "--help" | "-h" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "--version" => a.version = true,
            "--no-open" => a.open = false,
            "--port" => {
                i += 1;
                a.port = argv.get(i).and_then(|v| v.parse().ok()).unwrap_or(0);
            }
            "--host" => {
                i += 1;
                if let Some(h) = argv.get(i) {
                    a.host = h.clone();
                }
            }
            "--import" => {
                i += 1;
                while i < argv.len() && !argv[i].starts_with("--") {
                    for part in argv[i].split(',') {
                        let p = part.trim();
                        if !p.is_empty() {
                            a.imports.push(p.to_string());
                        }
                    }
                    i += 1;
                }
                continue;
            }
            other => {
                if positional.is_none() && !other.starts_with('-') {
                    positional = Some(other.to_string());
                }
            }
        }
        i += 1;
    }
    if a.version {
        println!("mycut {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }
    a.project = positional
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("Documents/MyCut/untitled.mycut"));
    a
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn load_or_create(path: &Path) -> ProjectDocument {
    if path.exists() {
        match mycut_projects::load_with_recovery(path) {
            Some((doc, _src)) => {
                if doc.schema_version == mycut_core::SCHEMA_VERSION {
                    return doc;
                }
                eprintln!(
                    "warning: project {} is version {} (supported: {}); starting fresh",
                    path.display(),
                    doc.schema_version,
                    mycut_core::SCHEMA_VERSION
                );
            }
            None => eprintln!("warning: could not load {}; starting fresh", path.display()),
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    ProjectDocument {
        schema_version: mycut_core::SCHEMA_VERSION.into(),
        project: Project::new("Untitled"),
        history: History::new(),
        saved_at_unix_ms: 0,
        app_version: env!("CARGO_PKG_VERSION").into(),
    }
}

/// Try to open the URL in the best available browser surface.
fn open_browser(url: &str) {
    if std::env::var("MYCUT_NO_OPEN").is_ok_and(|v| v == "1" || v == "true") {
        return;
    }
    if let Ok(b) = std::env::var("MYCUT_BROWSER") {
        if !b.trim().is_empty() {
            let _ = std::process::Command::new(b.trim()).arg(url).spawn();
            return;
        }
    }
    // Chromium-family: open a chromeless "app" window when possible.
    for b in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ] {
        if which(b) {
            let _ = std::process::Command::new(b)
                .arg(format!("--app={url}"))
                .spawn();
            return;
        }
    }
    if which("firefox") {
        let _ = std::process::Command::new("firefox")
            .arg("--new-window")
            .arg(url)
            .spawn();
        return;
    }
    if which("xdg-open") {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
        return;
    }
    eprintln!("info: no browser found — open {url} manually");
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p).any(|d| {
                let c = d.join(name);
                c.is_file()
            })
        })
        .unwrap_or(false)
}

fn main() {
    let args = parse_args();
    let settings = AppSettings::load(&mycut_projects::config_dir());

    // Sidecar preference: the packager sets MYCUT_FFMPEG/FFPROBE to bundled
    // copies; MYCUT_USE_SYSTEM_FFMPEG=1 undoes that (documented override).
    if std::env::var("MYCUT_USE_SYSTEM_FFMPEG").is_ok_and(|v| v == "1") {
        std::env::remove_var("MYCUT_FFMPEG");
        std::env::remove_var("MYCUT_FFPROBE");
    }

    let doc = load_or_create(&args.project);
    let config = mycut_projects::config_dir();
    let cache_dir = config.join("mycut/cache");
    let _ = std::fs::create_dir_all(&cache_dir);

    let state = AppState {
        doc: Mutex::new(doc),
        project_path: Mutex::new(args.project.clone()),
        settings: Mutex::new(settings),
        cache_dir,
        cancel: AtomicBool::new(false),
        models_cache: Mutex::new(mycut_server::state::ModelsCache::default()),
        export: mycut_server::jobs::ExportState::default(),
        conversation: Mutex::new(Vec::new()),
    };

    let server = match mycut_server::http::Server::bind(&args.host, args.port, state) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot bind {}:{}: {e}", args.host, args.port);
            std::process::exit(1);
        }
    };
    let url = match server.local_addr() {
        Ok(a) => format!("http://{}", a),
        Err(_) => format!("http://{}:{}", args.host, args.port),
    };

    println!("MyCut {} — local web runtime", env!("CARGO_PKG_VERSION"));
    println!("  project : {}", args.project.display());
    println!("  UI      : {url}");
    println!("  stop    : Ctrl-C");

    // FFmpeg tri-state up front — never fail silently later.
    let diag = mycut_server::doctor::ffmpeg_diagnostics();
    match &diag.state {
        mycut_server::doctor::FfmpegState::Ok { version } => println!(
            "  ffmpeg  : {version} ({})",
            diag.path.clone().unwrap_or_default()
        ),
        mycut_server::doctor::FfmpegState::TooOld { version, minimum } => eprintln!(
            "  ffmpeg  : WARNING {version} < required {minimum}; transitions will fail. {}",
            mycut_server::doctor::ffmpeg_message(&diag)
        ),
        mycut_server::doctor::FfmpegState::Missing => eprintln!(
            "  ffmpeg  : MISSING. {}",
            mycut_server::doctor::ffmpeg_message(&diag)
        ),
    }

    // Startup imports (before opening the browser so the snapshot is ready).
    if !args.imports.is_empty() {
        let st = server.state();
        match mycut_server::state::import_media(&st, &args.imports) {
            Ok(_) => println!("  imported: {} file(s)", args.imports.len()),
            Err(e) => eprintln!("  import failed: {}", e.message),
        }
    }

    if args.open {
        open_browser(&url);
    }

    server.serve()
}
