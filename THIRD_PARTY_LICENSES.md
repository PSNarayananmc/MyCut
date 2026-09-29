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
| FFmpeg / ffprobe (bundled sidecar) | GPL-3-or-later (BtbN linux64-gpl build; static, with libx264/libass) | Media engine. Source code: https://github.com/BtbN/FFmpeg-Builds and https://ffmpeg.org/download.html. Pinned build: `ffmpeg-n8.1.3-6-gff48edd8b2-linux64-gpl-8.1.tar.xz`, SHA256 `9f96ca3806df5926dc6645a93fab35c2ed3783c9824c319ca99e9dc6dc286875` (build/fetch-ffmpeg.sh). MyCut spawns it as a child process with argument arrays and exchanges data only via files/stdin/stdout; because the bundled binary is GPL-enabled, distributors of that binary must comply with GPL-3 for it. MyCut's own code remains MIT. |
| FFmpeg / ffprobe (system) | LGPL-2.1-or-later as shipped by your distro | Used when `MYCUT_USE_SYSTEM_FFMPEG=1`; nothing bundled by MyCut. |
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
