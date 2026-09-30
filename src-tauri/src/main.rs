//! MyCut Tauri 2 app shell: typed commands bridging the webview to the core
//! crates. State stays in Rust; the webview never sees media bytes, raw
//! paths of secrets, or the API key.
//!
//! Architecture notes (read me before editing):
//!
//! - The webview never holds the API key. It only sees a `keyStored: bool`
//!   flag and a `keyBackend: "OS keyring" | "file (0600)" | "none"` label.
//! - The secret store is a [`mycut_projects::secret::FallbackSecretStore`]
//!   that tries the OS keyring first and falls back to a 0600 file. This is
//!   the fix for the original "no-api-key" bug: on systems without a Secret
//!   Service daemon (headless CI, minimal Ubuntu, containers), the keyring
//!   silently failed and the next `get()` returned `None`. Now the key
//!   ALWAYS lands somewhere readable.
//! - The model list is cached in-memory for 10 minutes (spec §31). Changing
//!   the API key or Base URL invalidates the cache.
//! - `import_media` accepts file paths from the JS-side `tauri-plugin-dialog`
//!   picker (spec §13). The old code called a `pick_media_files()` stub that
//!   always returned `Vec::new()` — clicking Import did nothing. Removed.
//! - `ai_apply_request` returns an actionable error string when the key is
//!   missing, not the terse code `"no-api-key"` (spec §35).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use mycut_ai::{
    plan_and_apply, AiError, AIProvider, AnalysisDigest, ContextSummary, ModelInfo, NimConfig,
    NvidiaNimProvider, SourceSummary, TimelineSummary,
};
use mycut_core::{History, Project, TimeMs};
use mycut_engine::{HwChoice, RenderEngine};
use mycut_projects::{secret, ProjectDocument};
use mycut_schema::validate::PlanContext;
use serde::Serialize;
use serde_json::{json, Value};

mod settings;
use settings::{config_dir, AppSettings, SettingsDto};

struct AppState {
    doc: Mutex<ProjectDocument>,
    project_path: Mutex<PathBuf>,
    settings: Mutex<AppSettings>,
    cache_dir: PathBuf,
    cancel: AtomicBool,
    last_plan_json: Mutex<Option<String>>,
    /// In-memory model-list cache (spec §31). Short TTL; invalidated when
    /// the API key or Base URL changes.
    model_cache: Mutex<ModelCache>,
}

#[derive(Default)]
struct ModelCache {
    /// `None` = never fetched. `Some(vec)` = current list (may be empty).
    models: Option<Vec<ModelInfo>>,
    fetched_at: Option<Instant>,
    /// Snapshot of `(api_key, base_url)` used for the last fetch. If the
    /// user changes either, the cache is invalidated.
    key_fingerprint: Option<String>,
    base_url_fingerprint: Option<String>,
}

impl ModelCache {
    const TTL: Duration = Duration::from_secs(10 * 60);

    fn is_fresh_for(&self, key: &str, base_url: &str) -> bool {
        let fresh = self
            .fetched_at
            .map(|t| t.elapsed() < Self::TTL)
            .unwrap_or(false);
        let same_key = self
            .key_fingerprint
            .as_deref()
            .map(|k| k == key)
            .unwrap_or(false);
        let same_url = self
            .base_url_fingerprint
            .as_deref()
            .map(|u| u == base_url)
            .unwrap_or(false);
        fresh && same_key && same_url
    }

    fn store(&mut self, key: &str, base_url: &str, models: Vec<ModelInfo>) {
        self.models = Some(models);
        self.fetched_at = Some(Instant::now());
        self.key_fingerprint = Some(key.to_string());
        self.base_url_fingerprint = Some(base_url.to_string());
    }

    fn invalidate(&mut self) {
        self.models = None;
        self.fetched_at = None;
        self.key_fingerprint = None;
        self.base_url_fingerprint = None;
    }
}

// ---------------- snapshot DTO (unchanged, proven working) ----------------

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

// ---------------- import (spec §13: native file picker) ----------------

