# demo/kit/lib.sh — shared helpers for the customer demo kit.
#
# Sourced (never executed) by demo-up, demo-down, and every beat-* script.
# Design rules the kit lives by (ADR-0015):
#   - kit-owns-no-state: all mutable run state lives in the per-up workdir
#     under gitignored demo/runs/; the only kit-side file is the gitignored
#     .current pointer. Sibling repos are never written to.
#   - pre-baked/live split: every AI lane ships a pre-authored artifact in
#     prebaked/ (fast path), with --live recomputing it on local ollama.
#   - evidence-per-beat: each beat ends with machine output it just
#     produced (exit codes, shas, jq extracts) — never narration alone.
#
# Cross-repo references resolve per the suite convention (ADR-0014):
# env override -> sibling ../<repo> -> gitignored .como/deps clone cache
# (COMO_<REPO>_GIT / COMO_GIT_BASE) -> skip-with-notice / actionable error.

KIT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$KIT_DIR/../.." && pwd)" # the conduit repo root
CONDUIT="$ROOT/target/debug/conduit"
ADROIT="$ROOT/.conduit/bin/adroit"
CURRENT_FILE="$KIT_DIR/.current" # gitignored pointer to the active workdir
FORGE_URL="http://localhost:3000"
FORGE_API="$FORGE_URL/api/v1"

# ── Presentation ───────────────────────────────────────────────────────────

RULE="────────────────────────────────────────────────────────────────────"

banner() { # banner "BEAT 2 — ASSESS" "pre-baked fast path"
    echo "$RULE"
    echo " $1${2:+  ($2)}"
    echo "$RULE"
}

talking_point() { # talking_point <<'EOF' ... EOF — what the presenter SAYS
    echo " TALKING POINT:"
    sed 's/^/   /'
    echo
}

evidence() { # evidence "label" — header before machine output
    echo
    echo " EVIDENCE — $1:"
}

note() { echo "kit: $*" >&2; }

die() {
    echo "kit: ERROR — $*" >&2
    exit 1
}

# ── Timing ─────────────────────────────────────────────────────────────────
#
# MONOTONIC, deliberately (iteration-3 A3; rehearsal-2 wart): on WSL2 the
# realtime clock can be stepped by host clock-sync mid-demo — rehearsal 2
# recorded the kit's `date +%s` WALL-CLOCK lines ~23s short of the
# assessments binary's own monotonic elapsed marks in beat 2. The binaries
# all time monotonically, so the kit must too: /proc/uptime is the kernel's
# monotonic-since-boot clock (immune to steps), and the narration never
# shows two different numbers for the same run again. The label stays
# WALL-CLOCK — it is still elapsed real time, just measured on a clock that
# cannot jump. (Fallback: date +%s where /proc/uptime is unreadable.)

BEAT_T0=""
monotonic_secs() {
    if [ -r /proc/uptime ]; then
        awk '{print int($1)}' /proc/uptime
    else
        date +%s
    fi
}
beat_start() { BEAT_T0=$(monotonic_secs); }
beat_end() { # beat_end "beat-2-assess (pre-baked)"
    local secs=$(($(monotonic_secs) - BEAT_T0))
    echo
    echo " WALL-CLOCK: $1 took ${secs}s"
}

# ── Suite resolution (ADR-0014) ────────────────────────────────────────────

