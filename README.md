# MyCut

**MyCut** is an MIT-licensed, chat-driven video editor for Linux, built for
ordinary (low-end) hardware. Import footage, type a request — "Make this a
30-second gaming Short with captions, vibrant colors, subtle zooms on
important moments, vertical" — and get a genuinely edited MP4.

An LLM (NVIDIA NIM) **plans**; a deterministic local engine **executes**.
The LLM never touches the shell, the filesystem, or FFmpeg.

```
Prompt → Local analysis (cached) → Context builder → LLM planner (NIM)
      → Structured edit plan → Validator/repair → Project model (undoable)
      → Preview (proxy) → Final render (originals, single encode)
```

## Features

- **AI prompt-based editing** — natural-language edit requests become a
  validated, undoable plan (one AI plan = one transaction; Ctrl+Z undoes it
  as one step). Chat remembers the last exchanges, so follow-ups like
  "make it faster" work.
- **NVIDIA NIM integration with model discovery** — **Test Connection**
  verifies the key, the base URL and authentication, then lists the models
  your endpoint actually offers; the searchable **model selector** is fed by
  that real response (cached, refreshable, persisted across restarts), and
  **Test Model** proves the selected model answers. No hardcoded model list.
- **Real desktop workflow** — native OS **file picker** (multi-select) and
  **drag-and-drop** import (spaces/Unicode names fine), a media library with
  thumbnails/search/categories, a **multitrack timeline** (move, trim,
  split, delete, duplicate, snap, mute/lock, zoom), an inspector with live
  effect-parameter sliders, and full keyboard shortcuts (Space, S, Del,
  Ctrl+Z/Shift+Z, Ctrl+S, Ctrl+E).
- **Effects / text / captions / color panels** — every button maps to a real
  FFmpeg-backed operation from the registries: zoom/punch/shake/blur/sharpen/
  vignette/RGB-split/grain/pixelate/flash/bars, audio FX (normalize, denoise,
  fades, compressor, limiter), 9 caption styles, and project color grading.
- **Export presets with progress** — YouTube, YouTube Shorts, TikTok,
  Instagram Reels, Discord, X and Custom; live progress parsed from the
  encoder itself and a working **Cancel**.
- **Automatic cuts** — scene detection, silence removal, loudness (EBU R128),
  motion and highlight analysis, all cached by content hash.
- **Captions** — sentence-aware segmentation, multiple styles including
  karaoke/word-highlight, burned in via libass (ASS).
- **Transitions & effects** — xfade transitions (fade, dip, slide, push,
  zoom, wipe), all real FFmpeg filters (no fake previews).
- **Color correction** — exposure/contrast/saturation/vibrance/temperature/
  gamma plus creative LUTs.
- **FFmpeg rendering** — fixed pipeline order, single final encode from the
  original media (instructions stored, never pixels).
- **Plugin/effect architecture** — effects are data-driven registry entries;
  see [docs/EFFECT_PLUGIN_GUIDE.md](docs/EFFECT_PLUGIN_GUIDE.md).
- **Linux support** — Ubuntu 20.04+ packages below; runs on 8 GB / iGPU
  machines without GPU acceleration; no telemetry, no background daemons.

## Supported platforms

| Package | Target | Notes |
|---|---|---|
| `.deb` (amd64) | **Ubuntu 20.04 LTS+** | GLIBC ≤ 2.31 enforced by build gate; FFmpeg sidecar bundled |
| `.AppImage` (x86_64) | **Ubuntu 20.04 LTS+** | Self-contained; needs libfuse2 (or `--appimage-extract-and-run`) |
| Tauri 2 desktop build | distros with WebKitGTK 4.1 | Build from source (see below) |

Ubuntu 20.04 compatibility is real, not metadata: the binaries are linked
with the GLIBC symbol ceiling pinned at **2.31** (`cargo-zigbuild`) and every
release artifact goes through `build/check-glibc.sh`, which **fails the
build** if any shipped ELF requires anything newer (and rejects unexpected
library dependencies such as webkit2gtk).

