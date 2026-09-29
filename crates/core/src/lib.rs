//! mycut-core: canonical project model, time, command/undo layer.

pub mod command;
pub mod error;
pub mod model;
pub mod time;

pub use command::{apply_transaction, Command, History};
pub use error::CoreError;
pub use model::{
    AspectRatio, AudioCodec, AudioMaster, CaptionEntry, CaptionStyle, CaptionWord, ColorGrade,
    Container, EffectInstance, ExportSettings, Item, ItemKind, Keyframe, KeyframeTrack, Marker,
    MediaRole, ParamValue, PlanReport, Project, ReframeMode, ReframeSettings, Source, TextKind,
    Track, Position, TrackKind, TransitionKind, TransitionSetting, VideoCodec, SCHEMA_VERSION,
    now_unix_ms,
};
pub use time::{ms_to_seconds, new_id, seconds_to_ms, Easing, TimeMs};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ItemKind, Source, TrackKind};

    fn fixture_source(duration_ms: TimeMs) -> Source {
        serde_json::from_value(serde_json::json!({
            "id": "src_1", "name": "clip.mp4", "rel_path": "media/clip.mp4",
            "content_hash": "abc123", "duration_ms": duration_ms,
            "width": 1280, "height": 720, "fps_num": 30, "fps_den": 1,
            "has_audio": true, "role": "footage"
        }))
        .unwrap()
    }

    fn clip_item(source_id: &str, in_ms: TimeMs, out_ms: TimeMs, tl_start: TimeMs) -> Item {
        Item::new(
            ItemKind::VideoClip {
                source_id: source_id.to_string(),
                source_in_ms: in_ms,
                source_out_ms: out_ms,
                speed: 1.0,
            },
            tl_start,
            out_ms - in_ms,
        )
    }

    #[test]
    fn add_trim_undo_redo_roundtrip() {
        let mut p = Project::new("t");
        let mut h = History::new();
        h.apply(&mut p, Command::AddSource { source: fixture_source(10_000) }).unwrap();
        let item = clip_item("src_1", 0, 10_000, 0);
        h.apply(&mut p, Command::AddClip { track_kind: TrackKind::Video, item }).unwrap();
        assert_eq!(p.timeline_end_ms(), 10_000);

        let item_id0 = p.tracks[0].items[0].id.clone();
        h.apply(
            &mut p,
            Command::TrimClip {
                item_id: item_id0,
                new_source_in_ms: 2_000,
                new_source_out_ms: 6_000,
                new_timeline_start_ms: 0,
            },
        )
        .unwrap();
        assert_eq!(p.timeline_end_ms(), 4_000);
        h.undo(&mut p).unwrap();
        assert_eq!(p.timeline_end_ms(), 10_000);
        h.redo(&mut p).unwrap();
        assert_eq!(p.timeline_end_ms(), 4_000);
    }

    #[test]
    fn split_undo_restores_exact_state() {
        let mut p = Project::new("t");
        let mut h = History::new();
        h.apply(&mut p, Command::AddSource { source: fixture_source(10_000) }).unwrap();
        let item = clip_item("src_1", 0, 10_000, 0);
        h.apply(&mut p, Command::AddClip { track_kind: TrackKind::Video, item }).unwrap();
        let before = serde_json::to_string(&p).unwrap();

        let item_id = p.tracks[0].items[0].id.clone();
        h.apply(&mut p, Command::SplitClip { item_id, at_ms: 4_000 }).unwrap();
        assert_eq!(p.tracks[0].items.len(), 2);
        assert_eq!(p.tracks[0].items[1].kind_source_in(), 4_000);
        h.undo(&mut p).unwrap();
        let after = serde_json::to_string(&p).unwrap();
        assert_eq!(before, after, "undo(split) must restore identical state");
        h.redo(&mut p).unwrap();
        assert_eq!(p.tracks[0].items.len(), 2);
    }

    #[test]
    fn ai_transaction_undoes_as_one_step() {
        let mut p = Project::new("t");
        let mut h = History::new();
        h.apply(&mut p, Command::AddSource { source: fixture_source(10_000) }).unwrap();
        let cmds = vec![
            Command::SetColor { new_grade: ColorGrade { saturation: 1.4, ..Default::default() } },
            Command::AddClip { track_kind: TrackKind::Video, item: clip_item("src_1", 0, 5_000, 10_000) },
        ];
        let n = apply_transaction(&mut h, &mut p, cmds).unwrap();
        assert_eq!(n, 2);
        h.undo(&mut p).unwrap();
        assert!(p.tracks[0].items.is_empty(), "one undo reverts the whole AI edit");
        assert_eq!(p.color.saturation, 1.0);
        h.redo(&mut p).unwrap();
        assert_eq!(p.tracks[0].items.len(), 1);
        assert_eq!(p.color.saturation, 1.4);
    }

    #[test]
    fn transaction_rolls_back_on_failure() {
        let mut p = Project::new("t");
        let mut h = History::new();
        h.apply(&mut p, Command::AddSource { source: fixture_source(1_000) }).unwrap();
        let bad = vec![
            Command::SetColor { new_grade: ColorGrade { contrast: 1.2, ..Default::default() } },
            // Invalid: clip range beyond source duration.
            Command::AddClip { track_kind: TrackKind::Video, item: clip_item("src_1", 0, 9_999, 0) },
        ];
        assert!(apply_transaction(&mut h, &mut p, bad).is_err());
        assert_eq!(p.color.contrast, 1.0, "color change must be rolled back");
        // Only the earlier AddSource entry remains; the failed transaction left nothing.
        assert_eq!(h.undo_stack.len(), 1);
    }

    #[test]
    fn out_of_range_clip_rejected() {
        let mut p = Project::new("t");
        let mut h = History::new();
        h.apply(&mut p, Command::AddSource { source: fixture_source(1_000) }).unwrap();
        let r = h.apply(
            &mut p,
            Command::AddClip { track_kind: TrackKind::Video, item: clip_item("src_1", 500, 2_000, 0) },
        );
        assert!(matches!(r, Err(CoreError::Range(_))));
    }

    // Small test helper on ItemKind via JSON-agnostic accessor.
    trait SourceIn {
        fn kind_source_in(&self) -> TimeMs;
    }
    impl SourceIn for Item {
        fn kind_source_in(&self) -> TimeMs {
            match &self.kind {
                ItemKind::VideoClip { source_in_ms, .. } | ItemKind::AudioClip { source_in_ms, .. } => *source_in_ms,
                _ => -1,
            }
        }
    }
}
