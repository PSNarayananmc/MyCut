//! Manual editing commands: thin, validated wrappers over `mycut_core`
//! commands so the timeline UI mutates the project through the same
//! history/undo layer the AI uses (one undo stack for everything).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mycut_core::{
    Command, EffectInstance, Item, ItemKind, ParamValue, Position, ReframeMode,
    ReframeSettings, TextKind, Track, TrackKind, TransitionKind, TransitionSetting,
};
use mycut_projects::ProjectDocument;
use serde_json::{json, Value};

use crate::state::{project_dir, save_state, AppState, CommandError};

type CmdResult = Result<Value, CommandError>;

fn ce(status: u16, msg: impl Into<String>) -> CommandError {
    CommandError::new(status, msg)
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

fn ms(v: &Value, key: &str) -> Result<i64, CommandError> {
    v.get(key)
        .and_then(Value::as_f64)
        .map(|f| f.round() as i64)
        .ok_or_else(|| ce(400, format!("missing numeric field: {key}")))
}

fn str_field(v: &Value, key: &str) -> Result<String, CommandError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ce(400, format!("missing string field: {key}")))
}

fn apply_cmd(state: &AppState, cmd: Command) -> CmdResult {
    let mut doc = state.doc.lock().unwrap();
    let ProjectDocument {
        project, history, ..
    } = &mut *doc;
    history
        .apply(project, cmd)
        .map_err(|e| ce(400, e.to_string()))?;
    save_state(state, &doc)?;
    Ok(json!({"ok": true}))
}

// ---------------- registries (real data for the panels) ----------------

/// Video + audio effect registries for the Effects panel (names, params,
/// ranges — everything the UI needs to build real sliders).
pub fn effect_catalog() -> CmdResult {
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
    let audio: Vec<Value> = mycut_engine::audiofx::registry()
        .iter()
        .map(|d| {
            json!({
                "def_id": d.def_id, "label": d.label,
                "params": d.params.iter().map(|p| json!({
                    "name": p.name, "label": p.label, "default": p.default,
                    "min": p.min, "max": p.max, "step": 0.01,
                })).collect::<Vec<_>>(),
                "kind": "audio",
            })
        })
        .collect();
    Ok(json!({"video": video, "audio": audio}))
}

/// Transition kinds + the caption styles, straight from the core enums.
pub fn transition_catalog() -> CmdResult {
    let kinds = [
        ("cut", "Cut"),
        ("fade", "Fade"),
        ("crossfade", "Crossfade"),
        ("dip_to_black", "Dip to Black"),
        ("dip_to_white", "Dip to White"),
        ("slide", "Slide"),
        ("push", "Push"),
        ("zoom", "Zoom"),
        ("wipe", "Wipe"),
    ];
    Ok(json!({
        "kinds": kinds.iter().map(|(id, label)| json!({"id": id, "label": label})).collect::<Vec<_>>(),
    }))
}

pub fn caption_style_catalog() -> CmdResult {
    let styles = [
        ("minimal", "Minimal"),
        ("gaming", "Gaming"),
        ("tiktok", "TikTok"),
        ("youtube", "YouTube"),
        ("cinematic", "Cinematic"),
        ("bold", "Bold"),
        ("karaoke", "Karaoke"),
        ("word_highlight", "Word Highlight"),
        ("streamer", "Streamer"),
    ];
    Ok(json!({
        "styles": styles.iter().map(|(id, label)| json!({"id": id, "label": label})).collect::<Vec<_>>(),
    }))
}

pub fn export_presets() -> CmdResult {
    Ok(json!({"presets": mycut_engine::preset::builtin_presets()}))
}

// ---------------- clip editing ----------------

pub fn split_clip(state: &AppState, body: &Value) -> CmdResult {
    let item_id = str_field(body, "itemId")?;
    let at = ms(body, "atMs")?;
    apply_cmd(
        state,
        Command::SplitClip {
            item_id,
            at_ms: at,
        },
    )
}

pub fn move_item(state: &AppState, body: &Value) -> CmdResult {
    let item_id = str_field(body, "itemId")?;
    let at = ms(body, "timelineStartMs")?;
    apply_cmd(state, Command::MoveItem { item_id, new_timeline_start_ms: at })
}

