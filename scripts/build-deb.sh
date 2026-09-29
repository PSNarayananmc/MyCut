#!/usr/bin/env bash
# Build the production .deb for the Ubuntu 20.04 (GLIBC 2.31) target.
#
# Contents:
#   /opt/mycut/bin/mycut            local web runtime binary (GLIBC<=2.31)
#   /opt/mycut/bin/ffmpeg, ffprobe  pinned static sidecar (docs D16)
#   /usr/bin/mycut                  launcher wrapper (sidecar env + exec)
#   /usr/share/applications/mycut.desktop
#   /usr/share/icons/hicolor/*/apps/mycut.png
#   /usr/share/doc/mycut/           copyright (DEP-5)
#
# Dependency truth (no lying metadata): the binary links only the glibc
# family, so Depends is libc6 (>= 2.31). xdg-utils is a Recommend (used to
# open the browser; $BROWSER/xdg-open fallbacks exist). ffmpeg is a Suggest
# because the sidecar ships it.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
BIN_DIR="${MYCUT_RELEASE_BIN:-target/x86_64-unknown-linux-gnu/release}"
STAGE="target/deb/mycut_${VERSION}_amd64"
SIDE=build/_cache/ffmpeg-bin

[ -x "$BIN_DIR/mycut" ] || { echo "mycut binary missing at $BIN_DIR/mycut — run build/build.sh" >&2; exit 1; }

rm -rf "$STAGE"
mkdir -p "$STAGE/DEBIAN" \
         "$STAGE/opt/mycut/bin" \
         "$STAGE/usr/bin" \
         "$STAGE/usr/share/applications" \
         "$STAGE/usr/share/doc/mycut"

# Icons (hicolor set).
for s in 16 32 48 64 128 256 512; do
  icon="build/icons/mycut-${s}.png"
  [ -f "$icon" ] || continue
  mkdir -p "$STAGE/usr/share/icons/hicolor/${s}x${s}/apps"
  cp "$icon" "$STAGE/usr/share/icons/hicolor/${s}x${s}/apps/mycut.png"
done

# Application + sidecar binaries.
install -m 0755 "$BIN_DIR/mycut" "$STAGE/opt/mycut/bin/mycut"
if [ -x "$SIDE/ffmpeg" ] && [ -x "$SIDE/ffprobe" ]; then
  install -m 0755 "$SIDE/ffmpeg" "$STAGE/opt/mycut/bin/ffmpeg"
  install -m 0755 "$SIDE/ffprobe" "$STAGE/opt/mycut/bin/ffprobe"
else
  echo "warning: sidecar ffmpeg missing — package will require system ffmpeg (set MYCUT_USE_SYSTEM_FFMPEG=1)" >&2
fi

# Launcher wrapper: point the engine at the bundled sidecar (D16) unless the
# user prefers system ffmpeg.
cat > "$STAGE/usr/bin/mycut" <<'WRAPPER'
#!/usr/bin/env bash
# MyCut launcher: bundle-aware environment, then exec the real binary.
if [ -z "${MYCUT_USE_SYSTEM_FFMPEG:-}" ] && [ -z "${MYCUT_FFMPEG:-}" ] \
   && [ -x /opt/mycut/bin/ffmpeg ]; then
  export MYCUT_FFMPEG=/opt/mycut/bin/ffmpeg
  export MYCUT_FFPROBE=/opt/mycut/bin/ffprobe
fi
exec /opt/mycut/bin/mycut "$@"
WRAPPER
chmod 0755 "$STAGE/usr/bin/mycut"

# Desktop entry.
cat > "$STAGE/usr/share/applications/mycut.desktop" <<'DESKTOP'
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
DESKTOP

# Docs / license (DEP-5).
cp LICENSE "$STAGE/usr/share/doc/mycut/copyright"
chmod 0644 "$STAGE/usr/share/doc/mycut/copyright"

# Package metadata.
cat > "$STAGE/DEBIAN/control" <<CONTROL
Package: mycut
Version: ${VERSION}
Section: video
Priority: optional
Architecture: amd64
Depends: libc6 (>= 2.31)
Recommends: xdg-utils
Suggests: ffmpeg
Installed-Size: $(du -sk "$STAGE" | cut -f1)
Maintainer: MyCut contributors <mycut@users.noreply.github.com>
Homepage: https://github.com/PSNarayananmc/MyCut
Description: Chat-driven Linux video editor
 MyCut plans video edits from natural-language prompts (NVIDIA NIM) and
 executes them deterministically with a local FFmpeg engine. The UI runs in
 the browser and is served by a loopback-only local process; media never
 leaves the machine. Ships a bundled FFmpeg sidecar so transitions and
 captions work on Ubuntu 20.04 out of the box.
CONTROL

# Lintian-quiet postinst: refresh desktop/icon caches best-effort.
cat > "$STAGE/DEBIAN/postinst" <<'POST'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -qf /usr/share/icons/hicolor || true
fi
exit 0
POST
chmod 0755 "$STAGE/DEBIAN/postinst"

mkdir -p dist
dpkg-deb --build --root-owner-group "$STAGE" "dist/MyCut_${VERSION}_amd64.deb"
echo "built dist/MyCut_${VERSION}_amd64.deb"
