//! Video effect registry: data-driven definitions + real FFmpeg filter
//! builders. If an effect cannot be implemented faithfully with stock
//! filters, it is NOT in this registry (no fakes).
//!
//! Time semantics inside builders: `start`/`end` are **piece-local seconds**
//! (after input seeking, filter time `t` starts at 0 at the piece's first
//! decoded frame). The render engine converts source/timeline windows.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use mycut_core::ParamValue;

use crate::error::EngineError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectParam {
    pub name: String,
    pub label: String,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

impl EffectParam {
    fn new(name: &str, label: &str, default: f64, min: f64, max: f64, step: f64) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            default,
            min,
            max,
            step,
        }
    }
}

/// Values resolved and clamped by the validator before reaching the engine.
#[derive(Debug, Clone)]
pub struct EffectContext {
    pub start: f64,
    pub end: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub params: BTreeMap<String, f64>,
    /// Optional text color for effects that take one (flash).
    pub color: Option<String>,
}

impl EffectContext {
    fn p_or(&self, name: &str, default: f64) -> f64 {
        self.params.get(name).copied().unwrap_or(default)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectDefinition {
    pub def_id: String,
    pub label: String,
    pub description: String,
    pub params: Vec<EffectParam>,
}

impl EffectDefinition {
    fn new(def_id: &str, label: &str, description: &str, params: Vec<EffectParam>) -> Self {
        Self {
            def_id: def_id.to_string(),
            label: label.to_string(),
            description: description.to_string(),
            params,
        }
    }
}

/// Build the filter-chain fragment for `def_id` within the given context.
///
/// # Errors
/// [`EngineError::Param`] for values outside declared ranges (engine-side
/// double check; the validator already clamped).
pub fn build_effect(def_id: &str, ctx: &EffectContext) -> Result<String, EngineError> {
    let defs = registry();
    let def = defs
        .iter()
        .find(|d| d.def_id == def_id)
        .ok_or_else(|| EngineError::Param {
            name: "effect".into(),
            value: def_id.into(),
            reason: "unknown effect id".into(),
        })?;
    // Clamp params to declared ranges (defense in depth).
    let mut params = ctx.params.clone();
    for pd in &def.params {
        if let Some(v) = params.get_mut(&pd.name) {
            *v = v.clamp(pd.min, pd.max);
        }
    }
    let mut ctx2 = ctx.clone();
    ctx2.params = params;
    let chain = match def_id {
        "zoom" => build_zoom(&ctx2)?,
        "zoom_punch" => build_zoom_punch(&ctx2)?,
        "shake" => build_shake(&ctx2)?,
        "blur" => build_blur(&ctx2)?,
        "sharpen" => build_sharpen(&ctx2)?,
        "vignette" => build_vignette(&ctx2)?,
        "rgb_split" => build_rgb_split(&ctx2)?,
        "grain" => build_grain(&ctx2)?,
        "pixelate" => build_pixelate(&ctx2)?,
        "flash" => build_flash(&ctx2)?,
        "cinematic_bars" => build_bars(&ctx2)?,
        _ => {
            return Err(EngineError::Param {
                name: "effect".into(),
                value: def_id.into(),
                reason: "registered but not implemented".into(),
            })
        }
    };
    Ok(chain)
}

fn en(start: f64, end: f64) -> String {
    format!("enable='between(t,{start:.3},{end:.3})'")
}

fn build_zoom(ctx: &EffectContext) -> Result<String, EngineError> {
    let z = ctx.p_or("factor", 1.2).clamp(1.0, 3.0);
    if z <= 1.001 {
        return Ok(String::new());
    }
    Ok(format!(
        "scale=iw*{z:.4}:ih*{z:.4},crop=iw/{z:.4}:ih/{z:.4}:'(iw-ow)/2':'(ih-oh)/2',scale=iw:ih,{}",
        en(ctx.start, ctx.end)
    ))
}

fn build_zoom_punch(ctx: &EffectContext) -> Result<String, EngineError> {
    let s = ctx.p_or("strength", 1.18).clamp(1.0, 2.0);
    let d = (ctx.end - ctx.start).max(0.05);
    // zoompan produces per-frame zoom following a smooth pulse; d=1 keeps
    // 1 output frame per input frame (video, not slideshow).
    let z = format!(
        "1+({s:.4}-1)*sin(PI*clip((in_time-{:.3})/{d:.3},0,1))",
        ctx.start
    );
    Ok(format!(
        "zoompan=z='{z}':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':d=1:s={w}x{h}:fps={fps}",
        w = ctx.width,
        h = ctx.height,
        fps = ctx.fps.max(1.0)
    ))
}

fn build_shake(ctx: &EffectContext) -> Result<String, EngineError> {
    let amp = ctx.p_or("amplitude", 12.0).clamp(0.0, 80.0);
    let freq = ctx.p_or("frequency", 6.0).clamp(0.1, 30.0);
    if amp < 0.5 {
        return Ok(String::new());
    }
    // Overscan so the moving crop window never leaves the frame.
    let overscan = 1.0 + amp / f64::from(ctx.width.min(ctx.height)) * 2.5;
    Ok(format!(
        "scale=trunc(iw*{ov:.4}/2)*2:trunc(ih*{ov:.4}/2)*2,\
crop={w}:{h}:'(iw-ow)/2+{amp:.1}*sin((t-{st:.3})*2*PI*{fq:.2})':'(ih-oh)/2+{amp:.1}*cos((t-{st:.3})*2.7*PI*{fq:.2})':{en}",
        ov = overscan,
        w = ctx.width,
        h = ctx.height,
        amp = amp,
        st = ctx.start,
        fq = freq,
        en = en(ctx.start, ctx.end)
    ))
}

fn build_blur(ctx: &EffectContext) -> Result<String, EngineError> {
    let r = ctx.p_or("radius", 8.0).clamp(0.0, 40.0) as i64;
    if r < 1 {
        return Ok(String::new());
    }
    Ok(format!(
        "boxblur=luma_radius={r}:luma_power=2:{}",
        en(ctx.start, ctx.end)
    ))
}

fn build_sharpen(ctx: &EffectContext) -> Result<String, EngineError> {
    let a = ctx.p_or("amount", 1.0).clamp(-2.0, 3.0);
    if a.abs() < 0.05 {
        return Ok(String::new());
    }
    Ok(format!(
        "unsharp=5:5:{a:.2}:5:5:0.0:{}",
        en(ctx.start, ctx.end)
    ))
}

fn build_vignette(ctx: &EffectContext) -> Result<String, EngineError> {
    let s = ctx.p_or("strength", 0.5).clamp(0.0, 1.0);
    if s < 0.02 {
        return Ok(String::new());
    }
    let angle = s * std::f64::consts::PI * 0.5;
    Ok(format!(
        "vignette=angle={angle:.4}:{}",
        en(ctx.start, ctx.end)
    ))
}

fn build_rgb_split(ctx: &EffectContext) -> Result<String, EngineError> {
    let d = ctx.p_or("shift_px", 3.0).clamp(0.0, 20.0) as i64;
    if d < 1 {
        return Ok(String::new());
    }
    Ok(format!(
        "rgbashift=rh={d}:bv=-{d}:{}",
        en(ctx.start, ctx.end)
    ))
}

fn build_grain(ctx: &EffectContext) -> Result<String, EngineError> {
    let a = ctx.p_or("strength", 12.0).clamp(0.0, 64.0) as i64;
    if a < 1 {
        return Ok(String::new());
    }
    Ok(format!(
        "noise=alls={a}:allf=t+u:{}",
        en(ctx.start, ctx.end)
    ))
}

fn build_pixelate(ctx: &EffectContext) -> Result<String, EngineError> {
    let f = ctx.p_or("block", 8.0).clamp(2.0, 64.0) as u32;
    let sw = (ctx.width / f).max(8);
    let sh = (ctx.height / f).max(8);
    Ok(format!(
        "scale={sw}:{sh}:flags=neighbor:enable='between(t,{st:.3},{en:.3})',\
scale={w}:{h}:flags=neighbor:enable='between(t,{st:.3},{en:.3})'",
        st = ctx.start,
        en = ctx.end,
        w = ctx.width,
        h = ctx.height
    ))
}

fn build_flash(ctx: &EffectContext) -> Result<String, EngineError> {
    let d = ctx.p_or("duration", 0.18).clamp(0.05, 2.0);
    let c = ctx.color.clone().unwrap_or_else(|| "white".into());
    let dur = d.min((ctx.end - ctx.start) / 2.0);
    // Symmetric flash centered at window start: to-color, then from-color.
    Ok(format!(
        "fade=t=out:st={:.3}:d={dur:.3}:color={c},fade=t=in:st={:.3}:d={dur:.3}:color={c}",
        ctx.start,
        ctx.start + dur
    ))
}

fn build_bars(ctx: &EffectContext) -> Result<String, EngineError> {
    let frac = ctx.p_or("height_frac", 0.12).clamp(0.0, 0.35);
    let bar = ((ctx.height as f64) * frac).round() as u32;
    if bar < 2 {
        return Ok(String::new());
    }
    Ok(format!(
        "drawbox=x=0:y=0:w=iw:h={bar}:color=black:t=fill:{en},\
drawbox=x=0:y=ih-{bar}:w=iw:h={bar}:color=black:t=fill:{en}",
        en = en(ctx.start, ctx.end)
    ))
}

/// The registry (also serialized to the UI and sent to the planner).
#[must_use]
pub fn registry() -> Vec<EffectDefinition> {
    vec![
        EffectDefinition::new(
            "zoom",
            "Zoom",
            "Constant center zoom.",
            vec![EffectParam::new(
                "factor",
                "Zoom factor",
                1.2,
                1.0,
                3.0,
                0.05,
            )],
        ),
        EffectDefinition::new(
            "zoom_punch",
            "Zoom Punch",
            "Quick zoom-in-and-out pulse on an impact moment.",
            vec![EffectParam::new(
                "strength", "Strength", 1.18, 1.0, 2.0, 0.01,
            )],
        ),
        EffectDefinition::new(
            "shake",
            "Shake",
            "Camera shake (sine-driven crop pan with overscan).",
            vec![
                EffectParam::new("amplitude", "Amplitude (px)", 12.0, 0.0, 80.0, 1.0),
                EffectParam::new("frequency", "Frequency (Hz)", 6.0, 0.1, 30.0, 0.1),
            ],
        ),
        EffectDefinition::new(
            "blur",
            "Blur",
            "Gaussian-ish blur (boxblur) inside a window.",
            vec![EffectParam::new("radius", "Radius", 8.0, 0.0, 40.0, 1.0)],
        ),
        EffectDefinition::new(
            "sharpen",
            "Sharpen",
            "Unsharp mask.",
            vec![EffectParam::new("amount", "Amount", 1.0, 0.0, 3.0, 0.05)],
        ),
        EffectDefinition::new(
            "vignette",
            "Vignette",
            "Darken frame corners.",
            vec![EffectParam::new(
                "strength", "Strength", 0.5, 0.0, 1.0, 0.01,
            )],
        ),
        EffectDefinition::new(
            "rgb_split",
            "Chromatic Aberration",
            "RGB channel split shift.",
            vec![EffectParam::new(
                "shift_px",
                "Shift (px)",
                3.0,
                0.0,
                20.0,
                1.0,
            )],
        ),
        EffectDefinition::new(
            "grain",
            "Film Grain",
            "Temporal+uniform noise.",
            vec![EffectParam::new(
                "strength", "Strength", 12.0, 0.0, 64.0, 1.0,
            )],
        ),
        EffectDefinition::new(
            "pixelate",
            "Pixelate",
            "Mosaic block effect.",
            vec![EffectParam::new("block", "Block size", 8.0, 2.0, 64.0, 1.0)],
        ),
        EffectDefinition::new(
            "flash",
            "Flash",
            "White/color flash pulse.",
            vec![EffectParam::new(
                "duration",
                "Duration (s)",
                0.18,
                0.05,
                2.0,
                0.01,
            )],
        ),
        EffectDefinition::new(
            "cinematic_bars",
            "Cinematic Bars",
            "Letterbox bars.",
            vec![EffectParam::new(
                "height_frac",
                "Bar height (fraction)",
                0.12,
                0.0,
                0.35,
                0.01,
            )],
        ),
    ]
}

/// Convert a raw param map (possibly containing text params) to numeric map
/// for the builders, ignoring non-numeric entries.
#[must_use]
pub fn numeric_params(raw: &BTreeMap<String, ParamValue>) -> BTreeMap<String, f64> {
    raw.iter()
        .filter_map(|(k, v)| match v {
            ParamValue::Number(n) => Some((k.clone(), *n)),
            ParamValue::Text(t) => t.parse::<f64>().ok().map(|n| (k.clone(), n)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(start: f64, end: f64) -> EffectContext {
        EffectContext {
            start,
            end,
            fps: 30.0,
            width: 1280,
            height: 720,
            params: BTreeMap::new(),
            color: None,
        }
    }

    #[test]
    fn all_registered_effects_build() {
        for def in registry() {
            let out = build_effect(&def.def_id, &ctx(2.0, 4.0));
            assert!(out.is_ok(), "effect {} failed: {out:?}", def.def_id);
            let s = out.unwrap();
            if !s.is_empty() {
                assert!(
                    !s.contains(';'),
                    "single-effect chain must not contain graph separators"
                );
            }
        }
    }

    #[test]
    fn unknown_effect_rejected() {
        assert!(build_effect("hollywood_magic", &ctx(0.0, 1.0)).is_err());
    }

    #[test]
    fn params_are_clamped() {
        let mut c = ctx(1.0, 2.0);
        c.params.insert("factor".into(), 99.0);
        let s = build_effect("zoom", &c).unwrap();
        assert!(!s.contains("99"), "clamping failed: {s}");
    }

    #[test]
    fn zero_strength_effects_are_noops() {
        let mut c = ctx(1.0, 2.0);
        c.params.insert("radius".into(), 0.0);
        assert_eq!(build_effect("blur", &c).unwrap(), "");
        let mut c2 = ctx(1.0, 2.0);
        c2.params.insert("amplitude".into(), 0.0);
        assert_eq!(build_effect("shake", &c2).unwrap(), "");
    }

    #[test]
    fn flash_uses_only_fade_filters() {
        let s = build_effect("flash", &ctx(3.0, 4.0)).unwrap();
        assert!(s.contains("fade=t=out") && s.contains("fade=t=in"));
        assert!(s.contains("color=white"));
    }
}
