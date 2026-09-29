//! Local-first analysis: scenes, audio (silence/loudness/speech), motion,
//! auto-color, highlights. All results are cached by content hash + params
//! and every runner is cancellable and streams real progress.

pub mod cache;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::cache::{content_hash, Cache};

/// A detected scene (source-domain seconds).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scene {
    pub start_s: f64,
    pub end_s: f64,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SilenceRange {
    pub start_s: f64,
    pub end_s: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LoudnessInfo {
    /// Integrated loudness LUFS (from loudnorm first pass).
    pub integrated_lufs: Option<f64>,
    /// True peak dBTP.
    pub true_peak_db: Option<f64>,
}

/// Per-second activity curve (0..1) from frame-difference motion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MotionCurve {
    /// activity[i] = motion in second i.
    pub per_second: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HighlightWindow {
    pub start_s: f64,
    pub end_s: f64,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AutoColorEstimate {
    pub mean_luma_0_1: f64,
    pub mean_saturation_0_1: f64,
    pub r_g_balance: f64,
    pub b_g_balance: f64,
    pub suggested_exposure: f64,
    pub suggested_saturation: f64,
    pub suggested_temperature: f64,
}

/// The complete analysis bundle for one media file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Analysis {
    pub duration_ms: i64,
    pub scenes: Vec<Scene>,
    pub silences: Vec<SilenceRange>,
    pub speech_regions: Vec<SilenceRange>,
    pub loudness: LoudnessInfo,
    pub motion: MotionCurve,
    pub highlights: Vec<HighlightWindow>,
    pub auto_color: AutoColorEstimate,
}

#[derive(Debug, Clone)]
pub struct AnalysisParams {
    /// Scene detection sampling fps.
    pub scene_fps: u32,
    /// Silence threshold (dB).
    pub silence_threshold_db: i32,
    /// Motion sampling fps.
    pub motion_fps: u32,
}

impl Default for AnalysisParams {
    fn default() -> Self {
        Self {
            scene_fps: 5,
            silence_threshold_db: -35,
            motion_fps: 4,
        }
    }
}

/// Profile-driven adaptive depth (see docs/ARCHITECTURE.md §6).
#[must_use]
pub fn adaptive_params(duration_ms: i64, sparse: bool) -> AnalysisParams {
    let minutes = duration_ms as f64 / 60_000.0;
    let (scene_fps, motion_fps) = if sparse {
        (2, 2)
    } else if minutes < 2.0 {
        (8, 6)
    } else if minutes < 10.0 {
        (5, 4)
    } else {
        (3, 3)
    };
    AnalysisParams {
        scene_fps,
        motion_fps,
        silence_threshold_db: -35,
    }
}

/// Progress callback: (stage, percent 0..100).
pub type ProgressFn<'a> = dyn Fn(&str, u32) + Sync + Send + 'a;

/// Run the full analysis (cached). `cancel` aborts early.
///
/// # Errors
/// Returns [`AnalysisError`] when ffmpeg runs fail and no cache exists.
pub fn analyze(
    media: &Path,
    cache_dir: &Path,
    cache_limit_bytes: u64,
    params: &AnalysisParams,
    cancel: &std::sync::atomic::AtomicBool,
    progress: Option<&ProgressFn>,
) -> Result<Analysis, AnalysisError> {
    let hash = content_hash(media).map_err(AnalysisError::Io)?;
    let cache = Cache::new(cache_dir, cache_limit_bytes);
    let mut key_params: BTreeMap<String, String> = BTreeMap::new();
    key_params.insert("v".into(), "1".into());
    key_params.insert("scene_fps".into(), params.scene_fps.to_string());
    key_params.insert("motion_fps".into(), params.motion_fps.to_string());
    key_params.insert("thr".into(), params.silence_threshold_db.to_string());
    let key = Cache::key(&hash, &key_params);

    if let Some(hit) = cache.get::<Analysis>(&key) {
        if let Some(p) = progress {
            p("cache", 100);
        }
        return Ok(hit);
    }

    let result = analyze_uncached(media, params, cancel, progress)?;
    let _ = cache.put(&key, &result);
    Ok(result)
}

#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    #[error("ffmpeg failed: {0}")]
    Tool(String),
    #[error("tool missing: {0}")]
    ToolNotFound(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("cancelled")]
    Cancelled,
}

