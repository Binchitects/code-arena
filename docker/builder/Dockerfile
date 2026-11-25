# Code Arena's build image: builds, tests and packages the Linux binaries (static, musl) from any Linux host.
# Windows and macOS binaries are built on their own CI runners.
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

# Crates and build output live in volumes or the mounted tree, never in the image.
ENV CARGO_HOME=/src/.cargo-home \
    CARGO_TARGET_DIR=/src/target \
    CARGO_TERM_COLOR=always
