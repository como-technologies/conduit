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

# Build the user manual (mdbook)
book:
    mdbook build docs
    @echo "Book built -> docs/book"

# Serve the book locally with live reload
book-serve:
    mdbook serve docs --open
