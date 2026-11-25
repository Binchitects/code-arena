# Code Arena's build steps, the same on a developer's machine, in the build image and in CI.
CARGO ?= cargo
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

.PHONY: ci fmt lint test build dist clean

ci: fmt lint test

fmt:
	$(CARGO) fmt --all --check

lint:
	$(CARGO) clippy --workspace --all-targets --locked -- -D warnings

test:
	$(CARGO) test --workspace --locked

build:
	$(CARGO) build --release --locked

# Static Linux binary in dist/ (in the build image).
dist:
	$(CARGO) build --release --locked --target x86_64-unknown-linux-musl
	mkdir -p dist
	scripts/package.sh x86_64-unknown-linux-musl linux-x64 $(VERSION)

clean:
	$(CARGO) clean
	rm -rf dist
