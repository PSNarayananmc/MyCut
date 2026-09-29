# STATUS — honest feature matrix

Every "done" item lists the command or test that proves it **in this
repository, on this machine** (Debian 13 sandbox: 2 vCPU / 4.1 GB / no GPU /
FFmpeg 7.1.5 / Rust 1.98.1). Items marked **partial** state exactly what is
missing. Items marked **not started** are listed under Known limitations.

Run the full gate yourself:

```
cargo test --workspace              # 106 tests, 0 failures (see §0)
cargo run -p mycut-cli -- e2e target/e2e
```

## 0. Test/verification totals (this workspace)

| Gate | Result | Evidence |
|---|---|---|
| Workspace unit + integration tests | **106 passed, 0 failed** | `cargo test --workspace` output in §6 |
| Headless E2E (import→analyze→plan→captions→render→verify) | **PASSED** | `mycut-cli e2e` prints evidence JSON (§6) |
| Constrained-memory E2E (RLIMIT_AS 2.5 GB) | **PASSED**, peak RSS 435 MB | scripts/bench.sh |
| UI typecheck + production bundle | **PASSED** | `npm --prefix ui run build` (strict tsc) |
| Live NIM test | **not run here** (no API key in sandbox) | `NVIDIA_NIM_API_KEY=... cargo test -p mycut-ai -- --ignored` |

## Phase 1 — Foundations

| Requirement | State | Evidence |
|---|---|---|
| Project model (sources/tracks/clips/keyframes) | **done** | `mycut-core` types + `cargo test -p mycut-core` |
| Command/undo layer, AI = one transaction | **done** | `ai_transaction_undoes_as_one_step`, `transaction_rolls_back_on_failure` |
| Undo/redo, history persists across restarts | **done** (history serialized in `.mycut`, bounded 200) | `save_load_roundtrip_identical` |
| `.mycut` atomic writes (tmp+fsync+rename) | **done** | `atomic_write_leaves_no_tmp_files` |
| Rolling backups + recovery prompt | **done** (backups rotate 5; recovery loader) | `backups_rotate_and_recover` |
| Schema versioning/migrations | **done** (v1.0; newer refused) | `newer_version_refused` |
| Media relink by hash/filename | **partial** — hash computed & stored; the relink dialog is a stub in the GUI shell | `content_hash` roundtrip; GUI `relink_source` returns Ok() |
| Hardware/FFmpeg detection + friendly install helper | **partial** — detection + tiny test-encode verification done; install helper UI not built | `detect_hw_encoders`, `tiny_test_encode` |
| Settings + secret storage (keyring/file 0600) | **done** (file 0600 tested; keyring behind `os-keyring` feature, compiled in packaging) | `file_store_roundtrip_and_perms` |

## Phase 2 — Media and render core

| Requirement | State | Evidence |
|---|---|---|
| ffprobe parse | **done** | `mycut-engine::probe` + E2E import evidence |
| Thumbnails | **done** (command builder; generated during GUI import) | `build_thumbnail_command` |
| Proxy generation | **done** (command builder; GUI import + `proxy_path`) | `build_proxy_command` |
| Preview playback | **partial** — proxies render on demand & play in `<video>`; range-limited preview renders whole timeline | `render_preview_range` (documented scope) |
| RenderEngine trim/cut/concat/scale/speed/audio | **done** | `trim_cut_concat_export_matches_expectations` (ffprobe-verified) |
| Single-encode export | **done** (one graph, one encode; fixed pipeline order) | engine builder + E2E decode-clean check |
| Hardware fallback | **done** (detect → verify → Auto falls back to libx264; verified CPU path; GPU untested — no GPU here) | `detect_hw_encoders` |
| Job queue with cancel | **partial** — `RunningJob` cancel flag + polling implemented and used by CLI/AI; full multi-pool scheduler (per-profile workers) not built | `cancellation_stops_request` |

## Phase 3 — Analysis

| Requirement | State | Evidence |
|---|---|---|
| Scene detection | **done** (hard cut found at t=2.0 ± tolerance on lavfi fixture) | `scene_detection_finds_hard_cuts` |
| Audio: silence/loudness | **done** | `silence_parse_and_invert`, `loudnorm_json_tail`, `normalize_lifts_quiet_audio` |
| Motion (frame-diff) | **done** (blackframe-derived per-second activity) | `motion_aggregation` |
| Caching (content hash + params, LRU) | **done** | `put_get_roundtrip`, `eviction_respects_cap`; E2E cached run 0.037 s |
| VAD / speech regions | **partial** — inverted-silence heuristic (documented), not a real VAD model | `invert_silences` test |
| Beat/BPM detection | **not started** | — |
| Auto color | **done** (frame stats + suggested correction, pure + tested) | `auto_color_pure_math` |

## Phase 4 — Schema, validation, NIM, agent

