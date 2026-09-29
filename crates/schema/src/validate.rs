//! Plan validation + repair. Pure Rust, no LLM. Collects ALL errors (better
//! repair-loop feedback), auto-repairs only trivially safe issues, and
//! enforces the meaning-preservation heuristics for speech.

use std::path::PathBuf;

use mycut_core::TimeMs;

use crate::plan::{EditPlan, Operation, PlanError, RatioId};

/// Everything the validator needs to know about the current project state.
#[derive(Debug, Clone)]
pub struct PlanContext {
    /// Duration of the primary source (seconds).
    pub source_duration_s: f64,
    /// Transcript segments (start_s, end_s, text) — for meaning-preservation
    /// warnings. Empty when no transcript exists.
    pub transcript: Vec<(f64, f64, String)>,
    /// Highlight windows from analysis (start_s, end_s, score), best first.
    pub highlights: Vec<(f64, f64, f64)>,
    /// Directories a LUT path may resolve in.
    pub allowed_dirs: Vec<PathBuf>,
}

#[derive(Debug, Default, PartialEq)]
pub struct ValidationResult {
    pub errors: Vec<PlanError>,
    pub warnings: Vec<String>,
    /// Trivially-safe auto-repairs applied to the normalized plan.
    pub repairs: Vec<String>,
}

/// Boundaries for numeric plan fields (mirror of the JSON Schema).
pub mod limits {
    pub const SPEED_MIN: f64 = 0.25;
    pub const SPEED_MAX: f64 = 4.0;
    pub const TRANSITION_MIN: f64 = 0.05;
    pub const TRANSITION_MAX: f64 = 3.0;
    pub const TEXT_MAX_CHARS: usize = 300;
    /// Tolerance for auto-clamping timestamps past the end.
    pub const CLAMP_TOLERANCE_S: f64 = 0.02;
}

/// Validate a parsed plan against the context; on success returns the
/// (possibly repaired) operations in canonical order.
///
/// Repairs: timestamps ≤ 20 ms past the source end are clamped and reported.
pub fn validate_plan(
    plan: &EditPlan,
    ctx: &PlanContext,
) -> (Option<Vec<Operation>>, ValidationResult) {
    let mut v = ValidationResult::default();
    let mut ops: Vec<Operation> = Vec::new();
    let no_question = plan
        .needs_clarification
        .as_ref()
        .is_none_or(|q| q.trim().is_empty());
    if no_question && plan.operations.is_empty() {
        v.errors.push(PlanError::Invalid {
            op: "plan".into(),
            reason: "plan has no operations and no needs_clarification question".into(),
        });
    }
    if plan.needs_clarification.is_some() && !plan.operations.is_empty() {
        v.warnings.push(
            "plan contains both a clarification question and operations; ask the user first".into(),
        );
    }
    for op in &plan.operations {
        match validate_op(op, ctx, &mut v) {
            Ok(repaired) => ops.push(repaired.unwrap_or_else(|| op.clone())),
            Err(e) => v.errors.push(e),
        }
    }
    if v.errors.is_empty() {
        (Some(ops), v)
    } else {
        (None, v)
    }
}

fn check_range(
    op: &str,
    field: &str,
    start: f64,
    end: f64,
    dur: f64,
    v: &mut ValidationResult,
) -> Option<(f64, f64)> {
    if !start.is_finite() || !end.is_finite() {
        v.errors.push(PlanError::Invalid {
            op: op.into(),
            reason: format!("{field} must be finite"),
        });
        return None;
    }
    if start < 0.0 {
        v.errors.push(PlanError::RangeError(start, end));
        return None;
    }
    if start >= end {
        v.errors.push(PlanError::RangeError(start, end));
        return None;
    }
    // Auto-repair: tiny overshoot past duration clamps.
    if end > dur {
        if end - dur <= limits::CLAMP_TOLERANCE_S {
            v.repairs.push(format!(
                "{op}: clamped end {end:.3}s -> {dur:.3}s (≤20ms overshoot)"
            ));
            return Some((start, dur));
        }
        v.errors.push(PlanError::RangeError(start, end));
        return None;
    }
    Some((start, end))
}

