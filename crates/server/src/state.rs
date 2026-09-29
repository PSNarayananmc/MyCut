//! Application state and the typed command implementations. These mirror
//! `src-tauri/src/main.rs` one-for-one (same behavior, same persistence,
//! same secret store) minus Tauri types; the Tauri shell and this runtime
//! are two transports over one command layer.
//!
//! Extra commands beyond the Tauri surface: `doctor` (FFmpeg/storage
//! diagnostics, tri-state reporting) and `import_media` takes explicit
//! paths (browsers cannot hand the server a native file dialog).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use mycut_ai::{
    plan_and_apply, AnalysisDigest, ContextSummary, NimConfig, NvidiaNimProvider, SourceSummary,
    TimelineSummary,
};
use mycut_engine::{HwChoice, RenderEngine};
use mycut_projects::{config_dir, secret, ProjectDocument};
use mycut_schema::validate::PlanContext;
use serde_json::{json, Value};

use crate::doctor;

/// Same settings schema/file as the Tauri shell (`~/.config/mycut/settings.json`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub base_url: String,
    pub model: String,
    pub frames_enabled: bool,
    pub profile: String,
    pub cache_limit_bytes: u64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            base_url: "https://integrate.api.nvidia.com/v1".into(),
            model: "meta/llama-3.1-8b-instruct".into(),
            frames_enabled: false,
            profile: "low".into(),
            cache_limit_bytes: 10 * 1024 * 1024 * 1024 / 2,
        }
    }
}

impl AppSettings {
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join("mycut/settings.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let dir = config_dir().join("mycut");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("settings.json"), serde_json::to_vec_pretty(self)?)
    }

    pub fn apply(&mut self, key: &str, value: &str) {
        match key {
            "base_url" => self.base_url = value.to_string(),
            "model" => self.model = value.to_string(),
            "profile" => self.profile = value.to_string(),
            "frames_enabled" => self.frames_enabled = value == "true" || value == "1",
            "cache_limit_bytes" => {
                if let Ok(v) = value.parse() {
                    self.cache_limit_bytes = v;
                }
            }
            _ => {}
        }
    }
}

pub struct AppState {
    pub doc: Mutex<ProjectDocument>,
    pub project_path: Mutex<PathBuf>,
    pub settings: Mutex<AppSettings>,
    pub cache_dir: PathBuf,
    pub cancel: AtomicBool,
}

/// Errors returned to the browser as `{"error": ...}` with a status code.
#[derive(Debug)]
pub struct CommandError {
    pub status: u16,
    pub message: String,
}

impl CommandError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

type CmdResult = Result<Value, CommandError>;

fn project_dir(state: &AppState) -> PathBuf {
    state
        .project_path
        .lock()
        .unwrap()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn save_state(state: &AppState, doc: &ProjectDocument) -> Result<(), CommandError> {
    let path = state.project_path.lock().unwrap().clone();
    mycut_projects::save_project(&doc.project, &doc.history, &path)
        .map_err(|e| CommandError::new(500, e.to_string()))
}

fn read_api_key() -> String {
    let dir = config_dir();
    let store = secret::default_store(&dir);
    store.get().unwrap_or_default()
}

fn nim_provider(state: &AppState) -> NvidiaNimProvider {
    let settings = state.settings.lock().unwrap().clone();
    let key = read_api_key();
    NvidiaNimProvider::new(NimConfig {
        api_key: key,
        base_url: settings.base_url,
        model: settings.model,
        ..NimConfig::default()
    })
}

// ---------- snapshot serializers (same JSON as the Tauri shell) ----------

fn item_label(kind: &mycut_core::ItemKind) -> String {
    match kind {
        mycut_core::ItemKind::VideoClip {
            source_in_ms,
            source_out_ms,
            ..
        } => format!(
            "video {:.1}s",
            (source_out_ms - source_in_ms) as f64 / 1000.0
        ),
        mycut_core::ItemKind::AudioClip { .. } => "audio".into(),
        mycut_core::ItemKind::Text { text, .. } => text.chars().take(24).collect(),
        mycut_core::ItemKind::Captions { .. } => "captions".into(),
    }
}

fn kind_tag(kind: &mycut_core::ItemKind) -> &'static str {
    match kind {
        mycut_core::ItemKind::VideoClip { .. } => "video",
        mycut_core::ItemKind::AudioClip { .. } => "audio",
        mycut_core::ItemKind::Text { .. } => "text",
        mycut_core::ItemKind::Captions { .. } => "captions",
    }
}