fn check(cancel: &std::sync::atomic::AtomicBool) -> Result<(), AnalysisError> {
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        Err(AnalysisError::Cancelled)
    } else {
        Ok(())
    }
}

fn run_ffmpeg(
    args: &[&str],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(String, String), AnalysisError> {
    let ffmpeg = which_ffmpeg().ok_or_else(|| AnalysisError::ToolNotFound("ffmpeg".into()))?;
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin"]);
    cmd.args(args);
    cmd.stdin(std::process::Stdio::null());
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(AnalysisError::Io)?;
    // Cancellable wait: poll.
    loop {
        check(cancel)?;
        match child.try_wait().map_err(AnalysisError::Io)? {
            Some(status) => {
                let out = child
                    .stdout
                    .take()
                    .map(|mut s| {
                        use std::io::Read;
                        let mut b = Vec::new();
                        let _ = s.read_to_end(&mut b);
                        String::from_utf8_lossy(&b).into_owned()
                    })
                    .unwrap_or_default();
                let err = child
                    .stderr
                    .take()
                    .map(|mut s| {
                        use std::io::Read;
                        let mut b = Vec::new();
                        let _ = s.read_to_end(&mut b);
                        String::from_utf8_lossy(&b).into_owned()
                    })
                    .unwrap_or_default();
                if status.success() {
                    return Ok((out, err));
                }
                return Err(AnalysisError::Tool(
                    err.lines().rev().take(8).collect::<Vec<_>>().join("\n"),
                ));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(40)),
        }
    }
}

fn which_ffmpeg() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MYCUT_FFMPEG") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    which_in_path("ffmpeg")
}

fn which_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|c| c.is_file())
}

fn analyze_uncached(
    media: &Path,
    params: &AnalysisParams,
    cancel: &std::sync::atomic::AtomicBool,
    progress: Option<&ProgressFn>,
) -> Result<Analysis, AnalysisError> {
    let media_s = media.to_string_lossy().into_owned();
    let mut result = Analysis::default();

    // 1. Duration via ffprobe.
    if let Some(p) = progress {
        p("probe", 5);
    }
    let probe_json = run_ffprobe(&media_s, cancel)?;
    result.duration_ms = probe_duration_ms(&probe_json);

    // 2. Scenes.
    if let Some(p) = progress {
        p("scenes", 25);
    }
    check(cancel)?;
    let fps = params.scene_fps;
    let (_, stderr) = run_ffmpeg(
        &[
            "-i",
            &media_s,
            "-vf",
            &format!("fps={fps},scale=160:-2,select='gt(scene,0.15)',metadata=print"),
            "-an",
            "-f",
            "null",
            "-",
        ],
        cancel,
    )?;
    result.scenes = parse_scene_scores(&stderr, "", result.duration_ms);

    // 3. Audio: silence + loudness.
    if let Some(p) = progress {
        p("audio", 55);
    }
    check(cancel)?;
    let (_, err2) = run_ffmpeg(
        &[
            "-i",
            &media_s,
            "-af",
            &format!(
                "silencedetect=noise={}dB:d=0.4",
                params.silence_threshold_db
            ),
            "-f",
            "null",
            "-",
        ],
        cancel,
    )?;
    result.silences = parse_silence(&err2);
    result.speech_regions = invert_silences(&result.silences, result.duration_ms);

    let (_, err3) = run_ffmpeg(
        &[
            "-i",
            &media_s,
            "-af",
            "loudnorm=I=-16:TP=-1.5:print_format=json",
            "-f",
            "null",
            "-",
        ],
        cancel,
    )?;
    result.loudness = parse_loudnorm(&err3);

    // 4. Motion.
    if let Some(p) = progress {
        p("motion", 80);
    }
    check(cancel)?;
    let mfps = params.motion_fps;
    let (_, err4) = run_ffmpeg(&[
        "-i", &media_s,
        "-vf", &format!("fps={mfps},scale=64:36,tblend=all_mode=difference,blackframe=amount=0:threshold=32"),
        "-an", "-f", "null", "-",
    ], cancel)?;
    result.motion = parse_motion(&err4, result.duration_ms, mfps);

    // 5. Highlights (pure, deterministic).
    if let Some(p) = progress {
        p("highlights", 95);
    }
    result.highlights = highlight_windows(&result, 3, 2.0, 12.0);

    // 6. Auto color.
    check(cancel)?;
    result.auto_color = auto_color(media, cancel).unwrap_or_default();
    if let Some(p) = progress {
        p("done", 100);
    }
    Ok(result)
}

