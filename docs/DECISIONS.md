# MyCut — Decision Log

Each entry: the decision, why, and rejected alternatives. Deviations from the
brief's default table are called out explicitly.

## D1 — Time representation: `i64` milliseconds

**Decision.** The canonical model stores time as `i64` ms. `source_in_ms`,
`source_out_ms` are source-domain; `timeline_start_ms` is timeline-domain.
The AI plan schema uses f64 seconds at the boundary, converted+validated once.

**Why.** Integer-exact serialization/diff/undo; no float accumulation drift;
1 ms granularity is finer than a frame at 240 fps (≈4.17 ms). Rational frame
math (1/fps) is confined to the engine, which converts ms→frame counts using
the source's rational fps when emitting FFmpeg expressions.

**Rejected.** Rational `num_rational` everywhere (heavy, leaks into JSON);
f64 seconds everywhere (drift, ugly diffs); frames-only (breaks for mixed-fps
multitrack).

## D2 — Operation-based plan, not state dump

**Decision.** `EditPlan` is a list of operations with `mode` and optional
`target_id`.

**Why.** Conversation continuity: "make captions smaller" must not re-send the
whole timeline; ops diff cleanly into the AI-change report; smaller prompts.

**Rejected.** Full-state JSON (bigger prompts, harder diffs, encourages the
model to invent unrelated state).

## D3 — Blocking HTTP (`ureq`) instead of async `reqwest`/tokio in core

**Decision.** `mycut-ai` uses `ureq` with rustls, called from worker threads.

**Why.** The core crates must stay light and build fast on low-end machines.
The job scheduler already runs pools of OS threads; a blocking client is
simple to cancel (drop the worker), simple to bound, and avoids dragging a
tokio runtime into every crate. The Tauri shell owns its own async runtime
for IPC; that is independent of core.

**Rejected.** reqwest+tokio (≈10× dependency tree for no functional gain
here), curl subprocess (arg-array rule is for media tools; still no benefit).

**Deviation note.** The brief's stack table didn't pin the HTTP client; this
is an implementation choice within it, recorded here for auditability.

## D4 — OS keyring behind a cargo feature; file fallback always available

**Decision.** `SecretStore` trait. `FileSecretStore` (0600, user config dir)
always compiles. `KeyringSecretStore` (Secret Service via the `keyring`
crate) is enabled by the `os-keyring` feature, which the Tauri app crate
enables in release packaging.

**Why.** Keeps `cargo build` for core/test images free of D-Bus/zbus build
cost and failure modes, while shipping keyring storage in real packages.
Both stores zero the key from memory on drop as far as Rust allows and the
key is canary-tested to never appear in logs/IPC/errors.

**Rejected.** Env-var-only (spec requires UI override + persistence);
bundling keys in config (security).

## D5 — FFmpeg as child processes with argument arrays only

**Decision.** All media operations spawn `ffmpeg`/`ffprobe` via
`std::process::Command` with `Vec<OsString>`; no `shell=True` equivalent
exists in the codebase (asserted by a grep-based security test).

**Why.** Injection-proof by construction; matches the non-negotiables.

**Rejected.** libavfilter C bindings (unsafe FFI + license tangle),
GStreamer pipelines (heavy deps on low-end distros).

## D6 — Preview via proxy files + on-demand rendered ranges; no compositor

**Decision.** The webview `<video>` plays low-res proxies. Effects/captions/
color previews render short ranges on demand into proxy-resolution files.

**Why.** A real-time compositor needs GPU pipelines and years of tuning; the
target hardware (i3-7xxx iGPU) can't do it reliably. On-demand range renders
are deterministic and testable, and re-use the exact final filter code path —
so "what you preview is what you render" modulo resolution.

**Rejected.** WebCodecs/WebGL compositor in JS (complex, frame data in
webview — violates memory discipline); frame-server pipe into webview
(fragile across browsers).

## D7 — Canonical single pipeline order

**Decision.** `decode → cut/speed → reframe/scale → basic correction →
LUT/creative → effects → text/overlays/captions → encode`, built as ONE
filtergraph, ONE encode.

**Why.** Prevents double colorspace conversions and cumulative quality loss;
makes the "single final encode" principle mechanically true.

## D8 — Constrained-deps workspace layout (adapted from the brief's layout)

**Decision.** Crates: `core, projects, schema, engine, analysis, captions, ai,
cli` + `src-tauri` app. Effect/audio/color registries live in `engine`
instead of three separate crates.

**Why.** Same module boundaries, fewer crates to compile on a 2-core box;
the registries are table-driven data and belong beside the filter builders
that interpret them. `cli` exists so the whole pipeline is verifiable
headlessly (E2E evidence) without a GUI session.

## D9 — NIM integration details

**Decision.** OpenAI-compatible `chat/completions` against a configurable
base URL (default in config data, not code). Structured output via
`response_format: json_schema` when capability-reported; else strict
JSON-only prompting with validation + repair loop (≤2 retries). 429/5xx →
backoff with jitter, honoring `Retry-After`. Live tests gated on
`NVIDIA_NIM_API_KEY`; CI/dev uses a local stub server (tests only).

**Why.** Matches the provider's documented interface while keeping the
`AIProvider` trait the only seam — a second provider is one file.

## D10 — No bundled models, fonts, or LUT packs

**Decision.** Optional downloads with size+license shown. Ship at most a few
project-generated LUTs and no third-party fonts.

**Why.** License hygiene and install size on low-end machines.

## D11 — Whisper.cpp as the default transcriber (optional download)

**Decision.** Modular `Transcriber` trait; default local provider shells out
to a user-installed/downloaded whisper.cpp `main` binary with
`--output-json-full` for word timestamps; remote providers exist but are off.