/// Import media by file paths. The webview opens the native file picker via
/// `tauri-plugin-dialog` from JS and passes the picked paths here (spec §13).
/// Multiple selection is supported (spec §13). Drag-and-drop also reaches
/// this command via the same path (spec §14) — the JS side resolves the
/// dropped File list to paths when running under Tauri.
#[tauri::command]
fn import_media(state: tauri::State<'_, AppState>, paths: Vec<String>) -> Result<Value, String> {
    if paths.is_empty() {
        // User cancelled the picker, or JS forgot to pass paths. Don't error
        // — return an empty result so the UI doesn't show a scary toast.
        return Ok(json!({ "imported": 0, "skipped": 0 }));
    }
    let mut doc = state.doc.lock().unwrap();
    let project_dir = state.project_path.lock().unwrap().parent().unwrap_or_default().to_path_buf();
    let engine = RenderEngine::new().map_err(|e| e.to_string())?;
    let mut imported = 0usize;
    let mut skipped: Vec<(String, String)> = Vec::new();
    for raw in paths {
        let path = PathBuf::from(&raw);
        if !path.exists() {
            skipped.push((raw, "file not found".into()));
            continue;
        }
        let probe = match mycut_engine::probe::probe(&path) {
            Ok(p) => p,
            Err(e) => {
                skipped.push((raw, format!("probe failed: {e}")));
                continue;
            }
        };
        let hash = match mycut_analysis::cache::content_hash(&path) {
            Ok(h) => h,
            Err(e) => {
                skipped.push((raw, format!("hash failed: {e}")));
                continue;
            }
        };
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
        {
            let (project, history) = (&mut doc.project, &mut doc.history);
            history.apply(project, mycut_core::Command::AddSource { source: source.clone() })
                .map_err(|e| e.to_string())?;
        }
        let kind = if probe.video_stream().is_some() { mycut_core::TrackKind::Video } else { mycut_core::TrackKind::Audio };
        {
            let (project, history) = (&mut doc.project, &mut doc.history);
            history.apply(project, mycut_core::Command::AddClip { track_kind: kind, item })
                .map_err(|e| e.to_string())?;
        }
        if let Ok(thumb) = engine.build_thumbnail_command(&path, probe.duration_ms() / 3, &project_dir.join(format!("{}.jpg", source.id)), 160) {
            let _ = std::process::Command::new(&thumb.program).args(&thumb.args).status();
        }
        if probe.video_stream().is_some() {
            if let Ok(proxy) = engine.build_proxy_command(&path, &project_dir.join(format!("{}.proxy.mp4", source.id)), mycut_engine::preset::proxy_height_for_preview()) {
                let _ = std::process::Command::new(&proxy.program).args(&proxy.args).status();
            }
        }
        imported += 1;
    }
    let path = state.project_path.lock().unwrap().clone();
    mycut_projects::save_project(&doc.project, &doc.history, &path).map_err(|e| e.to_string())?;
    Ok(json!({
        "imported": imported,
        "skipped": skipped.iter().map(|(p, why)| json!({"path": p, "reason": why})).collect::<Vec<_>>(),
    }))
}

// ---------------- AI ----------------

/// Actionable error when the API key is missing (spec §35).
const NO_KEY_MESSAGE: &str =
    "No NVIDIA NIM API key is configured. Open Settings \u{2192} AI Provider, paste your key, and press Save Key.";

fn read_api_key(state: &tauri::State<'_, AppState>) -> String {
    let dir = config_dir();
    let store = secret::default_store(&dir);
    store.get().unwrap_or_default()
}

fn key_state_inner() -> (bool, String) {
    let dir = config_dir();
    let store = secret::default_store(&dir);
    let value = store.get();
    let stored = value.is_some();
    let backend = store.backend().label();
    (stored, backend.to_string())
}

#[tauri::command]
fn set_api_key(state: tauri::State<'_, AppState>, key: String) -> Result<Value, String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("API key is empty. Paste a valid NVIDIA NIM key (starts with nvapi-).".into());
    }
    let dir = config_dir();
    let store = secret::default_store(&dir);
    store.set(trimmed).map_err(|e| e.to_string())?;
    // Invalidate the model cache — a new key may have a different model set.
    state.model_cache.lock().unwrap().invalidate();
    let (stored, backend) = key_state_inner();
    Ok(json!({ "ok": stored, "backend": backend }))
}