### How Ubuntu 20.04 works without WebKitGTK 4.1

Tauri 2 on Linux requires `webkit2gtk-4.1`, which no Ubuntu 20.04 package
provides — and every distro that ships it needs GLIBC ≥ 2.34. MyCut's 20.04
packages therefore ship the **Local Web Runtime**: the same Rust core, the
same React UI, and the same command API, served over loopback HTTP to your
browser (Firefox/Chromium — lighter on low-end machines than an embedded
WebKit). The Tauri desktop build remains available for newer distros. Rationale
and rejected alternatives: [docs/DECISIONS.md](docs/DECISIONS.md) (D15/D16).

## Installation

### .deb

```bash
sudo dpkg -i MyCut_*_amd64.deb
# if apt reports missing deps (it shouldn't on a desktop install):
sudo apt-get install -f

mycut            # or launch "MyCut" from the application menu
```

### AppImage

```bash
chmod +x MyCut_*_amd64.AppImage
./MyCut_*_amd64.AppImage
# Without FUSE2:
./MyCut_*_amd64.AppImage --appimage-extract-and-run
```

The first launch starts the local runtime and opens your browser. Ctrl-C in
the terminal (or closing the process) stops it.

### FFmpeg

The Linux packages **bundle a pinned static FFmpeg/ffprobe** (GPL build with
libx264 + libass; see [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)), so
transitions and captions work on Ubuntu 20.04's FFmpeg 4.2-less feature set
out of the box. To use your system FFmpeg instead:

```bash
MYCUT_USE_SYSTEM_FFMPEG=1 mycut
```

The runtime diagnoses FFmpeg honestly — the `doctor` report and the startup
log tell you **missing / too old / ok** with the version, path and a fix
message. The minimum is FFmpeg 4.3 (for `xfade`).

## NVIDIA NIM API key configuration

1. Create an API key at <https://build.nvidia.com> (integrate.api.nvidia.com).
2. In MyCut, open **Settings → AI Provider** and paste the key, then press
   **Save Key**.
   - The key is stored in your OS secret service (GNOME Keyring) when
     available, otherwise in a `0600` file under `~/.config/mycut/`.
   - It never reaches the browser JavaScript, logs, or project files — the
     settings panel only ever shows whether a key is stored.
3. Press **Test Connection**. It checks the key, the base URL and
   authentication, then reports what it actually found, e.g.
   `✓ NVIDIA NIM connection successful · authentication successful · 42 models available`.
   Failures are specific (HTTP 401 → "the API key was rejected", 503 →
   "provider unavailable", network errors named as such — never a bare
   "server error").
4. Pick a model in the **searchable dropdown** populated by the provider's
   model list (`GET /v1/models`). Use **Refresh Models** to re-query; the
   list is cached and invalidated when the key or base URL changes. If your
   provider has no model-list endpoint, MyCut says so and offers a manual
   model-ID field instead. If a saved model later disappears from the
   provider, the UI tells you rather than silently switching.
5. Optional: **Test Model** sends a minimal request to verify the selected
   model is actually usable.

No key? Everything except AI planning works offline: import, manual edits,
analysis, preview, export.

## Development setup

Requirements: Linux, **Rust 1.98+** (stable), **Node.js ≥ 20**, and FFmpeg
≥ 4.3 on `PATH` (used by tests; any recent distro FFmpeg works).

```bash
# 1. Frontend
npm --prefix ui ci
npm --prefix ui run build       # outputs to src-tauri/dist

# 2. Tests (145+ tests: unit, integration, real-HTTP runtime E2E)
cargo test --workspace

# 3. Run the local web runtime in dev
cargo run -p mycut-server -- --no-open --port 8900
# → open http://127.0.0.1:8900

# 4. Headless pipeline check (no UI)
cargo run -p mycut-cli -- e2e target/e2e
```

