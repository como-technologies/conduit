# Default: list available recipes
default:
    @just --list

# Install project toolchain components and tools
init:
    rustup component add clippy rustfmt
    cargo install mdbook cargo-edit cargo-audit

# Run all CI checks: format, lint, tests, ADR corpus, book build (the house
# gate). crate-audit is deliberately NOT a leg — it runs as a separate CI job
# (plus a weekly schedule), so a fresh advisory can't mask the code gates.
ci: fmt-check lint test adr-check book

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

# Install the pinned adroit into .conduit/bin (suite resolution convention).
# adroit.rev carries leading comment lines; the rev is the last line.
# Source order: the remote URL ${COMO_ADROIT_GIT:-${COMO_GIT_BASE:-https://github.com/como-technologies}/adroit.git}
# — the pinned rev is VERIFIED to exist after a probe clone into the
# gitignored .como/deps/adroit cache — then, when the remote leg cannot
# serve the pin (rev not pushed yet, no network, COMO_OFFLINE=1), the
# sibling checkout as file://$(realpath ../adroit) — the LOCAL-DEV OVERRIDE
# ONLY, taken with a printed notice. Pin bumps stay explicit reviewed edits
# to adroit.rev. Self-contained: nothing here sources sibling code.
init-adroit:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    rev="$(grep -v '^#' adroit.rev | tail -1)"
    url="${COMO_ADROIT_GIT:-${COMO_GIT_BASE:-https://github.com/como-technologies}/adroit.git}"
    # Already at the pin (whatever source served it)? Nothing to resolve.
    if [ -x .conduit/bin/adroit ] && [ -f .conduit/.crates2.json ] \
        && grep -qF -- "?rev=$rev#" .conduit/.crates2.json; then
        echo "init-adroit: pinned rev $rev already installed — skipping install"
        .conduit/bin/adroit manifest -o json > /dev/null && echo "adroit handshake OK"
        exit 0
    fi
    cache=.como/deps/adroit
    remote_has_rev=0
    if [ "${COMO_OFFLINE:-0}" = "1" ]; then
        echo "init-adroit: COMO_OFFLINE=1 — skipping the remote leg ($url)"
    elif [ -d "$cache" ] && [ "$(git -C "$cache" remote get-url origin 2>/dev/null)" = "$url" ] \
        && git -C "$cache" cat-file -e "$rev^{commit}" 2>/dev/null; then
        remote_has_rev=1 # populated cache already proves the pin; never auto-fetched
    else
        # Re-probe: the cache is absent, stale against the pin, or was cloned
        # from a different URL. A probe clone is cheap (bare, blob-less).
        rm -rf "$cache"
        mkdir -p .como/deps
        if git clone --quiet --bare --filter=blob:none "$url" "$cache" 2>/dev/null \
            && git -C "$cache" cat-file -e "$rev^{commit}" 2>/dev/null; then
            remote_has_rev=1
        fi
    fi
    installed=0
    if [ "$remote_has_rev" = 1 ]; then
        # --force: replace an install recorded from another source (e.g. an
        # earlier sibling fallback) — the rev, not the source, is the pin.
        if cargo install --git "$url" --rev "$rev" --locked --force --root .conduit adroit; then
            installed=1
        else
            echo "init-adroit: NOTICE — install of rev $rev from $url failed" >&2
        fi
    elif [ "${COMO_OFFLINE:-0}" != "1" ]; then
        echo "init-adroit: NOTICE — pinned rev $rev is not reachable at $url (clone failed or rev absent — the tag may not be pushed yet)" >&2
    fi
    if [ "$installed" != 1 ]; then
        if [ -d ../adroit/.git ] && [ -f ../adroit/Cargo.toml ]; then
            sib="file://$(realpath ../adroit)"
            if git -C ../adroit cat-file -e "$rev^{commit}" 2>/dev/null; then
                # The sibling carries the pin: install exactly it (still local-dev,
                # but byte-for-byte the pinned contract).
                echo "init-adroit: falling back to the sibling checkout $sib at the pinned rev — LOCAL-DEV OVERRIDE ONLY" >&2
                cargo install --git "$sib" --rev "$rev" --locked --force --root .conduit adroit
            else
                # Cold-checkout reality: neither the remote nor a fresh sibling
                # clone carries the pin (it has not been pushed/tagged yet). Rather
                # than hard-fail, install the sibling's HEAD so the demo can run —
                # loudly, because this is NOT the pinned contract.
                sib_head="$(git -C ../adroit rev-parse HEAD)"
                echo "init-adroit: NOTICE — sibling ../adroit does not carry the pinned rev $rev" >&2
                echo "  (it is unpushed/untagged here). Falling back to the sibling HEAD $sib_head" >&2
                echo "  — LOCAL-DEV OVERRIDE ONLY, NOT the pinned contract. To make this reproducible," >&2
                echo "  push/tag the pinned rev to the adroit remote, or bump adroit.rev to a reachable rev." >&2
                cargo install --git "$sib" --rev "$sib_head" --locked --force --root .conduit adroit
            fi
        else
            echo "init-adroit: ERROR — cannot resolve adroit at the pinned rev $rev." >&2
            echo "  Set COMO_ADROIT_GIT (full git URL) or COMO_GIT_BASE (https/ssh/file:// base) to a remote" >&2
            echo "  carrying the rev, or provide a sibling checkout at ../adroit (its HEAD is used if it" >&2
            echo "  does not carry the exact pin)." >&2
            exit 1
        fi
    fi
    .conduit/bin/adroit manifest -o json > /dev/null && echo "adroit handshake OK"

# Validate conduit's own ADR corpus with the pinned adroit (a `ci` leg —
# every suite repo's ci carries adr-check). Depends on init-adroit, which is
# idempotent-fast once the pin is installed and resolves the pin from the
# adroit remote on a fresh checkout (CI included).
adr-check: init-adroit
    .conduit/bin/adroit check --dir docs/src/adr

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

# Upgrade dependencies (including incompatible versions)
crate-upgrade:
    cargo upgrade --incompatible

# Update Cargo.lock to latest compatible versions
crate-update:
    cargo update

# Audit dependencies for known vulnerabilities (skipped if cargo-audit isn't
# installed; `just init` installs it and GitHub CI always runs it)
crate-audit:
    @if command -v cargo-audit >/dev/null 2>&1; then cargo audit; else echo "skip: cargo-audit not installed (run 'just init')"; fi

# Upgrade deps, update lockfile, audit, and test
crate-refresh: crate-upgrade crate-update crate-audit test

# Build the user manual (mdbook)
book:
    mdbook build docs
    @echo "Book built -> docs/book"

# Serve the book locally with live reload
book-serve:
    mdbook serve docs --open
