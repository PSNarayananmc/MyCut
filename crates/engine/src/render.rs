//! RenderEngine: builds FFmpeg argument arrays from the validated project
//! model. Fixed pipeline order per piece:
//! decode → cut/speed → reframe/scale → correction/LUT → effects →
//! [composition: concat/xfade] → text/captions → encode. One graph, one
//! encode. No shell, argument arrays only.

use std::path::{Path, PathBuf};

use mycut_core::{
    AspectRatio, EffectInstance, Item, ItemKind, MediaRole, ParamValue, Position, Project,
    ReframeMode, TextKind, TimeMs, TransitionKind,
};

use crate::audiofx;
use crate::color::build_color_chain;
use crate::effects::{build_effect, numeric_params, EffectContext};
use crate::error::EngineError;
use crate::escape::{escape_drawtext, escape_filter_value};
use crate::probe::ProbeResult;

/// Hardware encoder choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HwChoice {
    /// Detect and verify, fall back to CPU automatically.
    #[default]
    Auto,
    None,
    Nvenc,
    Vaapi,
    Qsv,
}

/// One ffmpeg invocation, fully built.
#[derive(Debug, Clone)]
pub struct FfmpegGraph {
    pub program: String,
    pub args: Vec<String>,
    /// The composed -filter_complex string (exposed for golden tests + UI).
    pub filtergraph: String,
}

impl FfmpegGraph {
    #[must_use]
    pub fn argv(&self) -> Vec<String> {
        let mut v = vec![self.program.clone()];
        v.extend(self.args.iter().cloned());
        v
    }
}

pub struct RenderEngine {
    ffmpeg: String,
}

fn fmt_secs(ms: TimeMs) -> String {
    format!("{:.3}", ms as f64 / 1000.0)
}

impl RenderEngine {
    /// Resolve the ffmpeg binary (bundled override env var respected).
    ///
    /// # Errors
    /// [`EngineError::ToolNotFound`] when ffmpeg is absent.
    pub fn new() -> Result<Self, EngineError> {
        let path = crate::process::resolve_tool("ffmpeg")?;
        Ok(Self {
            ffmpeg: path.to_string_lossy().into_owned(),
        })
    }

    /// Build the final-export command for a project.
    ///
    /// # Errors
    /// [`EngineError`] for unsafe paths, unknown effects, or empty timelines.
    pub fn build_export_command(
        &self,
        project: &Project,
        project_dir: &Path,
        out_path: &Path,
        hw: HwChoice,
    ) -> Result<FfmpegGraph, EngineError> {
        let video_items = self.video_items(project);
        if video_items.is_empty() {
            return Err(EngineError::Tool("timeline has no video clips".into()));
        }
        let export = &project.export;
        let (out_w, out_h) = (
            export.width.max(2) - (export.width.max(2) % 2),
            export.height.max(2) - (export.height.max(2) % 2),
        );
        let fps = export.fps.max(1.0);

        // LUT jail: project dir + app-data LUT dir (env override for tests).
        let app_data = std::env::var("MYCUT_LUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/usr/share/mycut/luts"));
        let lut_roots = [project_dir.to_path_buf(), app_data];
        let jail = lut_jail(project_dir, &lut_roots);
        let color_chain = build_color_chain(&project.color, &jail)?;

        let mut args: Vec<String> = vec!["-hide_banner".into(), "-y".into()];

