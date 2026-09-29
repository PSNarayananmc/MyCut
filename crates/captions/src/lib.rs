//! CaptionEngine: transcribe→segment→style→position→animate→emit.
//! Independent of the renderer; emits ASS (burn-in via libass through
//! ffmpeg's subtitles filter) and SRT/VTT for export.

use serde::{Deserialize, Serialize};

use mycut_core::{CaptionEntry, CaptionStyle, CaptionWord, TimeMs};

pub mod transcribe;
pub use transcribe::{Transcriber, TranscriberConfig, TranscribeError, WhisperCppTranscriber};

/// A transcription segment/word with times (TIMELINE-domain ms here; the
/// transcription layer maps source→timeline before calling us).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimedWord {
    pub text: String,
    pub start_ms: TimeMs,
    pub end_ms: TimeMs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentOptions {
    pub max_words_per_line: usize,
    pub max_chars_per_line: usize,
    /// Minimum on-screen time per entry (ms).
    pub min_display_ms: TimeMs,
    /// Target reading speed (chars/second) for merging/splitting.
    pub max_cps: f64,
}

impl Default for SegmentOptions {
    fn default() -> Self {
        Self { max_words_per_line: 4, max_chars_per_line: 32, min_display_ms: 700, max_cps: 17.0 }
    }
}

const SENTENCE_END: [char; 3] = ['.', '!', '?'];

/// Segment timed words into caption entries.
/// Honors sentence boundaries, word/char limits, min display time.
#[must_use]
pub fn segment(words: &[TimedWord], opts: &SegmentOptions) -> Vec<CaptionEntry> {
    if words.is_empty() {
        return Vec::new();
    }
    let mut entries = Vec::new();
    let mut buf: Vec<&TimedWord> = Vec::new();

    let flush = |buf: &mut Vec<&TimedWord>, entries: &mut Vec<CaptionEntry>| {
        if buf.is_empty() {
            return;
        }
        let start = buf.first().map(|w| w.start_ms).unwrap_or(0);
        let end = buf.last().map(|w| w.end_ms).unwrap_or(start);
        let words: Vec<CaptionWord> = buf
            .iter()
            .map(|w| CaptionWord {
                text: w.text.clone(),
                start_ms: w.start_ms,
                end_ms: w.end_ms,
                emphasize: is_emphatic(&w.text, None),
            })
            .collect();
        entries.push(CaptionEntry { start_ms: start, end_ms: end.max(start + opts.min_display_ms), words });
        buf.clear();
    };

    for w in words {
        buf.push(w);
        let text_len: usize = buf.iter().map(|x| x.text.chars().count() + 1).sum();
        let boundary = w.text.chars().last().is_some_and(|c| SENTENCE_END.contains(&c));
        let dur_ms = (w.end_ms - buf.first().map(|x| x.start_ms).unwrap_or(w.start_ms)).max(1);
        let cps = text_len as f64 / (dur_ms as f64 / 1000.0);
        if boundary
            || buf.len() >= opts.max_words_per_line
            || text_len > opts.max_chars_per_line
            || cps > opts.max_cps * 2.0
        {
            flush(&mut buf, &mut entries);
        }
    }
    flush(&mut buf, &mut entries);
    entries
}

/// Emphasis heuristic: numbers, ALL-CAPS, keyword list, (optional loudness
/// from the caller's prosody data in v2).
#[must_use]
pub fn is_emphatic(word: &str, keywords: Option<&[&str]>) -> bool {
    let t = word.trim_matches(|c: char| !c.is_alphanumeric());
    if t.is_empty() {
        return false;
    }
    if t.chars().all(|c| c.is_ascii_digit() || !c.is_alphabetic()) && t.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    if t.chars().any(|c| c.is_alphabetic()) && t.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase()) && t.chars().filter(|c| c.is_alphabetic()).count() > 1 {
        return true;
    }
    if let Some(kws) = keywords {
        let lower = t.to_lowercase();
        if kws.iter().any(|k| k.to_lowercase() == lower) {
            return true;
        }
    }
    false
}