pub fn delete_item(state: &AppState, body: &Value) -> CmdResult {
    let item_id = str_field(body, "itemId")?;
    apply_cmd(state, Command::DeleteItem { item_id })
}

pub fn trim_clip(state: &AppState, body: &Value) -> CmdResult {
    let item_id = str_field(body, "itemId")?;
    let sin = ms(body, "sourceInMs")?;
    let sout = ms(body, "sourceOutMs")?;
    let start = ms(body, "timelineStartMs")?;
    apply_cmd(
        state,
        Command::TrimClip {
            item_id,
            new_source_in_ms: sin,
            new_source_out_ms: sout,
            new_timeline_start_ms: start,
        },
    )
}

/// Duplicate = clone the item with a fresh id, placed right after the
/// original (clips keep their source range; effects and volume carry over).
pub fn duplicate_item(state: &AppState, body: &Value) -> CmdResult {
    let item_id = str_field(body, "itemId")?;
    let mut doc = state.doc.lock().unwrap();
    let ProjectDocument {
        project, history, ..
    } = &mut *doc;
    let original = project
        .tracks
        .iter()
        .flat_map(|t| t.items.iter())
        .find(|i| i.id == item_id)
        .cloned()
        .ok_or_else(|| ce(404, format!("item {item_id} not found")))?;
    let track_kind = project
        .tracks
        .iter()
        .find(|t| t.items.iter().any(|i| i.id == item_id))
        .map(|t| t.kind)
        .ok_or_else(|| ce(404, "track not found"))?;
    let mut copy = original.clone();
    copy.id = mycut_core::new_id("item");
    copy.timeline_start_ms = original.timeline_start_ms + original.timeline_duration_ms;
    history
        .apply(project, Command::AddClip { track_kind, item: copy })
        .map_err(|e| ce(400, e.to_string()))?;
    save_state(state, &doc)?;
    Ok(json!({"ok": true}))
}

// ---------------- effects ----------------

pub fn add_effect(state: &AppState, body: &Value) -> CmdResult {
    let item_id = str_field(body, "itemId")?;
    let def_id = str_field(body, "defId")?;
    // Resolve defaults from the registry; overlay user values.
    let cat = effect_catalog()?;
    let all: Vec<Value> = cat["video"]
        .as_array()
        .unwrap()
        .iter()
        .chain(cat["audio"].as_array().unwrap())
        .cloned()
        .collect();
    let def = all
        .iter()
        .find(|d| d["def_id"] == json!(def_id))
        .ok_or_else(|| ce(404, format!("unknown effect: {def_id}")))?;
    let mut params: BTreeMap<String, ParamValue> = BTreeMap::new();
    if let Some(arr) = def["params"].as_array() {
        for p in arr {
            let name = p["name"].as_str().unwrap_or_default().to_string();
            let default = p["default"].as_f64().unwrap_or(0.0);
            params.insert(name, ParamValue::Number(default));
        }
    }
    if let Some(map) = body.get("params").and_then(Value::as_object) {
        for (k, v) in map {
            let pv = match v {
                Value::Number(n) => ParamValue::Number(n.as_f64().unwrap_or(0.0)),
                Value::String(s) => ParamValue::Text(s.clone()),
                _ => continue,
            };
            params.insert(k.clone(), pv);
        }
    }
    let effect = EffectInstance {
        id: mycut_core::new_id("fx"),
        def_id,
        params,
        window_start_ms: body.get("windowStartMs").and_then(Value::as_i64),
        window_end_ms: body.get("windowEndMs").and_then(Value::as_i64),
        keyframes: Vec::new(),
        easing: mycut_core::Easing::Linear,
    };
    let effect_id = effect.id.clone();
    apply_cmd(state, Command::AddEffect { item_id, effect })?;
    Ok(json!({"ok": true, "effectId": effect_id}))
}

pub fn remove_effect(state: &AppState, body: &Value) -> CmdResult {
    let item_id = str_field(body, "itemId")?;
    let effect_id = str_field(body, "effectId")?;
    apply_cmd(
        state,
        Command::RemoveEffect { item_id, effect_id },
    )
}

