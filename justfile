# Default: list available recipes
default:
    @just --list

# Install project toolchain components and tools
init:
    rustup component add clippy rustfmt
    cargo install mdbook

# Run all CI checks: format, lint, tests, book build (the house gate)
ci: fmt-check lint test book

# Format code
fmt:
    cargo fmt

# Check formatting without modifying files
fmt-check:
    cargo fmt --check

# Run clippy lints over all targets
lint:
    cargo clippy --all-targets -- -D warnings

# Run all tests (unit + integration)
test *ARGS:
    cargo test {{ARGS}}

# Type-check without building
check:
    cargo check

# Run the binary with arguments
run *ARGS:
    cargo run -- {{ARGS}}

# Build the pinned adroit into .conduit/bin (no network: file:// + --locked).
# adroit.rev carries leading comment lines; the rev is the last line.
init-adroit:
    cargo install --git file:///home/brett/repos/como-tech/adroit --rev $(grep -v '^#' adroit.rev | tail -1) --locked --root .conduit adroit
    .conduit/bin/adroit manifest -o json > /dev/null && echo "adroit handshake OK"

# Validate conduit's own ADR corpus with the pinned adroit.
# Standalone, not a `ci` leg: CI is a fresh checkout and the pin lives in
# gitignored .conduit/ — run after `just init-adroit`.
adr-check:
    .conduit/bin/adroit check --dir adr

# Scripted demo trigger: label the demo issue conduit:run as the reviewer (REPO_NAME selects the repo)
demo-trigger:
    bash demo/demo-trigger.sh

# The full demo walkthrough is docs/src/usage/demo.md (Task 14)
demo:
    @echo "Follow docs/src/usage/demo.md — `just forge-up` first."

# Conformance suite, all legs that need no secrets
conformance:
    cargo test --test conformance

# Throwaway Gitea on localhost:3000 — two users, labels, seeded repo (SEED_REPO_DIR/REPO_NAME parameterize the seeding)
forge-up:
    docker compose -f demo/docker-compose.yml up -d
    bash demo/gitea-init.sh

# Destroy the throwaway forge (container + volume — nothing survives)
forge-down:
    docker compose -f demo/docker-compose.yml down -v

# Build the user manual (mdbook)
book:
    mdbook build docs
    @echo "Book built -> docs/book"

# Serve the book locally with live reload
book-serve:
    mdbook serve docs --open
