//! Plan application: validated operations -> core Commands (one transaction)
//! + explicit pending tasks (e.g. transcription for captions). This module is
//! the ONLY bridge from AI output to the command layer.

use mycut_core::{
    Command, EffectInstance, Item, ItemKind, ParamValue, Project, TimeMs, TransitionKind,
    TransitionSetting, AudioMaster, ColorGrade,
};

use crate::plan::{EditPlan, Operation, PlanError, PresetId, RatioId, Selection};
use crate::validate::{s_to_ms, validate_plan, PlanContext};

/// A task the app must run after the transaction (never faked).
#[derive(Debug, Clone, PartialEq)]
pub enum PendingTask {
    /// Run transcription + CaptionEngine, then issue ReplaceCaptions.
    TranscribeAndCaption {
        style_id: crate::plan::CaptionStyleId,
        safe_area: crate::plan::SafeArea,
        max_words_per_line: Option<u32>,
        scale: Option<f64>,
    },
}

#[derive(Debug, Default)]
pub struct PlanApplication {
    pub commands: Vec<Command>,
    pub pending: Vec<PendingTask>,
    pub summary_parts: Vec<String>,
    pub warnings: Vec<String>,
    pub repairs: Vec<String>,
}

/// Apply a validated plan to the project via commands. `preview_plan` runs
/// the mapping without touching history (dry-run summary).
///
/// # Errors
/// [`PlanError`] when the validated plan no longer fits reality (e.g. no
/// video clip exists for effects).
pub fn plan_to_commands(
    plan: &EditPlan,
    project: &mut Project,
    history: &mut mycut_core::History,
    ctx: &PlanContext,
) -> Result<PlanApplication, PlanError> {
    let (validated, result) = validate_plan(plan, ctx);
    let Some(ops) = validated else {
        let err = result.errors.first().cloned().unwrap_or(PlanError::Invalid {
            op: "plan".into(), reason: "validation failed".into(),
        });
        return Err(err);
    };
    let mut app = PlanApplication {
        repairs: result.repairs.clone(),
        warnings: result.warnings.clone(),
        ..Default::default()
    };

    let mut cmds: Vec<Command> = Vec::new();
    // Ensure a clip exists before ops that need one.
    let primary = primary_clip_id(project);
    let mut clip_created = false;

    for op in &ops {
        match op {
            Operation::CutRanges { ranges, .. } => {
                let cid = ensure_clip(project, &mut cmds, &mut clip_created, ctx);
                let cuts: Vec<(TimeMs, TimeMs)> =
                    ranges.iter().map(|r| (s_to_ms(r.start), s_to_ms(r.end))).collect();
                cmds.extend(cut_commands(project, &cid, &cuts));
                let removed: f64 = ranges.iter().map(|r| r.end - r.start).sum();
                app.summary_parts.push(format!("Removed {removed:.1}s of footage"));
            }
            Operation::KeepRanges { ranges, .. } => {
                let cid = ensure_clip(project, &mut cmds, &mut clip_created, ctx);
                // Keep = cut the complement.
                let dur_ms = s_to_ms(ctx.source_duration_s);
                let mut cuts: Vec<(TimeMs, TimeMs)> = Vec::new();
                let mut cursor = 0;
                for r in ranges {
                    let s = s_to_ms(r.start);
                    let e = s_to_ms(r.end);
                    if s > cursor {
                        cuts.push((cursor, s));
                    }
                    cursor = cursor.max(e);
                }
                if cursor < dur_ms {
                    cuts.push((cursor, dur_ms));
                }
                if !cuts.is_empty() {
                    cmds.extend(cut_commands(project, &cid, &cuts));
                }
                app.summary_parts.push(format!("Kept {} ranges", ranges.len()));
            }
            Operation::SetTargetDuration { seconds, selection } => {
                let cid = ensure_clip(project, &mut cmds, &mut clip_created, ctx);
                let target_ms = s_to_ms(*seconds);
                let cur_ms = clip_len(project, &cid).unwrap_or(0);
                if cur_ms > target_ms {
                    // Choose windows to KEEP up to target from highlights (or
                    // start/even), then cut the complement.
                    let wins = selection_windows(ctx, selection, cur_ms, target_ms);
                    let mut cuts: Vec<(TimeMs, TimeMs)> = Vec::new();
                    let mut cursor = 0;
                    for (s, e) in wins {
                        if s > cursor {
                            cuts.push((cursor, s));
                        }
                        cursor = cursor.max(e);
                    }
                    if cursor < cur_ms {
                        cuts.push((cursor, cur_ms));
                    }
                    if !cuts.is_empty() {
                        cmds.extend(cut_commands(project, &cid, &cuts));
                    }
                    app.summary_parts.push(format!("Trimmed to {seconds:.0}s ({selection:?})"));
                } else {
                    app.summary_parts.push("Timeline already within target duration".into());
                }
            }
            Operation::AddEffect { effect, start, end, params, .. } => {
                let cid = ensure_clip(project, &mut cmds, &mut clip_created, ctx);
                let def_id = effect_def_id(effect);
                let mut pmap = std::collections::BTreeMap::new();
                for (k, v) in params {
                    if let Some(n) = v.as_f64() {
                        pmap.insert(k.clone(), ParamValue::Number(n));
                    }
                }
                // Window is source-absolute; store relative to clip start.
                let (in_ms, _) = clip_range(project, &cid);
                let eff = EffectInstance {
                    id: mycut_core::new_id("fx"),
                    def_id: def_id.to_string(),
                    params: pmap,
                    window_start_ms: Some((s_to_ms(*start) - in_ms).max(0)),
                    window_end_ms: Some(s_to_ms(*end) - in_ms),
                    keyframes: Vec::new(),
                    easing: mycut_core::Easing::EaseOut,
                };
                cmds.push(Command::AddEffect { item_id: cid.clone(), effect: eff });
                app.summary_parts.push(format!("Added {def_id} effect"));
            }
            Operation::RemoveEffect { effect } => {
                if let Some(cid) = primary_clip_id(project) {
                    let ids: Vec<String> = project
                        .find_item_mut(&cid)
                        .map(|it| {
                            it.effects
                                .iter()
                                .filter(|e| e.def_id == *effect)
                                .map(|e| e.id.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    for id in ids {
                        cmds.push(Command::RemoveEffect { item_id: cid.clone(), effect_id: id });
                    }
                }
                app.summary_parts.push(format!("Removed {effect} effects"));
            }
            Operation::SetColor { params, merge_with_existing } => {
                let mut grade = if *merge_with_existing { project.color.clone() } else { ColorGrade::default() };
                if let Some(v) = params.exposure { grade.exposure = v; }
                if let Some(v) = params.contrast { grade.contrast = v; }
                if let Some(v) = params.saturation { grade.saturation = v; }
                if let Some(v) = params.vibrance { grade.vibrance = v; }
                if let Some(v) = params.temperature { grade.temperature = v; }
                if let Some(v) = params.gamma { grade.gamma = v; }
                if let Some(v) = params.shadows { grade.shadows = v; }
                if let Some(v) = params.highlights { grade.highlights = v; }
                if let Some(l) = &params.lut { grade.lut = Some(l.clone()); }
                if let Some(v) = params.lut_intensity { grade.lut_intensity = v; }
                cmds.push(Command::SetColor { new_grade: grade });
                app.summary_parts.push("Applied color grade".into());
            }
            Operation::SetAspect { ratio, reframe } => {
                cmds.push(Command::SetReframe {
                    settings: Some(mycut_core::ReframeSettings {
                        ratio: crate::validate::ratio_to_aspect(*ratio),
                        mode: match reframe {
                            crate::plan::ReframeMode::Center => mycut_core::ReframeMode::Center,
                            crate::plan::ReframeMode::SubjectFollow | crate::plan::ReframeMode::Smart => {
                                mycut_core::ReframeMode::SubjectFollow
                            }
                        },
                    }),
                });
                let (w, h) = crate::validate::ratio_to_aspect(*ratio).wh();
                app.summary_parts.push(format!("Converted to {}:{}", w, h));
            }
            Operation::GenerateCaptions { style, safe_area, max_words_per_line, scale, .. } => {
                app.pending.push(PendingTask::TranscribeAndCaption {
                    style_id: *style,
                    safe_area: safe_area.unwrap_or(crate::plan::SafeArea::Default),
                    max_words_per_line: *max_words_per_line,
                    scale: *scale,
                });
                app.summary_parts.push("Queued automatic captions".into());
            }
            Operation::UpdateCaptions { style, scale, safe_area } => {
                // Update requires an existing captions item.
                if let Some(item) = project
                    .track_of_kind(mycut_core::TrackKind::Captions)
                    .and_then(|t| t.items.iter().find(|i| matches!(i.kind, ItemKind::Captions { .. })))
                {
                    let (cur_style, cur_scale, cur_safe, entries_json) = match &item.kind {
                        ItemKind::Captions { style, scale, safe_area, entries } => (
                            *style,
                            *scale,
                            safe_area.clone(),
                            serde_json::to_string(entries).unwrap_or_default(),
                        ),
                        _ => continue,
                    };
                    let new_style = style.map(caption_style).unwrap_or(cur_style);
                    let new_scale = scale.unwrap_or(cur_scale);
                    let new_safe = safe_area.as_ref().map(|sa| format!("{:?}", sa).to_lowercase()).unwrap_or(cur_safe);
                    cmds.push(Command::ReplaceCaptions {
                        style: new_style,
                        scale: new_scale,
                        safe_area: new_safe,
                        entries_json,
                    });
                    app.summary_parts.push("Updated captions style".into());
                } else {
                    app.warnings.push("update_captions: no captions exist yet".into());
                }
            }
            Operation::RemoveCaptions => {
                cmds.push(Command::RemoveCaptions);
                app.summary_parts.push("Removed captions".into());
            }
            Operation::AddTransition { transition, at_index, duration } => {
                let mut list = project.transitions.clone();
                let idx = at_index.unwrap_or(0) as usize;
                let dur_ms = s_to_ms(duration.unwrap_or(0.5));
                let kind = match transition {
                    crate::plan::TransitionId::Cut => TransitionKind::Cut,
                    crate::plan::TransitionId::Fade | crate::plan::TransitionId::Crossfade => TransitionKind::Crossfade,
                    crate::plan::TransitionId::DipToBlack => TransitionKind::DipToBlack,
                    crate::plan::TransitionId::DipToWhite => TransitionKind::DipToWhite,
                    crate::plan::TransitionId::Slide => TransitionKind::Slide,
                    crate::plan::TransitionId::Push => TransitionKind::Push,
                    crate::plan::TransitionId::Zoom => TransitionKind::Zoom,
                    crate::plan::TransitionId::Blur => TransitionKind::Cut,
                    crate::plan::TransitionId::Wipe => TransitionKind::Wipe,
                };
                list.retain(|t| t.after_index != idx);
                list.push(TransitionSetting { after_index: idx, kind, duration_ms: dur_ms });
                cmds.push(Command::SetTransitions { transitions: list });
                app.summary_parts.push(format!("Added {transition:?} transition"));
            }
            Operation::AddText { text, start, end, kind, position, scale, opacity } => {
                let item = Item::new(
                    ItemKind::Text {
                        kind: match kind {
                            crate::plan::TextKindId::Title => mycut_core::TextKind::Title,
                            crate::plan::TextKindId::LowerThird => mycut_core::TextKind::LowerThird,
                            crate::plan::TextKindId::Callout => mycut_core::TextKind::Callout,
                            crate::plan::TextKindId::Watermark => mycut_core::TextKind::Watermark,
                        },
                        text: text.clone(),
                        position: match position.unwrap_or(crate::plan::PositionId::Center) {
                            crate::plan::PositionId::TopLeft => mycut_core::Position::TopLeft,
                            crate::plan::PositionId::TopRight => mycut_core::Position::TopRight,
                            crate::plan::PositionId::BottomLeft => mycut_core::Position::BottomLeft,
                            crate::plan::PositionId::BottomRight => mycut_core::Position::BottomRight,
                            crate::plan::PositionId::Center => mycut_core::Position::Center,
                            crate::plan::PositionId::TopCenter => mycut_core::Position::TopCenter,
                            crate::plan::PositionId::BottomCenter => mycut_core::Position::BottomCenter,
                        },
                        scale: scale.unwrap_or(1.0),
                        opacity: opacity.unwrap_or(1.0),
                    },
                    s_to_ms(*start),
                    s_to_ms(*end) - s_to_ms(*start),
                );
                cmds.push(Command::AddClip {
                    track_kind: mycut_core::TrackKind::Text,
                    item,
                });
                app.summary_parts.push(format!("Added text: \"{}\"", text.chars().take(24).collect::<String>()));
            }
            Operation::UpdateText { target_id, text, scale, opacity, position } => {
                let mut changed = false;
                if let Some(item) = project.find_item_mut(target_id) {
                    if let ItemKind::Text { text: t, scale: s, opacity: o, position: pos, kind: _ } = &mut item.kind {
                        if let Some(nt) = text { *t = nt.clone(); changed = true; }
                        if let Some(ns) = scale { *s = *ns; changed = true; }
                        if let Some(no) = opacity { *o = *no; changed = true; }
                        if let Some(np) = position {
                            *pos = match np {
                                crate::plan::PositionId::TopLeft => mycut_core::Position::TopLeft,
                                crate::plan::PositionId::TopRight => mycut_core::Position::TopRight,
                                crate::plan::PositionId::BottomLeft => mycut_core::Position::BottomLeft,
                                crate::plan::PositionId::BottomRight => mycut_core::Position::BottomRight,
                                crate::plan::PositionId::Center => mycut_core::Position::Center,
                                crate::plan::PositionId::TopCenter => mycut_core::Position::TopCenter,
                                crate::plan::PositionId::BottomCenter => mycut_core::Position::BottomCenter,
                            };
                            changed = true;
                        }
                    }
                }
                if !changed {
                    app.warnings.push(format!("update_text: item {target_id} not found or not text"));
                }
            }
            Operation::RemoveText { target_id } => {
                cmds.push(Command::DeleteItem { item_id: target_id.clone() });
            }
            Operation::AdjustAudio { params } => {
                let mut master = project.audio.clone();
                if let Some(v) = params.normalize { master.normalize = v; }
                if let Some(v) = params.denoise { master.denoise = v; }
                if let Some(v) = params.remove_silence { master.remove_silence = v; }
                if let Some(v) = params.duck_music_under_speech { master.duck_music_under_speech = v; }
                if let Some(v) = params.fade_in { master.fade_in_s = v; }
                if let Some(v) = params.fade_out { master.fade_out_s = v; }
                cmds.push(Command::SetAudioMaster { new: master });
                if let Some(vol) = params.volume {
                    if let Some(cid) = primary_clip_id(project) {
                        cmds.push(Command::SetItemVolume { item_id: cid, volume: vol });
                    }
                }
                app.summary_parts.push("Adjusted audio".into());
            }
            Operation::SetSpeed { start, end, speed } => {
                let cid = ensure_clip(project, &mut cmds, &mut clip_created, ctx);
                let (in_ms, out_ms) = clip_range(project, &cid);
                let s_ms = s_to_ms(*start).max(in_ms);
                let e_ms = s_to_ms(*end).min(out_ms);
                if s_ms > in_ms + 20 {
                    cmds.push(Command::SplitClip { item_id: cid.clone(), at_ms: tl_for_src(project, &cid, s_ms) });
                }
                if e_ms < out_ms - 20 {
                    cmds.push(Command::SplitClip { item_id: cid.clone(), at_ms: tl_for_src(project, &cid, e_ms) });
                }
                // After splits, find the piece fully inside [s_ms, e_ms].
                let target = project
                    .tracks
                    .iter()
                    .flat_map(|t| t.items.iter())
                    .find(|i| matches!(&i.kind, ItemKind::VideoClip { source_in_ms, source_out_ms, .. }
                        if *source_in_ms >= s_ms - 20 && *source_out_ms <= e_ms + 20))
                    .map(|i| i.id.clone());
                if let Some(tid) = target {
                    cmds.push(Command::SetClipSpeed { item_id: tid, speed: *speed });
                    app.summary_parts.push(format!("Speed {speed:.2}x on segment"));
                } else {
                    app.warnings.push("set_speed: matching segment not found".into());
                }
            }
            Operation::AddMarker { time, label } => {
                cmds.push(Command::AddMarker { time_ms: s_to_ms(*time), label: label.clone() });
            }
            Operation::SetExportPreset { preset } => {
                if let Some(p) = preset_settings(preset, project) {
                    cmds.push(Command::SetExport { settings: p });
                    app.summary_parts.push(format!("Export preset: {preset:?}"));
                }
            }
            Operation::SetAspect { .. } => unreachable!(),
        }
    }

    // One transaction: AI edit undoes as ONE step.
    mycut_core::apply_transaction(history, project, std::mem::take(&mut cmds))
        .map_err(|e| PlanError::Invalid { op: "apply".into(), reason: e.to_string() })?;
    app.repairs = result.repairs;
    Ok(app)
}

fn preset_settings(preset: &PresetId, project: &Project) -> Option<mycut_core::ExportSettings> {
    let id = match preset {
        PresetId::Youtube => "youtube",
        PresetId::YoutubeShorts => "youtube_shorts",
        PresetId::InstagramReels => "instagram_reels",
        PresetId::Tiktok => "tiktok",
        PresetId::Discord => "discord",
        PresetId::TwitterX => "twitter_x",
        PresetId::Custom => "custom",
    };
    let presets = mycut_engine::preset::builtin_presets();
    let mut s = presets.iter().find(|p| p.id == id)?.settings.clone();
    // Keep timeline-derived duration untouched; adjust dims to reframe ratio.
    if let Some(r) = &project.reframe {
        let (w, h) = mycut_engine::preset::dimensions_for(r.ratio, 1920, 1080);
        s.width = w;
        s.height = h;
    }
    Some(s)
}

fn caption_style(id: crate::plan::CaptionStyleId) -> mycut_core::CaptionStyle {
    match id {
        crate::plan::CaptionStyleId::Minimal => mycut_core::CaptionStyle::Minimal,
        crate::plan::CaptionStyleId::Gaming => mycut_core::CaptionStyle::Gaming,
        crate::plan::CaptionStyleId::Tiktok => mycut_core::CaptionStyle::Tiktok,
        crate::plan::CaptionStyleId::Youtube => mycut_core::CaptionStyle::Youtube,
        crate::plan::CaptionStyleId::Cinematic => mycut_core::CaptionStyle::Cinematic,
        crate::plan::CaptionStyleId::Bold => mycut_core::CaptionStyle::Bold,
        crate::plan::CaptionStyleId::Karaoke => mycut_core::CaptionStyle::Karaoke,
        crate::plan::CaptionStyleId::WordHighlight => mycut_core::CaptionStyle::WordHighlight,
        crate::plan::CaptionStyleId::Streamer => mycut_core::CaptionStyle::Streamer,
    }
}

fn effect_def_id(e: &crate::plan::EffectId) -> &'static str {
    match e {
        crate::plan::EffectId::Zoom => "zoom",
        crate::plan::EffectId::ZoomPunch => "zoom_punch",
        crate::plan::EffectId::Shake => "shake",
        crate::plan::EffectId::Blur => "blur",
        crate::plan::EffectId::Sharpen => "sharpen",
        crate::plan::EffectId::Vignette => "vignette",
        crate::plan::EffectId::RgbSplit => "rgb_split",
        crate::plan::EffectId::Grain => "grain",
        crate::plan::EffectId::Pixelate => "pixelate",
        crate::plan::EffectId::Flash => "flash",
        crate::plan::EffectId::CinematicBars => "cinematic_bars",
        _ => "unsupported",
    }
}

fn selection_windows(
    ctx: &PlanContext,
    selection: &Selection,
    cur_ms: TimeMs,
    target_ms: TimeMs,
) -> Vec<(TimeMs, TimeMs)> {
    let dur_s = ctx.source_duration_s;
    let keep_total = target_ms as f64 / 1000.0;
    match selection {
        Selection::KeepStart => vec![(0, target_ms)],
        Selection::KeepEnd => vec![(cur_ms - target_ms, cur_ms)],
        Selection::Even => {
            let n = 4;
            let per = keep_total / n as f64;
            let stride = dur_s / n as f64;
            (0..n).map(|i| {
                let s = (i as f64 * stride).min((dur_s - per).max(0.0));
                (s_to_ms(s), s_to_ms((s + per).min(dur_s)))
            }).collect()
        }
        Selection::Highlights => {
            if ctx.highlights.is_empty() {
                return vec![(0, target_ms)];
            }
            let mut wins: Vec<(TimeMs, TimeMs)> = Vec::new();
            let mut total = 0;
            for (s, e, _score) in &ctx.highlights {
                if total >= target_ms {
                    break;
                }
                let len = s_to_ms(e - s).min(target_ms - total);
                wins.push((s_to_ms(*s), s_to_ms(*s) + len));
                total += len;
            }
            if wins.is_empty() {
                return vec![(0, target_ms)];
            }
            wins.sort();
            wins
        }
    }
}

fn primary_clip_id(project: &Project) -> Option<String> {
    project
        .track_of_kind(mycut_core::TrackKind::Video)
        .and_then(|t| t.items.iter().find(|i| matches!(i.kind, ItemKind::VideoClip { .. })))
        .map(|i| i.id.clone())
}

fn ensure_clip(
    project: &Project,
    cmds: &mut Vec<Command>,
    created: &mut bool,
    ctx: &PlanContext,
) -> String {
    if let Some(id) = primary_clip_id(project) {
        return id;
    }
    // Create a full-length clip from the first source.
    let src = project
        .sources
        .first()
        .cloned()
        .expect("context guarantees a source");
    let item = Item::new(
        ItemKind::VideoClip {
            source_id: src.id.clone(),
            source_in_ms: 0,
            source_out_ms: src.duration_ms.min(s_to_ms(ctx.source_duration_s)),
            speed: 1.0,
        },
        0,
        src.duration_ms.min(s_to_ms(ctx.source_duration_s)),
    );
    let id = item.id.clone();
    cmds.push(Command::AddClip { track_kind: mycut_core::TrackKind::Video, item });
    *created = true;
    id
}

fn clip_range(project: &Project, cid: &str) -> (TimeMs, TimeMs) {
    project
        .tracks
        .iter()
        .flat_map(|t| t.items.iter())
        .find(|i| i.id == cid)
        .and_then(|i| match &i.kind {
            ItemKind::VideoClip { source_in_ms, source_out_ms, .. } => Some((*source_in_ms, *source_out_ms)),
            _ => None,
        })
        .unwrap_or((0, 0))
}

fn clip_len(project: &Project, cid: &str) -> Option<TimeMs> {
    let (a, b) = clip_range(project, cid);
    Some(b - a)
}

/// Build commands that remove `cuts` from the clip: trim the primary to the
/// first keep-segment and insert the rest as new clips (timeline-compacted).
fn cut_commands(project: &Project, cid: &str, cuts: &[(TimeMs, TimeMs)]) -> Vec<Command> {
    let mut cmds = Vec::new();
    let Some((in_ms, out_ms)) = (project
        .tracks
        .iter()
        .flat_map(|t| t.items.iter())
        .find(|i| i.id == cid)
        .and_then(|i| match &i.kind {
            ItemKind::VideoClip { source_in_ms, source_out_ms, .. } => Some((*source_in_ms, *source_out_ms)),
            _ => None,
        })) else {
        return cmds;
    };
    // Build keep segments within [in_ms, out_ms].
    let mut sorted: Vec<(TimeMs, TimeMs)> = cuts.to_vec();
    sorted.sort();
    let mut keeps: Vec<(TimeMs, TimeMs)> = Vec::new();
    let mut cursor = in_ms;
    for (s, e) in sorted {
        let s = s.clamp(in_ms, out_ms);
        let e = e.clamp(in_ms, out_ms);
        if s > cursor {
            keeps.push((cursor, s));
        }
        cursor = cursor.max(e);
    }
    if cursor < out_ms {
        keeps.push((cursor, out_ms));
    }
    if keeps.is_empty() {
        return cmds;
    }
    // Trim primary to the first keep; add others compacted on the timeline.
    let first = keeps[0];
    cmds.push(Command::TrimClip {
        item_id: cid.to_string(),
        new_source_in_ms: first.0,
        new_source_out_ms: first.1,
        new_timeline_start_ms: 0,
    });
    let mut tl = first.1 - first.0;
    for (s, e) in keeps.iter().skip(1) {
        let len = e - s;
        let item = Item::new(
            ItemKind::VideoClip {
                source_id: project
                    .tracks
                    .iter()
                    .flat_map(|t| t.items.iter())
                    .find(|i| i.id == cid)
                    .map(|i| match &i.kind {
                        ItemKind::VideoClip { source_id, .. } => source_id.clone(),
                        _ => String::new(),
                    })
                    .unwrap_or_default(),
                source_in_ms: *s,
                source_out_ms: *e,
                speed: 1.0,
            },
            tl,
            len,
        );
        cmds.push(Command::AddClip { track_kind: mycut_core::TrackKind::Video, item });
        tl += len;
    }
    cmds
}

fn tl_for_src(project: &Project, cid: &str, src_ms: TimeMs) -> TimeMs {
    project
        .tracks
        .iter()
        .flat_map(|t| t.items.iter())
        .find(|i| i.id == cid)
        .and_then(|i| match &i.kind {
            ItemKind::VideoClip { source_in_ms, speed, .. } => {
                Some(i.timeline_start_ms + ((src_ms - *source_in_ms) as f64 / *speed) as TimeMs)
            }
            _ => None,
        })
        .unwrap_or(0)
}

// Re-export for the CLI/app layers.
pub use crate::validate::ValidationResult;

#[allow(unused_imports)]
use PlanError as _PlanErrorImport;