pub fn set_effect_param(state: &AppState, body: &Value) -> CmdResult {
    let item_id = str_field(body, "itemId")?;
    let effect_id = str_field(body, "effectId")?;
    let name = str_field(body, "name")?;
    let value = num(body, "value")
        .map(ParamValue::Number)
        .or_else(|| {
            body.get("value")
                .and_then(Value::as_str)
                .map(|s| ParamValue::Text(s.to_string()))
        })
        .ok_or_else(|| ce(400, "missing value"))?;
    let mut doc = state.doc.lock().unwrap();
    let ProjectDocument {
        project, history, ..
    } = &mut *doc;
    let effect = project
        .find_effect_mut(&effect_id)
        .cloned()
        .ok_or_else(|| ce(404, format!("effect {effect_id} not found")))?;
    // Express the param edit as a Remove+Add transaction so undo reverts it
    // as one step (no dedicated core command exists for param edits).
    let mut updated = effect.clone();
    updated.params.insert(name, value);
    let cmds = vec![
        Command::RemoveEffect {
            item_id: item_id.clone(),
            effect_id: effect_id.clone(),
        },
        Command::AddEffect {
            item_id,
            effect: updated,
        },
    ];
    mycut_core::apply_transaction(history, project, cmds)
        .map_err(|e| ce(400, e.to_string()))?;
    save_state(state, &doc)?;
    Ok(json!({"ok": true}))
}

// ---------------- item properties ----------------

pub fn set_item_volume(state: &AppState, body: &Value) -> CmdResult {
    let item_id = str_field(body, "itemId")?;
    let volume = num(body, "volume").ok_or_else(|| ce(400, "missing volume"))?;
    apply_cmd(state, Command::SetItemVolume { item_id, volume })
}

pub fn set_item_opacity(state: &AppState, body: &Value) -> CmdResult {
    let item_id = str_field(body, "itemId")?;
    let opacity = num(body, "opacity").ok_or_else(|| ce(400, "missing opacity"))?;
    apply_cmd(state, Command::SetItemOpacity { item_id, opacity })
}

pub fn set_clip_speed(state: &AppState, body: &Value) -> CmdResult {
    let item_id = str_field(body, "itemId")?;
    let speed = num(body, "speed").ok_or_else(|| ce(400, "missing speed"))?;
    apply_cmd(state, Command::SetClipSpeed { item_id, speed })
}

// ---------------- text ----------------

pub fn add_text(state: &AppState, body: &Value) -> CmdResult {
    let text = str_field(body, "text")?;
    if text.trim().is_empty() {
        return Err(ce(400, "text is empty"));
    }
    let kind = match body.get("kind").and_then(Value::as_str) {
        Some("lower_third") => TextKind::LowerThird,
        Some("callout") => TextKind::Callout,
        Some("watermark") => TextKind::Watermark,
        _ => TextKind::Title,
    };
    let position = parse_position(body.get("position").and_then(Value::as_str));
    let scale = num(body, "scale").unwrap_or(1.0);
    let start = ms(body, "startMs")?;
    let duration = ms(body, "durationMs")?;
    if duration <= 0 {
        return Err(ce(400, "durationMs must be positive"));
    }
    let item = Item::new(
        ItemKind::Text {
            kind,
            text,
            position,
            scale,
            opacity: 1.0,
        },
        start,
        duration,
    );
    apply_cmd(
        state,
        Command::AddClip {
            track_kind: TrackKind::Text,
            item,
        },
    )
}