fn run_ffprobe(
    media: &str,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<String, AnalysisError> {
    let ffprobe =
        which_in_path("ffprobe").ok_or_else(|| AnalysisError::ToolNotFound("ffprobe".into()))?;
    let mut cmd = std::process::Command::new(ffprobe);
    cmd.args([
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        media,
    ]);
    let out = cmd.output().map_err(AnalysisError::Io)?;
    check(cancel)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(AnalysisError::Tool("ffprobe failed".into()))
    }
}

fn probe_duration_ms(json: &str) -> i64 {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| {
            v["format"]["duration"]
                .as_str()
                .and_then(|d| d.parse::<f64>().ok())
        })
        .map(|s| (s * 1000.0).round() as i64)
        .unwrap_or(0)
}

/// Parse `metadata=print` STDERR output. A cut frame prints
/// `frame:N pts:... pts_time:T` followed by `lavfi.scene_score=S` (score
/// belongs to the frame AT T). Scenes are recorded as
/// `{start: previous_cut_end, end: T, score}`.
#[must_use]
pub fn parse_scene_scores(stderr: &str, _extra: &str, _duration_ms: i64) -> Vec<Scene> {
    let mut scenes: Vec<Scene> = Vec::new();
    let mut pending_time: Option<f64> = None;
    let mut last_cut_end: f64 = 0.0;
    for line in stderr.lines() {
        let line = line.trim();
        // Lines look like: "[Parsed_metadata_3 @ 0x..] frame:0 pts:10 pts_time:2"
        if let Some(pos) = line.find("pts_time:") {
            if let Ok(tv) = line[pos + "pts_time:".len()..]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .parse::<f64>()
            {
                pending_time = Some(tv);
            }
        } else if let Some(pos) = line.find("lavfi.scene_score=") {
            if let Ok(score) = line[pos + "lavfi.scene_score=".len()..]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .parse::<f64>()
            {
                if let Some(t) = pending_time.take() {
                    scenes.push(Scene {
                        start_s: last_cut_end,
                        end_s: t,
                        score: score.max(0.15),
                    });
                    last_cut_end = t;
                }
            }
        }
    }
    scenes
}

/// Parse silencedetect stderr.
#[must_use]
pub fn parse_silence(stderr: &str) -> Vec<SilenceRange> {
    let mut out = Vec::new();
    let mut start: Option<f64> = None;
    for line in stderr.lines() {
        if let Some(v) = line.strip_prefix("[silencedetect") {
            if let Some(pos) = v.find("silence_start:") {
                start = v[pos + "silence_start:".len()..].trim().parse::<f64>().ok();
            } else if let Some(pos) = v.find("silence_end:") {
                if let Some(s) = start.take() {
                    let rest = &v[pos + "silence_end:".len()..];
                    let end = rest
                        .split('|')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .parse::<f64>()
                        .unwrap_or(0.0);
                    out.push(SilenceRange {
                        start_s: s,
                        end_s: end,
                    });
                }
            }
        }
    }
    if let Some(s) = start {
        out.push(SilenceRange {
            start_s: s,
            end_s: f64::INFINITY,
        });
    }
    out
}

