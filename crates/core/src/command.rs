//! Command layer: every mutation (manual or AI) is a [`Command`] with an
//! inverse. An AI plan is applied as one [`apply_transaction`] so a single
//! `Ctrl+Z` reverts the whole AI edit. History is serializable (undo survives
//! restarts) and bounded.
//!
//! Inverse strategy: scalar/replace operations capture exact prior values
//! (cheap, precise). Structural operations (split) invert via
//! [`Command::SetTrackItems`], a bounded snapshot of one track's item list
//! (items are instructions, not pixels — a few KB at most). See DECISIONS.md.

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::model::{
    CaptionStyle as CaptionsStyle, ColorGrade, ExportSettings, Item, ItemKind, Project,
    ReframeSettings, TrackKind,
};
use crate::time::TimeMs;

/// A validated, invertible mutation. `apply` returns its inverse; history
/// records the pair. Variants marked *internal* appear only inside serialized
/// history entries, never from user/AI input paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    AddSource {
        source: crate::model::Source,
    },
    RemoveSource {
        source_id: String,
    },
    AddClip {
        track_kind: TrackKind,
        item: Item,
    },
    SplitClip {
        item_id: String,
        /// TIMELINE time where to split.
        at_ms: TimeMs,
    },
    /// Replace a track's whole item list (used for reorder + split inverse).
    /// *Internal* when used as an inverse; also exposed for manual reordering.
    SetTrackItems {
        track_kind: TrackKind,
        items: Vec<Item>,
    },
    TrimClip {
        item_id: String,
        new_source_in_ms: TimeMs,
        new_source_out_ms: TimeMs,
        new_timeline_start_ms: TimeMs,
    },
    MoveItem {
        item_id: String,
        new_timeline_start_ms: TimeMs,
    },
    DeleteItem {
        item_id: String,
    },
    SetItemVolume {
        item_id: String,
        volume: f64,
    },
    SetItemOpacity {
        item_id: String,
        opacity: f64,
    },
    SetClipSpeed {
        item_id: String,
        speed: f64,
    },
    SetColor {
        new_grade: ColorGrade,
    },
    SetReframe {
        settings: Option<ReframeSettings>,
    },
    SetExport {
        settings: ExportSettings,
    },
    AddMarker {
        time_ms: TimeMs,
        label: String,
    },
    RemoveMarker {
        time_ms: TimeMs,
    },
    AddEffect {
        item_id: String,
        effect: crate::model::EffectInstance,
    },
    RemoveEffect {
        item_id: String,
        effect_id: String,
    },
    SetTransitions {
        transitions: Vec<crate::model::TransitionSetting>,
    },
    SetAudioMaster {
        new: crate::model::AudioMaster,
    },
    /// Replace the whole captions item content (used by CaptionEngine).
    ReplaceCaptions {
        style: CaptionsStyle,
        scale: f64,
        safe_area: String,
        entries_json: String,
    },
    RemoveCaptions,
    /// *Internal.* Restores a previously removed captions item.
    RestoreCaptionsItem {
        item: Item,
    },
}