/// Update text content/position/scale/opacity via SetTrackItems on the Text
/// track (the exposed reorder command doubles as the item-replace path).
pub fn update_text(state: &AppState, body: &Value) -> CmdResult {
    let item_id = str_field(body, "itemId")?;
    let mut doc = state.doc.lock().unwrap();
    let ProjectDocument {
        project, history, ..
    } = &mut *doc;
    let track = project
        .tracks
        .iter_mut()
        .find(|t| t.kind == TrackKind::Text)
        .ok_or_else(|| ce(404, "no text track"))?;
    let idx = track
        .items
        .iter()
        .position(|i| i.id == item_id)
        .ok_or_else(|| ce(404, format!("text item {item_id} not found")))?;
    let mut item = track.items[idx].clone();
    let (mut tk, mut ttext, mut tpos, mut tscale, mut topacity) = match &item.kind {
        ItemKind::Text {
            kind,
            text,
            position,
            scale,
            opacity,
        } => (*kind, text.clone(), *position, *scale, *opacity),
        _ => return Err(ce(400, "not a text item")),
    };
    if let Some(t) = body.get("text").and_then(Value::as_str) {
        ttext = t.to_string();
    }
    if let Some(p) = body.get("position").and_then(Value::as_str) {
        tpos = parse_position(Some(p));
    }
    if let Some(s) = num(body, "scale") {
        tscale = s;
    }
    if let Some(o) = num(body, "opacity") {
        topacity = o.clamp(0.0, 1.0);
    }
    item.kind = ItemKind::Text {
        kind: tk,
        text: ttext,
        position: tpos,
        scale: tscale,
        opacity: topacity,
    };
    let _ = &mut tk;
    if let Some(d) = body.get("durationMs").and_then(Value::as_f64) {
        if d > 0.0 {
            item.timeline_duration_ms = d.round() as i64;
        }
    }
    if let Some(s) = body.get("startMs").and_then(Value::as_f64) {
        item.timeline_start_ms = s.round() as i64;
    }
    let track_kind = track.kind;
    let before = track.items.clone();
    track.items[idx] = item;
    let after = track.items.clone();
    // Express as one history step: apply SetTrackItems(after) — its inverse
    // captures `before` automatically.
    history
        .apply(project, Command::SetTrackItems { track_kind, items: after })
        .map_err(|e| ce(400, e.to_string()))?;
    let _ = before; // inverse recorded by the history layer
    save_state(state, &doc)?;
    Ok(json!({"ok": true}))
}

// ---------------- transitions / color / reframe / audio ----------------

pub fn set_transitions(state: &AppState, body: &Value) -> CmdResult {
    let arr = body
        .get("transitions")
        .and_then(Value::as_array)
        .ok_or_else(|| ce(400, "missing transitions array"))?;
    let mut out = Vec::with_capacity(arr.len());
    for t in arr {
        let after_index = t
            .get("afterIndex")
            .and_then(Value::as_u64)
            .ok_or_else(|| ce(400, "missing afterIndex"))? as usize;
        let kind = parse_transition(t.get("kind").and_then(Value::as_str))?;
        let duration_ms = t
            .get("durationMs")
            .and_then(Value::as_f64)
            .map(|f| f.round() as i64)
            .unwrap_or(500);
        out.push(TransitionSetting {
            after_index,
            kind,
            duration_ms,
        });
    }
    apply_cmd(state, Command::SetTransitions { transitions: out })
}

pub fn set_color(state: &AppState, body: &Value) -> CmdResult {
    let mut doc = state.doc.lock().unwrap();
    let mut grade = doc.project.color.clone();
    if let Some(v) = num(body, "exposure") {
        grade.exposure = v;
    }
    if let Some(v) = num(body, "contrast") {
        grade.contrast = v;
    }
    if let Some(v) = num(body, "saturation") {
        grade.saturation = v;
    }
    if let Some(v) = num(body, "temperature") {
        grade.temperature = v;
    }
    if let Some(v) = num(body, "gamma") {
        grade.gamma = v;
    }
    if let Some(v) = num(body, "vibrance") {
        grade.vibrance = v;
    }
    let cmd = Command::SetColor { new_grade: grade };
    let ProjectDocument {
        project, history, ..
    } = &mut *doc;
    history
        .apply(project, cmd)
        .map_err(|e| ce(400, e.to_string()))?;
    save_state(state, &doc)?;
    Ok(json!({"ok": true}))
}

pub fn set_reframe(state: &AppState, body: &Value) -> CmdResult {
    let settings = if body.get("enabled").and_then(Value::as_bool) == Some(false) {
        None
    } else {
        let ratio = match body.get("ratio").and_then(Value::as_str) {
            Some("1:1") | Some("r1x1") => mycut_core::AspectRatio::R1x1,
            Some("4:5") | Some("r4x5") => mycut_core::AspectRatio::R4x5,
            Some("9:16") | Some("r9x16") | None => mycut_core::AspectRatio::R9x16,
            _ => mycut_core::AspectRatio::R16x9,
        };
        let mode = match body.get("mode").and_then(Value::as_str) {
            Some("center") => ReframeMode::Center,
            Some("subject_follow") => ReframeMode::SubjectFollow,
            _ => ReframeMode::Smart,
        };
        Some(ReframeSettings { ratio, mode })
    };
    apply_cmd(state, Command::SetReframe { settings })
}

