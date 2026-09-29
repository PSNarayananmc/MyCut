//! mycut-schema: the AI contract. Edit-plan v1.0 types, the JSON Schema,
//! the validator (with repair policy) and the plan→command applier.

pub mod apply;
pub mod plan;
pub mod validate;

pub use plan::{EditPlan, Operation, PlanError};
pub use validate::{validate_plan, PlanContext, ValidationResult};

/// The JSON Schema document shipped in docs/schema (embedded for tests).
pub const EDIT_PLAN_SCHEMA_JSON: &str = include_str!("../../../docs/schema/edit-plan.schema.json");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::PLAN_SCHEMA_VERSION;

    fn ctx() -> PlanContext {
        PlanContext {
            source_duration_s: 60.0,
            transcript: vec![
                (1.0, 3.0, "Welcome back gamers.".into()),
                (4.0, 8.5, "This clutch is insane.".into()),
            ],
            highlights: vec![(4.0, 8.5, 9.0), (20.0, 25.0, 7.0)],
            allowed_dirs: vec!["/usr/share/mycut/luts".into()],
        }
    }

    const GOOD_PLAN: &str = r#"{
        "schema_version": "1.0",
        "intent_summary": "fast gaming cut",
        "assumptions": ["category: gaming"],
        "operations": [
            { "op": "cut_ranges", "strategy": "remove_low_activity",
              "ranges": [{ "start": 12.0, "end": 17.0 }], "reason": "low motion" },
            { "op": "set_color", "params": { "saturation": 1.15, "vibrance": 0.2 } },
            { "op": "add_effect", "effect": "zoom_punch", "start": 5.0, "end": 6.1,
              "params": { "strength": 1.18 } },
            { "op": "set_aspect", "ratio": "9:16", "reframe": "subject_follow" },
            { "op": "set_export_preset", "preset": "youtube_shorts" }
        ]
    }"#;

    #[test]
    fn schema_doc_is_v1() {
        let doc: serde_json::Value = serde_json::from_str(EDIT_PLAN_SCHEMA_JSON).unwrap();
        assert_eq!(doc["$id"], "https://mycut.app/schema/edit-plan-1.0.json");
    }

    #[test]
    fn good_plan_parses_and_validates() {
        let plan = EditPlan::parse(GOOD_PLAN).unwrap();
        assert_eq!(plan.schema_version, PLAN_SCHEMA_VERSION);
        let (validated, v) = validate_plan(&plan, &ctx());
        assert!(validated.is_some(), "errors: {:?}", v.errors);
        assert!(v.errors.is_empty());
    }

    #[test]
    fn unknown_fields_rejected_strictly() {
        let bad = r#"{
            "schema_version": "1.0",
            "intent_summary": "x",
            "operations": [
                { "op": "cut_ranges", "ranges": [{"start":0,"end":1}], "injected": "run_sh" }
            ]
        }"#;
        let r = EditPlan::parse(bad);
        assert!(r.is_err(), "unknown member must be rejected");
    }

    #[test]
    fn unknown_op_rejected() {
        let bad = r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [ { "op": "format_disk" } ] }"#;
        assert!(EditPlan::parse(bad).is_err());
    }

    #[test]
    fn timestamps_outside_source_rejected() {
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [ { "op": "cut_ranges", "strategy": "explicit",
              "ranges": [{ "start": 30.0, "end": 90.0 }] } ] }"#).unwrap();
        let (_, v) = validate_plan(&plan, &ctx());
        assert!(!v.errors.is_empty());
    }

    #[test]
    fn tiny_overshoot_is_autorepaired() {
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [ { "op": "cut_ranges", "strategy": "explicit",
              "ranges": [{ "start": 59.99, "end": 60.01 }] } ] }"#).unwrap();
        let (validated, v) = validate_plan(&plan, &ctx());
        assert!(validated.is_some());
        assert!(!v.repairs.is_empty(), "clamp must be recorded");
    }

    #[test]
    fn big_overshoot_rejected() {
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [ { "op": "cut_ranges", "strategy": "explicit",
              "ranges": [{ "start": 10.0, "end": 70.0 }] } ] }"#).unwrap();
        let (_, v) = validate_plan(&plan, &ctx());
        assert!(!v.errors.is_empty());
    }

    #[test]
    fn out_of_range_params_rejected() {
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [ { "op": "add_effect", "effect": "zoom_punch", "start": 1.0, "end": 2.0,
              "params": { "strength": 9.9 } } ] }"#).unwrap();
        let (_, v) = validate_plan(&plan, &ctx());
        assert!(v.errors.iter().any(|e| matches!(e, PlanError::OutOfRange { .. })));
    }

    #[test]
    fn unavailable_effect_rejected() {
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [ { "op": "add_effect", "effect": "glow", "start": 1.0, "end": 2.0 } ] }"#).unwrap();
        let (_, v) = validate_plan(&plan, &ctx());
        assert!(v.errors.iter().any(|e| e.to_string().contains("registry")));
    }

    #[test]
    fn lut_path_traversal_rejected() {
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [ { "op": "set_color", "params": { "lut": "../../../etc/evil.cube" } } ] }"#).unwrap();
        let (_, v) = validate_plan(&plan, &ctx());
        assert!(v.errors.iter().any(|e| matches!(e, PlanError::UnsafePath { .. })));
    }

    #[test]
    fn lut_inside_allowed_dir_ok() {
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [ { "op": "set_color", "params": { "lut": "/usr/share/mycut/luts/warm.cube" } } ] }"#).unwrap();
        let (_, v) = validate_plan(&plan, &ctx());
        assert!(v.errors.is_empty(), "{:?}", v.errors);
    }

    #[test]
    fn overlapping_cuts_rejected() {
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [ { "op": "cut_ranges", "strategy": "explicit",
              "ranges": [{ "start": 10.0, "end": 15.0 }, { "start": 14.0, "end": 20.0 }] } ] }"#).unwrap();
        let (_, v) = validate_plan(&plan, &ctx());
        assert!(!v.errors.is_empty(), "overlaps must be rejected");
    }

    #[test]
    fn mid_sentence_cut_warns() {
        // Transcript 1..3; cut 1.5..2.0 splits it.
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [ { "op": "cut_ranges", "strategy": "explicit",
              "ranges": [{ "start": 1.5, "end": 2.0 }] } ] }"#).unwrap();
        let (_, v) = validate_plan(&plan, &ctx());
        assert!(v.warnings.iter().any(|w| w.contains("splits a spoken sentence")));
    }

    #[test]
    fn blur_transition_rejected_honestly() {
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [ { "op": "add_transition", "transition": "blur", "duration": 0.5 } ] }"#).unwrap();
        let (_, v) = validate_plan(&plan, &ctx());
        assert!(v.errors.iter().any(|e| e.to_string().contains("not available")));
    }

    #[test]
    fn speed_range_enforced() {
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [ { "op": "set_speed", "start": 0.0, "end": 5.0, "speed": 10.0 } ] }"#).unwrap();
        let (_, v) = validate_plan(&plan, &ctx());
        assert!(v.errors.iter().any(|e| matches!(e, PlanError::OutOfRange { .. })));
    }

    #[test]
    fn needs_clarification_without_ops_ok() {
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "needs_clarification": "Which clip should I shorten?" }"#).unwrap();
        let (validated, v) = validate_plan(&plan, &ctx());
        assert!(validated.is_some(), "{:?}", v.errors);
        assert!(v.errors.is_empty());
    }

    #[test]
    fn empty_plan_rejected() {
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x" }"#).unwrap();
        let (_, v) = validate_plan(&plan, &ctx());
        assert!(!v.errors.is_empty());
    }

    #[test]
    fn adversarial_oversized_and_shell_text() {
        // Shell metacharacters in text are neutralized by escaping, but plans
        // attempting obviously hostile payloads are rejected outright.
        let plan = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [ { "op": "add_text", "text": "$(rm -rf /); `sh -c x`", "start": 0.0, "end": 2.0, "kind": "title" } ] }"#).unwrap();
        let (_, v) = validate_plan(&plan, &ctx());
        assert!(v.errors.iter().any(|e| e.to_string().contains("forbidden")));
    }

    #[test]
    fn plan_to_commands_applies_as_transaction() {
        use crate::apply::plan_to_commands;
        let mut project = mycut_core::Project::new("t");
        let source = serde_json::from_value(serde_json::json!({
            "id": "src_1", "name": "c.mp4", "rel_path": "media/c.mp4", "content_hash": "h",
            "duration_ms": 60_000, "width": 1280, "height": 720,
            "fps_num": 30, "fps_den": 1, "has_audio": true, "role": "footage"
        })).unwrap();
        let mut history = mycut_core::History::new();
        history.apply(&mut project, mycut_core::Command::AddSource { source }).unwrap();
        let plan = EditPlan::parse(GOOD_PLAN).unwrap();
        let app = plan_to_commands(&plan, &mut project, &mut history, &ctx()).unwrap();
        assert!(!app.summary_parts.is_empty());
        assert_eq!(project.color.saturation, 1.15);
        assert!(project.reframe.is_some());
        // One undo reverts the whole plan.
        history.undo(&mut project).unwrap();
        assert_eq!(project.color.saturation, 1.0);
        assert!(project.reframe.is_none());
    }

    #[test]
    fn followup_updates_modify_existing_state() {
        use crate::apply::plan_to_commands;
        let mut project = mycut_core::Project::new("t");
        let source = serde_json::from_value(serde_json::json!({
            "id": "src_1", "name": "c.mp4", "rel_path": "media/c.mp4", "content_hash": "h",
            "duration_ms": 60_000, "width": 1280, "height": 720,
            "fps_num": 30, "fps_den": 1, "has_audio": true, "role": "footage"
        })).unwrap();
        let mut history = mycut_core::History::new();
        history.apply(&mut project, mycut_core::Command::AddSource { source }).unwrap();

        let p1 = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "x",
            "operations": [
                { "op": "set_color", "params": { "saturation": 1.2, "temperature": 0.3 } },
                { "op": "add_effect", "effect": "vignette", "start": 1.0, "end": 3.0, "params": { "strength": 0.4 } }
            ] }"#).unwrap();
        plan_to_commands(&p1, &mut project, &mut history, &ctx()).unwrap();
        assert_eq!(project.color.saturation, 1.2);

        // Follow-up "more dramatic" modifies existing color instead of replacing.
        let p2 = EditPlan::parse(r#"{ "schema_version": "1.0", "intent_summary": "more dramatic",
            "operations": [ { "op": "set_color", "params": { "saturation": 1.5 } } ] }"#).unwrap();
        plan_to_commands(&p2, &mut project, &mut history, &ctx()).unwrap();
        assert_eq!(project.color.saturation, 1.5);
        assert_eq!(project.color.temperature, 0.3, "merge preserves prior fields");
        assert_eq!(project.tracks[0].items[0].effects.len(), 1, "vignette kept");
    }
}