impl Command {
    /// Human label for history UI.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Command::AddSource { .. } => "Import media",
            Command::RemoveSource { .. } => "Remove media",
            Command::AddClip { .. } => "Add clip",
            Command::SplitClip { .. } => "Split clip",
            Command::SetTrackItems { .. } => "Rearrange items",
            Command::TrimClip { .. } => "Trim clip",
            Command::MoveItem { .. } => "Move item",
            Command::DeleteItem { .. } => "Delete item",
            Command::SetItemVolume { .. } => "Set volume",
            Command::SetItemOpacity { .. } => "Set opacity",
            Command::SetClipSpeed { .. } => "Set speed",
            Command::SetColor { .. } => "Color grade",
            Command::SetReframe { .. } => "Reframe",
            Command::SetExport { .. } => "Export settings",
            Command::AddMarker { .. } => "Add marker",
            Command::RemoveMarker { .. } => "Remove marker",
            Command::AddEffect { .. } => "Add effect",
            Command::RemoveEffect { .. } => "Remove effect",
            Command::SetTransitions { .. } => "Set transitions",
            Command::SetAudioMaster { .. } => "Audio settings",
            Command::ReplaceCaptions { .. } => "Generate captions",
            Command::RemoveCaptions => "Remove captions",
            Command::RestoreCaptionsItem { .. } => "Restore captions",
        }
    }

    /// Apply the command; returns the inverse command (capturing prior state).
    ///
    /// # Errors
    /// [`CoreError`] when the mutation is invalid (bad ids, out-of-range
    /// times, locked tracks...). The project is left unchanged on error.
    pub fn apply(&self, p: &mut Project) -> Result<Command, CoreError> {
        match self {
            Command::AddSource { source } => {
                if p.sources.iter().any(|s| s.id == source.id) {
                    return Err(CoreError::Invalid("duplicate source id".into()));
                }
                p.sources.push(source.clone());
                Ok(Command::RemoveSource {
                    source_id: source.id.clone(),
                })
            }
            Command::RemoveSource { source_id } => {
                let idx = p
                    .sources
                    .iter()
                    .position(|s| s.id == *source_id)
                    .ok_or_else(|| CoreError::NotFound(format!("source {source_id}")))?;
                let in_use = p.tracks.iter().flat_map(|t| &t.items).any(|i| {
                    matches!(
                        &i.kind,
                        ItemKind::VideoClip { source_id: sid, .. }
                            | ItemKind::AudioClip { source_id: sid, .. } if sid == source_id
                    )
                });
                if in_use {
                    return Err(CoreError::Invalid(
                        "source is in use; delete clips first".into(),
                    ));
                }
                let source = p.sources.remove(idx);
                Ok(Command::AddSource { source })
            }
            Command::AddClip { track_kind, item } => {
                if let ItemKind::VideoClip {
                    source_id,
                    source_in_ms,
                    source_out_ms,
                    speed,
                }
                | ItemKind::AudioClip {
                    source_id,
                    source_in_ms,
                    source_out_ms,
                    speed,
                } = &item.kind
                {
                    let src = p
                        .source(source_id)
                        .ok_or_else(|| CoreError::NotFound(format!("source {source_id}")))?;
                    if *source_in_ms < 0 || *source_out_ms > src.duration_ms {
                        return Err(CoreError::Range(
                            "clip range outside source duration".into(),
                        ));
                    }
                    if source_out_ms <= source_in_ms {
                        return Err(CoreError::Range("source_out must be > source_in".into()));
                    }
                    if !(*speed > 0.0 && (0.25..=4.0).contains(speed)) {
                        return Err(CoreError::Range("speed must be in [0.25, 4]".into()));
                    }
                }
                let track = p
                    .track_of_kind_mut(*track_kind)
                    .ok_or_else(|| CoreError::NotFound(format!("track {track_kind:?}")))?;
                if track.locked {
                    return Err(CoreError::Locked("track is locked".into()));
                }
                track.items.push(item.clone());
                Ok(Command::DeleteItem {
                    item_id: item.id.clone(),
                })
            }
            Command::SplitClip { item_id, at_ms } => {
                let (track_idx, item_idx) = locate(p, item_id)?;
                if p.tracks[track_idx].locked {
                    return Err(CoreError::Locked("track is locked".into()));
                }
                let before: Vec<Item> = p.tracks[track_idx].items.clone();
                let item = p.tracks[track_idx].items[item_idx].clone();
                let start = item.timeline_start_ms;
                let end = start + item.timeline_duration_ms;
                if *at_ms <= start + 1 || *at_ms >= end - 1 {
                    return Err(CoreError::Range("split point too close to edges".into()));
                }
                let left_len = at_ms - start;
                let right_len = end - at_ms;
                let speed = match &item.kind {
                    ItemKind::VideoClip { speed, .. } | ItemKind::AudioClip { speed, .. } => *speed,
                    _ => 1.0,
                };
                let mut left = item.clone();
                left.timeline_duration_ms = left_len;
                let mut right = item.clone();
                right.id = crate::time::new_id("item");
                right.timeline_start_ms = *at_ms;
                right.timeline_duration_ms = right_len;
                match (&mut right.kind, &mut left.kind) {
                    (
                        ItemKind::VideoClip { source_in_ms, .. },
                        ItemKind::VideoClip { source_out_ms, .. },
                    )
                    | (
                        ItemKind::AudioClip { source_in_ms, .. },
                        ItemKind::AudioClip { source_out_ms, .. },
                    ) => {
                        let delta_src = (left_len as f64 * speed).round() as TimeMs;
                        *source_in_ms += delta_src;
                        *source_out_ms -= (right_len as f64 * speed).round() as TimeMs;
                    }
                    _ => {}
                }
                // Windowed effects stay on the left; whole-item effects duplicate.
                right
                    .effects
                    .retain(|e| e.window_start_ms.is_none() && e.window_end_ms.is_none());
                let track = &mut p.tracks[track_idx];
                let kind = track.kind;
                track.items[item_idx] = left;
                track.items.insert(item_idx + 1, right);
                // Exact inverse: restore the whole pre-split item list.
                Ok(Command::SetTrackItems {
                    track_kind: kind,
                    items: before,
                })
            }
            Command::SetTrackItems { track_kind, items } => {
                let track = p
                    .track_of_kind_mut(*track_kind)
                    .ok_or_else(|| CoreError::NotFound(format!("track {track_kind:?}")))?;
                if track.locked {
                    return Err(CoreError::Locked("track is locked".into()));
                }
                let before = track.items.clone();
                track.items = items.clone();
                Ok(Command::SetTrackItems {
                    track_kind: *track_kind,
                    items: before,
                })
            }
            Command::TrimClip {
                item_id,
                new_source_in_ms,
                new_source_out_ms,
                new_timeline_start_ms,
            } => {
                let (track_idx, item_idx) = locate(p, item_id)?;
                if p.tracks[track_idx].locked {
                    return Err(CoreError::Locked("track is locked".into()));
                }
                let item = p.tracks[track_idx].items[item_idx].clone();
                let (old_in, old_out, old_start, speed, source_id) = match &item.kind {
                    ItemKind::VideoClip {
                        source_id,
                        source_in_ms,
                        source_out_ms,
                        speed,
                    }
                    | ItemKind::AudioClip {
                        source_id,
                        source_in_ms,
                        source_out_ms,
                        speed,
                    } => (
                        *source_in_ms,
                        *source_out_ms,
                        item.timeline_start_ms,
                        *speed,
                        source_id.clone(),
                    ),
                    _ => return Err(CoreError::Invalid("only clips can be trimmed".into())),
                };
                let src = p
                    .source(&source_id)
                    .ok_or_else(|| CoreError::NotFound(format!("source {source_id}")))?;
                if *new_source_in_ms < 0 || *new_source_out_ms > src.duration_ms {
                    return Err(CoreError::Range("trim outside source duration".into()));
                }
                if new_source_out_ms <= new_source_in_ms {
                    return Err(CoreError::Range("source_out must be > source_in".into()));
                }
                let item = p.tracks[track_idx].items.get_mut(item_idx).unwrap();
                if let ItemKind::VideoClip {
                    source_in_ms,
                    source_out_ms,
                    ..
                }
                | ItemKind::AudioClip {
                    source_in_ms,
                    source_out_ms,
                    ..
                } = &mut item.kind
                {
                    *source_in_ms = *new_source_in_ms;
                    *source_out_ms = *new_source_out_ms;
                }
                item.timeline_start_ms = *new_timeline_start_ms;
                item.timeline_duration_ms =
                    ((new_source_out_ms - new_source_in_ms) as f64 / speed).round() as TimeMs;
                Ok(Command::TrimClip {
                    item_id: item_id.clone(),
                    new_source_in_ms: old_in,
                    new_source_out_ms: old_out,
                    new_timeline_start_ms: old_start,
                })
            }
            Command::MoveItem {
                item_id,
                new_timeline_start_ms,
            } => {
                let (track_idx, item_idx) = locate(p, item_id)?;
                if p.tracks[track_idx].locked {
                    return Err(CoreError::Locked("track is locked".into()));
                }
                let item = &mut p.tracks[track_idx].items[item_idx];
                let old = item.timeline_start_ms;
                if *new_timeline_start_ms < 0 {
                    return Err(CoreError::Range("timeline_start must be >= 0".into()));
                }
                item.timeline_start_ms = *new_timeline_start_ms;
                Ok(Command::MoveItem {
                    item_id: item_id.clone(),
                    new_timeline_start_ms: old,
                })
            }
            Command::DeleteItem { item_id } => {
                let (track_idx, item_idx) = locate(p, item_id)?;
                if p.tracks[track_idx].locked {
                    return Err(CoreError::Locked("track is locked".into()));
                }
                let kind = p.tracks[track_idx].kind;
                let item = p.tracks[track_idx].items.remove(item_idx);
                Ok(Command::AddClip {
                    track_kind: kind,
                    item,
                })
            }
            Command::SetItemVolume { item_id, volume } => {
                let item = p
                    .find_item_mut(item_id)
                    .ok_or_else(|| CoreError::NotFound(format!("item {item_id}")))?;
                if !(0.0..=4.0).contains(volume) {
                    return Err(CoreError::Range("volume must be in [0, 4]".into()));
                }
                let old = item.volume;
                item.volume = *volume;
                Ok(Command::SetItemVolume {
                    item_id: item_id.clone(),
                    volume: old,
                })
            }
            Command::SetItemOpacity { item_id, opacity } => {
                let item = p
                    .find_item_mut(item_id)
                    .ok_or_else(|| CoreError::NotFound(format!("item {item_id}")))?;
                if !(0.0..=1.0).contains(opacity) {
                    return Err(CoreError::Range("opacity must be in [0, 1]".into()));
                }
                let old = item.opacity;
                item.opacity = *opacity;
                Ok(Command::SetItemOpacity {
                    item_id: item_id.clone(),
                    opacity: old,
                })
            }
            Command::SetClipSpeed { item_id, speed } => {
                let item = p
                    .find_item_mut(item_id)
                    .ok_or_else(|| CoreError::NotFound(format!("item {item_id}")))?;
                if !(*speed > 0.0 && (0.25..=4.0).contains(speed)) {
                    return Err(CoreError::Range("speed must be in [0.25, 4]".into()));
                }
                match &mut item.kind {
                    ItemKind::VideoClip {
                        speed: s,
                        source_in_ms,
                        source_out_ms,
                        ..
                    }
                    | ItemKind::AudioClip {
                        speed: s,
                        source_in_ms,
                        source_out_ms,
                        ..
                    } => {
                        let old = *s;
                        let src_len = *source_out_ms - *source_in_ms;
                        *s = *speed;
                        item.timeline_duration_ms = (src_len as f64 / speed).round() as TimeMs;
                        Ok(Command::SetClipSpeed {
                            item_id: item_id.clone(),
                            speed: old,
                        })
                    }
                    _ => Err(CoreError::Invalid("only clips support speed".into())),
                }
            }
            Command::SetColor { new_grade } => {
                let old = p.color.clone();
                p.color = new_grade.clone();
                Ok(Command::SetColor { new_grade: old })
            }
            Command::SetReframe { settings } => {
                let old = p.reframe.clone();
                p.reframe = settings.clone();
                Ok(Command::SetReframe { settings: old })
            }
            Command::SetExport { settings } => {
                let old = p.export.clone();
                p.export = settings.clone();
                Ok(Command::SetExport { settings: old })
            }
            Command::AddMarker { time_ms, label } => {
                p.markers.push(crate::model::Marker {
                    time_ms: *time_ms,
                    label: label.clone(),
                });
                Ok(Command::RemoveMarker { time_ms: *time_ms })
            }
            Command::RemoveMarker { time_ms } => {
                let pos = p
                    .markers
                    .iter()
                    .position(|m| m.time_ms == *time_ms)
                    .ok_or_else(|| CoreError::NotFound("marker".into()))?;
                let m = p.markers.remove(pos);
                Ok(Command::AddMarker {
                    time_ms: m.time_ms,
                    label: m.label,
                })
            }
            Command::AddEffect { item_id, effect } => {
                let item = p
                    .find_item_mut(item_id)
                    .ok_or_else(|| CoreError::NotFound(format!("item {item_id}")))?;
                if item.effects.iter().any(|e| e.id == effect.id) {
                    return Err(CoreError::Invalid("duplicate effect id".into()));
                }
                item.effects.push(effect.clone());
                Ok(Command::RemoveEffect {
                    item_id: item_id.clone(),
                    effect_id: effect.id.clone(),
                })
            }
            Command::RemoveEffect { item_id, effect_id } => {
                let item = p
                    .find_item_mut(item_id)
                    .ok_or_else(|| CoreError::NotFound(format!("item {item_id}")))?;
                let pos = item
                    .effects
                    .iter()
                    .position(|e| e.id == *effect_id)
                    .ok_or_else(|| CoreError::NotFound(format!("effect {effect_id}")))?;
                let effect = item.effects.remove(pos);
                Ok(Command::AddEffect {
                    item_id: item_id.clone(),
                    effect,
                })
            }
            Command::SetTransitions { transitions } => {
                let old = p.transitions.clone();
                p.transitions = transitions.clone();
                Ok(Command::SetTransitions { transitions: old })
            }
            Command::SetAudioMaster { new } => {
                let old = p.audio.clone();
                p.audio = new.clone();
                Ok(Command::SetAudioMaster { new: old })
            }
            Command::ReplaceCaptions {
                style,
                scale,
                safe_area,
                entries_json,
            } => {
                let entries: Vec<crate::model::CaptionEntry> =
                    serde_json::from_str(entries_json)
                        .map_err(|e| CoreError::Invalid(format!("captions entries: {e}")))?;
                let timeline_end = p.timeline_end_ms();
                let track = p
                    .track_of_kind_mut(TrackKind::Captions)
                    .ok_or_else(|| CoreError::NotFound("captions track".into()))?;
                let old_item = track
                    .items
                    .iter()
                    .find(|i| matches!(i.kind, ItemKind::Captions { .. }))
                    .cloned();
                let new_kind = ItemKind::Captions {
                    style: *style,
                    scale: *scale,
                    safe_area: safe_area.clone(),
                    entries,
                };
                let inverse = match old_item.clone() {
                    Some(it) => Command::RestoreCaptionsItem { item: it },
                    None => Command::RemoveCaptions,
                };
                track
                    .items
                    .retain(|i| !matches!(i.kind, ItemKind::Captions { .. }));
                let mut new_item = Item::new(new_kind, 0, timeline_end);
                if let Some(old) = old_item {
                    new_item.id = old.id;
                    new_item.timeline_start_ms = old.timeline_start_ms;
                    new_item.timeline_duration_ms = old.timeline_duration_ms;
                }
                track.items.push(new_item);
                Ok(inverse)
            }
            Command::RemoveCaptions => {
                let track = p
                    .track_of_kind_mut(TrackKind::Captions)
                    .ok_or_else(|| CoreError::NotFound("captions track".into()))?;
                let pos = track
                    .items
                    .iter()
                    .position(|i| matches!(i.kind, ItemKind::Captions { .. }))
                    .ok_or_else(|| CoreError::NotFound("captions item".into()))?;
                let item = track.items.remove(pos);
                Ok(Command::RestoreCaptionsItem { item })
            }
            Command::RestoreCaptionsItem { item } => {
                let track = p
                    .track_of_kind_mut(TrackKind::Captions)
                    .ok_or_else(|| CoreError::NotFound("captions track".into()))?;
                track
                    .items
                    .retain(|i| !matches!(i.kind, ItemKind::Captions { .. }));
                let restored = item.clone();
                track.items.push(restored);
                Ok(Command::RemoveCaptions)
            }
        }
    }
}