        // ---- video inputs + per-piece chains ----
        let mut piece_labels: Vec<String> = Vec::new();
        let mut piece_durations: Vec<f64> = Vec::new();
        let mut piece_has_audio: Vec<bool> = Vec::new();
        let mut chains: Vec<String> = Vec::new();
        for (idx, item) in video_items.iter().enumerate() {
            let (source_id, src_in, src_out, speed) = clip_range(&item.kind);
            let source = project
                .source(&source_id)
                .ok_or_else(|| EngineError::Tool(format!("missing source {source_id}")))?;
            let media = crate::pathing::resolve_in_project(project_dir, &source.rel_path)?;
            args.push("-ss".into());
            args.push(fmt_secs(src_in));
            args.push("-t".into());
            args.push(fmt_secs(src_out - src_in));
            args.push("-i".into());
            args.push(media.to_string_lossy().into_owned());

            let piece_dur = (src_out - src_in).max(1) as f64 / 1000.0 / speed;
            let mut chain: Vec<String> = Vec::new();
            if (speed - 1.0).abs() > 0.001 {
                chain.push(format!("setpts=PTS/{speed:.6}"));
            }
            // reframe/scale (fixed pipeline position #2)
            let src_w = source.width.max(2);
            let src_h = source.height.max(2);
            match project.reframe.as_ref() {
                Some(r) => {
                    let (cw, ch) = crop_window_for(r.ratio, src_w, src_h);
                    let (cx, cy) = if r.mode == ReframeMode::SubjectFollow {
                        crop_center_keyframed(item, src_w, src_h, cw, ch)
                    } else {
                        ("(iw-ow)/2".to_string(), "(ih-oh)/2".to_string())
                    };
                    chain.push(format!("crop={cw}:{ch}:{cx}:{cy}"));
                    chain.push(format!("scale={out_w}:{out_h}"));
                }
                None => {
                    if approx_ratio(src_w, src_h, out_w, out_h) {
                        chain.push(format!("scale={out_w}:{out_h}"));
                    } else {
                        chain.push(format!(
                            "scale={out_w}:{out_h}:force_original_aspect_ratio=decrease,pad={out_w}:{out_h}:(ow-iw)/2:(oh-ih)/2"
                        ));
                    }
                }
            }
            chain.push(format!("fps={fps:.6}"));
            // correction/LUT (position #3/#4)
            if !color_chain.is_empty() {
                chain.push(color_chain.clone());
            }
            // effects (position #5); windows are SOURCE-domain ms
            for eff in &item.effects {
                let (ws, we) = effect_window_local(eff, src_in, speed);
                let ctx = EffectContext {
                    start: ws,
                    end: we,
                    fps,
                    width: out_w,
                    height: out_h,
                    params: numeric_params(&eff.params),
                    color: text_param(&eff.params, "color"),
                };
                let frag = build_effect(&eff.def_id, &ctx)?;
                if !frag.is_empty() {
                    chain.push(frag);
                }
            }
            let label = format!("v{idx}");
            piece_labels.push(label.clone());
            piece_durations.push(piece_dur);
            piece_has_audio.push(source.has_audio);
            chains.push(format!("[{idx}:v]{}[{label}]", chain.join(",")));
        }

        // ---- extra audio inputs (AudioClip items on audio track) ----
        let audio_items = self.audio_items(project);
        let video_input_count = video_items.len();
        for item in audio_items.iter() {
            let (source_id, _in, _out, _speed) = clip_range(&item.kind);
            let source = project
                .source(&source_id)
                .ok_or_else(|| EngineError::Tool(format!("missing source {source_id}")))?;
            let media = crate::pathing::resolve_in_project(project_dir, &source.rel_path)?;
            args.push("-i".into());
            args.push(media.to_string_lossy().into_owned());
        }

