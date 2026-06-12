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

# Build the pinned adroit into .conduit/bin (no network: file:// + --locked)
init-adroit:
    cargo install --git file:///home/brett/repos/como-tech/adroit --rev $(cat adroit.rev) --locked --root .conduit adroit
    .conduit/bin/adroit manifest -o json > /dev/null && echo "adroit handshake OK"

# Throwaway Gitea on localhost:3000 — two users, labels, seeded repo
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
