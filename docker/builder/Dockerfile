# Code Arena's build image: builds, tests and packages the Linux binaries (static, musl) from any Linux host.
# Windows and macOS binaries are built on their own CI runners. Node and npm build the IDE's page.
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