        // ---- video composition: concat (cuts) or xfade chain ----
        let n = video_items.len();
        let transitions = transition_slots(project, n);
        let any_xfade = transitions.iter().any(|slot| slot.map(|t| t.1).is_some());
        let all_xfade = n > 1 && transitions.iter().all(|slot| matches!(slot, Some((_, Some(_)))));
        let composed_label = "vcomp";
        let video_graph: String = if n == 1 {
            format!("[v0]format=yuv420p[{composed_label}];")
        } else if all_xfade {
            let mut g = String::new();
            let mut cur = "v0".to_string();
            let mut composite_dur = piece_durations[0];
            for i in 1..n {
                let Some((dur_ms, Some(name))) = transitions[i - 1] else {
                    continue;
                };
                let d = (dur_ms as f64 / 1000.0).clamp(0.05, 3.0);
                let offset = (composite_dur - d).max(0.0);
                let outl = format!("vx{i}");
                g.push_str(&format!(
                    "[{cur}][v{i}]xfade=transition={name}:duration={d:.3}:offset={offset:.3}[{outl}];"
                ));
                cur = outl;
                composite_dur = offset + piece_durations[i];
            }
            format!("{g}[{cur}]format=yuv420p[{composed_label}];")
        } else if !any_xfade {
            let inputs: Vec<&str> = piece_labels.iter().map(|s| s.as_str()).collect();
            format!(
                "[{}]concat=n={n}:v=1:a=0[{composed_label}];",
                inputs.join("][")
            )
        } else {
            // Mixed xfade/cut: honest fallback to cuts (recorded in plan report
            // by the planner layer); cut joins are still correct.
            let inputs: Vec<&str> = piece_labels.iter().map(|s| s.as_str()).collect();
            format!(
                "[{}]concat=n={n}:v=1:a=0[{composed_label}];",
                inputs.join("][")
            )
        };

        // ---- post-composition: captions + text (TIMELINE domain) ----
        let mut post: Vec<String> = Vec::new();
        let caps = captions_entries(project).unwrap_or_default();
        if !caps.is_empty() {
            let ass_path = project_dir.join("captions.ass");
            let escaped = escape_filter_value(&ass_path.to_string_lossy());
            post.push(format!("subtitles=filename='{escaped}'"));
        }
        for text in self.text_items(project) {
            let frag = drawtext_fragment(text, out_w, out_h);
            if !frag.is_empty() {
                post.push(frag);
            }
        }
        let final_label = if post.is_empty() {
            composed_label.to_string()
        } else {
            "voutx".to_string()
        };

        // ---- audio graph ----
        let mut audio_graph = String::new();
        let mut audio_specs: Vec<(String, String)> = Vec::new(); // (label, role)
        for (idx, item) in video_items.iter().enumerate() {
            if !piece_has_audio[idx] {
                continue;
            }
            let (source_id, _in, _out, speed) = clip_range(&item.kind);
            let source = project
                .source(&source_id)
                .ok_or_else(|| EngineError::Tool(format!("missing source {source_id}")))?;
            let mut a: Vec<String> = Vec::new();
            if (speed - 1.0).abs() > 0.001 {
                a.extend(atempo_chain(speed));
            }
            a.push(format!("adelay={}|all=1", item.timeline_start_ms.max(0)));
            if (item.volume - 1.0).abs() > 0.001 {
                a.push(format!("volume={:.3}", item.volume));
            }
            let label = format!("a{idx}");
            audio_graph.push_str(&format!("[{idx}:a]{}[{label}];", a.join(",")));
            audio_specs.push((label, role_tag(source.role)));
        }
        for (k, item) in audio_items.iter().enumerate() {
            let (source_id, src_in, src_out, speed) = clip_range(&item.kind);
            let source = project
                .source(&source_id)
                .ok_or_else(|| EngineError::Tool(format!("missing source {source_id}")))?;
            let input_idx = video_input_count + k;
            let mut a: Vec<String> = vec![
                format!("atrim=start={}:end={}", fmt_secs(src_in), fmt_secs(src_out)),
                "asetpts=PTS-STARTPTS".into(),
            ];
            if (speed - 1.0).abs() > 0.001 {
                a.extend(atempo_chain(speed));
            }
            a.push(format!("adelay={}|all=1", item.timeline_start_ms.max(0)));
            if (item.volume - 1.0).abs() > 0.001 {
                a.push(format!("volume={:.3}", item.volume));
            }
            let label = format!("b{k}");
            audio_graph.push_str(&format!("[{input_idx}:a]{}[{label}];", a.join(",")));
            audio_specs.push((label, role_tag(source.role)));
        }

