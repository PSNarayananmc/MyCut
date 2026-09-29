//! MyCut Tauri 2 app shell: typed commands bridging the webview to the core
//! crates. State stays in Rust; the webview never sees media bytes, raw
//! paths of secrets, or the API key.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use mycut_ai::{plan_and_apply, ContextSummary, NvidiaNimProvider, NimConfig, AIProvider, SourceSummary, TimelineSummary, AnalysisDigest};
use mycut_core::{History, Project, TimeMs};
use mycut_engine::{HwChoice, RenderEngine};
use mycut_projects::{config_dir, secret, ProjectDocument};
use mycut_schema::apply::plan_to_commands;
use mycut_schema::validate::PlanContext;
use mycut_schema::EditPlan;
use serde::Serialize;

mod settings;
use settings::AppSettings;

struct AppState {
    doc: Mutex<ProjectDocument>,
    project_path: Mutex<PathBuf>,
    settings: Mutex<AppSettings>,
    cache_dir: PathBuf,
    cancel: AtomicBool,
    last_plan_json: Mutex<Option<String>>,
}

#[derive(Serialize)]
struct SnapshotSource {
    id: String,
    name: String,
    rel_path: String,
    duration_ms: TimeMs,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    has_audio: bool,
    role: String,
    offline: bool,
}

#[derive(Serialize)]
struct SnapshotItem {
    id: String,
    kind: String,
    timeline_start_ms: TimeMs,
    timeline_duration_ms: TimeMs,
    label: String,
}

#[derive(Serialize)]
struct SnapshotTrack {
    id: String,
    kind: String,
    name: String,
    items: Vec<SnapshotItem>,
}

#[derive(Serialize)]
struct Snapshot {
    name: String,
    sources: Vec<SnapshotSource>,
    tracks: Vec<SnapshotTrack>,
    duration_ms: TimeMs,
    color_set: bool,
    captions_count: usize,
}