/// Visual style parameters (data, not hardcoded designs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleSpec {
    pub name: String,
    /// Font size as fraction of frame height.
    pub size_frac: f64,
    pub color: String,
    pub highlight_color: String,
    pub stroke_frac: f64,
    pub shadow: bool,
    pub bold: bool,
    pub uppercase: bool,
    /// Karaoke-style per-word \k fill.
    pub karaoke: bool,
    /// Scale-in pop animation via \t.
    pub pop: bool,
    /// Vertical anchor: 2 = bottom-center alignment (ASS alignment number).
    pub alignment: u32,
    /// Bottom margin fraction of height (safe area aware).
    pub margin_v_frac: f64,
}

#[must_use]
pub fn style_for(style: CaptionStyle) -> StyleSpec {
    let s = |name: &str, size: f64, color: &str, hl: &str, stroke: f64, shadow: bool, bold: bool, upper: bool, karaoke: bool, pop: bool, margin: f64| StyleSpec {
        name: name.into(),
        size_frac: size,
        color: color.into(),
        highlight_color: hl.into(),
        stroke_frac: stroke,
        shadow,
        bold,
        uppercase: upper,
        karaoke,
        pop,
        alignment: 2,
        margin_v_frac: margin,
    };
    match style {
        CaptionStyle::Minimal => s("minimal", 0.045, "&H00FFFFFF", "&H00FFFFFF", 0.10, false, false, false, false, false, 0.06),
        CaptionStyle::Gaming => s("gaming", 0.06, "&H0000F8FF", "&H0030F8FF", 0.16, true, true, true, true, true, 0.14),
        CaptionStyle::Tiktok => s("tiktok", 0.058, "&H00FFFFFF", "&H0000D7FF", 0.14, true, true, false, false, true, 0.16),
        CaptionStyle::Youtube => s("youtube", 0.05, "&H00FFFFFF", "&H00FFFFFF", 0.12, true, false, false, false, false, 0.10),
        CaptionStyle::Cinematic => s("cinematic", 0.04, "&H00F0F0F0", "&H00F0F0F0", 0.08, false, false, false, false, false, 0.08),
        CaptionStyle::Bold => s("bold", 0.062, "&H00FFFFFF", "&H00FFFFFF", 0.18, false, true, true, false, false, 0.12),
        CaptionStyle::Karaoke => s("karaoke", 0.055, "&H00D0D0D0", "&H0000F8FF", 0.14, true, true, false, true, false, 0.12),
        CaptionStyle::WordHighlight => s("word_highlight", 0.056, "&H00B0B0B0", "&H00FFFFFF", 0.13, true, true, false, false, false, 0.14),
        CaptionStyle::Streamer => s("streamer", 0.055, "&H0000FFF2", "&H00FFFFFF", 0.15, true, true, false, false, true, 0.14),
    }
}

/// ASS escape: braces and backslashes in content.
fn ass_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('{', "\\{").replace('}', "\\}")
}