        let total_s = project.timeline_end_ms().max(1) as f64 / 1000.0;
        let master_chain = audiofx::build_master_chain(&project.audio, total_s);
        let has_audio = !audio_specs.is_empty();
        if has_audio {
            let duck = project.audio.duck_music_under_speech
                && audio_specs.iter().any(|(_, r)| r == "music")
                && audio_specs.iter().any(|(_, r)| r != "music");
            if duck {
                let speech: Vec<&str> = audio_specs
                    .iter()
                    .filter(|(_, r)| r != "music")
                    .map(|(l, _)| l.as_str())
                    .collect();
                let music: Vec<&str> = audio_specs
                    .iter()
                    .filter(|(_, r)| r == "music")
                    .map(|(l, _)| l.as_str())
                    .collect();
                if speech.len() == 1 {
                    audio_graph.push_str(&format!("[{}]anull[sp];", speech[0]));
                } else {
                    audio_graph.push_str(&format!(
                        "[{}]amix=inputs={}:normalize=0[sp];",
                        speech.join("]["),
                        speech.len()
                    ));
                }
                if music.len() == 1 {
                    audio_graph.push_str(&format!("[{}]anull[mu];", music[0]));
                } else {
                    audio_graph.push_str(&format!(
                        "[{}]amix=inputs={}:normalize=0[mu];",
                        music.join("]["),
                        music.len()
                    ));
                }
                audio_graph.push_str(
                    "[mu][sp]sidechaincompress=threshold=0.02:ratio=8:attack=5:release=300[duck];",
                );
                if master_chain.is_empty() {
                    audio_graph.push_str("[duck][sp]amix=inputs=2:normalize=0[aout];");
                } else {
                    audio_graph.push_str("[duck][sp]amix=inputs=2:normalize=0[duckmx];");
                    audio_graph.push_str(&format!("[duckmx]{master_chain}[aout];"));
                }
            } else {
                let inputs: Vec<&str> = audio_specs.iter().map(|(l, _)| l.as_str()).collect();
                if inputs.len() == 1 {
                    audio_graph.push_str(&format!("[{}]anull[amixpre];", inputs[0]));
                } else {
                    audio_graph.push_str(&format!(
                        "[{}]amix=inputs={}:normalize=0[amixpre];",
                        inputs.join("]["),
                        inputs.len()
                    ));
                }
                if master_chain.is_empty() {
                    audio_graph.push_str("[amixpre]anull[aout];");
                } else {
                    audio_graph.push_str(&format!("[amixpre]{master_chain}[aout];"));
                }
            }
        }

        // ---- assemble filter_complex ----
        let mut fg = String::new();
        for c in &chains {
            fg.push_str(c);
            fg.push(';');
        }
        fg.push_str(&video_graph);
        if !post.is_empty() {
            fg.push_str(&format!(
                "[{composed_label}]{}[{final_label}];",
                post.join(",")
            ));
        }
        if has_audio {
            fg.push_str(&audio_graph);
        }
        let fg = fg.trim_end_matches(';').to_string();
        if !fg.is_empty() {
            args.push("-filter_complex".into());
            args.push(fg.clone());
        }