#[tauri::command]
fn clear_api_key(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let dir = config_dir();
    let store = secret::default_store(&dir);
    store.delete().map_err(|e| e.to_string())?;
    state.model_cache.lock().unwrap().invalidate();
    let (stored, backend) = key_state_inner();
    Ok(json!({ "ok": !stored, "backend": backend }))
}

#[tauri::command]
fn key_state(_state: tauri::State<'_, AppState>) -> Value {
    let (stored, backend) = key_state_inner();
    json!({ "stored": stored, "backend": backend })
}

#[tauri::command]
fn ai_apply_request(
    state: tauri::State<'_, AppState>,
    request: String,
) -> Result<Value, String> {
    let settings = state.settings.lock().unwrap().clone();
    let key = read_api_key(&state);
    if key.is_empty() {
        // Spec §35: actionable text, not the terse "no-api-key" code.
        return Err(NO_KEY_MESSAGE.into());
    }
    let cfg = NimConfig { api_key: key, base_url: settings.base_url.clone(), model: settings.model.clone(), ..NimConfig::default() };
    let provider = NvidiaNimProvider::new(cfg);

    let mut doc = state.doc.lock().unwrap();
    let project_dir = state.project_path.lock().unwrap().parent().unwrap_or_default().to_path_buf();

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
        transcript: Vec::new(),
        conversation: Vec::new(),
        request: request.clone(),
    };

    let schema_ctx = PlanContext {
        source_duration_s: doc.project.sources.first().map(|s| s.duration_ms as f64 / 1000.0).unwrap_or(0.0),
        transcript: Vec::new(),
        highlights: Vec::new(),
        allowed_dirs: vec![project_dir.clone(), PathBuf::from("/usr/share/mycut/luts")],
    };

    let outcome = {
        let (project, history) = (&mut doc.project, &mut doc.history);
        plan_and_apply(&provider, project, history, &schema_ctx, &summary, settings.frames_enabled, &state.cancel)
    };
    let path = state.project_path.lock().unwrap().clone();
    let _ = mycut_projects::save_project(&doc.project, &doc.history, &path);
    if !outcome.applied {
        let err_msg = outcome
            .error
            .map(|e| humanize_ai_error(&e))
            .unwrap_or_else(|| "AI request failed for an unknown reason.".into());
        return Err(err_msg);
    }
    Ok(json!({
        "summary_parts": [outcome.summary],
        "warnings": outcome.warnings,
        "repairs": outcome.repairs,
        "clarification": outcome.clarification,
    }))
}

/// Map `AiError` variants to actionable user-facing text (spec §35).
/// NEVER includes the API key, the request body, or internal stack traces.
fn humanize_ai_error(e: &AiError) -> String {
    match e {
        AiError::Connected => "Connected.".into(),
        AiError::InvalidApiKey =>
            "NVIDIA NIM rejected the API key (HTTP 401/403). Open Settings \u{2192} AI Provider and re-paste your key.".into(),
        AiError::Network =>
            "Could not reach NVIDIA NIM. Check your internet connection and Base URL.".into(),
        AiError::RateLimited =>
            "NVIDIA NIM is rate-limiting this account (HTTP 429). Wait a moment and try again.".into(),
        AiError::ModelUnavailable =>
            "The selected model is unavailable (HTTP 404). Pick another model in Settings \u{2192} Model.".into(),
        AiError::MissingCapability =>
            "The selected model lacks a required capability (structured JSON output). Pick a model that supports JSON output.".into(),
        AiError::Timeout =>
            "The request to NVIDIA NIM timed out. Try again, or pick a faster model in Settings.".into(),
        AiError::BadOutput(reason) =>
            format!("The model returned output that could not be repaired into a valid edit plan. {reason}"),
        AiError::DiscoveryUnavailable =>
            "Connected to NVIDIA NIM, but model discovery is not available for this provider. Enter a model ID manually in Settings.".into(),
        AiError::Provider(reason) =>
            format!("NVIDIA NIM returned an error: {reason}"),
    }
}