/// Emit an ASS subtitle file. `play_res` = frame size the times/styles map to.
#[must_use]
pub fn to_ass(
    entries: &[CaptionEntry],
    style: &StyleSpec,
    play_res: (u32, u32),
    safe_area: &str,
) -> String {
    let fontsize = ((play_res.1 as f64) * style.size_frac).round().max(10.0) as u32;
    let stroke = ((play_res.1 as f64) * style.stroke_frac).round().max(1.0) as u32;
    let margin_v = ((play_res.1 as f64) * style.margin_v_frac).round() as u32;
    let shadow = u32::from(style.shadow);
    let fontweight = if style.bold { 1 } else { 0 };
    let mut out = String::new();
    out.push_str("[Script Info]\n");
    out.push_str("ScriptType: v4.00+\n");
    out.push_str(&format!("PlayResX: {}\nPlayResY: {}\nWrapStyle: 2\n", play_res.0, play_res.1));
    out.push_str("ScaledBorderAndShadow: yes\n\n");
    out.push_str("[V4+ Styles]\n");
    out.push_str("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
    out.push_str(&format!(
        "Style: MyCut,Sans,{fs},{c},{hl},&H00101010,&H80000000,{bw},0,0,0,100,100,0,0,1,{st},{sh},{al},40,40,{mv},1\n",
        fs = fontsize,
        c = style.color,
        hl = style.highlight_color,
        bw = fontweight,
        st = stroke,
        sh = shadow,
        al = style.alignment,
        mv = margin_v
    ));
    out.push_str("\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    for e in entries {
        let start = ass_time(e.start_ms);
        let end = ass_time(e.end_ms);
        let text = build_entry_text(e, style, safe_area);
        out.push_str(&format!("Dialogue: 0,{start},{end},MyCut,,0,0,0,,{text}\n"));
    }
    out
}

fn build_entry_text(e: &CaptionEntry, style: &StyleSpec, safe_area: &str) -> String {
    let mut prefix = String::new();
    if style.pop {
        prefix.push_str("\\fscx80\\fscy80\\t(0,120,\\fscx100\\fscy100)");
    }
    if safe_area == "avoid_center" {
        // Push captions to the lower third (already margin-based); nudge by
        // explicit position tag for vertical formats.
        prefix.push_str("\\pos(");
        prefix.push_str(&format!("{},{}", "mid_x", "PLACEHOLDER"));
        prefix.push(')');
        prefix.clear(); // v1: margins handle placement; explicit pos reserved.
    }
    if style.karaoke {
        // \k durations in centiseconds per word.
        let mut text = prefix;
        for w in &e.words {
            let cs = ((w.end_ms - w.start_ms).max(80) as f64 / 10.0).round() as i64;
            text.push_str(&format!("{{\\k{cs}}}{}", ass_escape(&normalize_word(&w.text, style))));
            text.push(' ');
        }
        text.trim_end().to_string()
    } else if style.name == "word_highlight" || style.name == "tiktok" || style.name == "gaming" {
        // Emphasized words rendered in highlight color via inline overrides.
        let mut text = prefix;
        for w in &e.words {
            let norm = normalize_word(&w.text, style);
            if w.emphasize {
                text.push_str(&format!("{{\\c{}\\b1}}{}{{\\c{}\\b0}} ", style.highlight_color, ass_escape(&norm), style.color));
            } else {
                text.push_str(&ass_escape(&norm));
                text.push(' ');
            }
        }
        text.trim_end().to_string()
    } else {
        let joined: Vec<String> = e.words.iter().map(|w| normalize_word(&w.text, style)).collect();
        format!("{prefix}{}", ass_escape(&joined.join(" ")))
    }
}

fn normalize_word(w: &str, style: &StyleSpec) -> String {
    if style.uppercase {
        w.to_uppercase()
    } else {
        w.to_string()
    }
}

/// ASS timestamp: H:MM:SS.CS
fn ass_time(ms: TimeMs) -> String {
    let ms = ms.max(0);
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1000;
    let cs = (ms % 1000) / 10;
    format!("{h}:{m:02}:{s:02}.{cs:02}")
}

/// SRT export.
#[must_use]
pub fn to_srt(entries: &[CaptionEntry]) -> String {
    let mut out = String::new();
    for (i, e) in entries.iter().enumerate() {
        let text: String = e.words.iter().map(|w| w.text.clone()).collect::<Vec<_>>().join(" ");
        out.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, srt_time(e.start_ms), srt_time(e.end_ms), text));
    }
    out
}

/// WebVTT export (with header).
#[must_use]
pub fn to_vtt(entries: &[CaptionEntry]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for e in entries {
        let text: String = e.words.iter().map(|w| w.text.clone()).collect::<Vec<_>>().join(" ");
        out.push_str(&format!("{} --> {}\n{}\n\n", srt_time(e.start_ms), srt_time(e.end_ms), text));
    }
    out
}