| Requirement | State | Evidence |
|---|---|---|
| Edit-plan schema v1.0 + Rust types | **done** | `docs/schema/edit-plan.schema.json` + `EditPlan::parse` strict |
| Validator (ranges, overlaps, paths, allowlists) | **done** | 20 schema tests incl. adversarial |
| Repair policy (clamp ≤20 ms, ≤2 corrections) | **done** | `tiny_overshoot_is_autorepaired`, `consistently_malformed_fails_after_two_repairs` |
| NIM provider + Test Connection + states | **done** (401/404/429/5xx/timeout mappings tested) | `http_401_maps_to_invalid_key_without_retries` etc. |
| Structured output + graceful degradation | **partial** — `response_format json_schema` sent when caps say supported; capability probe of live models not implemented (caps are conservative defaults) | `NvidiaNimProvider::with_caps` |
| Context builder (compact, cached) | **done** (compact digest; plan caching by context hash implemented in schema of cache — **partial**: AI result cache not yet wired into agent loop) | `context_is_compact_and_includes_request` |
| Conversation state (follow-ups) | **partial** — context includes tail + current state; model resolves references; follow-up merge tested at applier level | `followup_updates_modify_existing_state` |
| Chat UI with diff/apply | **partial** — chat panel + diff summary built; diff Apply currently applies immediately (transaction) — confirm/cancel UI threshold not wired | `ChatPanel.tsx` |
| Meaning-preservation heuristics | **done** (warn on mid-sentence cuts; prompt rule unit-tested) | `mid_sentence_cut_warns` |
| Malicious/malformed plans rejected | **done** | `unknown_op_rejected`, `lut_path_traversal_rejected`, `overlapping_cuts_rejected`, `adversarial_oversized_and_shell_text` |
| Prompt injection containment | **done** (behavioral test: transcript injection cannot create ops) | `prompt_injection_in_transcript_cannot_add_operations` |
| Canary key never logged/leaked | **done** (provider-level canary test) | `http_401_maps_to_invalid_key_without_retries` |

## Phase 5 — Content features