pub fn set_audio_master(state: &AppState, body: &Value) -> CmdResult {
    let mut doc = state.doc.lock().unwrap();
    let mut master = doc.project.audio.clone();
    if let Some(v) = body.get("normalize").and_then(Value::as_bool) {
        master.normalize = v;
    }
    if let Some(v) = body.get("denoise").and_then(Value::as_bool) {
        master.denoise = v;
    }
    if let Some(v) = body.get("duckMusicUnderSpeech").and_then(Value::as_bool) {
        master.duck_music_under_speech = v;
    }
    if let Some(v) = num(body, "fadeInS") {
        master.fade_in_s = v.clamp(0.0, 30.0);
    }
    if let Some(v) = num(body, "fadeOutS") {
        master.fade_out_s = v.clamp(0.0, 30.0);
    }
    let cmd = Command::SetAudioMaster { new: master };
    let ProjectDocument {
        project, history, ..
    } = &mut *doc;
    history
        .apply(project, cmd)
        .map_err(|e| ce(400, e.to_string()))?;
    save_state(state, &doc)?;
    Ok(json!({"ok": true}))
}

pub fn set_export_settings(state: &AppState, body: &Value) -> CmdResult {
    let mut doc = state.doc.lock().unwrap();
    let mut s = doc.project.export.clone();
    if let Some(v) = body.get("width").and_then(Value::as_u64) {
        s.width = v as u32;
    }
    if let Some(v) = body.get("height").and_then(Value::as_u64) {
        s.height = v as u32;
    }
    if let Some(v) = num(body, "fps") {
        s.fps = v;
    }
    if let Some(v) = body.get("quality").and_then(Value::as_u64) {
        s.quality = v as u32;
    }
    if let Some(v) = body.get("audioBitrateKbps").and_then(Value::as_u64) {
        s.audio_bitrate_kbps = v as u32;
    }
    if let Some(codec) = body.get("videoCodec").and_then(Value::as_str) {
        s.video_codec = match codec {
            "h264" => mycut_core::VideoCodec::H264,
            "hevc" => mycut_core::VideoCodec::Hevc,
            "vp9" => mycut_core::VideoCodec::Vp9,
            other => return Err(ce(400, format!("unknown video codec: {other}"))),
        };
    }
    if let Some(codec) = body.get("audioCodec").and_then(Value::as_str) {
        s.audio_codec = match codec {
            "aac" => mycut_core::AudioCodec::Aac,
            "mp3" => mycut_core::AudioCodec::Mp3,
            "opus" => mycut_core::AudioCodec::Opus,
            other => return Err(ce(400, format!("unknown audio codec: {other}"))),
        };
    }
    if let Some(c) = body.get("container").and_then(Value::as_str) {
        s.container = match c {
            "mp4" => mycut_core::Container::Mp4,
            "mkv" => mycut_core::Container::Mkv,
            "webm" => mycut_core::Container::Webm,
            "mov" => mycut_core::Container::Mov,
            other => return Err(ce(400, format!("unknown container: {other}"))),
        };
    }
    let cmd = Command::SetExport { settings: s };
    let ProjectDocument {
        project, history, ..
    } = &mut *doc;
    history
        .apply(project, cmd)
        .map_err(|e| ce(400, e.to_string()))?;
    save_state(state, &doc)?;
    Ok(json!({"ok": true}))
}

// ---------------- tracks ----------------

pub fn set_track_props(state: &AppState, body: &Value) -> CmdResult {
    let track_id = str_field(body, "trackId")?;
    let muted = body.get("muted").and_then(Value::as_bool);
    let solo = body.get("solo").and_then(Value::as_bool);
    let locked = body.get("locked").and_then(Value::as_bool);
    apply_cmd(
        state,
        Command::SetTrackProps {
            track_id,
            muted,
            solo,
            locked,
        },
    )
}

pub fn add_track(state: &AppState, body: &Value) -> CmdResult {
    let kind = parse_track_kind(body.get("kind").and_then(Value::as_str))?;
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("New")
        .to_string();
    let track = Track {
        id: mycut_core::new_id("track"),
        kind,
        name,
        muted: false,
        solo: false,
        locked: false,
        items: Vec::new(),
    };
    apply_cmd(state, Command::AddTrack { track })
}

