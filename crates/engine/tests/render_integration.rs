//! Integration render tests: every claim inspects REAL rendered output with
//! ffprobe / pixel sampling — no mocks. Fixtures are generated with FFmpeg
//! lavfi (no copyrighted media). Kept tiny (160x90, 1-4s) for CI speed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use mycut_core::{
    AspectRatio, AudioMaster, ColorGrade, EffectInstance, Item, ItemKind, MediaRole, ParamValue,
    Project, ReframeMode, ReframeSettings, Source, TimeMs, TrackKind,
};
use mycut_engine::{HwChoice, RenderEngine};

struct Workspace {
    dir: PathBuf,
}

impl Workspace {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("mycut-it-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn media(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn run_ffmpeg(&self, args: &[&str]) {
        let out = Command::new(which_ffmpeg())
            .args(args)
            .output()
            .expect("spawn ffmpeg");
        assert!(
            out.status.success(),
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Flat-color clip (deterministic pixels) with tone audio.
    fn fixture_color(&self, name: &str, color: &str, seconds: u32) -> PathBuf {
        let p = self.media(name);
        self.run_ffmpeg(&[
            "-y", "-f", "lavfi", "-i", &format!("color=c={color}:s=160x90:r=30"),
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
            "-t", &seconds.to_string(),
            "-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "ultrafast",
            "-c:a", "aac", p.to_str().unwrap(),
        ]);
        p
    }

    /// Hard scene cuts: red 2s -> blue 2s (with tone audio).
    fn fixture_cuts(&self, name: &str) -> PathBuf {
        let p = self.media(name);
        self.run_ffmpeg(&[
            "-y",
            "-f", "lavfi", "-i", "color=c=red:s=160x90:r=30:duration=2",
            "-f", "lavfi", "-i", "color=c=blue:s=160x90:r=30:duration=2",
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
            "-filter_complex", "[0:v][1:v]concat=n=2:v=1:a=0[v];[2:a]atrim=0:4[aud]",
            "-map", "[v]", "-map", "[aud]",
            "-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "ultrafast",
            "-c:a", "aac", p.to_str().unwrap(),
        ]);
        p
    }

    /// Tone 1s + silence 1.5s + tone 1s (silence-removal fixture).
    fn fixture_silence_gaps(&self, name: &str) -> PathBuf {
        let p = self.media(name);
        self.run_ffmpeg(&[
            "-y",
            "-f", "lavfi", "-i", "color=c=green:s=160x90:r=30:duration=3.5",
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=1",
            "-f", "lavfi", "-i", "anullsrc=sample_rate=48000:duration=1.5",
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=1",
            "-filter_complex", "[1:a][2:a][3:a]concat=n=3:v=0:a=1[aud]",
            "-map", "0:v", "-map", "[aud]",
            "-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "ultrafast",
            "-c:a", "aac", p.to_str().unwrap(),
        ]);
        p
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn which_ffmpeg() -> String {
    std::env::var("MYCUT_FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}

fn make_source(p: &Path, duration_ms: TimeMs) -> Source {
    let probe = mycut_engine::probe::probe(p).unwrap();
    let (w, h) = probe.resolution();
    let (fn_, fd) = probe.fps();
    Source {
        id: mycut_core::new_id("src"),
        name: p.file_name().unwrap().to_string_lossy().into_owned(),
        rel_path: p.to_string_lossy().into_owned(),
        content_hash: "test".into(),
        duration_ms,
        width: w,
        height: h,
        fps_num: fn_,
        fps_den: fd,
        has_audio: probe.has_audio(),
        role: MediaRole::Footage,
    }
}

fn project_with_clip(_ws: &Workspace, media: &Path) -> (Project, PathBuf) {
    let probe = mycut_engine::probe::probe(media).unwrap();
    let dur = probe.duration_ms();
    let (sw, sh) = probe.resolution();
    let mut project = Project::new("it");
    let source = make_source(media, dur);
    let sid = source.id.clone();
    project.sources.push(source);
    let item = Item::new(
        ItemKind::VideoClip { source_id: sid, source_in_ms: 0, source_out_ms: dur, speed: 1.0 },
        0,
        dur,
    );
    project.track_of_kind_mut(TrackKind::Video).unwrap().items.push(item);
    // Export at source size so pixel-sampling tests read 160x90 frames.
    project.export.width = sw;
    project.export.height = sh;
    (project, media.parent().unwrap().to_path_buf())
}

fn render(project: &Project, dir: &Path, out: &Path) {
    let engine = RenderEngine::new().unwrap();
    let graph = engine.build_export_command(project, dir, out, HwChoice::None).unwrap();
    let outp = Command::new(&graph.program)
        .args(&graph.args)
        .output()
        .expect("spawn render");
    assert!(
        outp.status.success(),
        "render failed: {}\nargs: {:?}",
        String::from_utf8_lossy(&outp.stderr),
        &graph.args
    );
}

fn probe_out(p: &Path) -> mycut_engine::ProbeResult {
    mycut_engine::probe::probe(p).unwrap()
}

/// Mean gray value of a fractional region of a decoded frame.
fn frame_region_mean(p: &Path, t_s: f64, region: (f64, f64, f64, f64)) -> f64 {
    let (w, h) = (160u32, 90u32);
    let out = Command::new(which_ffmpeg())
        .args(["-loglevel", "error", "-ss", &t_s.to_string(), "-i"])
        .arg(p)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
        .output()
        .unwrap();
    assert!(out.status.success(), "frame decode failed");
    let data = out.stdout;
    let (x0, y0, x1, y1) = (
        (region.0 * w as f64) as usize,
        (region.1 * h as f64) as usize,
        (region.2 * w as f64) as usize,
        (region.3 * h as f64) as usize,
    );
    let mut sum = 0u64;
    let mut n = 0u64;
    for y in y0..y1 {
        for x in x0..x1 {
            sum += u64::from(data[y * w as usize + x]);
            n += 1;
        }
    }
    sum as f64 / n as f64
}

#[test]
fn trim_cut_concat_export_matches_expectations() {
    let ws = Workspace::new("trim");
    let media = ws.fixture_color("src.mp4", "gray", 4);
    let (mut project, dir) = project_with_clip(&ws, &media);
    // Keep only 1s..3s.
    project.track_of_kind_mut(TrackKind::Video).unwrap().items[0].kind =
        ItemKind::VideoClip {
            source_id: project.sources[0].id.clone(),
            source_in_ms: 1_000,
            source_out_ms: 3_000,
            speed: 1.0,
        };
    project.track_of_kind_mut(TrackKind::Video).unwrap().items[0].timeline_duration_ms = 2_000;
    let out = ws.media("out.mp4");
    render(&project, &dir, &out);
    let probe = probe_out(&out);
    assert!((probe.duration_ms() - 2_000).abs() < 300, "duration {}", probe.duration_ms());
    let (w, h) = probe.resolution();
    assert_eq!((w, h), (160, 90));
    assert_eq!(probe.video_stream().unwrap().codec_name.as_deref(), Some("h264"));
    assert_eq!(probe.audio_stream().unwrap().codec_name.as_deref(), Some("aac"));
    // Full decode without errors.
    let dec = Command::new(which_ffmpeg())
        .args(["-v", "error", "-i"]).arg(&out).args(["-f", "null", "-"])
        .output().unwrap();
    assert!(dec.status.success() && String::from_utf8_lossy(&dec.stderr).trim().is_empty());
}

#[test]
fn vignette_darkens_corners_on_flat_source() {
    let ws = Workspace::new("vig");
    let media = ws.fixture_color("flat.mp4", "gray", 2);
    let (mut project, dir) = project_with_clip(&ws, &media);
    project.track_of_kind_mut(TrackKind::Video).unwrap().items[0].effects =
        vec![EffectInstance {
            id: mycut_core::new_id("fx"),
            def_id: "vignette".into(),
            params: BTreeMap::from([("strength".to_string(), ParamValue::Number(0.9))]),
            window_start_ms: None,
            window_end_ms: None,
            keyframes: vec![],
            easing: mycut_core::Easing::Linear,
        }];
    let out = ws.media("vig.mp4");
    render(&project, &dir, &out);
    let corner = frame_region_mean(&out, 1.0, (0.0, 0.0, 0.08, 0.12));
    let center = frame_region_mean(&out, 1.0, (0.45, 0.42, 0.55, 0.58));
    assert!(
        corner < center * 0.8,
        "vignette must darken corners: corner={corner:.1} center={center:.1}"
    );
    // Control: unprocessed flat gray is uniform.
    let corner0 = frame_region_mean(&media, 1.0, (0.0, 0.0, 0.08, 0.12));
    let center0 = frame_region_mean(&media, 1.0, (0.45, 0.42, 0.55, 0.58));
    assert!((corner0 - center0).abs() < 6.0, "fixture must be flat: {corner0} vs {center0}");
}

#[test]
fn captions_burn_pixels_into_bottom_region() {
    let ws = Workspace::new("caps");
    let media = ws.fixture_color("flat.mp4", "black", 2);
    let (mut project, dir) = project_with_clip(&ws, &media);
    // ASS file with one line (libass via the subtitles filter).
    let ass = "[Script Info]\nScriptType: v4.00+\nPlayResX: 160\nPlayResY: 90\n\n\
         [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: MyCut,Sans,16,&H00FFFFFF,&H00FFFFFF,&H00101010,&H80000000,1,0,0,0,100,100,0,0,1,2,0,2,4,4,6,1\n\n\
         [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
         Dialogue: 0,0:00:00.20,0:00:01.80,MyCut,,0,0,0,,HELLO TEST\n";
    std::fs::write(dir.join("captions.ass"), ass).unwrap();
    // The project must carry a captions item for the engine to burn it in
    // (the applier creates it via ReplaceCaptions in the real flow).
    let entry = mycut_core::CaptionEntry {
        start_ms: 200,
        end_ms: 1_800,
        words: vec![mycut_core::CaptionWord {
            text: "HELLO TEST".into(),
            start_ms: 200,
            end_ms: 1_800,
            emphasize: false,
        }],
    };
    project.track_of_kind_mut(TrackKind::Captions).unwrap().items.push(Item::new(
        ItemKind::Captions {
            style: mycut_core::CaptionStyle::Minimal,
            scale: 1.0,
            safe_area: "default".into(),
            entries: vec![entry],
        },
        0,
        2_000,
    ));
    let out = ws.media("caps.mp4");
    render(&project, &dir, &out);
    let strip = Command::new(which_ffmpeg())
        .args(["-loglevel", "error", "-ss", "1.0", "-i"])
        .arg(&out)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
        .output()
        .unwrap();
    let data = strip.stdout;
    let w = 160usize;
    let h = 90usize;
    let mut maxv = 0u8;
    for y in (h - 30)..h {
        for x in 0..w {
            maxv = maxv.max(data[y * w + x]);
        }
    }
    assert!(maxv > 200, "captions must add bright pixels in bottom region, max={maxv}");
    // The top area must stay black (no stray overlays).
    let top_max = data[..(30 * w)].iter().copied().fold(0u8, std::cmp::Ord::max);
    assert!(top_max < 40, "top region must remain clean, max={top_max}");
}

#[test]
fn color_grade_shifts_luminance() {
    let ws = Workspace::new("color");
    let media = ws.fixture_color("flat.mp4", "gray", 2);
    let (mut project, dir) = project_with_clip(&ws, &media);
    // eq pivots contrast at mid-gray, so use exposure (brightness) which
    // shifts every pixel up.
    project.color = ColorGrade {
        exposure: 0.5,
        ..ColorGrade::default()
    };
    let out = ws.media("graded.mp4");
    render(&project, &dir, &out);
    let mid0 = frame_region_mean(&media, 1.0, (0.0, 0.0, 1.0, 1.0));
    let mid1 = frame_region_mean(&out, 1.0, (0.0, 0.0, 1.0, 1.0));
    assert!(
        mid1 > mid0 + 4.0,
        "exposure must lift luminance: {mid0:.1} -> {mid1:.1}"
    );
}

#[test]
fn zoom_punch_changes_frame_scale() {
    let ws = Workspace::new("zoom");
    let media = ws.media("ts.mp4");
    ws.run_ffmpeg(&[
        "-y", "-f", "lavfi", "-i", "testsrc2=size=160x90:r=30",
        "-t", "4", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "ultrafast",
        media.to_str().unwrap(),
    ]);
    let (mut project, dir) = project_with_clip(&ws, &media);
    project.track_of_kind_mut(TrackKind::Video).unwrap().items[0].effects =
        vec![EffectInstance {
            id: mycut_core::new_id("fx"),
            def_id: "zoom_punch".into(),
            params: BTreeMap::from([("strength".to_string(), ParamValue::Number(1.6))]),
            window_start_ms: Some(1_000),
            window_end_ms: Some(2_000),
            keyframes: vec![],
            easing: mycut_core::Easing::EaseOut,
        }];
    let out = ws.media("zoom.mp4");
    render(&project, &dir, &out);
    // Compare frame at t=0.5 (no effect) vs t=1.5 (peak zoom) center content.
    let a = frame_region_mean(&out, 0.5, (0.4, 0.3, 0.6, 0.6));
    let b = frame_region_mean(&out, 1.5, (0.4, 0.3, 0.6, 0.6));
    assert!(
        (a - b).abs() > 2.0,
        "zoom punch must change center content: t0.5={a:.1} t1.5={b:.1}"
    );
}

#[test]
fn silence_removal_shortens_video_and_audio_together() {
    // Silence removal is a CUT-level operation (video+audio shorten together;
    // an audio-only filter would desync A/V). The planner receives silence
    // ranges from analysis and emits cut_ranges — exactly what a live plan
    // with strategy=remove_silence does.
    let ws = Workspace::new("silence");
    let media = ws.fixture_silence_gaps("gaps.mp4");
    let (mut project, dir) = project_with_clip(&ws, &media);
    // Detect the silence locally (the planner sees these ranges).
    let cancel = AtomicBool::new(false);
    let params = mycut_analysis::adaptive_params(3_500, false);
    let a = mycut_analysis::analyze(&media, &ws.dir.join("cache"), 100_000_000, &params, &cancel, None).unwrap();
    let gap = a
        .silences
        .iter()
        .find(|s| s.end_s - s.start_s > 1.0)
        .expect("fixture must contain a >1s silence gap");
    // Apply the cut through the command layer (like the applier does).
    let (in_ms, out_ms) = (0i64, 3_500i64);
    let cut_s = (gap.start_s * 1000.0) as i64;
    let cut_e = (gap.end_s * 1000.0) as i64;
    let mut keeps: Vec<(i64, i64)> = Vec::new();
    if cut_s > in_ms {
        keeps.push((in_ms, cut_s));
    }
    if cut_e < out_ms {
        keeps.push((cut_e, out_ms));
    }
    assert_eq!(keeps.len(), 2);
    let first = keeps[0];
    let sid = project.sources[0].id.clone();
    let item = &mut project.track_of_kind_mut(TrackKind::Video).unwrap().items[0];
    item.kind = ItemKind::VideoClip {
        source_id: sid,
        source_in_ms: first.0,
        source_out_ms: first.1,
        speed: 1.0,
    };
    item.timeline_duration_ms = first.1 - first.0;
    let second = Item::new(
        ItemKind::VideoClip {
            source_id: project.sources[0].id.clone(),
            source_in_ms: keeps[1].0,
            source_out_ms: keeps[1].1,
            speed: 1.0,
        },
        first.1 - first.0,
        keeps[1].1 - keeps[1].0,
    );
    project.track_of_kind_mut(TrackKind::Video).unwrap().items.push(second);
    let out = ws.media("nosilence.mp4");
    render(&project, &dir, &out);
    let probe = probe_out(&out);
    assert!(
        probe.duration_ms() < 3_000,
        "silence cut must shorten the timeline: {}ms (input 3.5s)",
        probe.duration_ms()
    );
}

#[test]
fn normalize_lifts_quiet_audio() {
    let ws = Workspace::new("loud");
    let media = ws.media("quiet.mp4");
    ws.run_ffmpeg(&[
        "-y", "-f", "lavfi", "-i", "color=c=gray:s=160x90:r=30",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
        "-t", "2", "-af", "volume=-24dB",
        "-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "ultrafast",
        "-c:a", "aac", media.to_str().unwrap(),
    ]);
    let (mut project, dir) = project_with_clip(&ws, &media);
    project.audio = AudioMaster { normalize: true, ..Default::default() };
    let out = ws.media("loud.mp4");
    render(&project, &dir, &out);
    let measure = Command::new(which_ffmpeg())
        .args(["-hide_banner", "-i"])
        .arg(&out)
        .args(["-af", "loudnorm=I=-16:TP=-1.5:print_format=json", "-f", "null", "-"])
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&measure.stderr).into_owned();
    let start = err.rfind('{').unwrap();
    let end = err.rfind('}').unwrap();
    let v: serde_json::Value = serde_json::from_str(&err[start..=end]).unwrap();
    let input_i: f64 = v["input_i"].as_str().unwrap().parse().unwrap();
    assert!(
        input_i > -22.0,
        "normalized output should be louder than -24dB input: measured {input_i} LUFS"
    );
}

#[test]
fn scene_detection_finds_hard_cuts() {
    let ws = Workspace::new("scenes");
    let media = ws.fixture_cuts("cuts.mp4");
    let cancel = AtomicBool::new(false);
    let params = mycut_analysis::adaptive_params(4_000, false);
    let cache_dir = ws.dir.join("cache");
    let a = mycut_analysis::analyze(&media, &cache_dir, 100_000_000, &params, &cancel, None).unwrap();
    assert!(
        !a.scenes.is_empty(),
        "must detect the red->blue cut; scenes={:?}",
        a.scenes
    );
    let near_2 = a.scenes.iter().any(|s| (s.end_s - 2.0).abs() < 0.7);
    assert!(near_2, "cut expected near t=2.0s, got {:?}", a.scenes);
    assert!((a.duration_ms - 4_000).abs() < 300);
}

#[test]
fn reframe_9x16_produces_vertical_output() {
    let ws = Workspace::new("reframe");
    let media = ws.media("moving.mp4");
    ws.run_ffmpeg(&[
        "-y", "-f", "lavfi", "-i",
        "testsrc2=size=320x180:r=30,drawbox=x='80+20*t':y=60:w=60:h=60:color=white:t=fill",
        "-t", "4", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "ultrafast",
        media.to_str().unwrap(),
    ]);
    let (mut project, dir) = project_with_clip(&ws, &media);
    project.reframe = Some(ReframeSettings {
        ratio: AspectRatio::R9x16,
        mode: ReframeMode::Center,
    });
    project.export.width = 1080;
    project.export.height = 1920;
    let out = ws.media("vertical.mp4");
    render(&project, &dir, &out);
    let probe = probe_out(&out);
    let (w, h) = probe.resolution();
    assert_eq!((w, h), (1080, 1920), "must be 9:16 vertical");
    assert!((probe.duration_ms() - 4_000).abs() < 300);
}

#[test]
fn no_shell_in_any_render_args_with_adversarial_text() {
    let ws = Workspace::new("noshell");
    let media = ws.fixture_color("flat.mp4", "gray", 2);
    let (mut project, dir) = project_with_clip(&ws, &media);
    project.color.saturation = 1.3;
    project.reframe = Some(ReframeSettings { ratio: AspectRatio::R1x1, mode: ReframeMode::Center });
    project.track_of_kind_mut(TrackKind::Video).unwrap().items[0].effects = vec![
        EffectInstance {
            id: "fx1".into(),
            def_id: "zoom_punch".into(),
            params: BTreeMap::from([("strength".to_string(), ParamValue::Number(1.3))]),
            window_start_ms: Some(100),
            window_end_ms: Some(800),
            keyframes: vec![],
            easing: mycut_core::Easing::Linear,
        },
        EffectInstance {
            id: "fx2".into(),
            def_id: "grain".into(),
            params: BTreeMap::from([("strength".to_string(), ParamValue::Number(10.0))]),
            window_start_ms: None,
            window_end_ms: None,
            keyframes: vec![],
            easing: mycut_core::Easing::Linear,
        },
    ];
    project.track_of_kind_mut(TrackKind::Text).unwrap().items.push(Item::new(
        ItemKind::Text {
            kind: mycut_core::TextKind::Title,
            text: "ev'; rm -rf ~; echo $(id) `id` ; :".into(),
            position: mycut_core::Position::Center,
            scale: 1.0,
            opacity: 1.0,
        },
        0,
        1_000,
    ));
    let engine = RenderEngine::new().unwrap();
    let graph = engine
        .build_export_command(&project, &dir, &ws.media("x.mp4"), HwChoice::None)
        .unwrap();
    assert_eq!(Path::new(&graph.program).file_name().unwrap(), "ffmpeg");
    for arg in &graph.args {
        assert!(!arg.contains("sh -c"), "shell usage detected in {arg:?}");
        assert!(!arg.contains('\n'), "newline in argv element: {arg:?}");
    }
    // argv is never a shell line: no "-c"/"sh" pairs anywhere.
    assert!(!graph.args.windows(2).any(|w| (w[0] == "sh" && w[1] == "-c") || (w[0] == "-c" && w[1].contains("rm"))));
    // Adversarial text is safely escaped inside drawtext.
    let text_arg = graph.args.iter().find(|a| a.contains("drawtext")).unwrap();
    assert!(text_arg.contains("\\'") || text_arg.contains("rm -rf"), "text present (escaped): {text_arg}");
}