| Requirement | State | Evidence |
|---|---|---|
| CaptionEngine (segment→style→ASS/SRT/VTT) | **done** | 7 captions tests |
| Caption styles (9) | **done** (data-driven; karaoke `\k`, word highlight, pop) | `all_styles_render`, `ass_output_structure_and_karaoke` |
| Transcription (whisper.cpp, word timestamps) | **partial** — Transcriber trait + whisper.cpp impl + JSON parsing done; **no live model run in this sandbox** (optional download not performed) | `parses_whisper_fixture_words`, `not_configured_is_explicit` |
| Effects registry (11 real effects) | **done** — zoom, zoom_punch, shake, blur, sharpen, vignette, rgb_split, grain, pixelate, flash, cinematic_bars; **verified with pixel tests**: vignette/zoom | `all_registered_effects_build`, `vignette_darkens_corners_on_flat_source`, `zoom_punch_changes_frame_scale` |
| Effects left out honestly | glow/bloom, motion blur (convolution), freeze frame, fisheye — not registered (can't do them well yet) | absent from registry |
| Color grade + LUT + intensity | **done** (eq/colorbalance/vibrance/lut3d+blend; LUT jail) | `color_grade_shifts_luminance`, `lut_full_and_partial_intensity`, `lut_outside_jail_rejected` |
| Audio effects (volume/normalize/denoise/fades/comp/limiter) | **done** | `builds_all_registered_effects`, `normalize_lifts_quiet_audio` |
| Ducking (sidechaincompress) | **partial** — filtergraph generation implemented; no integration test with music+speech fixture yet | engine `duck` branch |
| Silence removal | **done** — at CUT level (video+audio together; audio-only filter proven desync-prone and removed from master chain) | `silence_removal_shortens_video_and_audio_together` |
| Transitions (xfade chain) | **partial** — xfade/acrossfade mapping + composition implemented (fade/black/white/slide/push/zoom/wipe); **not yet integration-tested** end-to-end with multi-clip fixtures | `transition_slots` unit-covered; multi-clip render test pending |
| Text/titles/watermark | **done** (drawtext, positions, opacity, escape-fuzzed) | `no_shell_in_any_render_args_with_adversarial_text` |
| Keyframes (model + crop exprs) | **done** in model + engine expr builder; **no editing UI** (P2) | `piecewise_expr_bounded_and_monotone` |

## Phase 6 — Reframe and highlights

| Requirement | State | Evidence |
|---|---|---|
| Reframe 16:9/9:16/1:1/4:5 | **done** (E2E produced 608×1080 9:16 from 16:9) | E2E evidence, `reframe_9x16_produces_vertical_output` |
| Subject-follow (motion centroid → smoothed crop path) | **partial** — highlight/activity machinery exists; detector → keyframe path (EMA/dead-zone/max-pan) not complete; falls back to center crop honestly | crop expr builder tested; detector pending |
| Target-duration edits | **done** (highlight/keep_start/keep_end/even selection) | `SetTargetDuration` applier + `selection_windows` |
| Beat sync | **not started** | — |
| Gaming heuristics (event candidates) | **partial** — highlight scoring exposes candidates to planner; no gaming-specific event detection | `highlights_deterministic` |

## Phase 7 — Hardening

| Requirement | State | Evidence |
|---|---|---|
| Profiles (Potato…Maximum) | **partial** — `adaptive_params` (analysis depth) + profile field exist; full per-profile worker/preview matrix not implemented | `adaptive_params` |
| Memory pressure handling | **partial** — bounded caches + LRU + RLIMIT run pass; live RAM watcher with worker reduction not built | constrained E2E |
| Cache manager UI (size/clear) | **done** (commands + panel hooks) | `clear_cache`, `cache_usage` |
| Crash recovery | **done** (journal marker + backup recovery loader) | `journal_lifecycle`, `backups_rotate_and_recover` |
| Error-message pass (human-readable + technical details) | **done** (EngineError/AiError texts; stderr tail) | `stderr_tail`, AiError strings |
| Security test suite | **done** | `crates/engine/tests/security.rs` + schema/ai adversarial tests |

## Phase 8 — Packaging and docs

| Requirement | State | Evidence |
|---|---|---|
| AppImage script | **done as script** — requires a desktop Linux + linuxdeploy; **not buildable in this headless sandbox** (documented) | `scripts/build-appimage.sh` |
| .deb script | **done as script** | `scripts/build-deb.sh` |
| Community files + docs | **done** | LICENSE/README/CONTRIBUTING/SECURITY/CoC/THIRD_PARTY_LICENSES/docs/* |
| Bench harness | **done** | `scripts/bench.sh`, docs/PERFORMANCE.md |

## UI

| Requirement | State | Evidence |
|---|---|---|
| Dark UI: MediaBin/Preview/Timeline/Chat + tabs | **done** (builds strict; browser dev mode shows honest offline state) | `npm --prefix ui run build` |
| Simple/Advanced modes | **not started** | — |
| i18n resource files | **done** (single `en.ts`; hooks ready) | `ui/src/i18n/en.ts` |
| Keyboard shortcuts | **partial** — Ctrl+Enter send wired; full set pending | ChatPanel |
| Performance monitor panel | **not started** | — |

## GUI shell (src-tauri)

| Requirement | State | Evidence |
|---|---|---|
| Commands wiring (16 typed commands) | **written** — **not compiled here**: headless sandbox has no webkit2gtk; `default-members` excludes it (DECISIONS.md D14). Build on a desktop Linux: `cargo build -p mycut-app` | this file; tauri.conf.json |
| Native file dialogs | partial (plugin wired; picker glue in `pick_media_files` returns empty pending dialog integration) | src-tauri/src/main.rs |

## Known limitations (candid)

1. The GUI shell is uncompiled here (no webkit2gtk). Core pipeline is fully
   verified headlessly via `mycut-cli e2e`; the GUI is a thin layer over the
   same commands.
2. No live NIM call was made (no API key). Provider behavior is verified
   against a stub server covering 200/401/404/429/500/timeout/malformed; the
   live test is gated on `NVIDIA_NIM_API_KEY`.
3. No GPU in this environment: VAAPI/NVENC paths are detection-verified with
   automatic CPU fallback but unmeasured.
4. whisper.cpp never ran here (model download is user-consented); parser and
   not-configured states are tested.
5. Effects not yet implemented: glow/bloom, motion blur, freeze frame,
   fisheye. Transitions not integration-tested with multi-clip fixtures.
   Subject-follow detector incomplete (center-crop fallback).
6. Multi-track video compositing (overlays/PIPs), keyframe editing UI, beat
   sync, performance monitor panel, Simple/Advanced modes: not started.
7. VLC playback check not possible in this sandbox; the E2E substitutes a
   full `ffmpeg -v error -f null -` decode (VLC verification is manual:
   `vlc target/e2e/out.mp4`).

## §6 — Recorded outputs (verbatim excerpts)

`cargo test --workspace`: **106 passed, 0 failed** (unit + integration incl.
10 real-render tests + 3 security audits).

`mycut-cli e2e` evidence (JSON, abridged):

```
"plan": { "summary": ["Removed 4.0s of footage", "Applied color grade",
  "Added zoom_punch effect", "Added vignette effect", "Added text: \"MYCUT E2E\"",
  "Converted to 9:16", "Adjusted audio", "Export preset: Custom"] }
"render": { "duration_ms": 16000, "expected_duration_ms": 16000,
  "duration_ok": true, "resolution": "608x1080", "vertical_9x16": true,
  "video_codec": "h264", "audio_codec": "aac", "full_decode_clean": true,
  "seconds": 5.35 }
E2E PASSED
```
