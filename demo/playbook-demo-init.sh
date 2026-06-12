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
#   PLAYBOOK_DIR  playbook checkout (default ../playbook; must contain
#                 src/adrs)
# Prints the created workdir's absolute path on stdout; everything else
# goes to stderr.
set -euo pipefail

cd "$(dirname "$0")/.." # conduit repo root
ROOT="$PWD"
RUN_DIR="${RUN_DIR:-demo/runs/$(date -u +%Y%m%dT%H%M%SZ)}"
PLAYBOOK_DIR="${PLAYBOOK_DIR:-../playbook}"

if [ ! -d "$PLAYBOOK_DIR/src/adrs" ]; then
  echo "ERROR: PLAYBOOK_DIR=$PLAYBOOK_DIR has no src/adrs corpus" >&2
  exit 1
fi
ADRS="$(cd "$PLAYBOOK_DIR/src/adrs" && pwd)"

if [ -e "$RUN_DIR" ]; then
  echo "ERROR: RUN_DIR=$RUN_DIR already exists — each run gets a fresh workdir" >&2
  exit 1
fi
mkdir -p "$RUN_DIR/.conduit"

sed "s|@ADROIT_DIR@|$ADRS|" demo/playbook.conduit.toml >"$RUN_DIR/conduit.toml"
# Symlinks, not copies: tokens stay in one gitignored place (and may be
# re-minted by a later forge-up); the pinned adroit is shared read-only.
ln -s "$ROOT/.secrets" "$RUN_DIR/.secrets"
ln -s "$ROOT/.conduit/bin" "$RUN_DIR/.conduit/bin"

ABS="$(cd "$RUN_DIR" && pwd)"
echo "demo workdir ready: $ABS (adroit corpus: $ADRS)" >&2
echo "$ABS"
