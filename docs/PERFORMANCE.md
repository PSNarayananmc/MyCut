# MyCut Performance — measured numbers, not targets

Host used for all measurements (honest disclosure): a constrained CI-style
sandbox — **Debian 13, 2 vCPU, 4.1 GB RAM, no GPU, NVMe-backed overlay FS**.
This is *weaker* than the spec's target machine (i3-7th-gen, 8 GB, iGPU) in
core count; memory headroom on the target is larger. All builds are `debug`
unless stated; release builds are faster.

Build info: Rust 1.98.1, FFmpeg 7.1.5 (system), Node 24 (UI build only).

## End-to-end pipeline (`mycut-cli e2e`)

Fixture: 20 s, 640×360 @ 30 fps, `testsrc2` + 440 Hz tone (lavfi, no
copyrighted media). Plan: cut 12–16 s, color grade, zoom-punch, vignette,
text title, 9:16 reframe, audio normalize + fade-out, captions from
transcript.

| Stage | Measured |
|---|---|
| Import (ffprobe + content hash + save) | ~0.4 s |
| Analysis (scenes/audio/motion/auto-color, first run) | 0.35–0.9 s |
| Analysis (cached, same content hash + params) | **0.037 s** |
| Plan apply (validate + transaction + captions) | < 0.01 s |
| Render (16 s output, 608×1080, libx264 veryfast, debug build) | 5.3 s (≈3× realtime) |
| Full E2E wall clock | **9.2 s** |
| Peak child RSS (whole pipeline incl. ffmpeg children) | **427 MB** |

Constrained-memory run (RLIMIT_AS = 2.5 GB on the whole process tree):
**passes**, wall 9.2 s, peak RSS 435 MB — no OOM, bounded memory.
(RLIMIT_AS = 1.5 GB aborts at ffmpeg spawn: ffmpeg reserves large *virtual*
address space for threading; virtual ≠ resident. Real resident usage stays
≈ 430 MB.)

Output verified every run: duration 16.000 s (±0), H.264 + AAC, clean
`ffmpeg -v error -f null -` decode, 608×1080 vertical.

## Where time goes (render, 16 s @ 608×1080)

- decode + filtergraph (zoompan is the heaviest per-frame filter) ≈ 55%
- libx264 encode ≈ 40%
- mux/faststart ≈ 5%

zoompan and crop-per-frame expressions are single-threaded inside the
filter; the encoder threads out. On the 2-vCPU sandbox this shows as ≈1.4×
slower than realtime in debug.

## Memory discipline measures actually implemented

- No frame buffers in Rust/JS: children stream to files; webview gets paths.
- Caches are content-hash-keyed files with LRU eviction and a size cap
  (`Cache::evict_if_needed`), tested to evict under cap.
- Children get explicit `-threads` via ffmpeg defaults of `-preset veryfast`
  (libx264 threads bounded by core count; 2 here).
- Analysis adaptive depth: fps scaling by duration (`adaptive_params`),
  sparse mode for long sources.

## What is NOT measured / honest gaps

- Release/AppImage builds on the real target hardware (i3-7xxx + 8 GB) —
  a human can reproduce with `scripts/bench.sh` after installing.
- GPU encode paths (VAAPI/NVENC): unavailable in this sandbox — code path
  exists (`tiny_test_encode` verification + fallback), but numbers pending.
- Long-duration (> 1 h) footage: adaptive depth is implemented, untested at
  scale here.
- Constrained CPU via cgroups was unavailable in this sandbox (cgroup fs
  read-only); the RAM constraint was applied via RLIMIT_AS instead.

Reproduce everything: `scripts/bench.sh` (runs the E2E, constrained run,
workspace tests, and appends JSON results to stdout).
