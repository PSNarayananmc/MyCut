#!/usr/bin/env bash
# Build the .deb (P1). Requires cargo build --release -p mycut-app to have run.
# The deb DECLARES ffmpeg as a dependency (no bundling) per docs/DECISIONS.md.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=target/deb/mycut_0.1.0_amd64/DEBIAN
mkdir -p "$OUT/usr/bin" "$OUT/usr/share/applications" target/deb
cp target/release/mycut-app target/deb/mycut_0.1.0_amd64/usr/bin/mycut
cat > "$OUT/control" <<CONTROL
Package: mycut
Version: 0.1.0
Section: video
Priority: optional
Architecture: amd64
Depends: ffmpeg (>= 5.0), libwebkit2gtk-4.1-0
Maintainer: MyCut contributors
Description: Chat-driven Linux video editor
 Plans edits with an LLM, executes them locally with FFmpeg. Private by
 default; works offline for manual editing.
CONTROL
cat > target/deb/mycut_0.1.0_amd64/usr/share/applications/mycut.desktop <<DESKTOP
[Desktop Entry]
Name=MyCut
Exec=mycut
Type=Application
Categories=AudioVideo;Video;
DESKTOP
dpkg-deb --build target/deb/mycut_0.1.0_amd64 target/deb/mycut_0.1.0_amd64.deb
echo "built target/deb/mycut_0.1.0_amd64.deb"
