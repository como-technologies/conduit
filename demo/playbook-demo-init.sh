#!/usr/bin/env bash
# Create a per-run UNIQUE demo workdir for the playbook-corpus demo.
#
# Run-1 learning: the repo's shared .conduit/ store is not single-writer —
# two demo flows interleaving in one store stomp each other's cursors and
# task records. Every demo run therefore gets its own workdir carrying
# everything `conduit` resolves from its cwd:
#
#   conduit.toml   demo/playbook.conduit.toml with @ADROIT_DIR@ resolved to
#                  the absolute playbook corpus path
#   .secrets       symlink to this repo's .secrets/ (the pinned token
#                  filenames gitea-init.sh mints)
#   .conduit/bin   symlink to this repo's pinned adroit install
#
# All run state (tasks/plans/cursor/cache/workspaces) lands under
# <workdir>/.conduit — unique per run, inspectable, disposable.
#
# Usage:
#   RUN_DIR=$(bash demo/playbook-demo-init.sh)   # then: cd "$RUN_DIR"
# Env:
#   RUN_DIR       workdir to create (default demo/runs/<UTC timestamp>;
#                 must not already exist — unique per run)
#   PLAYBOOK_DIR  playbook checkout (must contain docs/src/adr). When unset, the
#                 suite resolution convention applies: COMO_PLAYBOOK_DIR ->
#                 sibling ../playbook -> the gitignored .como/deps/playbook
#                 clone cache (COMO_PLAYBOOK_GIT / COMO_GIT_BASE; playbook
#                 has NO public remote yet, so this leg needs e.g. a file://
#                 base) -> a hard, actionable error. COMO_OFFLINE=1 uses a
#                 populated cache as-is and never clones.
# Prints the created workdir's absolute path on stdout; everything else
# goes to stderr.
set -euo pipefail

cd "$(dirname "$0")/.." # conduit repo root
ROOT="$PWD"
RUN_DIR="${RUN_DIR:-demo/runs/$(date -u +%Y%m%dT%H%M%SZ)}"

# Resolve the playbook corpus (suite resolution convention, self-contained —
# never sources sibling code): explicit PLAYBOOK_DIR -> COMO_PLAYBOOK_DIR ->
# sibling -> clone cache -> hard error (the demo needs the corpus).
PLAYBOOK_DIR="${PLAYBOOK_DIR:-${COMO_PLAYBOOK_DIR:-}}"
if [ -z "$PLAYBOOK_DIR" ]; then
  if [ -d ../playbook/docs/src/adr ]; then
    PLAYBOOK_DIR=../playbook
  elif [ -d .como/deps/playbook/docs/src/adr ]; then
    PLAYBOOK_DIR=.como/deps/playbook # populated cache, used as-is (never auto-fetched)
    echo "playbook-demo-init: NOTICE — using the clone cache $PLAYBOOK_DIR" >&2
  elif [ "${COMO_OFFLINE:-0}" != "1" ]; then
    url="${COMO_PLAYBOOK_GIT:-${COMO_GIT_BASE:-https://github.com/como-technologies}/playbook.git}"
    mkdir -p .como/deps
    if git clone --filter=blob:none "$url" .como/deps/playbook 2>/dev/null; then
      PLAYBOOK_DIR=.como/deps/playbook
      echo "playbook-demo-init: NOTICE — no sibling ../playbook; cloned $url into $PLAYBOOK_DIR" >&2
    fi
  fi
fi
if [ ! -d "${PLAYBOOK_DIR:-}/docs/src/adr" ]; then
  echo "ERROR: no playbook corpus found (need a checkout containing docs/src/adr)." >&2
  echo "  Knobs: PLAYBOOK_DIR or COMO_PLAYBOOK_DIR (a playbook checkout; sibling ../playbook is" >&2
  echo "  the default), or COMO_PLAYBOOK_GIT / COMO_GIT_BASE for the .como/deps/playbook clone cache." >&2
  echo "  Note: playbook has NO public remote yet — the clone leg needs a file:// base" >&2
  echo "  (e.g. COMO_GIT_BASE=file:///path/to/local/mirrors) until the owner pushes it." >&2
  exit 1
fi
ADRS="$(cd "$PLAYBOOK_DIR/docs/src/adr" && pwd)"

# Forge repo name the demo seeds + drives. Knob (default "playbook") so the
# machinery can target a non-playbook corpus repo without hand-editing the
# generated conduit.toml — the third sighting of the hardcoded-target lesson
# (gitea-init.sh / demo-trigger.sh already take REPO_NAME; this seam was missed).
REPO_NAME="${REPO_NAME:-playbook}"

if [ -e "$RUN_DIR" ]; then
  echo "ERROR: RUN_DIR=$RUN_DIR already exists — each run gets a fresh workdir" >&2
  exit 1
fi
mkdir -p "$RUN_DIR/.conduit"

sed -e "s|@ADROIT_DIR@|$ADRS|" -e "s|@REPO_NAME@|$REPO_NAME|" \
  demo/playbook.conduit.toml >"$RUN_DIR/conduit.toml"
# Symlinks, not copies: tokens stay in one gitignored place (and may be
# re-minted by a later forge-up); the pinned adroit is shared read-only.
ln -s "$ROOT/.secrets" "$RUN_DIR/.secrets"
ln -s "$ROOT/.conduit/bin" "$RUN_DIR/.conduit/bin"

ABS="$(cd "$RUN_DIR" && pwd)"
echo "demo workdir ready: $ABS (adroit corpus: $ADRS)" >&2
echo "$ABS"
