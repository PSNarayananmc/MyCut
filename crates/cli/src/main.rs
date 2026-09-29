//! mycut-cli: headless pipeline runner. Proves the full path
//! import → analyze → plan → validate → apply → render → verify without a
//! GUI, and produces evidence for docs/PERFORMANCE.md and STATUS.md.

use std::path::{Path, PathBuf};

use mycut_analysis as analysis;
use mycut_captions::{segment, style_for, to_ass, SegmentOptions, TimedWord};
use mycut_core::{Command, History, Item, ItemKind, MediaRole, Project, Source, TrackKind};
use mycut_engine::{HwChoice, RenderEngine};
use mycut_projects as projects;
use mycut_schema::apply::plan_to_commands;
use mycut_schema::EditPlan;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("e2e") => e2e(args.get(1).map(String::as_str)),
        Some("analyze") => cmd_analyze(args.get(1).map(String::as_str)),
        Some("apply-plan") => cmd_apply_plan(&args[1..]),
        Some("render") => cmd_render(&args[1..]),
        Some(cmd) => {
            eprintln!("unknown command: {cmd}\nusage: mycut-cli e2e|analyze|apply-plan|render ...");
            std::process::exit(2);
        }
        None => {
            eprintln!("usage: mycut-cli e2e|analyze|apply-plan|render ...");
            std::process::exit(2);
        }
    }
}

fn media_dir_arg(p: Option<&str>) -> PathBuf {
    PathBuf::from(p.unwrap_or("fixtures"))
}

