#!/usr/bin/env bash
# Build the AppImage on a desktop Linux with webkit2gtk + libfuse2.
# This CANNOT run in headless sandboxes (documented in STATUS.md);
# requirements: cargo, node, npm, linuxdeploy-x86_64.AppImage, webkit2gtk-4.1-dev.
set -euo pipefail
cd "$(dirname "$0")/.."

npm --prefix ui ci || npm --prefix ui install
npm --prefix ui run build          # outputs to src-tauri/dist
cargo build --release -p mycut-app

APP_DIR=target/appimage/MyCut.AppDir
mkdir -p "$APP_DIR/usr/bin" "$APP_DIR/usr/share/icons/hicolor/256x256/apps" "$APP_DIR/usr/lib"
cp target/release/mycut-app "$APP_DIR/usr/bin/mycut"

# FFmpeg: bundle a static build (documented license: LGPL configuration).
# Download from johnvansickle static builds or build your own; verify the
# configuration contains --enable-gpl only if you accept GPL obligations.
# Place the binary as usr/bin/ffmpeg (LGPL build preferred).
if [ -n "${MYCUT_STATIC_FFMPEG:-}" ]; then
  cp "$MYCUT_STATIC_FFMPEG" "$APP_DIR/usr/bin/ffmpeg"
  cp "$MYCUT_STATIC_FFMPEG" "$APP_DIR/usr/bin/ffprobe" 2>/dev/null || true
fi

cat > "$APP_DIR/mycut.desktop" <<DESKTOP
[Desktop Entry]
Name=MyCut
Exec=mycut
Icon=mycut
Type=Application
Categories=AudioVideo;Video;
DESKTOP

# linuxdeploy bundles webkit2gtk & friends and sets AppRun.
linuxdeploy-x86_64.AppImage --appdir "$APP_DIR" --output appimage
echo "AppImage built under target/appimage/"
