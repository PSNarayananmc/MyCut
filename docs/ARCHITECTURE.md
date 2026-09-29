# MyCut Architecture

Status: living document. Schema version: 1.0. Last reviewed: 2026-09-29.

MyCut is an MIT-licensed, chat-driven desktop video editor for Linux. An LLM
plans edits; a deterministic local engine executes them. The LLM never touches
the shell, the filesystem, or FFmpeg. This document describes the components,
data flow, process model, schema versioning, and dependency justifications.

## 1. Design pillars

1. **Non-destructive.** Source media is opened read-only. Projects store
   *instructions* (a timeline model), never pixels. Renders write new files.
2. **Planner/executor separation.** The LLM emits a versioned `EditPlan` JSON
   document. Only the local Rust engine turns validated plans into FFmpeg
   argument arrays. There is no code path from chat text to a process spawn.
3. **Safe by construction.** FFmpeg is always spawned with an argument array
   (`Command::new(prog).args(...)`) — never a shell string. Filters, codecs,
   containers, and parameter ranges are allowlisted. Paths are canonicalized
   and jailed to the project directory plus explicit asset roots.
4. **Private by default.** No telemetry. No network call happens unless the
   user types a request or enables frame sharing; the UI shows a visible
   indicator when text/frames leave the machine.
5. **Offline-capable.** Manual editing, analysis, proxies, preview and export
   need no network. Only AI planning (and optional remote transcription) does.
6. **Graceful degradation.** GPU, extra RAM, vision models and remote
   transcription are accelerators, never requirements.
7. **Single final encode.** Nothing re-encodes intermediates. Preview proxies
   are throwaway low-resolution H.264 files; finals render once from originals.

## 2. Process model

```
┌───────────────────────────── mycut (Tauri 2 process) ─────────────────────────────┐
│                                                                                   │
│  Webview (React + TS)                    Rust core                                │
│  ┌───────────────────────┐   IPC (invoke/emit)   ┌──────────────────────────────┐ │
│  │ MediaBin Preview      │ ◄──────────────────► │ Command router (typed)       │ │
│  │ Timeline Chat Export  │                      │  ├─ project model + history   │ │
│  │ Settings Effects      │                      │  ├─ job scheduler (pools)     │ │
│  └───────────────────────┘                      │  ├─ analysis workers          │ │
│        No media bytes in JS.                    │  ├─ RenderEngine (FFmpeg)     │ │
│        <video> plays proxy files.               │  ├─ AI provider client        │ │
│                                                 │  └─ secret store              │ │
│                                                 └──────────┬───────────────────┘ │
└────────────────────────────────────────────────────────────┼─────────────────────┘
                                            child processes │ (argument arrays only)
                                              ffmpeg / ffprobe / (optional) whisper.cpp
```

- **One UI process, one core, N short-lived child processes.** FFmpeg/ffprobe
  run as children with explicit `-threads` limits. No daemon, no server.
- **Frame data never crosses the IPC boundary.** The webview receives file
  paths (proxy/preview/thumbnail) and scalar state; it plays them with the
  browser's own `<video>` element.
- **Job scheduler.** Separate bounded queues: `analysis`, `ai`, `thumbnails`,
  `proxies`, `transcription`, `render`. Every job is cancellable, runs on a
  worker thread from a profile-defined pool, and cleans its temp files on
  cancel/failure/next startup.

## 3. Components (Rust workspace)

| Crate            | Responsibility                                                                 |
|------------------|--------------------------------------------------------------------------------|
| `mycut-core`     | Canonical project model, time representation, command/undo layer, transactions. |
| `mycut-projects` | `.mycut` persistence: versioned JSON, atomic writes, rolling backups, recovery. |
| `mycut-schema`   | Edit-plan types (v1.0), the JSON Schema, the validator + repair policy.          |
| `mycut-engine`   | FFmpeg argument builders, effect/audio/color registries, transitions, presets, encoder detection. |
| `mycut-analysis` | ffprobe parsing, thumbnails, proxies, scene/audio/motion analysis, highlight scoring, content-hash cache. |
| `mycut-captions` | Transcription-to-captions: segmentation, styles, ASS/SRT/VTT emission.           |
| `mycut-ai`       | `AIProvider` trait, NVIDIA NIM client, context builder, system prompt, repair loop, conversation state. |
| `mycut-cli`      | Headless pipeline runner (import → analyze → apply plan → render → verify). Used for E2E tests. |
| `src-tauri`      | Tauri 2 app: typed commands, event bridge, secret storage, settings UI glue.     |

