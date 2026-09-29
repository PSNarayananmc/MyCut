# build/ — reproducible Linux build infrastructure

```
build/
  build.sh            one-command build: UI -> GLIBC-2.31 binaries -> gate -> packages
  check-glibc.sh      hard gate: every shipped ELF must require GLIBC <= 2.31 and
                      link only the glibc family (+libgcc/libstdc++/libz)
  fetch-ffmpeg.sh     pinned, checksum-verified static FFmpeg/ffprobe sidecar
  ubuntu-20.04/
    Dockerfile        pinned build container (node 20.18.1, rust 1.98.1,
                      ziglang 0.16.0, cargo-zigbuild 0.23.4)
  icons/              brand assets (generate_icons.py -> mycut-{16..512}.png)
  _cache/             downloaded tool artifacts (gitignored)
```

## Usage

```bash
# on a dev machine (rustup + cargo-zigbuild via `pip install ziglang cargo-zigbuild`):
bash build/build.sh

# fully pinned container:
docker build -t mycut-build build/ubuntu-20.04
docker run --rm -v "$PWD:/workspace" mycut-build bash build/build.sh
```

Outputs: `dist/MyCut_<version>_amd64.deb` and
`dist/MyCut_<version>_amd64.AppImage`.

## Why the glibc ceiling exists

The target machine runs Ubuntu 20.04.6 LTS (GLIBC 2.31). Binaries built on
newer distros silently pick up newer GLIBC symbol requirements. The zig
link stage (`cargo zigbuild --target x86_64-unknown-linux-gnu.2.31`) links
against a 2.31 sysroot so the binaries never require newer symbols, and
`check-glibc.sh` fails the build if they ever do — for every ELF that ships,
including the FFmpeg sidecar.