fn validate_op(
    op: &Operation,
    ctx: &PlanContext,
    v: &mut ValidationResult,
) -> Result<Option<Operation>, PlanError> {
    let dur = ctx.source_duration_s;
    match op {
        Operation::CutRanges {
            strategy: _,
            ranges,
            reason: _,
        } => {
            let mut cleaned: Vec<(f64, f64)> = Vec::new();
            for r in ranges {
                let Some(rng) = check_range("cut_ranges", "range", r.start, r.end, dur, v) else {
                    continue;
                };
                // Overlap rejection within the same op.
                if cleaned.iter().any(|(s, e)| rng.0 < *e && rng.1 > *s) {
                    v.errors.push(PlanError::RangeError(rng.0, rng.1));
                    continue;
                }
                cleaned.push(rng);
                meaning_preservation_check("cut_ranges", rng, ctx, v);
            }
            if cleaned.len() == ranges.len() {
                Ok(None) // no repairs needed; keep original
            } else if !cleaned.is_empty() {
                v.repairs
                    .push("cut_ranges: dropped invalid/overlapping ranges".into());
                Ok(Some(Operation::CutRanges {
                    strategy: crate::plan::CutStrategy::Explicit,
                    ranges: cleaned
                        .into_iter()
                        .map(|(s, e)| crate::plan::Range { start: s, end: e })
                        .collect(),
                    reason: String::new(),
                }))
            } else {
                Ok(None)
            }
        }
        Operation::KeepRanges { ranges, reason: _ } => {
            for r in ranges {
                check_range("keep_ranges", "range", r.start, r.end, dur, v);
            }
            Ok(None)
        }
        Operation::SetTargetDuration {
            seconds,
            selection: _,
        } => {
            if !(1.0..=3600.0).contains(seconds) {
                v.errors.push(PlanError::OutOfRange {
                    op: "set_target_duration".into(),
                    field: "seconds".into(),
                    value: *seconds,
                    min: 1.0,
                    max: 3600.0,
                });
            }
            Ok(None)
        }
        Operation::AddEffect {
            effect,
            start,
            end,
            params,
            mode: _,
            reason: _,
        } => {
            let Some(rng) = check_range("add_effect", "window", *start, *end, dur, v) else {
                return Ok(None);
            };
            // Effect-specific param ranges (engine registry).
            let (id, def_params): (&str, Vec<(String, f64, f64)>) = match effect {
                crate::plan::EffectId::ZoomPunch => {
                    ("zoom_punch", vec![("strength".into(), 1.0, 2.0)])
                }
                crate::plan::EffectId::Zoom => ("zoom", vec![("factor".into(), 1.0, 3.0)]),
                crate::plan::EffectId::Shake => (
                    "shake",
                    vec![
                        ("amplitude".into(), 0.0, 80.0),
                        ("frequency".into(), 0.1, 30.0),
                    ],
                ),
                crate::plan::EffectId::Blur => ("blur", vec![("radius".into(), 0.0, 40.0)]),
                crate::plan::EffectId::Sharpen => ("sharpen", vec![("amount".into(), 0.0, 3.0)]),
                crate::plan::EffectId::Vignette => {
                    ("vignette", vec![("strength".into(), 0.0, 1.0)])
                }
                crate::plan::EffectId::RgbSplit => {
                    ("rgb_split", vec![("shift_px".into(), 0.0, 20.0)])
                }
                crate::plan::EffectId::Grain => ("grain", vec![("strength".into(), 0.0, 64.0)]),
                crate::plan::EffectId::Pixelate => ("pixelate", vec![("block".into(), 2.0, 64.0)]),
                crate::plan::EffectId::Flash => ("flash", vec![("duration".into(), 0.05, 2.0)]),
                crate::plan::EffectId::CinematicBars => {
                    ("cinematic_bars", vec![("height_frac".into(), 0.0, 0.35)])
                }
                other => {
                    v.errors.push(PlanError::Invalid {
                        op: "add_effect".into(),
                        reason: format!("effect {other:?} is not in the available registry"),
                    });
                    return Ok(None);
                }
            };
            for (k, val) in params {
                let Some(n) = val.as_f64() else {
                    v.errors.push(PlanError::Invalid {
                        op: "add_effect".into(),
                        reason: format!("param {k} must be numeric"),
                    });
                    continue;
                };
                if let Some((_, min, max)) = def_params.iter().find(|(name, _, _)| name == k) {
                    if n < *min || n > *max {
                        v.errors.push(PlanError::OutOfRange {
                            op: "add_effect".into(),
                            field: format!("{id}.{k}"),
                            value: n,
                            min: *min,
                            max: *max,
                        });
                    }
                } else {
                    v.warnings
                        .push(format!("add_effect: unknown param {k} for {id} ignored"));
                }
            }
            let _ = rng;
            Ok(None)
        }
        Operation::RemoveEffect { effect } => {
            if effect.trim().is_empty() || effect.len() > 48 {
                v.errors.push(PlanError::Invalid {
                    op: "remove_effect".into(),
                    reason: "bad effect id".into(),
                });
            }
            Ok(None)
        }
        Operation::SetColor {
            params,
            merge_with_existing: _,
        } => {
            const RANGES: &[(&str, f64, f64)] = &[
                ("exposure", -3.0, 3.0),
                ("contrast", 0.1, 3.0),
                ("saturation", 0.0, 3.0),
                ("vibrance", -1.0, 1.0),
                ("temperature", -1.0, 1.0),
                ("gamma", 0.1, 3.0),
                ("shadows", -1.0, 1.0),
                ("highlights", -1.0, 1.0),
                ("lut_intensity", 0.0, 1.0),
            ];
            for (name, val) in [
                ("exposure", params.exposure),
                ("contrast", params.contrast),
                ("saturation", params.saturation),
                ("vibrance", params.vibrance),
                ("temperature", params.temperature),
                ("gamma", params.gamma),
                ("shadows", params.shadows),
                ("highlights", params.highlights),
                ("lut_intensity", params.lut_intensity),
            ] {
                if let Some(n) = val {
                    if let Some((_, min, max)) = RANGES.iter().find(|(n2, _, _)| *n2 == name) {
                        if n < *min || n > *max {
                            v.errors.push(PlanError::OutOfRange {
                                op: "set_color".into(),
                                field: name.into(),
                                value: n,
                                min: *min,
                                max: *max,
                            });
                        }
                    }
                }
            }
            if let Some(lut) = &params.lut {
                if !lut_allowed(lut, &ctx.allowed_dirs) {
                    v.errors.push(PlanError::UnsafePath {
                        op: "set_color".into(),
                        path: lut.clone(),
                    });
                }
            }
            Ok(None)
        }
        Operation::SetAspect { ratio, reframe: _ } => {
            let _ = ratio; // enum-constrained already
            Ok(None)
        }
        Operation::GenerateCaptions {
            style: _,
            language,
            safe_area: _,
            max_words_per_line,
            scale,
        } => {
            if let Some(mw) = max_words_per_line {
                if !(1..=12).contains(mw) {
                    v.errors.push(PlanError::OutOfRange {
                        op: "generate_captions".into(),
                        field: "max_words_per_line".into(),
                        value: f64::from(*mw),
                        min: 1.0,
                        max: 12.0,
                    });
                }
            }
            if let Some(s) = scale {
                if !(0.3..=2.5).contains(s) {
                    v.errors.push(PlanError::OutOfRange {
                        op: "generate_captions".into(),
                        field: "scale".into(),
                        value: *s,
                        min: 0.3,
                        max: 2.5,
                    });
                }
            }
            if let Some(lang) = language {
                if lang.len() > 12 {
                    v.warnings
                        .push("generate_captions: language code too long, ignoring".into());
                }
            }
            Ok(None)
        }
        Operation::UpdateCaptions {
            style: _,
            scale,
            safe_area: _,
        } => {
            if let Some(s) = scale {
                if !(0.3..=2.5).contains(s) {
                    v.errors.push(PlanError::OutOfRange {
                        op: "update_captions".into(),
                        field: "scale".into(),
                        value: *s,
                        min: 0.3,
                        max: 2.5,
                    });
                }
            }
            Ok(None)
        }
        Operation::RemoveCaptions => Ok(None),
        Operation::AddTransition {
            transition,
            at_index: _,
            duration,
        } => {
            if let Some(d) = duration {
                if !(limits::TRANSITION_MIN..=limits::TRANSITION_MAX).contains(d) {
                    v.errors.push(PlanError::OutOfRange {
                        op: "add_transition".into(),
                        field: "duration".into(),
                        value: *d,
                        min: limits::TRANSITION_MIN,
                        max: limits::TRANSITION_MAX,
                    });
                }
            }
            if matches!(transition, crate::plan::TransitionId::Blur) {
                v.errors.push(PlanError::Invalid {
                    op: "add_transition".into(),
                    reason: "blur transition is not available; use crossfade or dissolve-free cuts"
                        .into(),
                });
            }
            Ok(None)
        }
        Operation::AddText {
            text,
            start,
            end,
            kind: _,
            position: _,
            scale,
            opacity,
        } => {
            // Text windows are TIMELINE-domain (documented in the schema).
            if text.is_empty() || text.chars().count() > limits::TEXT_MAX_CHARS {
                v.errors.push(PlanError::Invalid {
                    op: "add_text".into(),
                    reason: format!("text length must be 1..={}", limits::TEXT_MAX_CHARS),
                });
            }
            if text.contains("sh -c") || text.contains('\u{0}') {
                v.errors.push(PlanError::Invalid {
                    op: "add_text".into(),
                    reason: "text contains forbidden content".into(),
                });
            }
            let Some(rng) = check_range("add_text", "window", *start, *end, dur, v) else {
                return Ok(None);
            };
            if let Some(s) = scale {
                if !(0.3..=3.0).contains(s) {
                    v.errors.push(PlanError::OutOfRange {
                        op: "add_text".into(),
                        field: "scale".into(),
                        value: *s,
                        min: 0.3,
                        max: 3.0,
                    });
                }
            }
            if let Some(o) = opacity {
                if !(0.05..=1.0).contains(o) {
                    v.errors.push(PlanError::OutOfRange {
                        op: "add_text".into(),
                        field: "opacity".into(),
                        value: *o,
                        min: 0.05,
                        max: 1.0,
                    });
                }
            }
            let _ = rng;
            Ok(None)
        }
        Operation::UpdateText {
            target_id,
            text,
            scale,
            opacity,
            position: _,
        } => {
            if target_id.trim().is_empty() {
                v.errors.push(PlanError::Invalid {
                    op: "update_text".into(),
                    reason: "target_id required".into(),
                });
            }
            if let Some(t) = text {
                if t.is_empty() || t.chars().count() > limits::TEXT_MAX_CHARS {
                    v.errors.push(PlanError::Invalid {
                        op: "update_text".into(),
                        reason: "bad text length".into(),
                    });
                }
            }
            if let Some(s) = scale {
                if !(0.3..=3.0).contains(s) {
                    v.errors.push(PlanError::OutOfRange {
                        op: "update_text".into(),
                        field: "scale".into(),
                        value: *s,
                        min: 0.3,
                        max: 3.0,
                    });
                }
            }
            if let Some(o) = opacity {
                if !(0.05..=1.0).contains(o) {
                    v.errors.push(PlanError::OutOfRange {
                        op: "update_text".into(),
                        field: "opacity".into(),
                        value: *o,
                        min: 0.05,
                        max: 1.0,
                    });
                }
            }
            Ok(None)
        }
        Operation::RemoveText { target_id } => {
            if target_id.trim().is_empty() {
                v.errors.push(PlanError::Invalid {
                    op: "remove_text".into(),
                    reason: "target_id required".into(),
                });
            }
            Ok(None)
        }
        Operation::AdjustAudio { params } => {
            if let Some(vol) = params.volume {
                if !(0.0..=4.0).contains(&vol) {
                    v.errors.push(PlanError::OutOfRange {
                        op: "adjust_audio".into(),
                        field: "volume".into(),
                        value: vol,
                        min: 0.0,
                        max: 4.0,
                    });
                }
            }
            for (name, val) in [("fade_in", params.fade_in), ("fade_out", params.fade_out)] {
                if let Some(f) = val {
                    if !(0.0..=10.0).contains(&f) {
                        v.errors.push(PlanError::OutOfRange {
                            op: "adjust_audio".into(),
                            field: name.into(),
                            value: f,
                            min: 0.0,
                            max: 10.0,
                        });
                    }
                }
            }
            Ok(None)
        }
        Operation::SetSpeed { start, end, speed } => {
            let Some(rng) = check_range("set_speed", "range", *start, *end, dur, v) else {
                return Ok(None);
            };
            if !(limits::SPEED_MIN..=limits::SPEED_MAX).contains(speed) {
                v.errors.push(PlanError::OutOfRange {
                    op: "set_speed".into(),
                    field: "speed".into(),
                    value: *speed,
                    min: limits::SPEED_MIN,
                    max: limits::SPEED_MAX,
                });
            }
            let _ = rng;
            Ok(None)
        }
        Operation::AddMarker { time, label } => {
            if !time.is_finite() || *time < 0.0 || *time > dur {
                v.errors.push(PlanError::RangeError(*time, *time));
            }
            if label.len() > 80 {
                v.warnings.push("add_marker: label truncated".into());
            }
            Ok(None)
        }
        Operation::SetExportPreset { preset: _ } => Ok(None),
    }
}

