#!/usr/bin/env bash
# Fetch the pinned static FFmpeg/ffprobe sidecar used by the MyCut Linux
# packages (docs/DECISIONS.md D16). Verifies SHA256 — a checksum mismatch
# or dead URL FAILS the build (that is the pin; re-pin consciously).
#
# Output: <repo>/build/_cache/ffmpeg-bin/{ffmpeg,ffprobe}
set -euo pipefail
cd "$(dirname "$0")/.."

BASE_URL="https://github.com/BtbN/FFmpeg-Builds/releases/download"
TAG="autobuild-2026-09-29-13-10"
ASSET="ffmpeg-n8.1.3-6-gff48edd8b2-linux64-gpl-8.1.tar.xz"
SHA256="9f96ca3806df5926dc6645a93fab35c2ed3783c9824c319ca99e9dc6dc286875"
# Alternative pinned source (SHA256 must be re-recorded if switched):
#   https://johnvansickle.com/ffmpeg/releases/ffmpeg-7.1.1-amd64-static.tar.xz

CACHE=build/_cache
OUT_DIR="$CACHE/ffmpeg-bin"
ARCHIVE="$CACHE/ffmpeg-sidecar.tar.xz"

mkdir -p "$CACHE"

verify_sha() {
  echo "$1  $2" | sha256sum -c - >/dev/null 2>&1
}

fetch() { # fetch URL OUT
  if command -v curl >/dev/null; then
    curl -fL --retry 3 --max-time 600 -o "$2" "$1"
  else
    wget -q -O "$2" "$1"
  fi
}

if [ -x "$OUT_DIR/ffmpeg" ] && [ -x "$OUT_DIR/ffprobe" ]; then
  echo "sidecar already present: $OUT_DIR (delete to re-fetch)"
  exit 0
fi

if [ -f "$ARCHIVE" ] && verify_sha "$SHA256" "$ARCHIVE"; then
  echo "archive cache hit: $ARCHIVE"
else
  echo "downloading pinned FFmpeg sidecar: $ASSET"
  rm -f "$ARCHIVE"
  fetch "$BASE_URL/$TAG/$ASSET" "$ARCHIVE"
  if ! verify_sha "$SHA256" "$ARCHIVE"; then
    echo "FATAL: SHA256 mismatch for $ASSET" >&2
    echo "  expected: $SHA256" >&2
    sha256sum "$ARCHIVE" >&2 || true
    exit 1
  fi
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
tar -xf "$ARCHIVE" -C "$WORK" --wildcards "*/bin/ffmpeg" "*/bin/ffprobe"
SRC_BIN="$(dirname "$(dirname "$(find "$WORK" -type f -name ffmpeg | head -1)")")"
mkdir -p "$OUT_DIR"
cp "$SRC_BIN/bin/ffmpeg" "$SRC_BIN/bin/ffprobe" "$OUT_DIR/"
chmod +x "$OUT_DIR/"*
# Drop ffplay (unused; keeps packages smaller) — already only copied ffmpeg/ffprobe.
echo "sidecar ready: $OUT_DIR"
