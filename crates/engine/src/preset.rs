//! Export presets: editable data (serialized), never hardcoded logic.
//! Values chosen for platform norms; users can override everything.

use serde::{Deserialize, Serialize};

use mycut_core::{AspectRatio, AudioCodec, Container, ExportSettings, VideoCodec};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportPreset {
    pub id: String,
    pub label: String,
    pub settings: ExportSettings,
}

/// The built-in preset table (shipped as data; UI edits produce user presets).
#[must_use]
pub fn builtin_presets() -> Vec<ExportPreset> {
    fn preset(id: &str, label: &str, w: u32, h: u32, fps: f64, q: u32, ab: u32) -> ExportPreset {
        ExportPreset {
            id: id.to_string(),
            label: label.to_string(),
            settings: ExportSettings {
                width: w,
                height: h,
                fps,
                video_codec: VideoCodec::H264,
                audio_codec: AudioCodec::Aac,
                container: Container::Mp4,
                quality: q,
                audio_bitrate_kbps: ab,
            },
        }
    }
    vec![
        preset("youtube", "YouTube 1080p", 1920, 1080, 30.0, 18, 192),
        preset("youtube_shorts", "YouTube Shorts", 1080, 1920, 30.0, 19, 192),
        preset("instagram_reels", "Instagram Reels", 1080, 1920, 30.0, 20, 128),
        preset("tiktok", "TikTok", 1080, 1920, 30.0, 20, 128),
        preset("discord", "Discord (small file)", 1280, 720, 30.0, 26, 128),
        preset("twitter_x", "Twitter / X", 1280, 720, 30.0, 23, 128),
        preset("custom", "Custom", 1920, 1080, 30.0, 20, 192),
    ]
}

/// Map an aspect ratio + source resolution to output dimensions: even
/// integers, target side derived from the *smaller* source dimension to
/// avoid upscaling beyond source quality.
#[must_use]
pub fn dimensions_for(ratio: AspectRatio, src_w: u32, src_h: u32) -> (u32, u32) {
    let (rn, rd) = ratio.wh();
    let target_ratio = f64::from(rn) / f64::from(rd);
    // Base on source: fit inside source box preserving ratio.
    let src_ratio = f64::from(src_w.max(1)) / f64::from(src_h.max(1));
    let (w, h) = if src_ratio > target_ratio {
        let h = src_h;
        let w = (h as f64 * target_ratio).round() as u32;
        (w, h)
    } else {
        let w = src_w;
        let h = (w as f64 / target_ratio).round() as u32;
        (w, h)
    };
    // Cap long side at 1920 to keep low-end renders tractable.
    let (w, h) = if w.max(h) > 1920 {
        let k = 1920.0 / f64::from(w.max(h));
        (((w as f64) * k).round() as u32, ((h as f64) * k).round() as u32)
    } else {
        (w, h)
    };
    (even_down(w.max(2)), even_down(h.max(2)))
}

fn even_down(v: u32) -> u32 {
    v - (v % 2)
}

/// Default proxy settings for preview (throwaway, low-res).
#[must_use]
pub fn proxy_height_for_preview() -> u32 {
    360
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_table_complete() {
        let presets = builtin_presets();
        let ids: Vec<_> = presets.iter().map(|p| p.id.as_str()).collect();
        for id in ["youtube", "youtube_shorts", "instagram_reels", "tiktok", "discord", "twitter_x", "custom"] {
            assert!(ids.contains(&id), "missing preset {id}");
        }
        // Shorts are vertical.
        let shorts = presets.iter().find(|p| p.id == "youtube_shorts").unwrap();
        assert!(shorts.settings.height > shorts.settings.width);
    }

    #[test]
    fn dimensions_are_even_and_capped() {
        let (w, h) = dimensions_for(AspectRatio::R9x16, 1920, 1080);
        assert_eq!((w, h), (608, 1080)); // 9:16 of 1080 height
        assert_eq!(w % 2, 0);
        let (w2, h2) = dimensions_for(AspectRatio::R16x9, 3840, 2160);
        assert_eq!((w2, h2), (1920, 1080), "long side capped at 1920");
        let (w3, _) = dimensions_for(AspectRatio::R1x1, 1000, 1000);
        assert_eq!(w3 % 2, 0);
    }
}