pub fn remove_track(state: &AppState, body: &Value) -> CmdResult {
    let track_id = str_field(body, "trackId")?;
    apply_cmd(state, Command::RemoveTrack { track_id })
}

// ---------------- captions ----------------

/// Manual captions: user provides timed text; the pipeline segments it with
/// the same engine the AI path uses. `entries: [{startMs,endMs,text}]`.
pub fn set_captions(state: &AppState, body: &Value) -> CmdResult {
    let style = parse_caption_style(body.get("style").and_then(Value::as_str));
    let scale = num(body, "scale").unwrap_or(1.0);
    let arr = body
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| ce(400, "missing entries array"))?;
    let mut entries = Vec::with_capacity(arr.len());
    for e in arr {
        let start_ms = ms(e, "startMs")?;
        let end_ms = ms(e, "endMs")?;
        let text = str_field(e, "text")?;
        if end_ms <= start_ms {
            return Err(ce(400, "caption end must be after start"));
        }
        let words: Vec<mycut_core::CaptionWord> = text
            .split_whitespace()
            .map(|w| mycut_core::CaptionWord {
                text: w.to_string(),
                start_ms,
                end_ms,
                emphasize: false,
            })
            .collect();
        entries.push(mycut_core::CaptionEntry {
            start_ms,
            end_ms,
            words,
        });
    }
    let entries_json = serde_json::to_string(&entries).unwrap_or_default();
    apply_cmd(
        state,
        Command::ReplaceCaptions {
            style,
            scale,
            safe_area: "default".into(),
            entries_json,
        },
    )
}

pub fn remove_captions(state: &AppState) -> CmdResult {
    apply_cmd(state, Command::RemoveCaptions)
}

// ---------------- media library helpers ----------------

/// Remove a source from the library (fails while clips use it — the error
/// explains what to do first).
pub fn remove_source(state: &AppState, body: &Value) -> CmdResult {
    let source_id = str_field(body, "sourceId")?;
    apply_cmd(state, Command::RemoveSource { source_id })
}

/// Add an existing library source to the timeline again (at a given time).
pub fn add_source_to_timeline(state: &AppState, body: &Value) -> CmdResult {
    let source_id = str_field(body, "sourceId")?;
    let start = ms(body, "startMs").unwrap_or(0);
    let mut doc = state.doc.lock().unwrap();
    let ProjectDocument {
        project, history, ..
    } = &mut *doc;
    let source = project
        .source(&source_id)
        .cloned()
        .ok_or_else(|| ce(404, format!("source {source_id} not found")))?;
    let kind = if source.height > 0 && source.width > 0 && source.duration_ms > 0 {
        // Videos: keep full range. Images (ffprobe gives ~1 frame) also land
        // here; their timeline presence follows the probed duration.
        TrackKind::Video
    } else {
        TrackKind::Audio
    };
    let dur = if kind == TrackKind::Video {
        source.duration_ms
    } else {
        source.duration_ms
    };
    let item = Item::new(
        ItemKind::VideoClip {
            source_id: source.id.clone(),
            source_in_ms: 0,
            source_out_ms: source.duration_ms,
            speed: 1.0,
        },
        start,
        dur,
    );
    let item = if kind == TrackKind::Audio {
        let _ = dur;
        Item::new(
            ItemKind::AudioClip {
                source_id: source.id.clone(),
                source_in_ms: 0,
                source_out_ms: source.duration_ms,
                speed: 1.0,
            },
            start,
            source.duration_ms,
        )
    } else {
        item
    };
    history
        .apply(project, Command::AddClip { track_kind: kind, item })
        .map_err(|e| ce(400, e.to_string()))?;
    save_state(state, &doc)?;
    Ok(json!({"ok": true}))
}