/// Speech regions = inverse of silences (merged, min 0.5 s).
#[must_use]
pub fn invert_silences(silences: &[SilenceRange], duration_ms: i64) -> Vec<SilenceRange> {
    let dur_s = duration_ms as f64 / 1000.0;
    let mut speech = Vec::new();
    let mut cursor = 0.0;
    for s in silences {
        let s_start = s.start_s.min(dur_s);
        if s_start - cursor >= 0.5 {
            speech.push(SilenceRange {
                start_s: cursor,
                end_s: s_start,
            });
        }
        cursor = cursor.max(s.end_s.min(dur_s));
    }
    if dur_s - cursor >= 0.5 {
        speech.push(SilenceRange {
            start_s: cursor,
            end_s: dur_s,
        });
    }
    speech
}

/// Parse loudnorm's JSON tail from stderr.
#[must_use]
pub fn parse_loudnorm(stderr: &str) -> LoudnessInfo {
    let start = stderr.rfind('{');
    let end = stderr.rfind('}');
    if let (Some(s), Some(e)) = (start, end) {
        if e > s {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stderr[s..=e]) {
                return LoudnessInfo {
                    integrated_lufs: v["input_i"].as_str().and_then(|x| x.parse().ok()),
                    true_peak_db: v["input_tp"].as_str().and_then(|x| x.parse().ok()),
                };
            }
        }
    }
    LoudnessInfo::default()
}

/// Parse blackframe output into a per-second activity curve (0..1).
/// High pblack = little pixel change = low activity.
#[must_use]
pub fn parse_motion(stderr: &str, duration_ms: i64, fps: u32) -> MotionCurve {
    let secs = (duration_ms as f64 / 1000.0).ceil().max(1.0) as usize;
    let mut sums = vec![0.0f64; secs];
    let mut counts = vec![0u64; secs];
    for line in stderr.lines() {
        if let Some(p) = line.find("pblack:") {
            let pts_t = line.find("pts_time:").and_then(|i| {
                line[i + "pts_time:".len()..]
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())
            });
            let Some(t) = pts_t else { continue };
            let pb: f64 = line[p + "pblack:".len()..]
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100.0);
            let idx = (t as usize).min(secs - 1);
            sums[idx] += (100.0 - pb) / 100.0;
            counts[idx] += 1;
        }
    }
    // Mean activity across sampled frames in each second (0..1).
    let _ = fps;
    MotionCurve {
        per_second: sums
            .iter()
            .zip(counts.iter())
            .map(|(s, c)| {
                if *c > 0 {
                    (s / (*c as f64)).clamp(0.0, 1.0) as f32
                } else {
                    0.0
                }
            })
            .collect(),
    }
}

