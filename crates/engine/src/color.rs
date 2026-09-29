//! Color grade -> FFmpeg filter chain. Neutral grades produce empty chains
//! (zero cost). LUT intensity uses split+blend over the lut3d output.

use mycut_core::ColorGrade;

use crate::error::EngineError;
use crate::escape::escape_filter_value;

/// Build the color chain for one video piece. Applies in this order:
/// exposure/contrast/saturation/gamma (eq) -> temperature (colorbalance) ->
/// vibrance (vibrance) -> LUT (lut3d with intensity blend).
///
/// # Errors
/// [`EngineError::Param`] for out-of-range values (double check) or a LUT
/// path that escapes the jail (path must be pre-validated by caller).
pub fn build_color_chain(
    grade: &ColorGrade,
    lut_jail_check: &dyn Fn(&str) -> bool,
) -> Result<String, EngineError> {
    if grade.is_neutral() {
        return Ok(String::new());
    }
    let mut parts: Vec<String> = Vec::new();

    // eq: brightness (exposure proxy), contrast, saturation, gamma.
    let mut eq: Vec<String> = Vec::new();
    if grade.exposure.abs() > 0.01 {
        // Exposure stops -> eq brightness approx (documented mapping).
        eq.push(format!(
            "brightness={:.3}",
            (grade.exposure * 0.4).clamp(-1.0, 1.0)
        ));
    }
    if (grade.contrast - 1.0).abs() > 0.01 {
        eq.push(format!("contrast={:.3}", grade.contrast.clamp(0.1, 3.0)));
    }
    if (grade.saturation - 1.0).abs() > 0.01 {
        eq.push(format!(
            "saturation={:.3}",
            grade.saturation.clamp(0.0, 3.0)
        ));
    }
    if (grade.gamma - 1.0).abs() > 0.01 {
        eq.push(format!("gamma={:.3}", grade.gamma.clamp(0.1, 3.0)));
    }
    if !eq.is_empty() {
        parts.push(format!("eq={}", eq.join(":")));
    }

    // Temperature via colorbalance (positive = warmer: more red, less blue).
    if grade.temperature.abs() > 0.01 {
        let t = grade.temperature.clamp(-1.0, 1.0) * 0.3;
        parts.push(format!("colorbalance=rs={t:.3}:bs={:.3}", -t));
    }

    // Vibrance filter (intensity -2..2; we map plan range -1..1).
    if grade.vibrance.abs() > 0.01 {
        parts.push(format!(
            "vibrance=intensity={:.3}",
            grade.vibrance.clamp(-1.0, 1.0)
        ));
    }

    // LUT with intensity via split + blend.
    if let Some(lut) = &grade.lut {
        if !lut.is_empty() {
            if !lut_jail_check(lut) {
                return Err(EngineError::UnsafePath(format!(
                    "LUT path escapes jail: {lut}"
                )));
            }
            let escaped = escape_filter_value(lut);
            let intensity = grade.lut_intensity.clamp(0.0, 1.0);
            if intensity >= 0.999 {
                parts.push(format!("lut3d=file='{escaped}'"));
            } else if intensity > 0.001 {
                parts.push(format!(
                    "split[a][b];[b]lut3d=file='{escaped}'[lut];[a][lut]blend=all_mode=normal:all_opacity={intensity:.3}"
                ));
            }
        }
    }

    Ok(parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_jail(_p: &str) -> bool {
        true
    }
    fn bad_jail(_p: &str) -> bool {
        false
    }

    #[test]
    fn neutral_grade_is_empty() {
        assert_eq!(
            build_color_chain(&ColorGrade::default(), &ok_jail).unwrap(),
            ""
        );
    }

    #[test]
    fn vibrant_grade_maps_all_channels() {
        let g = ColorGrade {
            exposure: 0.5,
            contrast: 1.1,
            saturation: 1.3,
            vibrance: 0.2,
            temperature: 0.4,
            gamma: 1.05,
            ..Default::default()
        };
        let s = build_color_chain(&g, &ok_jail).unwrap();
        assert!(
            s.contains("eq=brightness=0.200:contrast=1.100:saturation=1.300:gamma=1.050"),
            "{s}"
        );
        assert!(s.contains("colorbalance=rs=0.120:bs=-0.120"), "{s}");
        assert!(s.contains("vibrance=intensity=0.200"), "{s}");
    }

    #[test]
    fn lut_full_and_partial_intensity() {
        let g = ColorGrade {
            lut: Some("/app/luts/warm.cube".into()),
            lut_intensity: 1.0,
            ..Default::default()
        };
        let s = build_color_chain(&g, &ok_jail).unwrap();
        assert!(s.starts_with("lut3d=file='"), "{s}");
        let g2 = ColorGrade {
            lut: Some("warm.cube".into()),
            lut_intensity: 0.4,
            ..Default::default()
        };
        let s2 = build_color_chain(&g2, &ok_jail).unwrap();
        assert!(
            s2.contains("blend=all_mode=normal:all_opacity=0.400"),
            "{s2}"
        );
    }

    #[test]
    fn lut_outside_jail_rejected() {
        let g = ColorGrade {
            lut: Some("../../etc/passwd.cube".into()),
            ..Default::default()
        };
        assert!(matches!(
            build_color_chain(&g, &bad_jail),
            Err(EngineError::UnsafePath(_))
        ));
    }
}
