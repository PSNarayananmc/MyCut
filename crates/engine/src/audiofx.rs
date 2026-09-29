//! Audio effect registry + project-level master audio chain.

use serde::{Deserialize, Serialize};

use mycut_core::{AudioMaster, ParamValue};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioParam {
    pub name: String,
    pub label: String,
    pub default: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioEffectDefinition {
    pub def_id: String,
    pub label: String,
    pub params: Vec<AudioParam>,
}

/// Build filter fragment for one audio effect. Times in seconds.
///
/// # Errors
/// [`crate::error::EngineError::Param`] for unknown effects.
pub fn build_audio_effect(def_id: &str, params: &BTreeMap<String, ParamValue>, piece_end_s: f64) -> Result<String, crate::error::EngineError> {
    let num = |k: &str, d: f64| -> f64 {
        params.get(k).map(|v| match v {
            ParamValue::Number(n) => *n,
            ParamValue::Text(t) => t.parse().unwrap_or(d),
        }).unwrap_or(d)
    };
    let s = match def_id {
        "volume" => {
            let v = num("volume", 1.0).clamp(0.0, 4.0);
            if (v - 1.0).abs() < 0.001 { String::new() } else { format!("volume={v:.3}") }
        }
        "normalize" => {
            let i = num("target_lufs", -16.0).clamp(-30.0, -8.0);
            format!("loudnorm=I={i}:TP=-1.5:LRA=11")
        }
        "denoise" => {
            let nr = num("strength", 12.0).clamp(0.01, 60.0);
            format!("afftdn=nr={nr:.1}:nf=-25")
        }
        "remove_silence" => {
            let min_s = num("min_silence", 0.5).clamp(0.05, 10.0);
            let thr = num("threshold_db", -35.0).clamp(-60.0, -20.0);
            format!("silenceremove=stop_periods=-1:stop_duration={min_s:.2}:stop_threshold={thr:.0}dB")
        }
        "fade_in" => {
            let d = num("duration", 0.5).clamp(0.05, 10.0).min(piece_end_s.max(0.1));
            format!("afade=t=in:st=0:d={d:.2}")
        }
        "fade_out" => {
            let d = num("duration", 0.8).clamp(0.05, 10.0).min(piece_end_s.max(0.1));
            format!("afade=t=out:st={:.2}:d={d:.2}", (piece_end_s - d).max(0.0))
        }
        "compressor" => "acompressor=threshold=-20dB:ratio=4:attack=10:release=200".into(),
        "limiter" => "alimiter=limit=-1dB:level=false".into(),
        _ => {
            return Err(crate::error::EngineError::Param {
                name: "audio_effect".into(),
                value: def_id.into(),
                reason: "unknown audio effect".into(),
            })
        }
    };
    Ok(s)
}

#[must_use]
pub fn registry() -> Vec<AudioEffectDefinition> {
    fn p(name: &str, label: &str, default: f64, min: f64, max: f64) -> AudioParam {
        AudioParam { name: name.into(), label: label.into(), default, min, max }
    }
    vec![
        AudioEffectDefinition { def_id: "volume".into(), label: "Volume".into(), params: vec![p("volume", "Linear gain", 1.0, 0.0, 4.0)] },
        AudioEffectDefinition { def_id: "normalize".into(), label: "Normalize (EBU R128)".into(), params: vec![p("target_lufs", "Target LUFS", -16.0, -30.0, -8.0)] },
        AudioEffectDefinition { def_id: "denoise".into(), label: "Denoise (afftdn)".into(), params: vec![p("strength", "Strength", 12.0, 0.01, 60.0)] },
        AudioEffectDefinition { def_id: "remove_silence".into(), label: "Remove silence".into(), params: vec![p("min_silence", "Min silence (s)", 0.5, 0.05, 10.0), p("threshold_db", "Threshold (dB)", -35.0, -60.0, -20.0)] },
        AudioEffectDefinition { def_id: "fade_in".into(), label: "Fade in".into(), params: vec![p("duration", "Duration (s)", 0.5, 0.05, 10.0)] },
        AudioEffectDefinition { def_id: "fade_out".into(), label: "Fade out".into(), params: vec![p("duration", "Duration (s)", 0.8, 0.05, 10.0)] },
        AudioEffectDefinition { def_id: "compressor".into(), label: "Compressor".into(), params: vec![] },
        AudioEffectDefinition { def_id: "limiter".into(), label: "Limiter".into(), params: vec![] },
    ]
}

#[must_use]
pub fn build_master_chain(master: &AudioMaster, total_s: f64) -> String {
    let mut chain: Vec<String> = Vec::new();
    if master.denoise {
        chain.push("afftdn=nr=12:nf=-25".to_string());
    }
    // NOTE: silence removal is intentionally NOT in the master chain: an
    // audio-only time change desyncs video. Plan-level cut_ranges with
    // strategy remove_silence shortens video+audio together.
    if master.normalize {
        chain.push("loudnorm=I=-16:TP=-1.5:LRA=11".to_string());
    }
    if master.fade_in_s > 0.01 {
        let s = format!("afade=t=in:st=0:d={:.2}", master.fade_in_s.min(total_s));
        chain.push(s);
    }
    if master.fade_out_s > 0.01 && total_s > 0.2 {
        let d = master.fade_out_s.min(total_s * 0.5);
        let s = format!("afade=t=out:st={:.2}:d={d:.2}", total_s - d);
        chain.push(s);
    }
    chain.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_all_registered_effects() {
        for def in registry() {
            let r = build_audio_effect(&def.def_id, &BTreeMap::new(), 10.0);
            assert!(r.is_ok(), "{} failed", def.def_id);
        }
    }

    #[test]
    fn volume_neutral_is_empty() {
        assert_eq!(build_audio_effect("volume", &BTreeMap::new(), 10.0).unwrap(), "");
    }

    #[test]
    fn master_chain_order() {
        let m = AudioMaster { normalize: true, denoise: true, fade_out_s: 1.0, ..Default::default() };
        let s = build_master_chain(&m, 30.0);
        let den = s.find("afftdn").unwrap();
        let nor = s.find("loudnorm").unwrap();
        let fo = s.find("afade=t=out").unwrap();
        assert!(den < nor && nor < fo, "chain order wrong: {s}");
        assert!(s.contains("st=29.00:d=1.00"));
    }

    #[test]
    fn unknown_rejected() {
        assert!(build_audio_effect("autotune", &BTreeMap::new(), 1.0).is_err());
    }
}
