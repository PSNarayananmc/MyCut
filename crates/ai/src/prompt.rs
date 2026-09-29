//! System prompt (versioned, unit-tested for the presence of key rules) and
//! the compact context builder that controls cost and latency.

/// Bump when the prompt changes meaningfully; cached plan keys include it.
pub const PROMPT_VERSION: &str = "1.0.0";

/// The planner system prompt. The model is a professional video-editing
/// planner; it executes nothing and outputs only schema-valid operations.
#[must_use]
pub fn system_prompt() -> String {
    String::from(
        r#"You are MyCut's professional video-editing planner. You PLAN edits; you never execute anything.

STRICT OUTPUT RULES
1. Output ONLY a JSON document conforming to the MyCut Edit Plan schema version 1.0. No prose, no markdown fences.
2. Every operation must use the allowed op names, effect ids, styles, and preset ids exactly as listed in the provided catalog. Unknown ids are rejected.
3. Every timestamp you emit must lie within the source duration provided in the context. Never invent timestamps outside it. start < end always.
4. All numeric parameters must be within the parameter ranges listed in the catalog.
5. Never produce destructive operations on source files: you cannot delete or modify media files, only timeline instructions.
6. Reference only files given to you in the context. Never invent LUT, logo, or font paths.
7. Never promise outcomes (e.g. virality). State what the edit does, not what it will achieve.
8. MEANING PRESERVATION: never cut or reorder speech in a way that changes its meaning. When trimming speech, cut at sentence boundaries from the transcript.
9. Prefer operations that modify existing state (merge color, update captions) over replacing everything, so follow-up requests stay small.
10. If the request is genuinely ambiguous, return a needs_clarification question instead of guessing operations.
11. Only use capabilities listed in the context (e.g. do not rely on vision frames unless the context says frames are attached).
12. Do not include shell commands, file paths, or code in any field except fields designed for paths (which must come from the context)."#,
    )
}

/// Compact project + analysis summary (NOT the whole project JSON).
#[derive(Debug, Clone, Default)]
pub struct ContextSummary {
    pub sources: Vec<SourceSummary>,
    pub timeline: TimelineSummary,
    pub analysis: Option<AnalysisDigest>,
    pub transcript: Vec<(f64, f64, String)>,
    pub conversation: Vec<(String, String)>, // (role, content) last few turns
    pub request: String,
}