/// Test connection with step-by-step status (spec §4) + dynamic model
/// discovery (spec §5). Returns a structured result the UI renders as
/// "✓ NVIDIA NIM connection successful · ✓ Authentication successful ·
/// ✓ 37 models available".
#[tauri::command]
fn ai_test_connection(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let settings = state.settings.lock().unwrap().clone();
    let key = read_api_key(&state);
    if key.is_empty() {
        return Ok(json!({
            "ok": false,
            "step": "key",
            "state": NO_KEY_MESSAGE,
            "model_count": 0,
        }));
    }
    if settings.base_url.trim().is_empty() || !settings.base_url.starts_with("http") {
        return Ok(json!({
            "ok": false,
            "step": "base_url",
            "state": "Base URL is missing or invalid. Open Settings \u{2192} AI Provider and set it to https://integrate.api.nvidia.com/v1",
            "model_count": 0,
        }));
    }
    let cfg = NimConfig { api_key: key.clone(), base_url: settings.base_url.clone(), model: settings.model.clone(), ..NimConfig::default() };
    let provider = NvidiaNimProvider::new(cfg);

    // STEP 1+2+3: hit the discovery endpoint — this authenticates AND lists
    // models in one round-trip. 401/403 = bad key; 404 = discovery unavailable.
    match provider.list_models() {
        Ok(models) => {
            let count = models.len();
            // Cache for the model selector (spec §31).
            state
                .model_cache
                .lock()
                .unwrap()
                .store(&key, &settings.base_url, models.clone());
            Ok(json!({
                "ok": true,
                "step": "models",
                "state": format!("\u{2713} NVIDIA NIM connection successful \u{00b7} \u{2713} Authentication successful \u{00b7} \u{2713} {} models available", count),
                "model_count": count,
            }))
        }
        Err(AiError::DiscoveryUnavailable) => {
            // Provider connected and authenticated, but doesn't expose /models.
            // Fall back to manual model entry (spec §30).
            Ok(json!({
                "ok": true,
                "step": "models",
                "state": "\u{2713} NVIDIA NIM connection successful \u{00b7} \u{2713} Authentication successful \u{00b7} \u{26a0} Automatic model discovery is not available for this provider. Enter a model ID manually.",
                "model_count": 0,
                "discovery_unavailable": true,
            }))
        }
        Err(e) => {
            // Don't cache; the next call may succeed.
            Ok(json!({
                "ok": false,
                "step": "models",
                "state": humanize_ai_error(&e),
                "model_count": 0,
            }))
        }
    }
}

/// List models, cached for 10 minutes (spec §31). `refresh=true` forces a
/// fresh fetch. Search is done locally by the UI (spec §32) — no per-keystroke
/// network requests.
#[tauri::command]
fn ai_list_models(state: tauri::State<'_, AppState>, refresh: bool) -> Result<Value, String> {
    let settings = state.settings.lock().unwrap().clone();
    let key = read_api_key(&state);
    if key.is_empty() {
        return Ok(json!({
            "models": [],
            "cached": false,
            "count": 0,
            "warning": NO_KEY_MESSAGE,
        }));
    }
    // Return cached if fresh.
    if !refresh {
        let cache = state.model_cache.lock().unwrap();
        if cache.is_fresh_for(&key, &settings.base_url) {
            if let Some(models) = &cache.models {
                return Ok(json!({
                    "models": models,
                    "cached": true,
                    "count": models.len(),
                }));
            }
        }
    }
    let cfg = NimConfig { api_key: key.clone(), base_url: settings.base_url.clone(), model: settings.model.clone(), ..NimConfig::default() };
    let provider = NvidiaNimProvider::new(cfg);
    match provider.list_models() {
        Ok(models) => {
            let count = models.len();
            state
                .model_cache
                .lock()
                .unwrap()
                .store(&key, &settings.base_url, models.clone());
            Ok(json!({
                "models": models,
                "cached": false,
                "count": count,
            }))
        }
        Err(AiError::DiscoveryUnavailable) => {
            // Provider connected but doesn't expose /models. Surface a
            // warning so the UI can fall back to manual model entry (spec §30).
            Ok(json!({
                "models": [],
                "cached": false,
                "count": 0,
                "warning": "Connected to NVIDIA NIM, but model discovery is not available for this provider. Enter a model ID manually.",
            }))
        }
        Err(e) => Err(humanize_ai_error(&e)),
    }
}