Lint gates (CI-enforced): `cargo clippy --workspace` (0 warnings),
`cargo fmt --all --check`, `tsc -b` (strict).

### Build commands (reproducible)

```bash
# Everything: UI → GLIBC-2.31 release binaries → gate → .deb + .AppImage
bash build/build.sh

# Or inside the pinned Ubuntu 20.04 container
docker build -t mycut-build build/ubuntu-20.04
docker run --rm -v "$PWD:/workspace" mycut-build bash build/build.sh
```

Artifacts land in `dist/`:
- `MyCut_<version>_amd64.deb`
- `MyCut_<version>_amd64.AppImage`

### Building the Tauri desktop shell (newer distros)

Requires `libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev
libssl-dev libayatana-appindicator3-dev librsvg2-dev` (Ubuntu 22.04+):

```bash
cargo build --release -p mycut-app        # crate `mycut-app`, excluded from workspace gates
```

## Troubleshooting

| Symptom | Diagnosis / fix |
|---|---|
| `FFmpeg not found` at startup | Install FFmpeg (`sudo apt install ffmpeg`), keep using the bundled sidecar, or set `MYCUT_FFMPEG`/`MYCUT_FFPROBE` to explicit paths. |
| `FFmpeg x.y is too old (need >= 4.3)` | Your system FFmpeg lacks `xfade`. Stop overriding the sidecar (`unset MYCUT_USE_SYSTEM_FFMPEG`) or install a newer FFmpeg. |
| AppImage does not start (FUSE error) | `sudo apt install libfuse2`, or run with `--appimage-extract-and-run`. |
| Browser shows "backend offline (UI dev mode)" | The runtime process exited — restart `mycut` and reload the page. |
| AI says a key is not configured | Settings → AI Provider → paste the key → **Save Key** (typing the key alone does not store it). AI planning is the only feature that needs it. |
| "Selected model is no longer available" | Your saved model ID vanished from the provider's list — pick another in Settings. MyCut never switches models silently. |
| `version ... is newer than this app supports` | The `.mycut` file was written by a newer MyCut; upgrade the app. |

Runtime environment variables: `MYCUT_NO_OPEN=1` (don't open a browser),
`MYCUT_BROWSER=<cmd>`, `MYCUT_PORT`/`MYCUT_HOST`, `MYCUT_EXPORT_DIR`
(default `~/Videos`), `MYCUT_LUT_DIR`, `MYCUT_USE_SYSTEM_FFMPEG=1`.

## Security & privacy

- The LLM never executes anything: it emits JSON validated by a strict
  Rust verifier (path traversal, injection, overlaps — adversarially
  tested; see `crates/schema` and `crates/engine/tests/security.rs`).
- All media tooling runs as argument arrays; there is no shell anywhere in
  the codebase (asserted by tests).
- The Local Web Runtime binds loopback only, requires an `X-MyCut` header
  (blocks cross-origin form posts) and serves media only from jailed roots.
- No telemetry, ever. Nothing leaves your machine except requests you
  explicitly send to NVIDIA NIM.

## Status & limitations (honest)

See [STATUS.md](STATUS.md) for the full evidence-backed matrix. Known
limitations of this release: preview is proxy-based (not a real-time
compositor), images import into the library but timeline placement of
stills follows the probed single-frame duration, transcription ships via
whisper.cpp but is not wired into the captions panel yet, and the Tauri
shell is built/tested only where WebKitGTK 4.1 exists. Multi-track video
compositing and keyframe editing are on the roadmap.

## Contributing

PRs welcome — see [CONTRIBUTING.md](CONTRIBUTING.md). Every feature lands
with tests that verify real output (ffprobe/pixel assertions), never mocks.
Security-sensitive reports: [SECURITY.md](SECURITY.md).

## License

MIT — see [LICENSE](LICENSE). The bundled FFmpeg sidecar is GPL-licensed
separately; attribution and source links in
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