// ---------- commands ----------

pub fn project_snapshot(state: &AppState) -> CmdResult {
    let doc = state.doc.lock().unwrap();
    let pdir = project_dir(state);
    let sources: Vec<Value> = doc
        .project
        .sources
        .iter()
        .map(|s| {
            json!({
                "id": s.id, "name": s.name, "rel_path": s.rel_path,
                "duration_ms": s.duration_ms, "width": s.width, "height": s.height,
                "fps_num": s.fps_num, "fps_den": s.fps_den, "has_audio": s.has_audio,
                "role": format!("{:?}", s.role).to_lowercase(),
                "offline": !mycut_projects::resolve_source_path(&s.rel_path, &pdir)
                    .map(|p| p.exists()).unwrap_or(false),
            })
        })
        .collect();
    let tracks: Vec<Value> = doc
        .project
        .tracks
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "kind": format!("{:?}", t.kind).to_lowercase(),
                "name": t.name,
                "items": t.items.iter().map(|i| json!({
                    "id": i.id,
                    "kind": kind_tag(&i.kind),
                    "timeline_start_ms": i.timeline_start_ms,
                    "timeline_duration_ms": i.timeline_duration_ms,
                    "label": item_label(&i.kind),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({
        "name": doc.project.name,
        "sources": sources,
        "tracks": tracks,
        "duration_ms": doc.project.timeline_end_ms(),
        "color_set": !doc.project.color.is_neutral(),
        "captions_count": mycut_engine::render::captions_entries(&doc.project)
            .map(|e| e.len()).unwrap_or(0),
    }))
}

pub fn import_media(state: &AppState, paths: &[String]) -> CmdResult {
    if paths.is_empty() {
        return Err(CommandError::new(
            400,
            "no paths given — the local web runtime takes explicit file paths \
             (the Tauri build has the native file dialog)",
        ));
    }
    let mut doc = state.doc.lock().unwrap();
    let pdir = project_dir(state);
    let engine =
        RenderEngine::new().map_err(|e| CommandError::new(500, ffmpeg_context(&e.to_string())))?;
    for raw in paths {
        let path = PathBuf::from(raw);
        if !path.is_file() {
            return Err(CommandError::new(400, format!("file not found: {raw}")));
        }
        let probe = mycut_engine::probe::probe(&path)
            .map_err(|e| CommandError::new(500, ffmpeg_context(&e.to_string())))?;
        let hash = mycut_analysis::cache::content_hash(&path)
            .map_err(|e| CommandError::new(500, e.to_string()))?;
        let (w, h) = probe.resolution();
        let (fn_, fd) = probe.fps();
        let role = if probe.video_stream().is_some() {
            mycut_core::MediaRole::Footage
        } else {
            mycut_core::MediaRole::Music
        };
        let source = mycut_core::Source {
            id: mycut_core::new_id("src"),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            rel_path: mycut_projects::to_project_relative(&path, &pdir),
            content_hash: hash,
            duration_ms: probe.duration_ms(),
            width: w,
            height: h,
            fps_num: fn_,
            fps_den: fd,
            has_audio: probe.has_audio(),
            role,
        };
        let item = mycut_core::Item::new(
            mycut_core::ItemKind::VideoClip {
                source_id: source.id.clone(),
                source_in_ms: 0,
                source_out_ms: source.duration_ms,
                speed: 1.0,
            },
            0,
            source.duration_ms,
        );
        let ProjectDocument {
            project, history, ..
        } = &mut *doc;
        history
            .apply(
                project,
                mycut_core::Command::AddSource {
                    source: source.clone(),
                },
            )
            .map_err(|e| CommandError::new(500, e.to_string()))?;
        let kind = if probe.video_stream().is_some() {
            mycut_core::TrackKind::Video
        } else {
            mycut_core::TrackKind::Audio
        };
        history
            .apply(
                project,
                mycut_core::Command::AddClip {
                    track_kind: kind,
                    item,
                },
            )
            .map_err(|e| CommandError::new(500, e.to_string()))?;
        // Thumbnail + proxy best-effort, same as the Tauri shell.
        if let Ok(thumb) = engine.build_thumbnail_command(
            &path,
            probe.duration_ms() / 3,
            &pdir.join(format!("{}.jpg", source.id)),
            160,
        ) {
            let _ = std::process::Command::new(&thumb.program)
                .args(&thumb.args)
                .status();
        }
        if probe.video_stream().is_some() {
            if let Ok(proxy) = engine.build_proxy_command(
                &path,
                &pdir.join(format!("{}.proxy.mp4", source.id)),
                mycut_engine::preset::proxy_height_for_preview(),
            ) {
                let _ = std::process::Command::new(&proxy.program)
                    .args(&proxy.args)
                    .status();
            }
        }
    }
    save_state(state, &doc)?;
    Ok(json!({"ok": true}))
}

/// Wrap engine failures with FFmpeg context so the UI can show an
/// actionable message instead of a bare tool error.
fn ffmpeg_context(msg: &str) -> String {
    let d = doctor::ffmpeg_diagnostics();
    match d.state {
        doctor::FfmpegState::Missing => format!(
            "{msg}\nFFmpeg was not found. Install it (e.g. `sudo apt install ffmpeg`) \
             or use a package that bundles it, or set MYCUT_FFMPEG."
        ),
        doctor::FfmpegState::TooOld { version, minimum } => format!(
            "{msg}\nFFmpeg {version} was found but MyCut requires >= {minimum} \
             (for xfade/transitions). Install a newer FFmpeg or use the bundled one."
        ),
        _ => msg.to_string(),
    }
}

pub fn ai_apply_request(state: &AppState, request: &str) -> CmdResult {
    if read_api_key().is_empty() {
        return Err(CommandError::new(400, "no-api-key"));
    }
    let provider = nim_provider(state);
    let mut doc = state.doc.lock().unwrap();
    let pdir = project_dir(state);

    let digest = doc.project.sources.first().and_then(|s| {
        let media = mycut_projects::resolve_source_path(&s.rel_path, &pdir)?;
        let params = mycut_analysis::adaptive_params(s.duration_ms, false);
        mycut_analysis::analyze(
            &media,
            &state.cache_dir,
            state.settings.lock().unwrap().cache_limit_bytes,
            &params,
            &state.cancel,
            None,
        )
        .ok()
        .map(|a| AnalysisDigest {
            scenes: a
                .scenes
                .iter()
                .take(40)
                .map(|sc| (sc.start_s, sc.end_s, sc.score))
                .collect(),
            silences: a
                .silences
                .iter()
                .take(30)
                .map(|sl| (sl.start_s, sl.end_s))
                .collect(),
            speech_regions: a
                .speech_regions
                .iter()
                .take(30)
                .map(|sp| (sp.start_s, sp.end_s))
                .collect(),
            loudness_lufs: a.loudness.integrated_lufs,
            highlights: a
                .highlights
                .iter()
                .take(6)
                .map(|hl| (hl.start_s, hl.end_s, hl.score))
                .collect(),
            activity_brief: format!(
                "{}s of footage; {} scene changes",
                a.duration_ms / 1000,
                a.scenes.len()
            ),
        })
    });

    let summary = ContextSummary {
        sources: doc
            .project
            .sources
            .iter()
            .map(|s| SourceSummary {
                name: s.name.clone(),
                role: format!("{:?}", s.role).to_lowercase(),
                duration_s: s.duration_ms as f64 / 1000.0,
                resolution: format!("{}x{}", s.width, s.height),
                fps: s.fps(),
                has_audio: s.has_audio,
            })
            .collect(),
        timeline: TimelineSummary {
            clips: doc.project.tracks.iter().map(|t| t.items.len()).sum(),
            duration_s: doc.project.timeline_end_ms() as f64 / 1000.0,
            effects: doc
                .project
                .tracks
                .iter()
                .flat_map(|t| t.items.iter())
                .flat_map(|i| i.effects.iter().map(|e| e.def_id.clone()))
                .collect(),
            color_set: !doc.project.color.is_neutral(),
            captions_set: mycut_engine::render::captions_entries(&doc.project)
                .map(|e| !e.is_empty())
                .unwrap_or(false),
            reframe: doc
                .project
                .reframe
                .as_ref()
                .map(|r| format!("{:?} {:?}", r.ratio, r.mode)),
        },
        analysis: digest,
        transcript: Vec::new(),
        conversation: Vec::new(),
        request: request.to_string(),
    };

    let schema_ctx = PlanContext {
        source_duration_s: doc
            .project
            .sources
            .first()
            .map(|s| s.duration_ms as f64 / 1000.0)
            .unwrap_or(0.0),
        transcript: Vec::new(),
        highlights: Vec::new(),
        allowed_dirs: vec![pdir.clone(), PathBuf::from("/usr/share/mycut/luts")],
    };

    let ProjectDocument {
        project, history, ..
    } = &mut *doc;
    let outcome = plan_and_apply(
        &provider,
        project,
        history,
        &schema_ctx,
        &summary,
        state.settings.lock().unwrap().frames_enabled,
        &state.cancel,
    );
    save_state(state, &doc)?;
    if !outcome.applied {
        return Err(CommandError::new(
            502,
            outcome
                .error
                .map(|e| e.to_string())
                .unwrap_or_else(|| "ai failed".into()),
        ));
    }
    Ok(json!({
        "summary_parts": [outcome.summary],
        "warnings": outcome.warnings,
        "repairs": outcome.repairs,
        "clarification": outcome.clarification,
    }))
}

pub fn set_api_key(key: &str) -> CmdResult {
    let dir = config_dir();
    let store = secret::default_store(&dir);
    store
        .set(key)
        .map_err(|e| CommandError::new(500, e.to_string()))?;
    Ok(json!({"ok": true}))
}

pub fn ai_test_connection(state: &AppState) -> CmdResult {
    let provider = nim_provider(state);
    match provider.test_connection_cancellable(&state.cancel) {
        Ok(()) => Ok(json!({"ok": true, "state": "Connected."})),
        Err(e) => Ok(json!({"ok": false, "state": e.to_string()})),
    }
}

pub fn write_captions_ass(doc: &ProjectDocument, pdir: &Path) -> Result<(), CommandError> {
    if let Some(entries) = mycut_engine::render::captions_entries(&doc.project) {
        if !entries.is_empty() {
            let style_id = doc
                .project
                .track_of_kind(mycut_core::TrackKind::Captions)
                .and_then(|t| {
                    t.items
                        .iter()
                        .find(|i| matches!(i.kind, mycut_core::ItemKind::Captions { .. }))
                })
                .map(|i| match &i.kind {
                    mycut_core::ItemKind::Captions { style, .. } => *style,
                    _ => mycut_core::CaptionStyle::Minimal,
                })
                .unwrap_or(mycut_core::CaptionStyle::Minimal);
            let style = mycut_captions::style_for(style_id);
            let (w, h) = (doc.project.export.width, doc.project.export.height);
            let ass = mycut_captions::to_ass(&entries, &style, (w, h), "default");
            std::fs::write(pdir.join("captions.ass"), ass)
                .map_err(|e| CommandError::new(500, e.to_string()))?;
        }
    }
    Ok(())
}

pub fn export_dir() -> PathBuf {
    let dir = if let Ok(d) = std::env::var("MYCUT_EXPORT_DIR") {
        PathBuf::from(d)
    } else {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join("Videos"))
            .unwrap_or_else(|_| PathBuf::from("."))
    };
    // First-run friendliness: create the export dir if absent (best effort;
    // ffmpeg reports a clear error if the FS truly refuses).
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn render_final(state: &AppState) -> CmdResult {
    let doc = state.doc.lock().unwrap();
    let pdir = project_dir(state);
    write_captions_ass(&doc, &pdir)?;
    let out = export_dir().join(format!(
        "{}-{}.mp4",
        doc.project.name,
        mycut_core::now_unix_ms()
    ));
    let engine =
        RenderEngine::new().map_err(|e| CommandError::new(500, ffmpeg_context(&e.to_string())))?;
    let graph = engine
        .build_export_command(&doc.project, &pdir, &out, HwChoice::Auto)
        .map_err(|e| CommandError::new(500, e.to_string()))?;
    let t0 = std::time::Instant::now();
    mycut_engine::process::run_tool(
        &graph.program,
        &graph.args,
        std::sync::Arc::new(AtomicBool::new(false)),
    )
    .map_err(|e| CommandError::new(500, ffmpeg_context(&e.to_string())))?;
    Ok(json!({"path": out.to_string_lossy(), "seconds": t0.elapsed().as_secs_f64()}))
}

pub fn render_preview_range(state: &AppState) -> CmdResult {
    let doc = state.doc.lock().unwrap();
    let pdir = project_dir(state);
    write_captions_ass(&doc, &pdir)?;
    let out = state.cache_dir.join("preview.mp4");
    let engine =
        RenderEngine::new().map_err(|e| CommandError::new(500, ffmpeg_context(&e.to_string())))?;
    let graph = engine
        .build_export_command(&doc.project, &pdir, &out, HwChoice::None)
        .map_err(|e| CommandError::new(500, e.to_string()))?;
    mycut_engine::process::run_tool(
        &graph.program,
        &graph.args,
        std::sync::Arc::new(AtomicBool::new(false)),
    )
    .map_err(|e| CommandError::new(500, ffmpeg_context(&e.to_string())))?;
    Ok(json!({"path": out.to_string_lossy()}))
}

pub fn undo(state: &AppState) -> CmdResult {
    let mut doc = state.doc.lock().unwrap();
    let label = {
        let ProjectDocument {
            project, history, ..
        } = &mut *doc;
        history.undo(project)
    }
    .map_err(|e| CommandError::new(400, e.to_string()))?;
    save_state(state, &doc)?;
    Ok(json!({"label": label}))
}

pub fn redo(state: &AppState) -> CmdResult {
    let mut doc = state.doc.lock().unwrap();
    let label = {
        let ProjectDocument {
            project, history, ..
        } = &mut *doc;
        history.redo(project)
    }
    .map_err(|e| CommandError::new(400, e.to_string()))?;
    save_state(state, &doc)?;
    Ok(json!({"label": label}))
}

pub fn save(state: &AppState) -> CmdResult {
    let doc = state.doc.lock().unwrap();
    save_state(state, &doc)?;
    Ok(json!({"ok": true}))
}

pub fn get_settings(state: &AppState) -> CmdResult {
    let s = state.settings.lock().unwrap().clone();
    Ok(json!({
        "baseUrl": s.base_url, "model": s.model,
        "framesEnabled": s.frames_enabled, "profile": s.profile,
    }))
}

pub fn set_setting(state: &AppState, key: &str, value: &str) -> CmdResult {
    {
        let mut settings = state.settings.lock().unwrap();
        settings.apply(key, value);
        settings
            .save()
            .map_err(|e| CommandError::new(500, e.to_string()))?;
    }
    Ok(json!({"ok": true}))
}

pub fn clear_cache(state: &AppState) -> CmdResult {
    let c = mycut_analysis::cache::Cache::new(
        &state.cache_dir,
        state.settings.lock().unwrap().cache_limit_bytes,
    );
    c.clear();
    Ok(json!({"ok": true}))
}

pub fn cache_usage(state: &AppState) -> CmdResult {
    let c = mycut_analysis::cache::Cache::new(
        &state.cache_dir,
        state.settings.lock().unwrap().cache_limit_bytes,
    );
    Ok(
        json!({"bytes": c.size_bytes(), "limitBytes": state.settings.lock().unwrap().cache_limit_bytes}),
    )
}

pub fn proxy_path(state: &AppState, source_id: &str) -> CmdResult {
    let doc = state.doc.lock().unwrap();
    let pdir = project_dir(state);
    let proxy = pdir.join(format!("{source_id}.proxy.mp4"));
    if proxy.exists() {
        return Ok(json!({"path": proxy.to_string_lossy()}));
    }
    let source = doc
        .project
        .sources
        .iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| CommandError::new(404, "no source"))?;
    let media = mycut_projects::resolve_source_path(&source.rel_path, &pdir)
        .ok_or_else(|| CommandError::new(409, "media offline"))?;
    let engine =
        RenderEngine::new().map_err(|e| CommandError::new(500, ffmpeg_context(&e.to_string())))?;
    let graph = engine
        .build_proxy_command(
            &media,
            &proxy,
            mycut_engine::preset::proxy_height_for_preview(),
        )
        .map_err(|e| CommandError::new(500, e.to_string()))?;
    mycut_engine::process::run_tool(
        &graph.program,
        &graph.args,
        std::sync::Arc::new(AtomicBool::new(false)),
    )
    .map_err(|e| CommandError::new(500, ffmpeg_context(&e.to_string())))?;
    Ok(json!({"path": proxy.to_string_lossy()}))
}

pub fn relink_source(state: &AppState, source_id: &str) -> CmdResult {
    // Matches the Tauri shell: relink by content hash, then filename, is a
    // stub until the relink dialog ships (tracked in STATUS.md).
    let doc = state.doc.lock().unwrap();
    let pdir = project_dir(state);
    let Some(source) = doc.project.sources.iter().find(|s| s.id == source_id) else {
        return Err(CommandError::new(404, "no source"));
    };
    if mycut_projects::resolve_source_path(&source.rel_path, &pdir)
        .map(|p| p.exists())
        .unwrap_or(false)
    {
        return Ok(json!({"ok": true, "relinked": false}));
    }
    Err(CommandError::new(
        501,
        "relink dialog not implemented yet — see STATUS.md (media relink is partial)",
    ))
}

pub fn doctor_json(state: &AppState) -> CmdResult {
    Ok(doctor::report(state))
}
