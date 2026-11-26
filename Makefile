# Code Arena's build steps, the same on a developer's machine, in the build image and in CI.
CARGO ?= cargo
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

.PHONY: ci fmt lint test build dist dist-all clean web

ci: fmt lint test

fmt:
	$(CARGO) fmt --all --check

lint:
	$(CARGO) clippy --workspace --all-targets --locked -- -D warnings

test:
	$(CARGO) test --workspace --locked

# The IDE's page (web/dist), built into the binary: npm ci when its packages are not there yet.
web:
	cd web && { [ -d node_modules ] || npm ci; } && npm test && npm run build

build: web
	$(CARGO) build --release --locked

# Static Linux binary in dist/ (in the build image).
dist: web
	$(CARGO) build --release --locked --target x86_64-unknown-linux-musl
	mkdir -p dist
	scripts/package.sh x86_64-unknown-linux-musl linux-x64 $(VERSION)

# Every system this image builds: Linux x64 and arm64 (static), Windows x64. macOS is built on macOS.
dist-all: web
	$(CARGO) build --release --locked -p ca-cli --target x86_64-unknown-linux-musl
	$(CARGO) build --release --locked -p ca-cli --target aarch64-unknown-linux-musl
	$(CARGO) build --release --locked -p ca-cli --target x86_64-pc-windows-gnu
	mkdir -p dist
	scripts/package.sh x86_64-unknown-linux-musl linux-x64 $(VERSION)
	scripts/package.sh aarch64-unknown-linux-musl linux-arm64 $(VERSION)
	scripts/package.sh x86_64-pc-windows-gnu win-x64 $(VERSION)

clean:
	$(CARGO) clean
	rm -rf dist web/dist