        // ---- map + encoder + mux ----
        args.push("-map".into());
        args.push(format!("[{final_label}]"));
        if has_audio {
            args.push("-map".into());
            args.push("[aout]".into());
        } else {
            args.push("-an".into());
        }
        let eff_hw = match hw {
            HwChoice::Auto => auto_hw(),
            other => other,
        };
        let q = i64::from(export.quality).clamp(14, 34);
        let mut vaapi = false;
        match eff_hw {
            HwChoice::Nvenc => {
                args.extend([
                    "-c:v".into(),
                    "h264_nvenc".into(),
                    "-preset".into(),
                    "p4".into(),
                    "-rc".into(),
                    "vbr".into(),
                    "-cq".into(),
                    format!("{q}"),
                    "-b:v".into(),
                    "0".into(),
                ]);
            }
            HwChoice::Qsv => {
                args.extend([
                    "-c:v".into(),
                    "h264_qsv".into(),
                    "-global_quality".into(),
                    format!("{q}"),
                ]);
            }
            HwChoice::Vaapi => {
                vaapi = true;
                args.extend(["-vaapi_device".into(), "/dev/dri/renderD128".into()]);
                if let Some(pos) = find_arg(&args, "-filter_complex") {
                    args[pos + 1] = format!("{},format=nv12,hwupload", args[pos + 1]);
                }
                args.extend([
                    "-c:v".into(),
                    "h264_vaapi".into(),
                    "-global_quality".into(),
                    format!("{q}"),
                ]);
            }
            _ => {
                args.extend([
                    "-c:v".into(),
                    "libx264".into(),
                    "-preset".into(),
                    "veryfast".into(),
                    "-crf".into(),
                    format!("{q}"),
                ]);
            }
        }
        if !vaapi {
            args.push("-pix_fmt".into());
            args.push("yuv420p".into());
        }
        args.push("-r".into());
        args.push(format!("{fps:.6}"));
        args.push("-c:a".into());
        args.push(audio_codec_name(export).to_string());
        args.push("-b:a".into());
        args.push(format!("{}k", export.audio_bitrate_kbps));
        if matches!(
            export.container,
            mycut_core::Container::Mp4 | mycut_core::Container::Mov
        ) {
            args.push("-movflags".into());
            args.push("+faststart".into());
        }
        args.push(out_path.to_string_lossy().into_owned());
        Ok(FfmpegGraph {
            program: self.ffmpeg.clone(),
            args,
            filtergraph: fg,
        })
    }

    /// Proxy render command (throwaway preview file).
    ///
    /// # Errors
    /// [`EngineError`] on tool resolution failure.
    pub fn build_proxy_command(
        &self,
        media: &Path,
        out: &Path,
        height: u32,
    ) -> Result<FfmpegGraph, EngineError> {
        let args = vec![
            "-hide_banner".into(),
            "-y".into(),
            "-i".into(),
            media.to_string_lossy().into_owned(),
            "-vf".into(),
            format!("scale=-2:{height}"),
            "-c:v".into(),
            "libx264".into(),
            "-preset".into(),
            "ultrafast".into(),
            "-tune".into(),
            "fastdecode".into(),
            "-crf".into(),
            "28".into(),
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "96k".into(),
            "-movflags".into(),
            "+faststart".into(),
            out.to_string_lossy().into_owned(),
        ];
        Ok(FfmpegGraph {
            program: self.ffmpeg.clone(),
            args,
            filtergraph: format!("scale=-2:{height}"),
        })
    }

    /// Thumbnail command at source time `t_ms`.
    ///
    /// # Errors
    /// [`EngineError`] on tool resolution failure.
    pub fn build_thumbnail_command(
        &self,
        media: &Path,
        t_ms: TimeMs,
        out: &Path,
        width: u32,
    ) -> Result<FfmpegGraph, EngineError> {
        let args = vec![
            "-hide_banner".into(),
            "-y".into(),
            "-ss".into(),
            fmt_secs(t_ms.max(0)),
            "-i".into(),
            media.to_string_lossy().into_owned(),
            "-frames:v".into(),
            "1".into(),
            "-vf".into(),
            format!("scale={width}:-2"),
            "-q:v".into(),
            "3".into(),
            out.to_string_lossy().into_owned(),
        ];
        Ok(FfmpegGraph {
            program: self.ffmpeg.clone(),
            args,
            filtergraph: String::new(),
        })
    }

    fn video_items<'a>(&self, project: &'a Project) -> Vec<&'a Item> {
        project
            .track_of_kind(mycut_core::TrackKind::Video)
            .map(|t| {
                t.items
                    .iter()
                    .filter(|i| matches!(i.kind, ItemKind::VideoClip { .. }))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn audio_items<'a>(&self, project: &'a Project) -> Vec<&'a Item> {
        project
            .track_of_kind(mycut_core::TrackKind::Audio)
            .map(|t| {
                t.items
                    .iter()
                    .filter(|i| matches!(i.kind, ItemKind::AudioClip { .. }))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn text_items<'a>(&self, project: &'a Project) -> Vec<&'a Item> {
        project
            .track_of_kind(mycut_core::TrackKind::Text)
            .map(|t| {
                t.items
                    .iter()
                    .filter(|i| matches!(i.kind, ItemKind::Text { .. }))
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn clip_range(kind: &ItemKind) -> (String, TimeMs, TimeMs, f64) {
    match kind {
        ItemKind::VideoClip {
            source_id,
            source_in_ms,
            source_out_ms,
            speed,
        }
        | ItemKind::AudioClip {
            source_id,
            source_in_ms,
            source_out_ms,
            speed,
        } => (source_id.clone(), *source_in_ms, *source_out_ms, *speed),
        _ => (String::new(), 0, 0, 1.0),
    }
}

fn role_tag(role: MediaRole) -> String {
    match role {
        MediaRole::Music => "music".into(),
        MediaRole::Voiceover => "voice".into(),
        _ => "footage".into(),
    }
}

fn audio_codec_name(export: &mycut_core::ExportSettings) -> &'static str {
    match export.audio_codec {
        mycut_core::AudioCodec::Aac => "aac",
        mycut_core::AudioCodec::Mp3 => "libmp3lame",
        mycut_core::AudioCodec::Opus => "libopus",
        mycut_core::AudioCodec::Flac => "flac",
        mycut_core::AudioCodec::PcmS16 => "pcm_s16le",
    }
}

fn find_arg(args: &[String], name: &str) -> Option<usize> {
    args.iter().position(|a| a == name)
}

/// Transition slots between consecutive video items (length n-1):
/// `Some((duration_ms, xfade_name))`; `None` = plain cut.
#[must_use]
pub fn transition_slots(
    project: &Project,
    n: usize,
) -> Vec<Option<(TimeMs, Option<&'static str>)>> {
    if n < 2 {
        return Vec::new();
    }
    let mut slots = vec![None; n - 1];
    for t in &project.transitions {
        if t.after_index < n - 1 && t.kind != TransitionKind::Cut {
            if let Some(name) = t.kind.ffmpeg_name() {
                slots[t.after_index] = Some((t.duration_ms.max(50), Some(name)));
            }
        }
    }
    slots
}

/// LUT jail closure: LUT paths must stay inside project dir or allowed roots.
fn lut_jail<'a>(_project_dir: &Path, roots: &'a [PathBuf]) -> impl Fn(&str) -> bool + 'a {
    move |p: &str| {
        if p.split('/').any(|seg| seg == "..") {
            return false;
        }
        let path = PathBuf::from(p);
        roots
            .iter()
            .any(|root| path.starts_with(root) || root.join(&path).exists())
    }
}

/// Convert an effect window (SOURCE-domain ms) to piece-local seconds.
#[must_use]
pub fn effect_window_local(eff: &EffectInstance, src_in: TimeMs, speed: f64) -> (f64, f64) {
    let ws = eff.window_start_ms.unwrap_or(0);
    let we = eff.window_end_ms.unwrap_or(i64::MAX / 2);
    let s = (ws - src_in).max(0) as f64 / 1000.0 / speed;
    if we > i64::MAX / 4 {
        (s, 3600.0)
    } else {
        (s, (we - src_in).max(0) as f64 / 1000.0 / speed)
    }
}

/// Crop window (w,h) for a ratio inside a source frame (even numbers).
#[must_use]
pub fn crop_window_for(ratio: AspectRatio, src_w: u32, src_h: u32) -> (u32, u32) {
    let (rn, rd) = ratio.wh();
    let target = f64::from(rn) / f64::from(rd);
    let src = f64::from(src_w) / f64::from(src_h);
    let (mut w, mut h) = if src > target {
        ((src_h as f64 * target).round() as u32, src_h)
    } else {
        (src_w, (src_w as f64 / target).round() as u32)
    };
    w = w.clamp(2, src_w);
    h = h.clamp(2, src_h);
    (w - w % 2, h - h % 2)
}

fn approx_ratio(w: u32, h: u32, tw: u32, th: u32) -> bool {
    ((f64::from(w) / f64::from(h)) - (f64::from(tw) / f64::from(th))).abs() < 0.02
}

/// Build piecewise-linear crop TOP-LEFT expressions from timeline-domain
/// keyframes (produced by subject-follow analysis). Falls back to center.
#[must_use]
pub fn crop_center_keyframed(
    item: &Item,
    src_w: u32,
    src_h: u32,
    crop_w: u32,
    crop_h: u32,
) -> (String, String) {
    let kf = |prop: &str| -> Vec<(f64, f64)> {
        item.effects
            .iter()
            .flat_map(|e| e.keyframes.iter())
            .find(|k| k.property == prop)
            .map(|k| {
                // Timeline ms -> piece-local seconds (input seek resets t=0).
                k.keyframes
                    .iter()
                    .map(|f| {
                        (
                            (f.t_ms - item.timeline_start_ms).max(0) as f64 / 1000.0,
                            f.value,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let kfx = kf("crop_x");
    let kfy = kf("crop_y");
    if kfx.is_empty() || kfy.is_empty() {
        return ("(iw-ow)/2".into(), "(ih-oh)/2".into());
    }
    let max_x = (src_w - crop_w) as f64;
    let max_y = (src_h - crop_h) as f64;
    // Keyframes store CENTER x/y in source px; crop x/y is top-left.
    let xs: Vec<(f64, f64)> = kfx
        .into_iter()
        .map(|(t, cx)| (t, (cx - f64::from(crop_w) / 2.0).clamp(0.0, max_x)))
        .collect();
    let ys: Vec<(f64, f64)> = kfy
        .into_iter()
        .map(|(t, cy)| (t, (cy - f64::from(crop_h) / 2.0).clamp(0.0, max_y)))
        .collect();
    (
        piecewise_expr(&xs, max_x / 2.0),
        piecewise_expr(&ys, max_y / 2.0),
    )
}

/// Nested `if(lt(t,...))` piecewise-linear expression (≤120 samples).
#[must_use]
pub fn piecewise_expr(points: &[(f64, f64)], fallback: f64) -> String {
    if points.is_empty() {
        return format!("{fallback:.1}");
    }
    if points.len() == 1 {
        return format!("{:.1}", points[0].1);
    }
    let pts: Vec<(f64, f64)> = if points.len() > 120 {
        let step = points.len() as f64 / 120.0;
        (0..120)
            .map(|i| points[((i as f64) * step) as usize])
            .collect()
    } else {
        points.to_vec()
    };
    let mut expr = format!("{:.1}", pts.last().unwrap().1);
    for w in pts.windows(2).rev() {
        let (t0, v0) = &w[0];
        let (t1, v1) = &w[1];
        let seg = if (t1 - t0).abs() < 1e-6 {
            format!("{v0:.1}")
        } else {
            format!("{v0:.1}+({v1:.1}-{v0:.1})*(t-{t0:.3})/{:.3}", t1 - t0)
        };
        expr = format!("if(lt(t,{t0:.3}),{seg},{expr})");
    }
    expr
}

fn text_param(
    params: &std::collections::BTreeMap<String, ParamValue>,
    key: &str,
) -> Option<String> {
    params.get(key).and_then(|v| match v {
        ParamValue::Text(t) => Some(t.clone()),
        ParamValue::Number(_) => None,
    })
}

fn drawtext_fragment(item: &Item, _out_w: u32, out_h: u32) -> String {
    let ItemKind::Text {
        kind,
        text,
        position,
        scale,
        opacity,
    } = &item.kind
    else {
        return String::new();
    };
    let base = match kind {
        TextKind::Watermark => out_h as f64 * 0.035,
        TextKind::LowerThird => out_h as f64 * 0.045,
        _ => out_h as f64 * 0.065,
    };
    let fontsize = (base * scale).round().max(8.0);
    let margin = (out_h as f64 * 0.05).round() as u32;
    let (x, y) = match position {
        Position::TopLeft => (format!("{margin}"), format!("{margin}")),
        Position::TopCenter => ("(w-text_w)/2".to_string(), format!("{margin}")),
        Position::TopRight => (format!("w-text_w-{margin}"), format!("{margin}")),
        Position::Center => ("(w-text_w)/2".to_string(), "(h-text_h)/2".to_string()),
        Position::BottomLeft => (format!("{margin}"), format!("h-text_h-{margin}")),
        Position::BottomCenter => ("(w-text_w)/2".to_string(), format!("h-text_h-{margin}")),
        Position::BottomRight => (format!("w-text_w-{margin}"), format!("h-text_h-{margin}")),
    };
    let escaped = escape_drawtext(text);
    let start = item.timeline_start_ms as f64 / 1000.0;
    let end = (item.timeline_start_ms + item.timeline_duration_ms) as f64 / 1000.0;
    format!(
        "drawtext=font='Sans':text='{escaped}':fontsize={fontsize}:fontcolor=white@{op:.2}:x={x}:y={y}:borderw=2:bordercolor=black@0.6:enable='between(t,{start:.3},{end:.3})'",
        op = opacity.clamp(0.05, 1.0)
    )
}

/// Captions entries from the captions track (timeline domain).
#[must_use]
pub fn captions_entries(project: &Project) -> Option<Vec<mycut_core::CaptionEntry>> {
    project
        .track_of_kind(mycut_core::TrackKind::Captions)
        .and_then(|t| {
            t.items
                .iter()
                .find(|i| matches!(i.kind, ItemKind::Captions { .. }))
        })
        .map(|i| match &i.kind {
            ItemKind::Captions { entries, .. } => entries.clone(),
            _ => Vec::new(),
        })
}

/// Probe summary used by the import flow and the AI context builder.
#[must_use]
pub fn summarize_probe(p: &ProbeResult) -> std::collections::BTreeMap<String, String> {
    let mut m = std::collections::BTreeMap::new();
    m.insert("duration_ms".into(), p.duration_ms().to_string());
    let (w, h) = p.resolution();
    m.insert("resolution".into(), format!("{w}x{h}"));
    let (fn_, fd) = p.fps();
    m.insert("fps".into(), format!("{fn_}/{fd}"));
    m.insert("has_audio".into(), p.has_audio().to_string());
    if let Some(v) = p.video_stream() {
        m.insert(
            "video_codec".into(),
            v.codec_name.clone().unwrap_or_default(),
        );
        m.insert("pix_fmt".into(), v.pix_fmt.clone().unwrap_or_default());
    }
    if let Some(a) = p.audio_stream() {
        m.insert(
            "audio_codec".into(),
            a.codec_name.clone().unwrap_or_default(),
        );
    }
    m
}

/// atempo supports 0.5..2 per instance; chain for 0.25..4.
#[must_use]
pub fn atempo_chain(speed: f64) -> Vec<String> {
    let mut s = speed.clamp(0.25, 4.0);
    let mut out = Vec::new();
    while s > 2.0 + f64::EPSILON {
        out.push("atempo=2.0".into());
        s /= 2.0;
    }
    while s < 0.5 - f64::EPSILON {
        out.push("atempo=0.5".into());
        s /= 0.5;
    }
    out.push(format!("atempo={s:.6}"));
    out
}

/// Cached hardware encoder detection → first usable (VAAPI preferred on the
/// low-end Intel iGPU target, then NVENC, QSV), else CPU.
#[must_use]
pub fn auto_hw() -> HwChoice {
    let map = crate::process::hw_encoders_cached();
    if map.get("h264_vaapi").copied().unwrap_or(false) {
        HwChoice::Vaapi
    } else if map.get("h264_nvenc").copied().unwrap_or(false) {
        HwChoice::Nvenc
    } else if map.get("h264_qsv").copied().unwrap_or(false) {
        HwChoice::Qsv
    } else {
        HwChoice::None
    }
}
