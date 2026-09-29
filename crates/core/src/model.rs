//! Canonical project model: the single source of truth.
//!
//! Both the manual UI and the AI mutate this model **only** through
//! [`crate::command::Command`] values. The model stores instructions, never
//! pixels; sources are read-only references to media files.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::time::{new_id, TimeMs};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MediaRole {
    #[default]
    Footage,
    Music,
    Logo,
    Voiceover,
    Sfx,
    Image,
}

/// Read-only reference to an imported media file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub name: String,
    /// Project-relative path (portable); resolved against the project dir.
    pub rel_path: String,
    /// BLAKE2s-256-derived content hash for cache keys and relink.
    pub content_hash: String,
    pub duration_ms: TimeMs,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub has_audio: bool,
    pub role: MediaRole,
}

impl Source {
    #[must_use]
    pub fn fps(&self) -> f64 {
        if self.fps_den == 0 {
            30.0
        } else {
            self.fps_num as f64 / self.fps_den as f64
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Video,
    Audio,
    Text,
    Captions,
    Overlay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub kind: TrackKind,
    pub name: String,
    pub muted: bool,
    pub solo: bool,
    pub locked: bool,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextKind {
    Title,
    LowerThird,
    Callout,
    Watermark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
    TopCenter,
    BottomCenter,
}

/// One caption word with per-word timing (source of karaoke/highlight).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptionWord {
    pub text: String,
    pub start_ms: TimeMs,
    pub end_ms: TimeMs,
    pub emphasize: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptionEntry {
    /// Timeline-domain start.
    pub start_ms: TimeMs,
    /// Timeline-domain end.
    pub end_ms: TimeMs,
    pub words: Vec<CaptionWord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionStyle {
    Minimal,
    Gaming,
    Tiktok,
    Youtube,
    Cinematic,
    Bold,
    Karaoke,
    WordHighlight,
    Streamer,
}

/// Item kinds live in one enum; a track only accepts matching kinds
/// (enforced by command validation).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "item_kind", rename_all = "snake_case")]
pub enum ItemKind {
    VideoClip {
        source_id: String,
        /// SOURCE time.
        source_in_ms: TimeMs,
        /// SOURCE time, exclusive.
        source_out_ms: TimeMs,
        speed: f64,
    },
    AudioClip {
        source_id: String,
        source_in_ms: TimeMs,
        source_out_ms: TimeMs,
        speed: f64,
    },
    Text {
        kind: TextKind,
        text: String,
        position: Position,
        scale: f64,
        opacity: f64,
    },
    Captions {
        style: CaptionStyle,
        scale: f64,
        safe_area: String,
        entries: Vec<CaptionEntry>,
    },
}

/// A single animatable-property keyframe track.
/// Keyframe times are TIMELINE-domain milliseconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyframeTrack {
    pub property: String,
    pub easing: crate::time::Easing,
    pub keyframes: Vec<Keyframe>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyframe {
    pub t_ms: TimeMs,
    /// Value space is effect-defined (e.g. zoom factor, crop x offset px).
    pub value: f64,
}

/// Effect/audio/color parameter value (numbers cover most; text for paths).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Number(f64),
    Text(String),
}

/// An effect instance attached to an item.
/// `window_*` are SOURCE-relative when the item is a clip (documented in the
/// plan schema); `None` means "whole item".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectInstance {
    pub id: String,
    pub def_id: String,
    pub params: BTreeMap<String, ParamValue>,
    pub window_start_ms: Option<TimeMs>,
    pub window_end_ms: Option<TimeMs>,
    /// Per-effect animated property overrides.
    pub keyframes: Vec<KeyframeTrack>,
    #[serde(default)]
    pub easing: crate::time::Easing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub kind: ItemKind,
    /// TIMELINE time.
    pub timeline_start_ms: TimeMs,
    /// TIMELINE duration (derived for clips, but stored for texts/captions).
    pub timeline_duration_ms: TimeMs,
    #[serde(default = "one")]
    pub volume: f64,
    #[serde(default = "one")]
    pub opacity: f64,
    #[serde(default)]
    pub effects: Vec<EffectInstance>,
}

fn one() -> f64 {
    1.0
}

impl Item {
    #[must_use]
    pub fn new(kind: ItemKind, timeline_start_ms: TimeMs, timeline_duration_ms: TimeMs) -> Self {
        Self {
            id: new_id("item"),
            kind,
            timeline_start_ms,
            timeline_duration_ms,
            volume: 1.0,
            opacity: 1.0,
            effects: Vec::new(),
        }
    }

    /// Source-domain length accounting for speed (clips only).
    #[must_use]
    pub fn source_len_ms(&self) -> Option<TimeMs> {
        match &self.kind {
            ItemKind::VideoClip {
                source_in_ms,
                source_out_ms,
                ..
            }
            | ItemKind::AudioClip {
                source_in_ms,
                source_out_ms,
                ..
            } => Some((source_out_ms - source_in_ms).max(0)),
            _ => None,
        }
    }
}

/// Color grade (project-level; merged onto the main video track at render).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorGrade {
    pub exposure: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub vibrance: f64,
    pub temperature: f64,
    pub gamma: f64,
    pub shadows: f64,
    pub highlights: f64,
    pub lut: Option<String>,
    pub lut_intensity: f64,
}

impl Default for ColorGrade {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            vibrance: 0.0,
            temperature: 0.0,
            gamma: 1.0,
            shadows: 0.0,
            highlights: 0.0,
            lut: None,
            lut_intensity: 1.0,
        }
    }
}