/// Open the containing folder of a library file in the desktop file manager
/// (Media Library → "Reveal in Files"). Uses the *directory* so selection
/// semantics cannot leak unexpected paths.
pub fn reveal_source(state: &AppState, body: &Value) -> CmdResult {
    let source_id = str_field(body, "sourceId")?;
    let pdir = project_dir(state);
    let doc = state.doc.lock().unwrap();
    let source = doc
        .project
        .sources
        .iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| ce(404, "source not found"))?;
    let path: PathBuf = mycut_projects::resolve_source_path(&source.rel_path, &pdir)
        .ok_or_else(|| ce(404, "media file not found on disk"))?;
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    drop(doc);
    let program = if Path::new("/usr/bin/xdg-open").exists() {
        "xdg-open".to_string()
    } else {
        return Err(ce(501, "no file manager integration available (xdg-open missing)"));
    };
    std::process::Command::new(program)
        .arg(&dir)
        .spawn()
        .map_err(|e| ce(500, format!("could not open file manager: {e}")))?;
    Ok(json!({"ok": true}))
}

// ---------------- parse helpers ----------------

fn parse_position(s: Option<&str>) -> Position {
    match s {
        Some("top_left") => Position::TopLeft,
        Some("top_right") => Position::TopRight,
        Some("bottom_left") => Position::BottomLeft,
        Some("bottom_right") => Position::BottomRight,
        Some("top_center") => Position::TopCenter,
        Some("bottom_center") => Position::BottomCenter,
        _ => Position::Center,
    }
}

fn parse_transition(s: Option<&str>) -> Result<TransitionKind, CommandError> {
    match s.unwrap_or("cut") {
        "cut" => Ok(TransitionKind::Cut),
        "fade" => Ok(TransitionKind::Fade),
        "crossfade" => Ok(TransitionKind::Crossfade),
        "dip_to_black" => Ok(TransitionKind::DipToBlack),
        "dip_to_white" => Ok(TransitionKind::DipToWhite),
        "slide" => Ok(TransitionKind::Slide),
        "push" => Ok(TransitionKind::Push),
        "zoom" => Ok(TransitionKind::Zoom),
        "wipe" => Ok(TransitionKind::Wipe),
        other => Err(ce(400, format!("unknown transition: {other}"))),
    }
}

fn parse_track_kind(s: Option<&str>) -> Result<TrackKind, CommandError> {
    match s.unwrap_or("video") {
        "video" => Ok(TrackKind::Video),
        "audio" => Ok(TrackKind::Audio),
        "text" => Ok(TrackKind::Text),
        "captions" => Ok(TrackKind::Captions),
        "overlay" => Ok(TrackKind::Overlay),
        other => Err(ce(400, format!("unknown track kind: {other}"))),
    }
}

fn parse_caption_style(s: Option<&str>) -> mycut_core::CaptionStyle {
    match s {
        Some("gaming") => mycut_core::CaptionStyle::Gaming,
        Some("tiktok") => mycut_core::CaptionStyle::Tiktok,
        Some("youtube") => mycut_core::CaptionStyle::Youtube,
        Some("cinematic") => mycut_core::CaptionStyle::Cinematic,
        Some("bold") => mycut_core::CaptionStyle::Bold,
        Some("karaoke") => mycut_core::CaptionStyle::Karaoke,
        Some("word_highlight") => mycut_core::CaptionStyle::WordHighlight,
        Some("streamer") => mycut_core::CaptionStyle::Streamer,
        _ => mycut_core::CaptionStyle::Minimal,
    }
}

// ---------------- tests ----------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_parse_covers_all_variants() {
        assert_eq!(parse_position(Some("top_left")), Position::TopLeft);
        assert_eq!(parse_position(Some("bottom_center")), Position::BottomCenter);
        assert_eq!(parse_position(Some("garbage")), Position::Center);
        assert_eq!(parse_position(None), Position::Center);
    }

    #[test]
    fn transition_parse_rejects_unknown() {
        assert!(parse_transition(Some("crossfade")).is_ok());
        assert!(parse_transition(Some("holodeck")).is_err());
    }

    #[test]
    fn effect_catalog_has_video_and_audio() {
        let cat = effect_catalog().unwrap();
        assert!(!cat["video"].as_array().unwrap().is_empty());
        assert!(!cat["audio"].as_array().unwrap().is_empty());
    }

    #[test]
    fn registries_match_project_needs() {
        let cat = effect_catalog().unwrap();
        let ids: Vec<String> = cat["video"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|d| d["def_id"].as_str().map(str::to_string))
            .collect();
        for want in ["zoom", "zoom_punch", "shake", "blur", "flash"] {
            assert!(ids.contains(&want.to_string()), "missing {want}");
        }
    }
}