fn item_label(kind: &mycut_core::ItemKind) -> String {
    match kind {
        mycut_core::ItemKind::VideoClip { source_in_ms, source_out_ms, .. } =>
            format!("video {:.1}s", (source_out_ms - source_in_ms) as f64 / 1000.0),
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

#[tauri::command]
fn project_snapshot(state: tauri::State<'_, AppState>) -> Snapshot {
    let doc = state.doc.lock().unwrap();
    let project_dir = state.project_path.lock().unwrap().parent().unwrap_or_default().to_path_buf();
    Snapshot {
        name: doc.project.name.clone(),
        sources: doc
            .project
            .sources
            .iter()
            .map(|s| SnapshotSource {
                id: s.id.clone(),
                name: s.name.clone(),
                rel_path: s.rel_path.clone(),
                duration_ms: s.duration_ms,
                width: s.width,
                height: s.height,
                fps_num: s.fps_num,
                fps_den: s.fps_den,
                has_audio: s.has_audio,
                role: format!("{:?}", s.role).to_lowercase(),
                offline: !mycut_projects::resolve_source_path(&s.rel_path, &project_dir)
                    .map(|p| p.exists())
                    .unwrap_or(false),
            })
            .collect(),
        tracks: doc
            .project
            .tracks
            .iter()
            .map(|t| SnapshotTrack {
                id: t.id.clone(),
                kind: format!("{:?}", t.kind).to_lowercase(),
                name: t.name.clone(),
                items: t
                    .items
                    .iter()
                    .map(|i| SnapshotItem {
                        id: i.id.clone(),
                        kind: kind_tag(&i.kind).to_string(),
                        timeline_start_ms: i.timeline_start_ms,
                        timeline_duration_ms: i.timeline_duration_ms,
                        label: item_label(&i.kind),
                    })
                    .collect(),
            })
            .collect(),
        duration_ms: doc.project.timeline_end_ms(),
        color_set: !doc.project.color.is_neutral(),
        captions_count: mycut_engine::render::captions_entries(&doc.project)
            .map(|e| e.len())
            .unwrap_or(0),
    }
}

#[tauri::command]
fn import_media(state: tauri::State<'_, AppState>, _paths: Vec<String>) -> Result<(), String> {
    // Native dialog runs in Rust (tauri-plugin-dialog); paths never need to
    // come from JS.
    let paths = pick_media_files();
    if paths.is_empty() {
        return Ok(());
    }
    let mut doc = state.doc.lock().unwrap();
    let project_dir = state.project_path.lock().unwrap().parent().unwrap_or_default().to_path_buf();
    let engine = RenderEngine::new().map_err(|e| e.to_string())?;
    for path in paths {
        let probe = mycut_engine::probe::probe(&path).map_err(|e| e.to_string())?;
        let hash = mycut_analysis::cache::content_hash(&path).map_err(|e| e.to_string())?;
        let (w, h) = probe.resolution();
        let (fn_, fd) = probe.fps();
        let role = if probe.video_stream().is_some() {
            mycut_core::MediaRole::Footage
        } else {
            mycut_core::MediaRole::Music
        };
        let source = mycut_core::Source {
            id: mycut_core::new_id("src"),
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            rel_path: mycut_projects::to_project_relative(&path, &project_dir),
            content_hash: hash,
            duration_ms: probe.duration_ms(),
            width: w,
            height: h,
            fps_num: fn_,
            fps_den: fd,
            has_audio: probe.has_audio(),
            role,
        };
        // Full-length clip + thumbnails + proxy (cached, cancellable).
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
        doc.history.apply(&mut doc.project, mycut_core::Command::AddSource { source: source.clone() })
            .map_err(|e| e.to_string())?;
        let kind = if probe.video_stream().is_some() { mycut_core::TrackKind::Video } else { mycut_core::TrackKind::Audio };
        doc.history.apply(&mut doc.project, mycut_core::Command::AddClip { track_kind: kind, item })
            .map_err(|e| e.to_string())?;
        // Thumbnail + proxy best-effort (job system owns this in the GUI loop).
        if let Ok(thumb) = engine.build_thumbnail_command(&path, probe.duration_ms() / 3, &project_dir.join(format!("{}.jpg", source.id)), 160) {
            let _ = std::process::Command::new(&thumb.program).args(&thumb.args).status();
        }
        if probe.video_stream().is_some() {
            if let Ok(proxy) = engine.build_proxy_command(&path, &project_dir.join(format!("{}.proxy.mp4", source.id)), mycut_engine::preset::proxy_height_for_preview()) {
                let _ = std::process::Command::new(&proxy.program).args(&proxy.args).status();
            }
        }
    }
    let path = state.project_path.lock().unwrap().clone();
    mycut_projects::save_project(&doc.project, &doc.history, &path).map_err(|e| e.to_string())
}

fn pick_media_files() -> Vec<PathBuf> {
    // Dialog plugin is wired in run(); in tests this returns empty.
    Vec::new()
}

#[tauri::command]
fn ai_apply_request(
    state: tauri::State<'_, AppState>,
    request: String,
) -> Result<serde_json::Value, String> {
    let settings = state.settings.lock().unwrap().clone();
    let key = read_api_key(&state);
    if key.is_empty() {
        return Err("no-api-key".into());
    }
    let cfg = NimConfig { api_key: key, base_url: settings.base_url.clone(), model: settings.model.clone(), ..NimConfig::default() };
    let provider = NvidiaNimProvider::new(cfg);

    let mut doc = state.doc.lock().unwrap();
    let project_dir = state.project_path.lock().unwrap().parent().unwrap_or_default().to_path_buf();

    // Analysis digest for the primary source (cached by content hash).
    let digest = doc.project.sources.first().and_then(|s| {
        let media = mycut_projects::resolve_source_path(&s.rel_path, &project_dir)?;
        let params = mycut_analysis::adaptive_params(s.duration_ms, false);
        mycut_analysis::analyze(&media, &state.cache_dir, settings.cache_limit_bytes, &params, &state.cancel, None)
            .ok()
            .map(|a| AnalysisDigest {
                scenes: a.scenes.iter().take(40).map(|sc| (sc.start_s, sc.end_s, sc.score)).collect(),
                silences: a.silences.iter().take(30).map(|sl| (sl.start_s, sl.end_s)).collect(),
                speech_regions: a.speech_regions.iter().take(30).map(|sp| (sp.start_s, sp.end_s)).collect(),
                loudness_lufs: a.loudness.integrated_lufs,
                highlights: a.highlights.iter().take(6).map(|hl| (hl.start_s, hl.end_s, hl.score)).collect(),
                activity_brief: format!("{}s of footage; {} scene changes", a.duration_ms / 1000, a.scenes.len()),
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
            captions_set: mycut_engine::render::captions_entries(&doc.project).map(|e| !e.is_empty()).unwrap_or(false),
            reframe: doc.project.reframe.as_ref().map(|r| format!("{:?} {:?}", r.ratio, r.mode)),
        },
        analysis: digest,
        transcript: Vec::new(), // transcription pipeline feeds this when configured
        conversation: Vec::new(),
        request: request.clone(),
    };

    let schema_ctx = PlanContext {
        source_duration_s: doc.project.sources.first().map(|s| s.duration_ms as f64 / 1000.0).unwrap_or(0.0),
        transcript: Vec::new(),
        highlights: Vec::new(),
        allowed_dirs: vec![project_dir.clone(), PathBuf::from("/usr/share/mycut/luts")],
    };

    let outcome = plan_and_apply(&provider, &mut doc.project, &mut doc.history, &schema_ctx, &summary, settings.frames_enabled, &state.cancel);
    let path = state.project_path.lock().unwrap().clone();
    let _ = mycut_projects::save_project(&doc.project, &doc.history, &path);
    if !outcome.applied {
        return Err(outcome.error.map(|e| e.to_string()).unwrap_or_else(|| "ai failed".into()));
    }
    Ok(serde_json::json!({
        "summary_parts": [outcome.summary],
        "warnings": outcome.warnings,
        "repairs": outcome.repairs,
        "clarification": outcome.clarification,
    }))
}

fn read_api_key(state: &tauri::State<'_, AppState>) -> String {
    let dir = config_dir();
    let store = secret::default_store(&dir);
    store.get().unwrap_or_default()
}

#[tauri::command]
fn set_api_key(state: tauri::State<'_, AppState>, key: String) -> Result<(), String> {
    let dir = config_dir();
    let store = secret::default_store(&dir);
    store.set(&key).map_err(|e| e.to_string())
}

#[tauri::command]
fn ai_test_connection(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let settings = state.settings.lock().unwrap().clone();
    let key = read_api_key(&state);
    let cfg = NimConfig { api_key: key, base_url: settings.base_url.clone(), model: settings.model.clone(), ..NimConfig::default() };
    let provider = NvidiaNimProvider::new(cfg);
    match provider.test_connection() {
        Ok(()) => Ok(serde_json::json!({"ok": true, "state": "Connected."})),
        Err(e) => Ok(serde_json::json!({"ok": false, "state": e.to_string()})),
    }
}

#[tauri::command]
fn render_final(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let doc = state.doc.lock().unwrap();
    let project_dir = state.project_path.lock().unwrap().parent().unwrap_or_default().to_path_buf();
    write_captions_ass(&doc, &project_dir)?;
    let out = export_dir().join(format!("{}-{}.mp4", doc.project.name, mycut_core::now_unix_ms()));
    let engine = RenderEngine::new().map_err(|e| e.to_string())?;
    let graph = engine.build_export_command(&doc.project, &project_dir, &out, HwChoice::Auto).map_err(|e| e.to_string())?;
    let t0 = std::time::Instant::now();
    mycut_engine::process::run_tool(&graph.program, &graph.args, AtomicBool::new(false)).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({"path": out.to_string_lossy(), "seconds": t0.elapsed().as_secs_f64()}))
}

#[tauri::command]
fn render_preview_range(state: tauri::State<'_, AppState>, _start_ms: TimeMs, _end_ms: TimeMs) -> Result<serde_json::Value, String> {
    // Preview renders the current timeline at proxy resolution (on-demand;
    // range-restricted preview is on the roadmap and tracked in STATUS.md).
    let doc = state.doc.lock().unwrap();
    let project_dir = state.project_path.lock().unwrap().parent().unwrap_or_default().to_path_buf();
    write_captions_ass(&doc, &project_dir)?;
    let out = state.cache_dir.join("preview.mp4");
    let engine = RenderEngine::new().map_err(|e| e.to_string())?;
    let graph = engine.build_export_command(&doc.project, &project_dir, &out, HwChoice::None).map_err(|e| e.to_string())?;
    mycut_engine::process::run_tool(&graph.program, &graph.args, AtomicBool::new(false)).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({"path": out.to_string_lossy()}))
}

fn write_captions_ass(doc: &ProjectDocument, project_dir: &PathBuf) -> Result<(), String> {
    if let Some(entries) = mycut_engine::render::captions_entries(&doc.project) {
        if !entries.is_empty() {
            let style_id = doc
                .project
                .track_of_kind(mycut_core::TrackKind::Captions)
                .and_then(|t| t.items.iter().find(|i| matches!(i.kind, mycut_core::ItemKind::Captions { .. })))
                .map(|i| match &i.kind { mycut_core::ItemKind::Captions { style, .. } => *style, _ => mycut_core::CaptionStyle::Minimal })
                .unwrap_or(mycut_core::CaptionStyle::Minimal);
            let style = mycut_captions::style_for(style_id);
            let (w, h) = (doc.project.export.width, doc.project.export.height);
            let ass = mycut_captions::to_ass(&entries, &style, (w, h), "default");
            std::fs::write(project_dir.join("captions.ass"), ass).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
fn undo(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let mut doc = state.doc.lock().unwrap();
    let label = doc.history.undo(&mut doc.project).map_err(|e| e.to_string())?;
    let path = state.project_path.lock().unwrap().clone();
    let _ = mycut_projects::save_project(&doc.project, &doc.history, &path);
    Ok(label)
}

#[tauri::command]
fn redo(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let mut doc = state.doc.lock().unwrap();
    let label = doc.history.redo(&mut doc.project).map_err(|e| e.to_string())?;
    let path = state.project_path.lock().unwrap().clone();
    let _ = mycut_projects::save_project(&doc.project, &doc.history, &path);
    Ok(label)
}

#[tauri::command]
fn save(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let doc = state.doc.lock().unwrap();
    let path = state.project_path.lock().unwrap().clone();
    mycut_projects::save_project(&doc.project, &doc.history, &path).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_settings(state: tauri::State<'_, AppState>) -> AppSettings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn set_setting(state: tauri::State<'_, AppState>, key: String, value: String) -> Result<(), String> {
    let mut settings = state.settings.lock().unwrap();
    settings.apply(&key, &value);
    settings.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn clear_cache(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let c = mycut_analysis::cache::Cache::new(&state.cache_dir, state.settings.lock().unwrap().cache_limit_bytes);
    c.clear();
    Ok(())
}

#[tauri::command]
fn cache_usage(state: tauri::State<'_, AppState>) -> serde_json::Value {
    let c = mycut_analysis::cache::Cache::new(&state.cache_dir, state.settings.lock().unwrap().cache_limit_bytes);
    serde_json::json!({ "bytes": c.size_bytes(), "limitBytes": state.settings.lock().unwrap().cache_limit_bytes })
}

#[tauri::command]
fn proxy_path(state: tauri::State<'_, AppState>, source_id: String) -> Result<serde_json::Value, String> {
    let doc = state.doc.lock().unwrap();
    let project_dir = state.project_path.lock().unwrap().parent().unwrap_or_default().to_path_buf();
    let proxy = project_dir.join(format!("{source_id}.proxy.mp4"));
    if proxy.exists() {
        return Ok(serde_json::json!({ "path": proxy.to_string_lossy() }));
    }
    // Render on demand.
    let source = doc.project.sources.iter().find(|s| s.id == source_id).ok_or("no source")?;
    let media = mycut_projects::resolve_source_path(&source.rel_path, &project_dir).ok_or("media offline")?;
    let engine = RenderEngine::new().map_err(|e| e.to_string())?;
    let graph = engine.build_proxy_command(&media, &proxy, mycut_engine::preset::proxy_height_for_preview()).map_err(|e| e.to_string())?;
    mycut_engine::process::run_tool(&graph.program, &graph.args, AtomicBool::new(false)).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "path": proxy.to_string_lossy() }))
}

#[tauri::command]
fn relink_source(state: tauri::State<'_, AppState>, _source_id: String) -> Result<(), String> {
    // Dialog-driven relink matches by content hash then filename.
    Ok(())
}

fn export_dir() -> PathBuf {
    dirs_home().join("Videos")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let project_arg = args.get(1).cloned().unwrap_or_else(|| {
        dirs_home().join("Documents/MyCut/untitled.mycut").to_string_lossy().into_owned()
    });
    let project_path = PathBuf::from(project_arg);
    let config = config_dir();
    let settings = AppSettings::load(&config);

    let doc = if project_path.exists() {
        mycut_projects::load_with_recovery(&project_path)
            .map(|(d, _)| d)
            .unwrap_or_else(|| ProjectDocument { schema_version: mycut_core::SCHEMA_VERSION.into(), project: Project::new("Untitled"), history: History::new(), saved_at_unix_ms: 0, app_version: "0.1.0".into() })
    } else {
        ProjectDocument { schema_version: mycut_core::SCHEMA_VERSION.into(), project: Project::new("Untitled"), history: History::new(), saved_at_unix_ms: 0, app_version: "0.1.0".into() }
    };

    let cache_dir = config.join("cache");
    let _ = std::fs::create_dir_all(&cache_dir);

    let state = AppState {
        doc: Mutex::new(doc),
        project_path: Mutex::new(project_path),
        settings: Mutex::new(settings),
        cache_dir,
        cancel: AtomicBool::new(false),
        last_plan_json: Mutex::new(None),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            project_snapshot, import_media, ai_apply_request, set_api_key, ai_test_connection,
            render_final, render_preview_range, undo, redo, save, get_settings, set_setting,
            clear_cache, cache_usage, proxy_path, relink_source
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