/// Test the currently selected model with a minimal harmless request
/// (spec §9). Verifies: authentication, model availability, request schema,
/// response parsing. Does NOT generate a real edit plan.
#[tauri::command]
fn ai_test_model(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let settings = state.settings.lock().unwrap().clone();
    let key = read_api_key(&state);
    if key.is_empty() {
        return Ok(json!({
            "ok": false,
            "model": settings.model,
            "state": NO_KEY_MESSAGE,
        }));
    }
    if settings.model.trim().is_empty() {
        return Ok(json!({
            "ok": false,
            "model": settings.model,
            "state": "No model selected. Open Settings \u{2192} Model and pick one.",
        }));
    }
    let cfg = NimConfig { api_key: key, base_url: settings.base_url.clone(), model: settings.model.clone(), ..NimConfig::default() };
    let provider = NvidiaNimProvider::new(cfg);
    let cancel = AtomicBool::new(false);
    match provider.test_connection_cancellable(&cancel) {
        Ok(()) => Ok(json!({
            "ok": true,
            "model": settings.model,
            "state": format!("\u{2713} Model ready ({})", settings.model),
        })),
        Err(e) => Ok(json!({
            "ok": false,
            "model": settings.model,
            "state": humanize_ai_error(&e),
        })),
    }
}

// ---------------- export ----------------

#[tauri::command]
fn render_final(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let doc = state.doc.lock().unwrap();
    let project_dir = state.project_path.lock().unwrap().parent().unwrap_or_default().to_path_buf();
    write_captions_ass(&doc, &project_dir)?;
    let out = export_dir().join(format!("{}-{}.mp4", doc.project.name, mycut_core::now_unix_ms()));
    let engine = RenderEngine::new().map_err(|e| e.to_string())?;
    let graph = engine.build_export_command(&doc.project, &project_dir, &out, HwChoice::Auto).map_err(|e| e.to_string())?;
    let t0 = std::time::Instant::now();
    mycut_engine::process::run_tool(&graph.program, &graph.args, AtomicBool::new(false)).map_err(|e| e.to_string())?;
    Ok(json!({"path": out.to_string_lossy(), "seconds": t0.elapsed().as_secs_f64()}))
}

