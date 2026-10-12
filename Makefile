# beam's everyday commands. Every target covers the whole workspace: the
# core (crates/beam-core) and the front ends (crates/beam), ADR-0048.
# (Until ADR-0010 this file drove the Go implementation.)

CARGO ?= cargo

.PHONY: all build release test fmt fmt-check clippy check audit clean

all: check build

build:
	$(CARGO) build --workspace

# The `beam` binary, optimised and stripped: target/release/beam(.exe).
release:
	$(CARGO) build --release -p beam

test:
	$(CARGO) test --workspace

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# What CI runs, and what must pass before a milestone is called done.
check: fmt-check clippy test

# Every dependency against the RustSec advisories, as the CI audit job does.
# Needs `cargo install cargo-audit --locked` once.
audit:
	$(CARGO) audit

clean:
	$(CARGO) clean
