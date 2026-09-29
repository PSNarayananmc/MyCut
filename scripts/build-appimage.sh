#!/usr/bin/env bash
# Build the production x86_64 AppImage for the Ubuntu 20.04 (GLIBC 2.31)
# target.
#
# The AppImage is self-contained: the `mycut` binary embeds the UI and only
# needs the glibc family at runtime (enforced by build/check-glibc.sh), so
# AppRun does NOT bundle any system libraries — it only points the engine at
# the bundled FFmpeg sidecar (docs D16).
#
# appimagetool is downloaded pinned + checksum-verified and run extracted
# (no FUSE required to BUILD). To RUN the AppImage: FUSE2 (libfuse2), or
# `./MyCut.AppImage --appimage-extract-and-run`.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
BIN_DIR="${MYCUT_RELEASE_BIN:-target/x86_64-unknown-linux-gnu/release}"
APP_DIR="target/appimage/MyCut.AppDir"
SIDE=build/_cache/ffmpeg-bin
CACHE=build/_cache

# appimagetool pin (AppImageKit continuous release, x86_64). SHA256 is the
# pin; a mismatch fails the build so a re-pin is a conscious act.
APPIMAGETOOL_URL="https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage"
APPIMAGETOOL_SHA256="b90f4a8b18967545fda78a445b27680a1642f1ef9488ced28b65398f2be7add2"

[ -x "$BIN_DIR/mycut" ] || { echo "mycut binary missing at $BIN_DIR/mycut — run build/build.sh" >&2; exit 1; }

# ---- appimagetool (pinned, run extracted so no FUSE is needed) ----------
AIT="$CACHE/appimagetool-x86_64.AppImage"
mkdir -p "$CACHE"
if [ ! -f "$AIT" ]; then
  echo "downloading appimagetool (pinned by checksum)"
  curl -fL --retry 3 --max-time 300 -o "$AIT" "$APPIMAGETOOL_URL"
fi
ACTUAL_SHA="$(sha256sum "$AIT" | cut -d' ' -f1)"
if [ "$ACTUAL_SHA" != "$APPIMAGETOOL_SHA256" ]; then
  echo "appimagetool checksum mismatch:" >&2
  echo "  expected $APPIMAGETOOL_SHA256" >&2
  echo "  actual   $ACTUAL_SHA" >&2
  echo "If AppImageKit republished, re-pin APPIMAGETOOL_SHA256 consciously." >&2
  exit 1
fi
AIT_EXTRACTED="$CACHE/appimagetool-extracted"
if [ ! -x "$AIT_EXTRACTED/AppRun" ]; then
  chmod +x "$AIT"
  rm -rf "$AIT_EXTRACTED"
  "$AIT" --appimage-extract >/dev/null   # -> squashfs-root
  rm -rf "$AIT_EXTRACTED" && mv squashfs-root "$AIT_EXTRACTED"
fi

# ---- AppDir --------------------------------------------------------------
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/usr/bin" \
         "$APP_DIR/usr/share/applications" \
         "$APP_DIR/usr/share/icons/hicolor/256x256/apps" \
         "$APP_DIR/usr/share/doc/mycut"

install -m 0755 "$BIN_DIR/mycut" "$APP_DIR/usr/bin/mycut"
if [ -x "$SIDE/ffmpeg" ] && [ -x "$SIDE/ffprobe" ]; then
  install -m 0755 "$SIDE/ffmpeg" "$APP_DIR/usr/bin/ffmpeg"
  install -m 0755 "$SIDE/ffprobe" "$APP_DIR/usr/bin/ffprobe"
else
  echo "warning: sidecar ffmpeg missing — AppImage will need system ffmpeg" >&2
fi

cp build/icons/mycut-256.png "$APP_DIR/usr/share/icons/hicolor/256x256/apps/mycut.png"
cp build/icons/mycut-256.png "$APP_DIR/mycut.png"
ln -sf mycut.png "$APP_DIR/.DirIcon"

cat > "$APP_DIR/mycut.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=MyCut
GenericName=Video Editor
Comment=Chat-driven AI video editor powered by local FFmpeg rendering
Exec=mycut %U
Icon=mycut
Terminal=false
Categories=AudioVideo;Video;
Keywords=video;editor;ai;cut;captions;
StartupNotify=true
X-AppImage-Version=${VERSION}
DESKTOP
cp "$APP_DIR/mycut.desktop" "$APP_DIR/usr/share/applications/mycut.desktop"
cp LICENSE "$APP_DIR/usr/share/doc/mycut/copyright"

# AppRun: bundle-aware env, exec real binary. No LD_LIBRARY_PATH tricks.
cat > "$APP_DIR/AppRun" <<'APPRUN'
#!/usr/bin/env bash
# Resolve this AppImage's location (works when invoked via $APPDIR or PATH).
APPDIR="${APPDIR:-$(cd "$(dirname "$0")" && pwd)}"
if [ -z "${MYCUT_USE_SYSTEM_FFMPEG:-}" ] && [ -z "${MYCUT_FFMPEG:-}" ] \
   && [ -x "$APPDIR/usr/bin/ffmpeg" ]; then
  export MYCUT_FFMPEG="$APPDIR/usr/bin/ffmpeg"
  export MYCUT_FFPROBE="$APPDIR/usr/bin/ffprobe"
fi
# Refuse developer paths leaking in: only AppDir-relative resources are used.
exec "$APPDIR/usr/bin/mycut" "$@"
APPRUN
chmod 0755 "$APP_DIR/AppRun"

# ---- tool ----------------------------------------------------------------
mkdir -p dist
"$AIT_EXTRACTED/AppRun" "$APP_DIR" "dist/MyCut_${VERSION}_amd64.AppImage" >/dev/null
chmod +x "dist/MyCut_${VERSION}_amd64.AppImage"
echo "built dist/MyCut_${VERSION}_amd64.AppImage"