#[tauri::command]
fn render_preview_range(state: tauri::State<'_, AppState>, _start_ms: TimeMs, _end_ms: TimeMs) -> Result<Value, String> {
    let doc = state.doc.lock().unwrap();
    let project_dir = state.project_path.lock().unwrap().parent().unwrap_or_default().to_path_buf();
    write_captions_ass(&doc, &project_dir)?;
    let out = state.cache_dir.join("preview.mp4");
    let engine = RenderEngine::new().map_err(|e| e.to_string())?;
    let graph = engine.build_export_command(&doc.project, &project_dir, &out, HwChoice::None).map_err(|e| e.to_string())?;
    mycut_engine::process::run_tool(&graph.program, &graph.args, AtomicBool::new(false)).map_err(|e| e.to_string())?;
    Ok(json!({"path": out.to_string_lossy()}))
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

// ---------------- undo/redo/save ----------------

#[tauri::command]
fn undo(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let mut doc = state.doc.lock().unwrap();
    let label = {
        let (project, history) = (&mut doc.project, &mut doc.history);
        history.undo(project)
    }
    .map_err(|e| e.to_string())?;
    let path = state.project_path.lock().unwrap().clone();
    let _ = mycut_projects::save_project(&doc.project, &doc.history, &path);
    Ok(label)
}

#[tauri::command]
fn redo(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let mut doc = state.doc.lock().unwrap();
    let label = {
        let (project, history) = (&mut doc.project, &mut doc.history);
        history.redo(project)
    }
    .map_err(|e| e.to_string())?;
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

// ---------------- settings ----------------

#[tauri::command]
fn get_settings(state: tauri::State<'_, AppState>) -> SettingsDto {
    let s = state.settings.lock().unwrap().clone();
    let (key_stored, key_backend) = key_state_inner();
    SettingsDto::from_settings(&s, key_stored, &key_backend)
}

/// Apply one setting. Accepts both string and boolean values (the UI sends
/// `true`/`false` for `framesEnabled` and strings for everything else).
#[tauri::command]
fn set_setting(
    state: tauri::State<'_, AppState>,
    key: String,
    value: Value,
) -> Result<(), String> {
    let mut settings = state.settings.lock().unwrap();
    let value_str = match &value {
        Value::Bool(b) => b.to_string(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    };
    // Detect key/base_url changes — invalidate the model cache (spec §31).
    let invalidates_cache = matches!(key.as_str(), "base_url" | "baseUrl");
    settings.apply(&key, &value_str);
    settings.save().map_err(|e| e.to_string())?;
    if invalidates_cache {
        state.model_cache.lock().unwrap().invalidate();
    }
    Ok(())
}

#[tauri::command]
fn clear_cache(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let c = mycut_analysis::cache::Cache::new(&state.cache_dir, state.settings.lock().unwrap().cache_limit_bytes);
    c.clear();
    Ok(())
}

#[tauri::command]
fn cache_usage(state: tauri::State<'_, AppState>) -> Value {
    let c = mycut_analysis::cache::Cache::new(&state.cache_dir, state.settings.lock().unwrap().cache_limit_bytes);
    json!({ "bytes": c.size_bytes(), "limitBytes": state.settings.lock().unwrap().cache_limit_bytes })
}

#[tauri::command]
fn proxy_path(state: tauri::State<'_, AppState>, source_id: String) -> Result<Value, String> {
    let doc = state.doc.lock().unwrap();
    let project_dir = state.project_path.lock().unwrap().parent().unwrap_or_default().to_path_buf();
    let proxy = project_dir.join(format!("{source_id}.proxy.mp4"));
    if proxy.exists() {
        return Ok(json!({ "path": proxy.to_string_lossy() }));
    }
    let source = doc.project.sources.iter().find(|s| s.id == source_id).ok_or("no source")?;
    let media = mycut_projects::resolve_source_path(&source.rel_path, &project_dir).ok_or("media offline")?;
    let engine = RenderEngine::new().map_err(|e| e.to_string())?;
    let graph = engine.build_proxy_command(&media, &proxy, mycut_engine::preset::proxy_height_for_preview()).map_err(|e| e.to_string())?;
    mycut_engine::process::run_tool(&graph.program, &graph.args, AtomicBool::new(false)).map_err(|e| e.to_string())?;
    Ok(json!({ "path": proxy.to_string_lossy() }))
}

#[tauri::command]
fn relink_source(_state: tauri::State<'_, AppState>, _source_id: String) -> Result<(), String> {
    // Dialog-driven relink matches by content hash then filename.
    // The dialog plugin is invoked from JS; this command is a placeholder
    // until that flow is wired up (tracked in STATUS.md).
    Ok(())
}

// ---------------- catalogs (spec §19, §20, §24) ----------------
//
// Static catalog data sourced from the engine/captions crates. These are
// the same definitions the AI planner and the FFmpeg renderer use, so the
// UI can never drift from what actually executes.

#[tauri::command]
fn effect_catalog() -> Value {
    let video: Vec<Value> = mycut_engine::effects::registry()
        .iter()
        .map(|d| {
            json!({
                "def_id": d.def_id, "label": d.label, "description": d.description,
                "params": d.params.iter().map(|p| json!({
                    "name": p.name, "label": p.label, "default": p.default,
                    "min": p.min, "max": p.max, "step": p.step,
                })).collect::<Vec<_>>(),
                "kind": "video",
            })
        })
        .collect();
    json!({ "video": video, "audio": [] })
}

#[tauri::command]
fn transition_catalog() -> Value {
    json!({ "kinds": [
        {"id": "cut", "label": "Cut"},
        {"id": "fade", "label": "Fade"},
        {"id": "crossfade", "label": "Crossfade"},
        {"id": "dip_to_black", "label": "Dip to Black"},
        {"id": "dip_to_white", "label": "Dip to White"},
        {"id": "slide", "label": "Slide"},
        {"id": "push", "label": "Push"},
        {"id": "zoom", "label": "Zoom"},
        {"id": "wipe", "label": "Wipe"},
    ]})
}

#[tauri::command]
fn caption_style_catalog() -> Value {
    json!({ "styles": [
        {"id": "minimal", "label": "Minimal"},
        {"id": "gaming", "label": "Gaming"},
        {"id": "tiktok", "label": "TikTok"},
        {"id": "youtube", "label": "YouTube"},
        {"id": "cinematic", "label": "Cinematic"},
        {"id": "bold", "label": "Bold"},
        {"id": "karaoke", "label": "Karaoke"},
        {"id": "word_highlight", "label": "Word Highlight"},
        {"id": "streamer", "label": "Streamer"},
    ]})
}

#[tauri::command]
fn export_presets() -> Value {
    json!({ "presets": mycut_engine::preset::builtin_presets() })
}

// ---------------- doctor (spec §35: nothing fails silently) ----------------

#[tauri::command]
fn doctor(_state: tauri::State<'_, AppState>) -> Value {
    // FFmpeg tri-state (missing / too old / ok) — mirrors the server crate's
    // doctor module so the UI shows the same actionable message.
    let ffmpeg = ffmpeg_diagnostics();
    json!({
        "appVersion": env!("CARGO_PKG_VERSION"),
        "runtime": "tauri-2",
        "ffmpeg": ffmpeg,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FfmpegDiagDto {
    state: String,
    version: Option<String>,
    minimum: String,
    path: Option<String>,
    message: String,
    using_bundled: bool,
}

fn ffmpeg_diagnostics() -> FfmpegDiagDto {
    let min_major = 4u32;
    let min_minor = 3u32;
    let p = match mycut_engine::process::resolve_tool("ffprobe") {
        Ok(p) => p,
        Err(_) => return FfmpegDiagDto {
            state: "missing".into(),
            version: None,
            minimum: format!("{min_major}.{min_minor}"),
            path: None,
            message: "FFmpeg not found. Install it (`sudo apt install ffmpeg`), use the MyCut package (bundles FFmpeg), or set MYCUT_FFMPEG/MYCUT_FFPROBE.".into(),
            using_bundled: false,
        },
    };
    let out = std::process::Command::new(&p).arg("-version").output();
    let version = out
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.strip_prefix("ffprobe version ").and_then(|r| {
                    let first = r.split_whitespace().next().unwrap_or("");
                    let digits: String = first
                        .trim_start_matches('n')
                        .chars()
                        .take_while(|c| c.is_ascii_digit() || *c == '.')
                        .collect();
                    (!digits.is_empty()).then_some(digits)
                }))
        });
    let (maj, min) = version
        .as_deref()
        .map(|v| {
            let mut it = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
            (it.next().unwrap_or(0), it.next().unwrap_or(0))
        })
        .unwrap_or((0, 0));
    let state = if maj > min_major || (maj == min_major && min >= min_minor) {
        "ok"
    } else if version.is_some() {
        "too-old"
    } else {
        "missing"
    };
    let message = match state {
        "ok" => format!("FFmpeg {} OK ({})", version.as_deref().unwrap_or("?"), p.display()),
        "too-old" => format!(
            "FFmpeg {} found at {} is too old (need >= {}.{} for xfade/transitions). Install a newer FFmpeg or keep using the bundled one.",
            version.as_deref().unwrap_or("?"), p.display(), min_major, min_minor
        ),
        _ => format!("FFmpeg not found at {}.", p.display()),
    };
    FfmpegDiagDto {
        state: state.into(),
        version,
        minimum: format!("{min_major}.{min_minor}"),
        path: Some(p.to_string_lossy().into_owned()),
        message,
        using_bundled: std::env::var_os("MYCUT_FFMPEG").is_some(),
    }
}

// ---------------- helpers ----------------

fn export_dir() -> PathBuf {
    dirs_home().join("Videos")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}

// Process-wide once-cell so the AppState can store a Mutex<ModelCache>
// without lifetime issues — actually we use Mutex directly in AppState
// above; this is here for future extensions and to silence dead_code
// warnings on the OnceLock import.
#[allow(dead_code)]
fn _unused_once() -> &'static OnceLock<()> {
    static CELL: OnceLock<()> = OnceLock::new();
    &CELL
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
        model_cache: Mutex::new(ModelCache::default()),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            // project + import
            project_snapshot, import_media,
            // AI
            ai_apply_request, ai_test_connection, ai_list_models, ai_test_model,
            // keys + provider
            set_api_key, clear_api_key, key_state,
            // render
            render_final, render_preview_range,
            // undo/redo/save
            undo, redo, save,
            // settings + cache
            get_settings, set_setting, clear_cache, cache_usage,
            // media
            proxy_path, relink_source,
            // catalogs
            effect_catalog, transition_catalog, caption_style_catalog, export_presets,
            // diagnostics
            doctor,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
