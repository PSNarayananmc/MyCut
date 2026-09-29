# Third-Party Licenses

MyCut is MIT-licensed. It depends on and ships with (or downloads) the
following third-party software. This file documents each dependency, its
license, why it exists, and where a **separate-process boundary** is relied
upon.

## Runtime dependencies (linked into the app binary or its workspace)

| Dependency | License | Purpose | Notes |
|---|---|---|---|
| serde, serde_json | MIT OR Apache-2.0 | serialization | linked |
| serde_path_to_error | MIT OR Apache-2.0 | strict JSON paths | linked |
| thiserror | MIT OR Apache-2.0 | error enums | linked |
| sha2, hex | MIT OR Apache-2.0 | content hashing | linked |
| ureq + rustls | MIT OR Apache-2.0; rustls: MIT OR Apache-2.0 / ISC | NIM HTTP client | linked |
| keyring (optional feature) | MIT OR Apache-2.0 | Secret Service storage | linked when enabled; uses D-Bus (zbus, MIT OR Apache-2.0) |
| Tauri 2 (tauri, tauri-build, tauri-plugin-dialog) | MIT OR Apache-2.0 | desktop shell | linked; uses the platform webview (WebKitGTK: LGPL-2.1+) |
| React, react-dom | MIT | UI runtime | bundled into app resources |
| Vite, TypeScript | MIT, Apache-2.0 | build tooling | build-time only |

## Separate processes (NOT linked — process boundary per LGPL §title)

| Binary | License | Why separate |
|---|---|---|
| FFmpeg / ffprobe | LGPL-2.1-or-later as configured for bundling (appimage script prefers an LGPL static build; the distro build used in CI is Debian's ffmpeg) | Media engine. MyCut spawns it as a child process with argument arrays and exchanges data only via files/stdin/stdout. If a GPL-enabled FFmpeg build is bundled, the distributor must comply with GPL-3 for that binary. |
| whisper.cpp (optional user download) | MIT | ASR. User-installed binary + user-downloaded ggml models; never bundled. |

## Optional downloads (never bundled; size + license shown at download time)
- whisper.cpp ggml models (MIT for code; model cards specify their own terms).
- ONNX face/saliency detectors (planned): model files with their own terms.

## Fonts
None bundled. The UI uses system fonts (Noto Sans / Inter if installed).
libass renders captions with fontconfig-provided system or user-imported
fonts. Font licenses are the user's concern only when they import fonts.

## LUTs
At most a few project-generated `.cube` files are shipped; no third-party
LUT packs are bundled.

## Full audit command
`cargo license --workspace` (cargo-install) prints the complete transitive
dependency list for the Rust crates; `npm --prefix ui ls` for the UI.