# resolve_repo <name> — echo the checkout dir (empty if unresolved) on
# stdout; provenance notes on stderr. Chain: COMO_<NAME>_DIR -> sibling
# ../<name> -> populated .como/deps/<name> cache (used as-is, never
# auto-fetched) -> clone from COMO_<NAME>_GIT / COMO_GIT_BASE ->
# empty (the caller decides skip-with-notice vs hard error).
resolve_repo() {
    local name="$1"
    local var
    var="COMO_$(echo "$name" | tr '[:lower:]-' '[:upper:]_')_DIR"
    local envdir="${!var:-}"
    if [ -n "$envdir" ]; then
        [ -d "$envdir" ] || die "$var=$envdir does not exist"
        note "$name -> $envdir (env $var)"
        echo "$envdir"
        return
    fi
    if [ -d "$ROOT/../$name/.git" ]; then
        note "$name -> ../$name (sibling checkout)"
        (cd "$ROOT/../$name" && pwd)
        return
    fi
    local cache="$ROOT/.como/deps/$name"
    if [ -d "$cache/.git" ]; then
        note "$name -> $cache (clone cache, used as-is)"
        echo "$cache"
        return
    fi
    if [ "${COMO_OFFLINE:-0}" != "1" ]; then
        local gitvar
        gitvar="COMO_$(echo "$name" | tr '[:lower:]-' '[:upper:]_')_GIT"
        local url="${!gitvar:-${COMO_GIT_BASE:-https://github.com/como-technologies}/$name.git}"
        mkdir -p "$ROOT/.como/deps"
        if git clone --quiet --filter=blob:none "$url" "$cache" 2>/dev/null; then
            note "$name -> $cache (cloned from $url)"
            echo "$cache"
            return
        fi
        note "NOTICE — $name unresolved: no $var, no sibling ../$name, and the clone of $url failed (the owner may not have published it yet)"
    else
        note "NOTICE — $name unresolved: COMO_OFFLINE=1 and no $var / sibling / cache"
    fi
    echo ""
}

require_repo() { # require_repo <name> — resolve or die with the knobs named
    local dir
    dir="$(resolve_repo "$1")"
    [ -n "$dir" ] || die "$1 is required for this beat. Provide COMO_$(echo "$1" | tr '[:lower:]-' '[:upper:]_')_DIR, a sibling ../$1 checkout, or COMO_GIT_BASE for the clone cache."
    echo "$dir"
}

# ── Workdir / forge state ──────────────────────────────────────────────────

workdir() { # the active per-up workdir, or die pointing at demo-up
    [ -f "$CURRENT_FILE" ] || die "no active demo — run demo/kit/demo-up first"
    local dir
    dir="$(cat "$CURRENT_FILE")"
    [ -d "$dir" ] || die "stale pointer $CURRENT_FILE -> $dir — run demo/kit/demo-up"
    echo "$dir"
}

# ── Docker (the throwaway forge runs in it) ──────────────────────────────────

docker_ready() { docker info >/dev/null 2>&1; }

# Fail fast, with guidance, BEFORE the expensive cross-repo builds — the forge
# is step 4 of demo-up, so a down daemon must not waste minutes of cargo first.
require_docker() {
    command -v docker >/dev/null 2>&1 ||
        die "docker is required for the demo forge but is not installed — run demo/kit/preflight for guidance"
    docker compose version >/dev/null 2>&1 ||
        die "the 'docker compose' plugin is required but unavailable — run demo/kit/preflight"
    docker_ready ||
        die "the docker daemon is not running — start it (sudo systemctl start docker, or Docker Desktop), then re-run (demo/kit/preflight verifies)"
}

forge_healthy() { curl -fsS "$FORGE_URL/api/healthz" >/dev/null 2>&1; }

forge_repo_exists() { curl -fsS "$FORGE_API/repos/como/playbook" >/dev/null 2>&1; }

require_forge() {
    forge_healthy && forge_repo_exists ||
        die "the demo forge is not up (or como/playbook is missing) — run demo/kit/demo-up"
}

reviewer_token() { cat "$ROOT/.secrets/reviewer.token"; }

# curl the forge API as the reviewer (the scripted human)
reviewer_api() { # reviewer_api METHOD PATH [JSON]
    local method="$1" path="$2" data="${3:-}"
    local args=(-fsS -X "$method" -H "Authorization: token $(reviewer_token)")
    [ -n "$data" ] && args+=(-H "Content-Type: application/json" -d "$data")
    curl "${args[@]}" "$FORGE_API$path"
}

# ── ollama (the live lanes) ────────────────────────────────────────────────

OLLAMA_MODEL="${OLLAMA_MODEL:-llama3.2}"

ollama_ready() {
    command -v ollama >/dev/null 2>&1 && ollama list 2>/dev/null | grep -q "$OLLAMA_MODEL"
}

require_ollama() {
    ollama_ready || die "--live needs ollama serving '$OLLAMA_MODEL' locally (ollama pull $OLLAMA_MODEL); the pre-baked fast path needs nothing"
}