/// Deterministic highlight scoring: combines motion, scene change density,
/// and speech presence. Returns up to `max_windows` windows of
/// `[min_len_s, max_len_s]` sorted by score, non-overlapping.
#[must_use]
pub fn highlight_windows(
    analysis: &Analysis,
    max_windows: usize,
    min_len_s: f64,
    max_len_s: f64,
) -> Vec<HighlightWindow> {
    let dur_s = analysis.duration_ms as f64 / 1000.0;
    if dur_s < min_len_s {
        return Vec::new();
    }
    let secs = analysis.motion.per_second.len().max(1);
    let mut scores: Vec<f64> = vec![0.0; secs];
    for (i, m) in analysis.motion.per_second.iter().enumerate() {
        scores[i] += f64::from(*m) * 1.0;
        let t = i as f64;
        let in_speech = analysis
            .speech_regions
            .iter()
            .any(|r| t >= r.start_s && t <= r.end_s);
        if in_speech {
            scores[i] += 0.3;
        }
        let near_scene = analysis.scenes.iter().any(|s| (s.end_s - t).abs() < 0.6);
        if near_scene {
            scores[i] += 0.4;
        }
    }
    // Window sums at 1s granularity.
    let wlen = min_len_s.round().max(1.0) as usize;
    let mut cands: Vec<(usize, f64)> = (0..secs.saturating_sub(wlen) + 1)
        .map(|i| {
            let sum: f64 = scores[i..(i + wlen).min(secs)].iter().sum();
            (i, sum)
        })
        .collect();
    cands.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut chosen: Vec<HighlightWindow> = Vec::new();
    let max_end = dur_s.min(max_len_s * (max_windows as f64) * 3.0 + wlen as f64);
    for (i, sum) in cands {
        if chosen.len() >= max_windows {
            break;
        }
        let start = i as f64;
        let end = (start + wlen as f64).min(dur_s);
        if end - start < min_len_s * 0.9 {
            continue;
        }
        let overlaps = chosen
            .iter()
            .any(|w| start < w.end_s + 1.0 && end > w.start_s - 1.0);
        if overlaps || start > max_end {
            continue;
        }
        chosen.push(HighlightWindow {
            start_s: start,
            end_s: end,
            score: sum,
        });
    }
    chosen.sort_by(|a, b| {
        a.start_s
            .partial_cmp(&b.start_s)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chosen
}

/// Estimate color stats from one downscaled RGB24 frame; produce suggested
/// correction. Pure over the byte buffer (unit-testable).
#[must_use]
pub fn auto_color_from_rgb(rgb: &[u8]) -> AutoColorEstimate {
    let n = rgb.len() / 3;
    if n == 0 {
        return AutoColorEstimate::default();
    }
    let mut r = 0u64;
    let mut g = 0u64;
    let mut b = 0u64;
    for px in rgb.chunks(3) {
        r += u64::from(px[0]);
        g += u64::from(px[1]);
        b += u64::from(px[2]);
    }
    let r = r as f64 / n as f64 / 255.0;
    let g = g as f64 / n as f64 / 255.0;
    let b = b as f64 / n as f64 / 255.0;
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let sat = if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
    AutoColorEstimate {
        mean_luma_0_1: luma,
        mean_saturation_0_1: sat,
        r_g_balance: if g > 0.0 { r / g } else { 1.0 },
        b_g_balance: if g > 0.0 { b / g } else { 1.0 },
        // Underexposed -> +exposure, oversaturated -> slight -saturation.
        suggested_exposure: ((0.42 - luma) * 1.5).clamp(-0.6, 0.6),
        suggested_saturation: if sat > 0.55 {
            0.92
        } else if sat < 0.18 {
            1.12
        } else {
            1.0
        },
        suggested_temperature: ((g - r) * 0.5 + (g - b) * 0.25).clamp(-0.3, 0.3),
    }
}

fn auto_color(
    media: &Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<AutoColorEstimate, AnalysisError> {
    let ffmpeg = which_ffmpeg().ok_or_else(|| AnalysisError::ToolNotFound("ffmpeg".into()))?;
    let media_s = media.to_string_lossy().into_owned();
    let (out, _) = run_ffmpeg_raw(
        &ffmpeg.to_string_lossy(),
        &[
            "-ss",
            "1",
            "-i",
            &media_s,
            "-frames:v",
            "1",
            "-vf",
            "scale=64:64",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ],
        cancel,
    )?;
    Ok(auto_color_from_rgb(&out))
}

fn run_ffmpeg_raw(
    ffmpeg: &str,
    args: &[&str],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(Vec<u8>, String), AnalysisError> {
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(args);
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(AnalysisError::Io)?;
    loop {
        check(cancel)?;
        match child.try_wait().map_err(AnalysisError::Io)? {
            Some(status) => {
                use std::io::Read;
                let mut out = Vec::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_end(&mut out);
                }
                let err = child
                    .stderr
                    .take()
                    .map(|mut s| {
                        let mut b = Vec::new();
                        let _ = s.read_to_end(&mut b);
                        String::from_utf8_lossy(&b).into_owned()
                    })
                    .unwrap_or_default();
                if status.success() {
                    return Ok((out, err));
                }
                return Err(AnalysisError::Tool(
                    err.lines().last().unwrap_or("unknown").to_string(),
                ));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(30)),
        }
    }
}

/// Shared progress counter helper for UI (atomic).
#[derive(Debug, Default)]
pub struct Progress {
    pub stage: std::sync::Mutex<String>,
    pub percent: AtomicU64,
    _arc: Option<Arc<()>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_parse_and_invert() {
        let err = "[silencedetect @ 0x0] silence_start: 1.02\n\
                   [silencedetect @ 0x0] silence_end: 2.48 | silence_duration: 1.46\n\
                   [silencedetect @ 0x0] silence_start: 3.5\n";
        let s = parse_silence(err);
        assert_eq!(s.len(), 2);
        assert!((s[0].start_s - 1.02).abs() < 1e-6);
        assert!((s[0].end_s - 2.48).abs() < 1e-6);
        let speech = invert_silences(&s, 5_000);
        assert!(!speech.is_empty());
        // Speech covers 0..1.02 and 2.48..3.5.
        assert!((speech[0].start_s - 0.0).abs() < 1e-6);
        assert!((speech[0].end_s - 1.02).abs() < 1e-6);
    }

    #[test]
    fn loudnorm_json_tail() {
        let err = "junk\n[Parsed_loudnorm] {\n\t\"input_i\" : \"-23.40\",\n\t\"input_tp\" : \"-1.20\"\n}\nmore";
        let li = parse_loudnorm(err);
        assert_eq!(li.integrated_lufs, Some(-23.4));
        assert_eq!(li.true_peak_db, Some(-1.2));
    }

    #[test]
    fn scene_score_pairing_score_after_time() {
        let err = "[Parsed_metadata_3 @ 0x0] frame:0 pts:10 pts_time:2\n\
                   [Parsed_metadata_3 @ 0x0] lavfi.scene_score=0.400000\n\
                   [Parsed_metadata_3 @ 0x0] frame:1 pts:150 pts_time:3\n\
                   [Parsed_metadata_3 @ 0x0] lavfi.scene_score=0.700000\n";
        let scenes = parse_scene_scores(err, "", 5_000);
        assert_eq!(scenes.len(), 2, "{scenes:?}");
        assert!((scenes[0].end_s - 2.0).abs() < 1e-6);
        assert!((scenes[0].score - 0.4).abs() < 1e-6);
        assert!((scenes[1].start_s - 2.0).abs() < 1e-6);
        assert!((scenes[1].end_s - 3.0).abs() < 1e-6);
    }

    #[test]
    fn motion_aggregation() {
        let err = "[blackframe @ 0x0] frame:1 pts:128 pts_time:0.5 pblack:80\n\
                   [blackframe @ 0x0] frame:2 pts:384 pts_time:1.5 pblack:20\n";
        let mc = parse_motion(err, 3_000, 4);
        assert_eq!(mc.per_second.len(), 3);
        assert!(
            mc.per_second[1] > mc.per_second[0],
            "low pblack = high motion"
        );
    }

    #[test]
    fn auto_color_pure_math() {
        // Dark blue-ish frame.
        let mut rgb = Vec::new();
        for _ in 0..100 {
            rgb.extend([30u8, 40, 90]);
        }
        let est = auto_color_from_rgb(&rgb);
        assert!(est.mean_luma_0_1 < 0.3);
        assert!(
            est.suggested_exposure > 0.0,
            "dark frame should suggest +exposure"
        );
        // Gray frame is neutral.
        let gray = vec![128u8; 300];
        let est2 = auto_color_from_rgb(&gray);
        assert!(est2.suggested_exposure.abs() < 0.2);
        assert!((est2.mean_saturation_0_1) < 0.01);
    }

    #[test]
    fn highlights_deterministic() {
        let mut a = Analysis::default();
        a.duration_ms = 30_000;
        a.motion.per_second = vec![0.1; 30];
        a.motion.per_second[10] = 0.9;
        a.motion.per_second[11] = 0.9;
        a.motion.per_second[25] = 0.8;
        let hl = highlight_windows(&a, 2, 2.0, 12.0);
        assert_eq!(hl.len(), 2);
        assert_eq!(hl[0].start_s, 10.0, "highest-activity window first");
    }
}
