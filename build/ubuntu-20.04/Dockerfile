# MyCut reproducible Linux build container (Ubuntu 20.04 baseline).
#
# Matches the GLIBC 2.31 target machine: Ubuntu 20.04.6 LTS x86_64.
# Used by CI (.github/workflows/build.yml) and by anyone who wants a
# controlled build environment: `docker build -t mycut-build build/ubuntu-20.04`
#
# The produced binaries additionally go through the zig link stage with the
# glibc symbol ceiling pinned at 2.31 (cargo-zigbuild), and the artifact set
# is gated by build/check-glibc.sh before packaging.
FROM ubuntu:20.04

ENV DEBIAN_FRONTEND=noninteractive \
    TZ=UTC \
    LANG=C.UTF-8

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential curl ca-certificates xz-utils git pkg-config \
        python3 python3-pip python3-distutils file binutils \
        nodejs npm \
        # Runtime QA tool used by package smoke tests:
        ffmpeg \
    && rm -rf /var/lib/apt/lists/*

# Exact Node.js 20 LTS (Ubuntu 20.04's npm/nodejs are too old for Vite 5).
# nodejs.org dist URLs are immutable per version, so this pin is stable.
RUN set -eux; \
    NODE_VERSION=v20.18.1; \
    curl -fsSL "https://nodejs.org/dist/${NODE_VERSION}/node-${NODE_VERSION}-linux-x64.tar.xz" \
      -o /tmp/node.tar.xz; \
    tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1; \
    rm /tmp/node.tar.xz; \
    node --version; npm --version

# Python-side toolchain: zig (linker with historical glibc sysroots) + cargo-zigbuild.
RUN pip3 install --no-cache-dir "ziglang==0.16.0" "cargo-zigbuild==0.23.4"

# Rust toolchain (pinned; bump consciously to rebuild identically).
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl -fsSf https://sh.rustup.rs | sh -s -- -y \
        --profile minimal --default-toolchain 1.98.1 \
    && rustc --version && cargo --version

WORKDIR /workspace
COPY . /workspace

CMD ["bash", "build/build.sh"]