fn locate(p: &Project, item_id: &str) -> Result<(usize, usize), CoreError> {
    for (ti, t) in p.tracks.iter().enumerate() {
        if let Some(ii) = t.items.iter().position(|i| i.id == item_id) {
            return Ok((ti, ii));
        }
    }
    Err(CoreError::NotFound(format!("item {item_id}")))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoEntry {
    pub label: String,
    /// Commands that undo the recorded action, in apply order (reversed).
    pub inverse: Vec<Command>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedoEntry {
    pub label: String,
    /// Forward commands to re-apply, in apply order.
    pub forward: Vec<Command>,
}

/// Bounded, restart-surviving history.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct History {
    pub undo_stack: Vec<UndoEntry>,
    pub redo_stack: Vec<RedoEntry>,
    pub max_len: usize,
}

impl History {
    #[must_use]
    pub fn new() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_len: 200,
        }
    }

    /// Apply a command and record its inverse (label = original command's).
    ///
    /// # Errors
    /// Propagates [`CoreError`]; history is unchanged on error.
    pub fn apply(&mut self, p: &mut Project, cmd: Command) -> Result<(), CoreError> {
        let label = cmd.label().to_string();
        let inverse = cmd.apply(p)?;
        self.push_undo(UndoEntry {
            label,
            inverse: vec![inverse],
        });
        self.redo_stack.clear();
        Ok(())
    }

    fn push_undo(&mut self, entry: UndoEntry) {
        self.undo_stack.push(entry);
        if self.undo_stack.len() > self.max_len {
            self.undo_stack.remove(0);
        }
    }

    /// Undo the last action; returns its label.
    ///
    /// # Errors
    /// [`CoreError::NothingToUndo`] when empty.
    pub fn undo(&mut self, p: &mut Project) -> Result<String, CoreError> {
        let entry = self.undo_stack.pop().ok_or(CoreError::NothingToUndo)?;
        // Inverses were recorded in forward order; undo applies them reversed.
        // Each inverse.apply() returns a forward-equivalent command for redo.
        let mut forward = Vec::with_capacity(entry.inverse.len());
        for inv in entry.inverse.iter().rev() {
            forward.push(inv.apply(p)?);
        }
        forward.reverse();
        self.redo_stack.push(RedoEntry {
            label: entry.label.clone(),
            forward,
        });
        Ok(entry.label)
    }

    /// Redo a previously undone action; returns its label.
    ///
    /// # Errors
    /// [`CoreError::NothingToRedo`] when empty.
    pub fn redo(&mut self, p: &mut Project) -> Result<String, CoreError> {
        let entry = self.redo_stack.pop().ok_or(CoreError::NothingToRedo)?;
        let mut inverse = Vec::with_capacity(entry.forward.len());
        for fwd in entry.forward.iter() {
            inverse.push(fwd.apply(p)?);
        }
        self.push_undo(UndoEntry {
            label: entry.label.clone(),
            inverse,
        });
        Ok(entry.label)
    }
}

/// Apply a sequence of commands atomically: any failure rolls back every
/// already-applied command. The whole transaction undoes as ONE step.
///
/// # Errors
/// [`CoreError`] from the first failing command, after rollback.
pub fn apply_transaction(
    history: &mut History,
    p: &mut Project,
    cmds: Vec<Command>,
) -> Result<usize, CoreError> {
    let mut applied_inverses: Vec<Command> = Vec::new();
    for cmd in &cmds {
        match cmd.apply(p) {
            Ok(inverse) => applied_inverses.push(inverse),
            Err(e) => {
                for inv in applied_inverses.iter().rev() {
                    // Best-effort rollback; report the original error.
                    let _ = inv.apply(p);
                }
                return Err(e);
            }
        }
    }
    history.push_undo(UndoEntry {
        label: "AI edit".to_string(),
        inverse: applied_inverses,
    });
    history.redo_stack.clear();
    Ok(cmds.len())
}