**Why.** NIM chat models are not assumed to do ASR; word-level timestamps are
required for karaoke captions; MIT-licensed, CPU-friendly on old hardware.

## D12 — History persistence across restarts

**Decision.** Serialize a bounded tail of the undo history with the project.

**Why.** Spec requires restart-surviving undo; commands are data, so this is
free. Bounded (last 200) to keep files small.

## D13 — Test strategy: fixtures from lavfi; stub server for NIM

**Decision.** All media fixtures are generated by FFmpeg `lavfi` (color bars,
sine/silence, hard cuts, moving box). NIM tests run against a hand-rolled
std-lib stub HTTP server covering 200/401/429/500/malformed/timeout. Live
NIM tests are `#[ignore]`-gated on `NVIDIA_NIM_API_KEY`.

**Why.** No copyrighted media; deterministic timestamps; security tests can
assert adversarial plans are rejected without any network.

## D14 — GUI build separation

**Decision.** `default-members` excludes `src-tauri`; CI on headless images
builds/tests workspace + UI; packaging machines build the app crate.

**Why.** webkit2gtk is unavailable in headless sandboxes; excluding it keeps
every core gate green and honest, and the split is documented.

## D15 — Ubuntu 20.04 (GLIBC 2.31) compatibility: local web runtime, Tauri GUI preserved for newer distros

**Decision.** The Ubuntu 20.04 packages (.deb, .AppImage) ship the **MyCut
Local Web Runtime**: a small loopback HTTP server (`mycut`, crate
`crates/server`) that serves the exact same React UI and exposes the exact
same typed command surface as the Tauri shell (`project_snapshot`,
`import_media`, `ai_apply_request`, … 16 commands + a `doctor` diagnostic).
The UI's typed bridge (`ui/src/lib/bridge.ts`) gains an HTTP transport for
this mode; the Tauri IPC transport is untouched. Tauri 2 (`src-tauri/`)
remains in-tree, unmodified in behavior, as the desktop build for distros
that ship WebKitGTK 4.1 (22.04+). Native Rust binaries are built with the
GLIBC symbol ceiling pinned at **2.31** (`cargo-zigbuild --target
x86_64-unknown-linux-gnu.2.31`), and the build gate
(`build/check-glibc.sh`) hard-fails on any `GLIBC_2.32+` requirement.

**Why.** Tauri 2 on Linux hard-depends on `webkit2gtk-4.1` (libsoup3). No
Ubuntu 20.04 package provides that API (20.04 ships WebKitGTK 2.28 /
`libwebkit2gtk-4.0-37` / libsoup2 only). Every distro that *does* ship 4.1
requires GLIBC ≥ 2.34, so bundling a WebKitGTK 4.1 stack would either break
the GLIBC 2.31 cap or require shipping a replacement glibc — explicitly
forbidden. Porting to Tauri 1.x would mean a second, EOL shell plus an
Ubuntu-20.04-only link environment while still fragmenting the codebase.
The local web runtime keeps **100% of the core** (engine, analysis, NIM
planning, captions, projects, undo), reuses the same UI code and the same
command implementations, adds zero WebKitGTK dependency, uses less RAM on
the 8 GB target machine (no embedded WebKit process), and works with any
browser the user already has. This is the "alternative Linux frontend
runtime strategy + separate 20.04 target preserving the modern build"
option, chosen after the others were ruled out on technical grounds.

**Rejected.**
- *Bundle WebKitGTK 4.1*: no GLIBC-2.31-compatible build exists; building
  WebKitGTK against glibc 2.31 from source is days of fragile compilation.
- *Port to Tauri 1.x*: EOL framework, duplicated shell + config, requires
  a 20.04 link environment for the 4.0 API; still a second codebase to
  maintain forever.
- *Fake it*: shipping `libwebkit2gtk-4.1-0` as a deb dependency that cannot
  be installed on 20.04 — dependency metadata lying, forbidden.

**Consequences.** In server mode there is no native file dialog (browsers
cannot hand the server absolute paths): `import_media` takes explicit
paths — the launcher accepts `--import PATH…`, and the Import button in
server mode asks for a path (honest UI, documented). Preview/export
download links go through the jailed `/media` endpoint. CI builds the
20.04 target inside an `ubuntu:20.04` container and runs the GLIBC gate;
the Tauri GUI is built separately on newer distros.

## D16 — FFmpeg delivery: bundled pinned static build, system FFmpeg override

**Decision.** Both Linux packages bundle a pinned, checksum-verified static
FFmpeg/ffprobe build (GPL build with libx264 + libass; sources linked from
THIRD_PARTY_LICENSES.md) under `/opt/mycut/bin` (.deb) or the AppImage's
`usr/bin`. The launcher points `MYCUT_FFMPEG` / `MYCUT_FFPROBE` (engine
sidecar overrides, already supported) at the bundled copies. Users can set
`MYCUT_USE_SYSTEM_FFMPEG=1` to prefer a system install instead. The
runtime `doctor` diagnostic reports the tri-state: **missing / too old /
ok**, with the minimum required version (4.3, for `xfade`) and an
actionable message in every failure path.

**Why.** Ubuntu 20.04's system FFmpeg is 4.2 — missing `xfade` (4.3+),
which the transitions feature requires; declaring `Depends: ffmpeg` alone
would produce a package that installs but cannot run transitions on 20.04.
Bundling guarantees feature parity on every supported distro and is fully
reproducible (pinned version + SHA256 in `build/fetch-ffmpeg.sh`).

**Rejected.** System-FFmpeg-only (silent feature gap on 20.04); compiling
FFmpeg from source at build time (slow, non-reproducible); shared-library
extraction of the 20.04 .deb (drags ~40 transitive libs into the bundle).
