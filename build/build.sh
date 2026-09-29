#!/usr/bin/env bash
# MyCut reproducible Linux build: UI -> GLIBC-2.31-capped Rust binaries ->
# GLIBC gate -> .deb + .AppImage.
#
# Usage: build/build.sh [--skip-deb] [--skip-appimage] [--skip-tests]
# Artifacts land in dist/:
#   dist/MyCut_<version>_amd64.deb
#   dist/MyCut_<version>_amd64.AppImage
#
# Works both on a developer machine (Linux, rustup + cargo-zigbuild + node
# >= 20) and inside build/ubuntu-20.04 container.
set -euo pipefail
cd "$(dirname "$0")/.."

SKIP_DEB=0; SKIP_APPIMAGE=0; SKIP_TESTS=0
for arg in "$@"; do case "$arg" in
  --skip-deb) SKIP_DEB=1;; --skip-appimage) SKIP_APPIMAGE=1;; --skip-tests) SKIP_TESTS=1;;
  *) echo "unknown flag: $arg" >&2; exit 2;;
esac; done

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
echo "== MyCut $VERSION build =="

# ---- 0. toolchain sanity ------------------------------------------------
command -v node >/dev/null || { echo "node missing (need >= 20)" >&2; exit 1; }
NODE_MAJOR="$(node -p 'process.versions.node.split(".")[0]')"
[ "$NODE_MAJOR" -ge 20 ] || { echo "node >= 20 required, found $(node --version)" >&2; exit 1; }

if command -v cargo-zigbuild >/dev/null; then
  CARGO=cargo-zigbuild
elif command -v cargo >/dev/null && cargo zigbuild --version >/dev/null 2>&1; then
  CARGO=cargo
else
  echo "cargo-zigbuild missing: pip install ziglang cargo-zigbuild" >&2
  exit 1
fi
ZIG_TARGET="x86_64-unknown-linux-gnu.2.31"

# ---- 1. frontend --------------------------------------------------------
echo "== 1/5 building UI (vite -> src-tauri/dist) =="
npm --prefix ui ci --no-audit --no-fund 2>/dev/null || npm --prefix ui install --no-audit --no-fund
npm --prefix ui run build

# ---- 2. Rust: workspace tests + GLIBC-2.31 release binaries -------------
if [ "$SKIP_TESTS" -eq 0 ]; then
  echo "== 2/5 workspace tests =="
  cargo test --workspace
fi

echo "== 3/5 release build (glibc ceiling 2.31) =="
"$CARGO" zigbuild --release --target "$ZIG_TARGET" -p mycut-server -p mycut-cli
BIN_DIR="target/x86_64-unknown-linux-gnu/release"

# ---- 3. GLIBC gate on every shipped ELF --------------------------------
echo "== 4/5 GLIBC gate =="
bash build/check-glibc.sh --max 2.31 "$BIN_DIR/mycut" "$BIN_DIR/mycut-cli"
if [ -f build/_cache/ffmpeg-bin/ffmpeg ]; then
  bash build/check-glibc.sh --max 2.31 build/_cache/ffmpeg-bin/ffmpeg build/_cache/ffmpeg-bin/ffprobe
fi

# ---- 4. packages --------------------------------------------------------
mkdir -p dist
export MYCUT_RELEASE_BIN="$BIN_DIR"
if [ "$SKIP_DEB" -eq 0 ]; then
  echo "== 5/5 .deb =="
  bash scripts/build-deb.sh
fi
if [ "$SKIP_APPIMAGE" -eq 0 ]; then
  echo "== 5/5 AppImage =="
  bash scripts/build-appimage.sh
fi

echo "== done =="
ls -la dist/ 2>/dev/null || true
