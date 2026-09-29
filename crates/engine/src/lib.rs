//! mycut-engine: FFmpeg abstraction — argument arrays only, no shell.
//!
//! Typed builders for probe/render/effects/audio/color/captions, the effect
//! and audio registries, export presets, and hardware-encoder detection.

pub mod audiofx;
pub mod color;
pub mod effects;
pub mod error;
pub mod escape;
pub mod pathing;
pub mod preset;
pub mod probe;
pub mod process;
pub mod render;

pub use audiofx::AudioEffectDefinition;
pub use effects::{EffectDefinition, EffectParam};
pub use error::EngineError;
pub use preset::ExportPreset;
pub use probe::ProbeResult;
pub use render::{FfmpegGraph, HwChoice, RenderEngine};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_resolves_ffmpeg() {
        assert!(RenderEngine::new().is_ok(), "ffmpeg must be on PATH for engine tests");
    }

    #[test]
    fn atempo_chain_ranges() {
        assert_eq!(render::atempo_chain(1.0), vec!["atempo=1.000000"]);
        assert_eq!(render::atempo_chain(4.0).len(), 2); // 2.0, 1.0
        assert_eq!(render::atempo_chain(0.25).len(), 2); // 0.5, 0.5
        assert_eq!(render::atempo_chain(3.5).len(), 2); // 2.0, 1.75
    }

    #[test]
    fn crop_window_ratio_math() {
        // 16:9 source → 9:16 crop keeps height.
        let (w, h) = render::crop_window_for(mycut_core::AspectRatio::R9x16, 1920, 1080);
        assert_eq!((w, h), (608, 1080));
        // 16:9 crop of square keeps width.
        let (w, h) = render::crop_window_for(mycut_core::AspectRatio::R16x9, 1000, 1000);
        assert_eq!((w, h), (1000, 562));
    }

    #[test]
    fn piecewise_expr_bounded_and_monotone() {
        let pts: Vec<(f64, f64)> = (0..1000).map(|i| (i as f64 * 0.1, i as f64)).collect();
        let e = render::piecewise_expr(&pts, 0.0);
        assert!(e.len() < 40_000, "expression too large: {}", e.len());
        assert!(e.starts_with("if(lt(t,"));
        let single = render::piecewise_expr(&[(1.0, 42.0)], 0.0);
        assert_eq!(single, "42.0");
    }
}
