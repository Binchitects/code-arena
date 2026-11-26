# Code Arena's build image: builds, tests and packages the Linux binaries (static, musl, x64 and arm64) and the
# Windows x64 one (MinGW) from any Linux host. macOS binaries need Apple's SDK: they are built on macOS runners.
# Node and npm build the IDE's page.
#
#   docker build -t code-arena-builder docker/builder
#   docker run --rm -v "$PWD":/src -w /src code-arena-builder make ci
ARG RUST_IMAGE=rust:1.100.0-slim-bookworm
FROM ${RUST_IMAGE}

RUN apt-get update \
 && apt-get install -y --no-install-recommends musl-tools git make ca-certificates \
 && rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt clippy \
 && rustup target add x86_64-unknown-linux-musl

# The other systems built from here: Linux on arm64 (static, musl) and Windows on x64 (MinGW), each with the C
# compiler its crates' C code needs (ring), and zip for the Windows package.
RUN dpkg --add-architecture arm64 \
 && apt-get update \
 && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross musl-dev:arm64 gcc-mingw-w64-x86-64 zip \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add aarch64-unknown-linux-musl x86_64-pc-windows-gnu
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_musl="aarch64-linux-gnu-gcc -specs /usr/lib/aarch64-linux-musl/musl-gcc.specs" \
    AR_aarch64_unknown_linux_musl=aarch64-linux-gnu-ar \
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
    CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
    AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar

# Node and npm, for the IDE's page (web/): Node's own build, checked against its published SHA-256.
ARG NODE_VERSION=22.23.5
ADD --checksum=sha256:cdeb8670a0c81f51b4dc4ac61919f9b1ad917b304bf8221eb939570663d240f8 \
    https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz /tmp/node.tar.xz
RUN apt-get update && apt-get install -y --no-install-recommends xz-utils && rm -rf /var/lib/apt/lists/* \
 && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 --exclude='*/CHANGELOG.md' --exclude='*/README.md' \
 && rm /tmp/node.tar.xz \
 && node --version && npm --version

# Crates, packages and build output live in volumes or the mounted tree, never in the image.
ENV CARGO_HOME=/src/.cargo-home \
    CARGO_TARGET_DIR=/src/target \
    CARGO_TERM_COLOR=always \
    npm_config_cache=/src/.cache/npm \
    npm_config_update_notifier=false \
    npm_config_fund=false \
    npm_config_audit=false