impl ColorGrade {
    /// True when every value is neutral (skip color filters entirely).
    #[must_use]
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AspectRatio {
    R16x9,
    R9x16,
    R1x1,
    R4x5,
}

impl AspectRatio {
    /// (numerator, denominator)
    #[must_use]
    pub fn wh(self) -> (u32, u32) {
        match self {
            AspectRatio::R16x9 => (16, 9),
            AspectRatio::R9x16 => (9, 16),
            AspectRatio::R1x1 => (1, 1),
            AspectRatio::R4x5 => (4, 5),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReframeMode {
    Center,
    SubjectFollow,
    Smart,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReframeSettings {
    pub ratio: AspectRatio,
    pub mode: ReframeMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    H264,
    Hevc,
    Vp9,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    Aac,
    Mp3,
    Opus,
    Flac,
    PcmS16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    Mp4,
    Mkv,
    Webm,
    Mov,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSettings {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub video_codec: VideoCodec,
    pub audio_codec: AudioCodec,
    pub container: Container,
    /// CRF for H.264/HEVC; bitrate kbps otherwise.
    pub quality: u32,
    pub audio_bitrate_kbps: u32,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 30.0,
            video_codec: VideoCodec::H264,
            audio_codec: AudioCodec::Aac,
            container: Container::Mp4,
            quality: 20,
            audio_bitrate_kbps: 192,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marker {
    pub time_ms: TimeMs,
    pub label: String,
}

/// Transition between consecutive video-track items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    Cut,
    Fade,
    Crossfade,
    DipToBlack,
    DipToWhite,
    Slide,
    Push,
    Zoom,
    Wipe,
}

impl TransitionKind {
    /// The xfade transition name used by the engine. `None` = hard cut.
    #[must_use]
    pub fn ffmpeg_name(self) -> Option<&'static str> {
        match self {
            TransitionKind::Cut => None,
            TransitionKind::Fade | TransitionKind::Crossfade => Some("fade"),
            TransitionKind::DipToBlack => Some("fadeblack"),
            TransitionKind::DipToWhite => Some("fadewhite"),
            TransitionKind::Slide => Some("slideleft"),
            TransitionKind::Push => Some("pushleft"),
            TransitionKind::Zoom => Some("zoomin"),
            TransitionKind::Wipe => Some("wipeleft"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionSetting {
    /// Applied between video item `index` and `index+1` (0-based, sorted by
    /// timeline start). `Cut` means "explicitly no transition".
    pub after_index: usize,
    pub kind: TransitionKind,
    pub duration_ms: TimeMs,
}

/// Project-level master audio settings (applied after track mixing).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AudioMaster {
    pub normalize: bool,
    pub denoise: bool,
    /// Silence removal is handled at CUT level (plan `cut_ranges
    /// remove_silence`) so video+audio shorten together; an audio-only
    /// filter here would desync A/V. Kept as a field for settings compat.
    #[serde(default)]
    pub remove_silence: bool,
    pub duck_music_under_speech: bool,
    pub fade_in_s: f64,
    pub fade_out_s: f64,
}

/// The canonical project. Serialize => `.mycut` content.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Project {
    pub schema_version: String,
    pub id: String,
    pub name: String,
    pub created_at_unix_ms: i64,
    pub sources: Vec<Source>,
    pub tracks: Vec<Track>,
    pub color: ColorGrade,
    pub audio: AudioMaster,
    pub reframe: Option<ReframeSettings>,
    pub export: ExportSettings,
    pub markers: Vec<Marker>,
    #[serde(default)]
    pub transitions: Vec<TransitionSetting>,
    /// Human-readable reports of applied AI plans (diff summaries, repairs).
    pub plan_reports: Vec<PlanReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanReport {
    pub at_unix_ms: i64,
    pub request: String,
    pub intent_summary: String,
    pub summary: String,
    pub removed_ops: usize,
    pub added_ops: usize,
    pub changed_ops: usize,
    pub repairs: Vec<String>,
}

pub const SCHEMA_VERSION: &str = "1.0";

impl Project {
    /// A fresh project with the standard track stack.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            id: new_id("proj"),
            name: name.to_string(),
            created_at_unix_ms: now_unix_ms(),
            sources: Vec::new(),
            tracks: vec![
                Self::track(TrackKind::Video, "V1"),
                Self::track(TrackKind::Audio, "A1"),
                Self::track(TrackKind::Captions, "Captions"),
                Self::track(TrackKind::Text, "Text"),
            ],
            color: ColorGrade::default(),
            audio: AudioMaster::default(),
            reframe: None,
            export: ExportSettings::default(),
            markers: Vec::new(),
            transitions: Vec::new(),
            plan_reports: Vec::new(),
        }
    }

    fn track(kind: TrackKind, name: &str) -> Track {
        Track {
            id: new_id("track"),
            kind,
            name: name.to_string(),
            muted: false,
            solo: false,
            locked: false,
            items: Vec::new(),
        }
    }

    #[must_use]
    pub fn source(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }

    #[must_use]
    pub fn track_of_kind(&self, kind: TrackKind) -> Option<&Track> {
        self.tracks.iter().find(|t| t.kind == kind)
    }

    #[must_use]
    pub fn track_of_kind_mut(&mut self, kind: TrackKind) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.kind == kind)
    }

    /// End of the timeline = last item end across tracks.
    #[must_use]
    pub fn timeline_end_ms(&self) -> TimeMs {
        self.tracks
            .iter()
            .flat_map(|t| t.items.iter())
            .map(|i| i.timeline_start_ms + i.timeline_duration_ms)
            .max()
            .unwrap_or(0)
    }

    #[must_use]
    pub fn find_item_mut(&mut self, item_id: &str) -> Option<&mut Item> {
        self.tracks
            .iter_mut()
            .flat_map(|t| t.items.iter_mut())
            .find(|i| i.id == item_id)
    }

    #[must_use]
    pub fn find_effect_mut(&mut self, effect_id: &str) -> Option<&mut EffectInstance> {
        self.tracks
            .iter_mut()
            .flat_map(|t| t.items.iter_mut())
            .flat_map(|i| i.effects.iter_mut())
            .find(|e| e.id == effect_id)
    }
}

/// Wall-clock helper (testable without mocks: pure passthrough).
#[must_use]
pub fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