/// LUT paths must resolve inside allowed dirs, no traversal.
fn lut_allowed(lut: &str, allowed: &[PathBuf]) -> bool {
    if lut.contains("..") || lut.contains('\u{0}') {
        return false;
    }
    let p = PathBuf::from(lut);
    if p.is_absolute() {
        allowed.iter().any(|a| p.starts_with(a))
    } else {
        allowed.iter().any(|a| a.join(&p).exists()) || allowed.iter().any(|a| p.starts_with(a))
    }
}

/// Meaning-preservation: warn when a cut splits a transcript segment
/// mid-sentence (start inside a segment and end inside the SAME segment).
fn meaning_preservation_check(
    op: &str,
    rng: (f64, f64),
    ctx: &PlanContext,
    v: &mut ValidationResult,
) {
    for (s, e, text) in &ctx.transcript {
        if rng.0 > *s && rng.1 < *e {
            let snippet: String = text.chars().take(40).collect();
            v.warnings.push(format!(
                "{op}: cut {rng:.2?}s splits a spoken sentence ({s:.1}-{e:.1}s, \"{snippet}…\"); verify meaning is preserved"
            ));
        }
    }
}

/// Convert ratio id to aspect enum.
#[must_use]
pub fn ratio_to_aspect(r: RatioId) -> mycut_core::AspectRatio {
    match r {
        RatioId::R16x9 => mycut_core::AspectRatio::R16x9,
        RatioId::R9x16 => mycut_core::AspectRatio::R9x16,
        RatioId::R1x1 => mycut_core::AspectRatio::R1x1,
        RatioId::R4x5 => mycut_core::AspectRatio::R4x5,
    }
}

/// Source-time (ms) helpers.
#[must_use]
pub fn s_to_ms(s: f64) -> TimeMs {
    (s * 1000.0).round() as TimeMs
}