fn srt_time(ms: TimeMs) -> String {
    let ms = ms.max(0);
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1000;
    let mmm = ms % 1000;
    format!("{h:02}:{m:02}:{s:02},{mmm:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(pairs: &[(&str, i64, i64)]) -> Vec<TimedWord> {
        pairs.iter().map(|(t, s, e)| TimedWord { text: t.to_string(), start_ms: *s, end_ms: *e }).collect()
    }

    #[test]
    fn segmentation_respects_limits_and_sentences() {
        let w = words(&[
            ("This", 0, 200), ("is", 200, 400), ("great.", 400, 600),
            ("Another", 600, 900), ("sentence", 900, 1200), ("here.", 1200, 1500),
        ]);
        let entries = segment(&w, &SegmentOptions::default());
        assert_eq!(entries.len(), 2, "sentence boundary must split: {entries:?}");
        assert!(entries[0].end_ms >= entries[0].start_ms + 700, "min display time");
    }

    #[test]
    fn segmentation_max_words() {
        let w = words(&[
            ("a", 0, 100), ("b", 100, 200), ("c", 200, 300), ("d", 300, 400), ("e", 400, 500),
        ]);
        let opts = SegmentOptions { max_words_per_line: 2, ..Default::default() };
        let entries = segment(&w, &opts);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].words.len(), 2);
    }

    #[test]
    fn emphasis_heuristics() {
        assert!(is_emphatic("100", None));
        assert!(is_emphatic("WOW", None));
        assert!(!is_emphatic("hello", None));
        assert!(is_emphatic("GG", None));
        assert!(is_emphatic("clutch", Some(&["clutch", "insane"])));
    }

    #[test]
    fn ass_output_structure_and_karaoke() {
        let w = words(&[("Go", 0, 300), ("team!", 300, 700)]);
        let entries = segment(&w, &SegmentOptions::default());
        let style = style_for(CaptionStyle::Karaoke);
        let ass = to_ass(&entries, &style, (1080, 1920), "default");
        assert!(ass.contains("[Script Info]"));
        assert!(ass.contains("PlayResX: 1080"));
        assert!(ass.contains("Style: MyCut"));
        assert!(ass.contains("\\k"), "karaoke style needs \\k tags");
        assert!(ass.starts_with("[Script Info]"));
        // No raw braces from content.
        let clean = to_ass(&segment(&words(&[("br{ace}", 0, 300)]), &SegmentOptions::default()), &style_for(CaptionStyle::Minimal), (1920, 1080), "default");
        assert!(clean.contains("br\\{ace\\}"), "braces escaped");
    }

    #[test]
    fn word_highlight_uses_color_overrides() {
        let w = words(&[("NICE", 0, 300), ("shot", 300, 600)]);
        let entries = segment(&w, &SegmentOptions::default());
        let ass = to_ass(&entries, &style_for(CaptionStyle::WordHighlight), (1080, 1920), "default");
        assert!(ass.contains("\\c"), "highlight override present");
    }

    #[test]
    fn srt_and_vtt_wellformed() {
        let w = words(&[("Hello", 0, 800), ("world", 800, 1500)]);
        let entries = segment(&w, &SegmentOptions::default());
        let srt = to_srt(&entries);
        assert!(srt.starts_with("1\n00:00:00,000 --> "));
        let vtt = to_vtt(&entries);
        assert!(vtt.starts_with("WEBVTT"));
    }

    #[test]
    fn all_styles_render() {
        let w = words(&[("hi", 0, 500)]);
        let entries = segment(&w, &SegmentOptions::default());
        for style in [
            CaptionStyle::Minimal, CaptionStyle::Gaming, CaptionStyle::Tiktok,
            CaptionStyle::Youtube, CaptionStyle::Cinematic, CaptionStyle::Bold,
            CaptionStyle::Karaoke, CaptionStyle::WordHighlight, CaptionStyle::Streamer,
        ] {
            let ass = to_ass(&entries, &style_for(style), (1920, 1080), "default");
            assert!(ass.contains("Dialogue:"), "style {style:?} must render");
        }
    }
}