### 3.1 Canonical project model (`mycut-core`)

```
Project
 ├─ id, name, created_at, schema_version
 ├─ sources: Vec<Source>            // read-only media references
 │    └─ id, path (project-relative), content_hash, duration_ms, width, height,
 │       fps (rational), audio streams, role (footage|music|logo|voiceover|…)
 ├─ tracks: Vec<Track>              // ordered; kinds: video, audio, text, captions, overlay
 │    └─ id, kind, name, muted, solo, locked
 │        └─ items: Vec<Item>       // clip / text / caption / effect-slot
 │             ├─ source_id, source_in_ms, source_out_ms   // SOURCE time
 │             ├─ timeline_start_ms                        // TIMELINE time
 │             ├─ speed, volume, opacity
 │             ├─ effects: Vec<EffectInstance{def_id, params, keyframes, window}>
 │             └─ keyframes: KeyframeTrack<T> per animatable property
 └─ markers, export_settings, ai_plan_reports
```

**Time.** One canonical representation: signed 64-bit **milliseconds**
(`TimeMs`). Rationale: sufficient precision (1 ms ≪ 1 frame at 240 fps),
integer-exact for serialization/diffing, and immune to float drift when
accumulating trims. Rational frame math happens only inside the engine when
emitting FFmpeg expressions (engine converts `ms → fps-rational` frames).
Every field name states its domain: `source_in_ms`/`source_out_ms` are in
**source** time; anything named `timeline_*` is in **timeline** time. The
AI-facing plan schema uses **seconds (f64)** for ergonomics and is converted
and range-checked at the validation boundary.

**Commands and history.** Every mutation — manual click or AI plan — goes
through `Command` values with `apply()` and `invert()`. An AI plan is applied
as **one transaction**: N commands applied atomically; `Ctrl+Z` reverts the
whole plan. History is serialized with the project (bounded, oldest-trimmed)
so undo/redo survives restarts. The manual UI and the AI share this one
command layer; there is no side door.

### 3.2 Persistence (`mycut-projects`)

