# MyCut

**MyCut** is an MIT-licensed, chat-driven desktop video editor for Linux,
built for ordinary (low-end) hardware. Import footage, type a request —
"Make this a 30-second gaming Short with captions, vibrant colors, subtle
zooms on important moments, vertical" — and get a genuinely edited MP4.

An LLM (NVIDIA NIM) **plans**; a deterministic local engine **executes**.
The LLM never touches the shell, the filesystem, or FFmpeg.

```
Prompt → Local analysis (cached) → Context builder → LLM planner (NIM)
      → Structured edit plan → Validator/repair → Project model (undoable)
      → Preview (proxy) → Final render (originals, single encode)
```

## Principles

- **Non-destructive**: projects store instructions, not pixels.
- **Planner/executor separation**: the LLM emits versioned JSON; only local
  Rust builds FFmpeg argument arrays.
- **Safe by construction**: no shell anywhere (audit-tested); path jails;
  allowlisted filters/codecs/containers; adversarial plans rejected.
- **Private by default**: no telemetry; frames only leave the machine if you
  explicitly enable it, visibly indicated.
- **Offline-capable**: manual editing, analysis, preview, export need no
  network.
- **Low-end first**: memory bounded (~430 MB peak RSS measured), single
  final encode, throwaway proxies, GPU never required.

## Status

See [STATUS.md](STATUS.md) for the honest, evidence-backed feature matrix,
and [docs/PERFORMANCE.md](docs/PERFORMANCE.md) for measured numbers.
This is an early project: some phases are partial — STATUS.md says exactly
which, with the test that proves each completed item.

## Build (developers)

Requirements: Rust (stable), Node ≥ 20, FFmpeg ≥ 5 with libx264 and libass.

```
cargo test --workspace      # 106 tests incl. real render verification
cargo run -p mycut-cli -- e2e target/e2e   # headless pipeline + evidence
npm --prefix ui install && npm --prefix ui run build   # UI bundle
```

The GUI (`src-tauri`) requires a desktop Linux with `libwebkit2gtk-4.1-dev`:

```
cargo build -p mycut-app
```

## NVIDIA NIM setup

1. Get a key from NVIDIA (build.nvidia.com), starts with `nvapi-`.
2. In MyCut: Settings → paste key → **Test Connection**.
3. Optional: change model / base URL (defaults in `settings.json`).
4. Live tests in CI: `NVIDIA_NIM_API_KEY=... cargo test -p mycut-ai -- --ignored`.

Without a key the app stays fully usable for manual editing; the chat panel
says exactly why AI is off.

## Install (users — when packaging machines produce artifacts)

- AppImage (primary): download, `chmod +x`, run. FFmpeg bundled where
  licensing permits; otherwise the app detects a system FFmpeg and offers an
  installation helper.
- `.deb` (Ubuntu 22.04/24.04, Debian 12+): declares `ffmpeg` dependency.

## Layout

```
crates/ core · projects · schema · engine · analysis · captions · ai · cli
src-tauri/   Tauri 2 shell (commands, settings, secrets)
ui/          React + TypeScript frontend
docs/        ARCHITECTURE · DECISIONS · PERFORMANCE · schema/ · guides
scripts/     bench.sh · build-appimage.sh · build-deb.sh
```

## License

MIT — see [LICENSE](LICENSE). Third-party licenses:
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
