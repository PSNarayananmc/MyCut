//! Edit-plan v1.0 types mirroring docs/schema/edit-plan.schema.json.
//! The validator is the ONLY path from AI output to the command layer.

use serde::{Deserialize, Serialize};

pub const PLAN_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Range {
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CutStrategy {
    RemoveLowActivity,
    RemoveSilence,
    Explicit,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    Highlights,
    KeepStart,
    KeepEnd,
    Even,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OpMode {
    Create,
    Modify,
    Remove,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EffectId {
    Zoom, ZoomPunch, Shake, Blur, MotionBlur, Sharpen,
    Vignette, Glow, RgbSplit, Grain, Pixelate, FreezeFrame,
    Flash, CinematicBars, Fisheye,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ColorParam {
    Exposure, Contrast, Saturation, Vibrance, Temperature,
    Gamma, Shadows, Highlights, LutIntensity,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReframeMode {
    Center,
    SubjectFollow,
    Smart,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CaptionStyleId {
    Minimal, Gaming, Tiktok, Youtube, Cinematic, Bold, Karaoke, WordHighlight, Streamer,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SafeArea {
    Default,
    AvoidCenter,
    Bottom,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TransitionId {
    Cut, Fade, Crossfade, DipToBlack, DipToWhite, Slide, Push, Zoom, Blur, Wipe,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TextKindId {
    Title, LowerThird, Callout, Watermark,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PositionId {
    TopLeft, TopRight, BottomLeft, BottomRight, Center, TopCenter, BottomCenter,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PresetId {
    Youtube, YoutubeShorts, InstagramReels, Tiktok, Discord, TwitterX, Custom,
}

/// Color parameters (lut is a validated path string).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ColorParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exposure: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contrast: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vibrance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gamma: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadows: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlights: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lut: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lut_intensity: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AudioParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalize: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denoise: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove_silence: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duck_music_under_speech: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fade_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fade_out: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    CutRanges {
        strategy: CutStrategy,
        ranges: Vec<Range>,
        #[serde(default)]
        reason: String,
    },
    KeepRanges {
        ranges: Vec<Range>,
        #[serde(default)]
        reason: String,
    },
    SetTargetDuration {
        seconds: f64,
        selection: Selection,
    },
    AddEffect {
        effect: EffectId,
        start: f64,
        end: f64,
        #[serde(default)]
        params: std::collections::BTreeMap<String, serde_json::Value>,
        #[serde(default)]
        mode: Option<OpMode>,
        #[serde(default)]
        reason: String,
    },
    RemoveEffect {
        effect: String,
    },
    SetColor {
        params: ColorParams,
        #[serde(default = "default_true")]
        merge_with_existing: bool,
    },
    SetAspect {
        ratio: RatioId,
        reframe: ReframeMode,
    },
    GenerateCaptions {
        style: CaptionStyleId,
        #[serde(default)]
        language: Option<String>,
        #[serde(default)]
        safe_area: Option<SafeArea>,
        #[serde(default)]
        max_words_per_line: Option<u32>,
        #[serde(default)]
        scale: Option<f64>,
    },
    UpdateCaptions {
        #[serde(default)]
        style: Option<CaptionStyleId>,
        #[serde(default)]
        scale: Option<f64>,
        #[serde(default)]
        safe_area: Option<SafeArea>,
    },
    RemoveCaptions,
    AddTransition {
        transition: TransitionId,
        #[serde(default)]
        at_index: Option<u32>,
        #[serde(default)]
        duration: Option<f64>,
    },
    AddText {
        text: String,
        start: f64,
        end: f64,
        kind: TextKindId,
        #[serde(default)]
        position: Option<PositionId>,
        #[serde(default)]
        scale: Option<f64>,
        #[serde(default)]
        opacity: Option<f64>,
    },
    UpdateText {
        target_id: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        scale: Option<f64>,
        #[serde(default)]
        opacity: Option<f64>,
        #[serde(default)]
        position: Option<PositionId>,
    },
    RemoveText {
        target_id: String,
    },
    AdjustAudio {
        params: AudioParams,
    },
    SetSpeed {
        start: f64,
        end: f64,
        speed: f64,
    },
    AddMarker {
        time: f64,
        #[serde(default)]
        label: String,
    },
    SetExportPreset {
        preset: PresetId,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RatioId {
    #[serde(rename = "16:9")]
    R16x9,
    #[serde(rename = "9:16")]
    R9x16,
    #[serde(rename = "1:1")]
    R1x1,
    #[serde(rename = "4:5")]
    R4x5,
}

/// Top-level plan document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditPlan {
    pub schema_version: String,
    pub intent_summary: String,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_clarification: Option<String>,
    #[serde(default)]
    pub operations: Vec<Operation>,
}

impl EditPlan {
    /// Parse from JSON with strict schema checks (unknown fields rejected).
    ///
    /// # Errors
    /// [`PlanError::Serde`] with the serde path on failure.
    pub fn parse(json: &str) -> Result<Self, PlanError> {
        let de = &mut serde_json::Deserializer::from_str(json);
        let plan: EditPlan = serde_path_to_error::deserialize(de)
            .map_err(|e| PlanError::Serde(format!("{} at {}", e, e.path())))?;
        if plan.schema_version != PLAN_SCHEMA_VERSION {
            return Err(PlanError::SchemaVersion(plan.schema_version));
        }
        Ok(plan)
    }
}

/// Typed plan errors surfaced to the model / UI.
#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum PlanError {
    #[error("invalid JSON: {0}")]
    Serde(String),
    #[error("unsupported plan schema version: {0}")]
    SchemaVersion(String),
    #[error("{op}: {reason}")]
    Invalid { op: String, reason: String },
    #[error("{op}: {field}={value} out of range [{min}, {max}]")]
    OutOfRange { op: String, field: String, value: f64, min: f64, max: f64 },
    #[error("{op}: unsafe path {path:?}")]
    UnsafePath { op: String, path: String },
    #[error("range {0}-{1} invalid or outside source")]
    RangeError(f64, f64),
}