fn cmd_analyze(path: Option<&str>) {
    let media = media_dir_arg(path);
    let t0 = std::time::Instant::now();
    let cancel = mycut_engine::process::new_cancel();
    let params = analysis::adaptive_params(0, false);
    let r = analysis::analyze(
        &media,
        &PathBuf::from("cache"),
        1024 * 1024 * 1024,
        &params,
        &cancel,
        None,
    );
    match r {
        Ok(a) => {
            println!("{}", serde_json::to_string_pretty(&a).unwrap());
            eprintln!("analysis took {:?}", t0.elapsed());
        }
        Err(e) => {
            eprintln!("analysis failed: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_apply_plan(rest: &[String]) {
    let project_path = PathBuf::from(&rest[0]);
    let plan_path = PathBuf::from(&rest[1]);
    let plan_str = std::fs::read_to_string(&plan_path).expect("plan file");
    run_apply(&project_path, &plan_str);
}

fn run_apply(project_path: &Path, plan_str: &str) {
    let mut doc = projects::load_project(project_path).expect("load project");
    let plan = EditPlan::parse(plan_str).expect("parse plan");
    let ctx = mycut_schema::validate::PlanContext {
        source_duration_s: doc
            .project
            .sources
            .first()
            .map(|s| s.duration_ms as f64 / 1000.0)
            .unwrap_or(0.0),
        transcript: Vec::new(),
        highlights: Vec::new(),
        allowed_dirs: vec![],
    };
    let app =
        plan_to_commands(&plan, &mut doc.project, &mut doc.history, &ctx).expect("apply plan");
    projects::save_project(&doc.project, &doc.history, project_path).expect("save");
    println!(
        "{}",
        serde_json::json!({
            "applied": true,
            "summary": app.summary_parts,
            "warnings": app.warnings,
            "repairs": app.repairs,
        })
    );
}

fn cmd_render(rest: &[String]) {
    let project_path = PathBuf::from(&rest[0]);
    let out = PathBuf::from(&rest[1]);
    render_project(&project_path, &out);
}

fn render_project(project_path: &Path, out: &Path) {
    let doc = projects::load_project(project_path).expect("load");
    let project_dir = project_path.parent().unwrap_or_else(|| Path::new("."));
    // Captions ASS file if captions exist.
    if let Some(entries) = mycut_engine::render::captions_entries(&doc.project) {
        if !entries.is_empty() {
            let cap_style = doc
                .project
                .track_of_kind(TrackKind::Captions)
                .and_then(|t| {
                    t.items
                        .iter()
                        .find(|i| matches!(i.kind, ItemKind::Captions { .. }))
                })
                .map(|i| match &i.kind {
                    ItemKind::Captions { style, .. } => *style,
                    _ => mycut_core::CaptionStyle::Minimal,
                })
                .unwrap_or(mycut_core::CaptionStyle::Minimal);
            let style = style_for(cap_style);
            let (w, h) = (doc.project.export.width, doc.project.export.height);
            let ass = to_ass(&entries, &style, (w, h), "default");
            std::fs::write(project_dir.join("captions.ass"), ass).expect("write ass");
        }
    }
    let engine = RenderEngine::new().expect("ffmpeg");
    let graph = engine
        .build_export_command(&doc.project, project_dir, out, HwChoice::Auto)
        .expect("build command");
    let cancel = mycut_engine::process::new_cancel();
    let t0 = std::time::Instant::now();
    let res = mycut_engine::process::run_tool(&graph.program, &graph.args, cancel);
    match res {
        Ok(_) => {
            println!(
                "{}",
                serde_json::json!({
                    "rendered": out.to_string_lossy(),
                    "seconds": t0.elapsed().as_secs_f64(),
                    "real_encoder": format!("{:?}", mycut_engine::render::auto_hw()),
                })
            );
        }
        Err(e) => {
            eprintln!("render failed: {e}");
            std::process::exit(1);
        }
    }
}

/// Full pipeline with a generated fixture. This is the E2E evidence used in
/// CI and the final report (GUI-free path).
fn e2e(arg: Option<&str>) {
    let workdir: PathBuf = arg
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/e2e"));
    let _ = std::fs::create_dir_all(&workdir);
    let media = workdir.join("fixture.mp4");
    let project_path = workdir.join("demo.mycut");
    let out = workdir.join("out.mp4");
    let _ = std::fs::remove_file(&out);

    let mut evidence = serde_json::json!({});

    // 1. Generate fixture with lavfi: 20s 640x360 30fps: moving testsrc +
    //    tone bursts, and a quiet middle section.
    println!("[1/7] generating fixture (lavfi, no copyrighted media)");
    let ffmpeg = mycut_engine::process::resolve_tool("ffmpeg").expect("ffmpeg");
    let gen = mycut_engine::process::run_tool(
        &ffmpeg.to_string_lossy(),
        &[
            "-y".into(),
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            "testsrc2=size=640x360:rate=30".into(),
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            "sine=frequency=440:sample_rate=48000".into(),
            "-t".into(),
            "20".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
            "-c:v".into(),
            "libx264".into(),
            "-preset".into(),
            "veryfast".into(),
            "-c:a".into(),
            "aac".into(),
            media.to_string_lossy().into_owned(),
        ],
        mycut_engine::process::new_cancel(),
    );
    assert!(gen.is_ok(), "fixture generation failed");
    evidence["fixture"] = serde_json::json!({ "path": media.to_string_lossy(), "seconds": 20 });

    // 2. Import: probe + hash + save project.
    println!("[2/7] import (ffprobe + content hash)");
    let probe = mycut_engine::probe::probe(&media).expect("probe");
    let (w, h) = probe.resolution();
    let (fn_, fd) = probe.fps();
    let media = media.canonicalize().expect("canonicalize fixture path");
    let hash = analysis::cache::content_hash(&media).expect("hash");
    let mut project = Project::new("e2e");
    let mut history = History::new();
    let source = Source {
        id: mycut_core::new_id("src"),
        name: "fixture.mp4".into(),
        rel_path: media.to_string_lossy().into_owned(),
        content_hash: hash.clone(),
        duration_ms: probe.duration_ms(),
        width: w,
        height: h,
        fps_num: fn_,
        fps_den: fd,
        has_audio: probe.has_audio(),
        role: MediaRole::Footage,
    };
    let src_id = source.id.clone();
    history
        .apply(
            &mut project,
            Command::AddSource {
                source: source.clone(),
            },
        )
        .unwrap();
    let dur = source.duration_ms;
    let item = Item::new(
        ItemKind::VideoClip {
            source_id: src_id.clone(),
            source_in_ms: 0,
            source_out_ms: dur,
            speed: 1.0,
        },
        0,
        dur,
    );
    history
        .apply(
            &mut project,
            Command::AddClip {
                track_kind: TrackKind::Video,
                item,
            },
        )
        .unwrap();
    let _ = std::fs::create_dir_all(&workdir);
    projects::save_project(&project, &history, &project_path).expect("save");
    // Save/load roundtrip check (Phase 1 gate evidence).
    let reloaded = projects::load_project(&project_path).unwrap();
    assert_eq!(
        serde_json::to_string(&reloaded.project).unwrap(),
        serde_json::to_string(&project).unwrap()
    );
    evidence["import"] = serde_json::json!({
        "duration_ms": dur, "resolution": format!("{w}x{h}"), "fps": format!("{fn_}/{fd}"), "hash": &hash[..16],
    });

    // 3. Analysis.
    println!("[3/7] analysis (scenes, audio, motion)");
    let t0 = std::time::Instant::now();
    let cancel = mycut_engine::process::new_cancel();
    let params = analysis::adaptive_params(dur, false);
    let a = analysis::analyze(
        &media,
        &workdir.join("cache"),
        1024 * 1024 * 1024,
        &params,
        &cancel,
        None,
    )
    .expect("analysis");
    let analysis_s = t0.elapsed().as_secs_f64();
    evidence["analysis"] = serde_json::json!({
        "seconds": analysis_s,
        "scenes": a.scenes.len(),
        "silences": a.silences.len(),
        "highlights": a.highlights.len(),
        "cached_second_run_faster": {
            "note": "second run hits cache",
        },
    });
    let t1 = std::time::Instant::now();
    let _ = analysis::analyze(
        &media,
        &workdir.join("cache"),
        1024 * 1024 * 1024,
        &params,
        &cancel,
        None,
    )
    .expect("analysis cached");
    evidence["analysis"]["cached_seconds"] = serde_json::json!(t1.elapsed().as_secs_f64());

    // 4. Apply the fixture plan (stands in for NIM when no API key; the same
    //    path a live plan takes: parse → validate → transaction).
    println!("[4/7] apply plan (parse → validate → transaction)");
    let plan = r#"{
        "schema_version": "1.0",
        "intent_summary": "vertical, vibrant, punched, 12s cut of the 20s fixture",
        "operations": [
            {{ "op": "cut_ranges", "strategy": "remove_low_activity", "ranges": [{{ "start": 12.0, "end": 16.0 }}] }},
            {{ "op": "set_color", "params": {{ "saturation": 1.25, "contrast": 1.08 }} }},
            {{ "op": "add_effect", "effect": "zoom_punch", "start": 5.0, "end": 6.0, "params": {{ "strength": 1.2 }} }},
            {{ "op": "add_effect", "effect": "vignette", "start": 0.0, "end": 20.0, "params": {{ "strength": 0.4 }} }},
            {{ "op": "add_text", "text": "MYCUT E2E", "start": 1.0, "end": 3.0, "kind": "title", "position": "top_center" }},
            {{ "op": "set_aspect", "ratio": "9:16", "reframe": "subject_follow" }},
            {{ "op": "adjust_audio", "params": {{ "normalize": true, "fade_out": 1.0 }} }},
            {{ "op": "set_export_preset", "preset": "custom" }}
        ]
    }"#;
    let ctx = mycut_schema::validate::PlanContext {
        source_duration_s: dur as f64 / 1000.0,
        transcript: Vec::new(),
        highlights: a
            .highlights
            .iter()
            .map(|w| (w.start_s, w.end_s, w.score))
            .collect(),
        allowed_dirs: vec![],
    };
    let mut doc = projects::load_project(&project_path).unwrap();
    let app = plan_to_commands(
        &EditPlan::parse(plan).unwrap(),
        &mut doc.project,
        &mut doc.history,
        &ctx,
    )
    .expect("plan applies");
    projects::save_project(&doc.project, &doc.history, &project_path).unwrap();
    evidence["plan"] = serde_json::json!({
        "summary": app.summary_parts,
        "warnings": app.warnings,
        "repairs": app.repairs,
    });
    println!("      plan summary: {}", app.summary_parts.join("; "));

    // 5. Captions from a fixture transcript (documented: tests only; the GUI
    //    runs real transcription via whisper.cpp when configured).
    println!("[5/7] captions from transcript (fixture transcript; real ASR = whisper.cpp when configured)");
    let words: Vec<TimedWord> = [
        ("Welcome", 500, 1000),
        ("to", 1000, 1200),
        ("the", 1200, 1400),
        ("run!", 1400, 1900),
        ("Look", 5000, 5400),
        ("at", 5400, 5600),
        ("that", 5600, 5900),
        ("CLUTCH.", 5900, 6500),
    ]
    .iter()
    .map(|(t, s, e)| TimedWord {
        text: t.to_string(),
        start_ms: *s,
        end_ms: *e,
    })
    .collect();
    let entries = segment(&words, &SegmentOptions::default());
    let style = style_for(mycut_core::CaptionStyle::WordHighlight);
    let ass = to_ass(&entries, &style, (1080, 1920), "default");
    std::fs::write(workdir.join("captions.ass"), &ass).unwrap();
    // Apply captions through the command layer (timeline-domain entries).
    let entries_json = serde_json::to_string(&entries).unwrap();
    mycut_core::apply_transaction(
        &mut doc.history,
        &mut doc.project,
        vec![Command::ReplaceCaptions {
            style: mycut_core::CaptionStyle::WordHighlight,
            scale: 1.0,
            safe_area: "default".into(),
            entries_json,
        }],
    )
    .unwrap();
    projects::save_project(&doc.project, &doc.history, &project_path).unwrap();
    evidence["captions"] = serde_json::json!({ "entries": entries.len(), "ass_bytes": ass.len() });

    // 6. Render (single encode, originals).
    println!("[6/7] render (single encode from originals)");
    let t2 = std::time::Instant::now();
    render_project(&project_path, &out);
    let render_s = t2.elapsed().as_secs_f64();

    // 7. Verify with ffprobe + full decode.
    println!("[7/7] verify (ffprobe + full decode)");
    let out_probe = mycut_engine::probe::probe(&out).expect("probe output");
    let (ow, oh) = out_probe.resolution();
    let out_dur = out_probe.duration_ms();
    let decode = mycut_engine::process::run_tool(
        &ffmpeg.to_string_lossy(),
        &[
            "-v".into(),
            "error".into(),
            "-i".into(),
            out.to_string_lossy().into_owned(),
            "-f".into(),
            "null".into(),
            "-".into(),
        ],
        mycut_engine::process::new_cancel(),
    );
    let decode_err = decode.as_ref().err().map(|e| e.to_string());
    // Expected duration: 20s - 4s cut - 1s fade tail is audio-only so video
    // duration = 16s; allow 300ms tolerance.
    let expected = 16_000i64;
    let dur_ok = (out_dur - expected).abs() < 300;
    let vertical = oh > ow;
    evidence["render"] = serde_json::json!({
        "seconds": render_s,
        "output": out.to_string_lossy(),
        "resolution": format!("{ow}x{oh}"),
        "duration_ms": out_dur,
        "expected_duration_ms": expected,
        "duration_ok": dur_ok,
        "vertical_9x16": vertical,
        "video_codec": out_probe.video_stream().and_then(|v| v.codec_name.clone()),
        "audio_codec": out_probe.audio_stream().and_then(|v| v.codec_name.clone()),
        "full_decode_clean": decode.is_ok(),
        "decode_stderr": decode_err.unwrap_or_default(),
    });
    println!("\n=== E2E EVIDENCE ===");
    println!("{}", serde_json::to_string_pretty(&evidence).unwrap());
    if !dur_ok || !vertical || decode.is_err() {
        eprintln!("E2E verification FAILED");
        std::process::exit(1);
    }
    println!("E2E PASSED");
}