- `.mycut` = JSON: `{schema_version, project, history_tail, plan_reports}`.
- **Atomic write:** write `name.mycut.tmp` → `fsync` → `rename` → `fsync` dir.
- **Backups:** keep the last 5 saves as `.mycut.bak.N`; on load, if the main
  file is corrupt, offer the newest loadable backup ("Recovered unsaved
  project" flow when the temp journal shows a crash).
- **Migrations:** `v1 → vN` migrator chain; unknown newer version ⇒ refuse with
  a clear message (never guess).
- **Relink:** sources store `content_hash` (BLAKE2s-256 of first/last 1 MiB +
  size) and file name; a missing file is shown as *Media Offline* with a
  `[Relink]` action that matches by hash, then by filename.

### 3.3 Edit-plan schema (`mycut-schema`, `docs/schema/`)

Operation-based (not a full-state dump) so follow-ups like "make captions
smaller" modify existing state. Every op carries `mode` (`create|modify|
remove`) where relevant and optional `target_id`. `needs_clarification` may
replace operations when the request is genuinely ambiguous.

The **validator** is pure Rust and total: schema conformance, allowlisted
ops/effects/params with declared ranges, timestamps inside source duration,
`start < end`, illegal-overlap rejection, crop-inside-frame checks, speed
range checks, path jail checks for LUT/logo/font references, codec/container
allowlist. **Repair policy:** auto-repair only trivially safe issues (e.g.
clamp a timestamp ≤ 20 ms over duration) and record every repair in the plan
report; otherwise return errors to the model for at most **2 correction
attempts**, then show a human-readable error. Unvalidated plans are never
applied. **Meaning preservation:** the planner prompt forbids reordering or
cutting speech in a meaning-changing way; the validator warns when a cut
splits a transcript sentence.

### 3.4 AI layer (`mycut-ai`)

- `trait AIProvider { capabilities() -> Caps; plan(request) -> Result<PlanRaw> }`.
  Caps: `text | vision | structured_output | max_context`.
- `NvidiaNimProvider`: OpenAI-compatible `POST {base}/chat/completions`.
  Uses `response_format: json_schema` when the model supports it, otherwise
  "JSON-only" prompting + validation/repair. Retries: exponential backoff +
  jitter on 429/5xx, honoring `Retry-After`; cancellable; timeouts.
- **Context builder** (cost/latency control) sends: compact project summary,
  per-source duration/res/fps + scene list, audio summary (silence, peaks,
  speech regions), segment-level transcript, effect/style catalog with ranges,
  rolling conversation summary + last 3 turns, user request. Frames only when
  the model has vision **and** the user enabled sharing **and** the task needs
  them (downscaled JPEG, small count).
- **Caching:** plan results keyed by hash(model, prompt_version, context,
  request); unchanged analysis is never re-sent.
- **Fallback:** with no key/provider, the app stays fully usable manually and
  the chat panel states exactly why AI is off.
- **Secrets:** OS keyring (Secret Service) when available; otherwise a file
  with `0600` perms under the user's config dir. The key is only ever read by
  Rust; it never reaches JS, logs, or error messages (canary-tested).

### 3.5 Engine (`mycut-engine`)

- Typed argument builders: `probe`, `trim`, `cut`, `concat`, `scale`, `crop`,
  `speed`, `audio`, `color`, `overlay`, `text`, `subtitle`, `transition`,
  `effect`, `render`. Builders return `Vec<OsString>`; there is no `shell`.
- **Fixed filter order** (one graph, one encode):
  `decode → cut/speed → reframe/scale → basic correction → LUT/creative →
   effects → text/overlays/captions → encode`.
- **Effects are data + a filter-builder fn** (`EffectDefinition`), versioned in
  the registry with parameter types/ranges/defaults. Registry ships only
  effects expressible with real filters (crop/zoompan exprs, gblur/boxblur,
  noise, vignette, rgbashift, unsharp, drawbox, tpad, convolution, blend…).
  If an effect cannot be done well, it is left out — no fake previews.
- **Transitions** via `xfade`/`acrossfade` (cut, fade, crossfade, dip, slide,
  push, zoom, blur, wipe). Planner defaults to cuts/crossfades.
- **Reframing** 16:9 / 9:16 / 1:1 / 4:5 with subject-follow: detector (motion
  centroid by default; optional ONNX face/saliency) → EMA-smoothed crop path
  with dead-zone and max-pan-speed clamp → crop expression; falls back to
  center crop with a visible notice when detection is unavailable.
- **Hardware acceleration:** enumerate `ffmpeg -encoders/-hwaccels`, verify
  with a tiny test encode; user chooses Auto/NVENC/VAAPI/QSV/CPU; automatic
  fallback to `libx264`. GPU is never required.
- **Errors** are translated to human-readable messages with a collapsible
  *Technical Details* section (stderr tail).

### 3.6 Analysis (`mycut-analysis`)

ffprobe metadata → thumbnails → proxy. Then adaptive-depth: scene score
(FFmpeg `select=gt(scene\,threshold)` on reduced fps/size), audio (RMS, EBU
R128 loudness via `loudnorm=print_format=json`, silencedetect, lightweight
VAD), frame-diff motion stats, auto-color estimate. All cached by
`(content_hash, params)` under the cache dir; LRU-evicted, size-capped.
Highlight scoring combines scene/motion/peaks/speech into ranked candidate
windows exposed to the planner as generic event candidates.

### 3.7 Captions (`mycut-captions`)

Independent of the renderer: `transcribe → segment → style → position →
animate → emit ASS (burn-in) and/or SRT/VTT`. Segmentation honors sentence
boundaries, punctuation, max words/chars per line, minimum display time.
Styles are data (Minimal, Gaming, TikTok, YouTube, Cinematic, Bold, Karaoke,
Word Highlight, Streamer) with font/size/colors/stroke/shadow/margins/
animation. Emphasis heuristics: numbers, ALL-CAPS, keyword list, loudness.

## 4. Data flow (the one true pipeline)

```
Import ─► ffprobe ─► content_hash ─► thumbnails+proxy (cached)
   │
   └─► Analysis (cached by hash): scenes · audio · motion · transcript · auto-color
   │
User request ─► Context builder ─► NIM (JSON-schema mode) ─► EditPlan JSON
   │                                                        │
   │                        Validator + repair loop ◄───────┘  (≤2 retries)
   │                              │ ok / repaired
   │                              ▼
   └─► Command layer (transaction) ─► Project model ─► diff report to UI
                                        │
              [Preview]  on-demand proxy-range render ─► <video>
              [Render Final]  validated model ─► ONE ffmpeg run on originals
                                        └─► ffprobe verify + full-decode check
```

## 5. Schema versioning

- `docs/schema/edit-plan.schema.json` (v1.0) is the contract for the LLM.
- Rust types in `mycut-schema` are generated-by-hand to mirror it and unit-
  tested for round-trip conformance against fixture plans.
- `Project` files carry `schema_version` and migrate forward via a chain;
  plans carry `schema_version` and are rejected if unknown.
- Additive changes bump the minor version; breaking changes bump major and
  require a migration + a validator update in the same PR.

## 6. Performance model (low-end first)

- Profiles in one table (`Potato | Low | Balanced | High | Maximum`) control:
  preview/proxy resolution, worker counts, analysis fps, concurrent jobs,
  AI frame count, RAM target. Default for 8 GB + iGPU: **Low**.
- Memory discipline: no whole-video buffers anywhere; streaming pipes/temp
  files; bounded caches; explicit `-threads`; children get `nice(5)`.
- Pressure handling: watch `MemAvailable`; when below profile threshold →
  reduce workers, drop preview caches, defer jobs. Never OOM-crash.
- Measured numbers live in `docs/PERFORMANCE.md` (no targets presented as
  results).

## 7. Dependency justifications (all audited in THIRD_PARTY_LICENSES.md)

| Dependency | Why | License | Rejected alternatives |
|---|---|---|---|
| `serde`/`serde_json` | Canonical serialization for model/plans/settings; de-facto standard | MIT/Apache-2.0 | manual JSON (error-prone), `nanoserde` (weaker tooling) |
| `thiserror` | Idiomatic error enums, zero runtime cost | MIT/Apache-2.0 | hand-written `Display` (noise) |
| `sha2` | Content hashing for cache keys/relink (stable across versions — std hasher is not) | MIT/Apache-2.0 | `blake3` (faster but larger tree; may revisit) |
| `ureq` + `rustls` | Tiny blocking HTTP client for NIM (worker threads, not async) | MIT/Apache-2.0 | `reqwest`+tokio (heavy, unneeded without a UI event loop in core) |
| `keyring` (feature `os-keyring`) | Secret Service integration | MIT/Apache-2.0 | hardcoding a file-only store (worse security) |
| FFmpeg/ffprobe child processes | The media engine | LGPL-2.1+ (build config dependent) | libav bindings (C FFI risk, license tangle); GStreamer (heavy) |
| Tauri 2 | Thin webview shell, no Electron, per-platform webview | MIT/Apache-2.0 | Electron (RAM), GTK-only UI (slow iteration) |
| React + TypeScript + Vite | Productive typed UI; huge ecosystem | MIT | Svelte/Solid (fine, but no functional gain here) |
| whisper.cpp (optional download) | Local word-timestamped ASR without Python | MIT | cloud-only ASR (privacy), huge ML frameworks |
| libass via FFmpeg `subtitles` filter | ASS karaoke/styling burn-in | ISCL (via ffmpeg build) | drawtext per word (brittle) |

Nothing else in the base install. Optional downloads (whisper models, ONNX
detectors, ffmpeg static build inside AppImage) show size + license before
download and are never bundled.

## 8. Known structural limitations

- Preview is **not** a real-time compositor: effect/caption previews render
  short on-demand clips at proxy resolution.
- NIM chat models are not assumed to do ASR; transcription is local/optional.
- GUI compile requires a desktop Linux with webkit2gtk; headless CI builds
  and tests the workspace and the UI bundle separately.
```