#[derive(Debug, Clone, Default)]
pub struct SourceSummary {
    pub name: String,
    pub role: String,
    pub duration_s: f64,
    pub resolution: String,
    pub fps: f64,
    pub has_audio: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TimelineSummary {
    pub clips: usize,
    pub duration_s: f64,
    pub effects: Vec<String>,
    pub color_set: bool,
    pub captions_set: bool,
    pub reframe: Option<String>,
}

/// Analysis digest: only what planning needs (compact).
#[derive(Debug, Clone, Default)]
pub struct AnalysisDigest {
    pub scenes: Vec<(f64, f64, f64)>, // start, end, score
    pub silences: Vec<(f64, f64)>,
    pub speech_regions: Vec<(f64, f64)>,
    pub loudness_lufs: Option<f64>,
    pub highlights: Vec<(f64, f64, f64)>,
    pub activity_brief: String,
}

/// The available operations catalog with parameter ranges (from the registry).
#[must_use]
pub fn catalog_text() -> String {
    let effects: Vec<String> = mycut_engine::effects::registry()
        .iter()
        .map(|d| {
            let params: Vec<String> = d
                .params
                .iter()
                .map(|p| format!("{} in [{}, {}] default {}", p.name, p.min, p.max, p.default))
                .collect();
            format!("  - {} ({}): {}", d.def_id, d.label, params.join("; "))
        })
        .collect();
    let styles = [
        "minimal",
        "gaming",
        "tiktok",
        "youtube",
        "cinematic",
        "bold",
        "karaoke",
        "word_highlight",
        "streamer",
    ];
    let presets = [
        "youtube",
        "youtube_shorts",
        "instagram_reels",
        "tiktok",
        "discord",
        "twitter_x",
        "custom",
    ];
    format!(
        "AVAILABLE EFFECTS (add_effect):\n{}\n\nCAPTION STYLES: {}\n\nEXPORT PRESETS: {}\n\nTRANSITIONS: cut, fade, crossfade, dip_to_black, dip_to_white, slide, push, zoom, wipe (blur is NOT available)\n\nASPECTS: 16:9, 9:16, 1:1, 4:5 with reframe center|subject_follow|smart\n\nCOLOR PARAMS: exposure[-3,3] contrast[0.1,3] saturation[0,3] vibrance[-1,1] temperature[-1,1] gamma[0.1,3]\n\nAUDIO: volume[0,4], normalize, denoise, remove_silence, duck_music_under_speech, fade_in/fade_out[0,10]\n\nSPEED range [0.25, 4]",
        effects.join("\n"),
        styles.join(", "),
        presets.join(", ")
    )
}

/// Build the chat messages for a planning request.
#[must_use]
pub fn build_messages(
    summary: &ContextSummary,
    frames_enabled: bool,
    has_vision: bool,
) -> Vec<crate::provider::ChatMessage> {
    use crate::provider::ChatMessage;
    let mut parts: Vec<String> = Vec::new();

    // Sources.
    for s in &summary.sources {
        parts.push(format!(
            "SOURCE: name={:?} role={} duration={:.2}s res={} fps={:.2} audio={}",
            s.name, s.role, s.duration_s, s.resolution, s.fps, s.has_audio
        ));
    }
    // Timeline.
    let t = &summary.timeline;
    parts.push(format!(
        "TIMELINE: clips={} duration={:.2}s effects={:?} color_set={} captions_set={} reframe={:?}",
        t.clips, t.duration_s, t.effects, t.color_set, t.captions_set, t.reframe
    ));
    // Analysis.
    if let Some(a) = &summary.analysis {
        let scenes: Vec<String> = a
            .scenes
            .iter()
            .take(40)
            .map(|(s, e, sc)| format!("{s:.1}-{e:.1}@{sc:.2}"))
            .collect();
        let sil: Vec<String> = a
            .silences
            .iter()
            .take(30)
            .map(|(s, e)| format!("{s:.1}-{e:.1}"))
            .collect();
        let speech: Vec<String> = a
            .speech_regions
            .iter()
            .take(30)
            .map(|(s, e)| format!("{s:.1}-{e:.1}"))
            .collect();
        let hl: Vec<String> = a
            .highlights
            .iter()
            .take(6)
            .map(|(s, e, sc)| format!("{s:.1}-{e:.1}@{sc:.1}"))
            .collect();
        parts.push(format!("SCENES (start-end@score): {}", scenes.join(", ")));
        parts.push(format!("SILENCE RANGES: {}", sil.join(", ")));
        parts.push(format!("SPEECH REGIONS: {}", speech.join(", ")));
        if let Some(l) = a.loudness_lufs {
            parts.push(format!("LOUDNESS: {l:.1} LUFS integrated"));
        }
        parts.push(format!(
            "HIGHLIGHT CANDIDATES (start-end@score): {}",
            hl.join(", ")
        ));
        parts.push(format!("ACTIVITY BRIEF: {}", a.activity_brief));
    }
    // Transcript (segment level).
    if !summary.transcript.is_empty() {
        let tr: Vec<String> = summary
            .transcript
            .iter()
            .take(80)
            .map(|(s, e, txt)| format!("[{s:.1}-{e:.1}] {}", txt.replace('\n', " ")))
            .collect();
        parts.push(format!("TRANSCRIPT (source seconds):\n{}", tr.join("\n")));
    }
    // Catalog.
    parts.push(catalog_text());
    // Capabilities note.
    if has_vision && frames_enabled {
        parts.push("FRAMES: a few representative frames are attached for this request.".into());
    } else if has_vision {
        parts.push("FRAMES: not attached (user disabled frame sharing). Plan from transcript and metadata only.".into());
    } else {
        parts.push(
            "FRAMES: this model cannot view frames; plan from transcript and metadata only.".into(),
        );
    }

    // Conversation tail (compact).
    let mut messages: Vec<ChatMessage> = Vec::new();
    let tail: &[(String, String)] = if summary.conversation.len() > 6 {
        &summary.conversation[summary.conversation.len() - 6..]
    } else {
        &summary.conversation
    };
    for (role, content) in tail {
        messages.push(ChatMessage {
            role: role.clone(),
            content: content.clone(),
            image_url: None,
        });
    }
    messages.push(ChatMessage {
        role: "user".into(),
        content: format!(
            "CONTEXT:\n{}\n\nREQUEST: {}",
            parts.join("\n"),
            summary.request
        ),
        image_url: None,
    });
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_contains_key_rules() {
        let p = system_prompt();
        for needle in [
            "Output ONLY a JSON document",
            "schema version 1.0",
            "within the source duration",
            "Never produce destructive operations",
            "MEANING PRESERVATION",
            "needs_clarification",
            "Never invent",
            "virality",
        ] {
            assert!(p.contains(needle), "system prompt missing rule: {needle}");
        }
    }

    #[test]
    fn catalog_lists_all_registry_effects() {
        let c = catalog_text();
        for def in mycut_engine::effects::registry() {
            assert!(c.contains(&def.def_id), "catalog missing {}", def.def_id);
        }
        assert!(c.contains("youtube_shorts"));
        assert!(c.contains("word_highlight"));
    }

    #[test]
    fn context_is_compact_and_includes_request() {
        let s = ContextSummary {
            sources: vec![SourceSummary {
                name: "run.mp4".into(),
                role: "footage".into(),
                duration_s: 95.0,
                resolution: "1920x1080".into(),
                fps: 30.0,
                has_audio: true,
            }],
            request: "Make this a 30 second gaming Short".into(),
            ..Default::default()
        };
        let msgs = build_messages(&s, false, false);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].content.contains("duration=95.00s"));
        assert!(msgs[0]
            .content
            .contains("Make this a 30 second gaming Short"));
        assert!(msgs[0].content.contains("cannot view frames"));
    }
}
