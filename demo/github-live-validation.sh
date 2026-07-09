#!/usr/bin/env bash
# github-live-validation — the OWNER-RUN, one-time mutation validation that
# ADR-0012 requires before any decision to lift DryRun on GitHub may be
# proposed. Exercises conduit's exact mutation payload shapes (from
# src/forge/github.rs) against a SACRIFICIAL PRIVATE repo, records every
# response, and cleans up after itself.
#
#   GITHUB_VALIDATION_REPO=<owner>/<sacrificial-private-repo> \
#     demo/github-live-validation.sh
#
# Auth is the caller's own `gh` login — this script is meaningful only when
# the owner personally runs it (ADR-0012: "the gate lifts only when the
# owner personally runs the one-time validation ... and records the result").
# Nothing in CI, the demo kit, or the engine ever calls this.
set -uo pipefail
cd "$(dirname "$0")/.."

REPO="${GITHUB_VALIDATION_REPO:?set GITHUB_VALIDATION_REPO=owner/sacrificial-private-repo}"

# Refuse the real suite repos outright — sacrificial means sacrificial.
case "$REPO" in
*/adroit|*/assessments|*/conduit|*/tuesday|*/pulse|*/portfolio|*/playbook|*/recipes|*/spectro-recipes)
    echo "REFUSED: $REPO is a real suite repo, not a sacrificial one" >&2; exit 2 ;;
esac

vis="$(gh api "repos/$REPO" --jq .visibility 2>/dev/null)" || {
    echo "cannot read repos/$REPO — check gh auth and the repo name" >&2; exit 2; }
[ "$vis" = "private" ] || {
    echo "REFUSED: $REPO is '$vis' — the validation repo must be PRIVATE" >&2; exit 2; }

TS="$(date -u +%Y%m%dT%H%M%SZ)"
BR="conduit-validation-$TS"
OUT="demo/runs/github-validation-$TS.jsonl"
mkdir -p demo/runs
PR_NUM=""

record() { # record <step> <method> <path> <status> <ok>
    printf '{"step":"%s","method":"%s","path":"%s","http_status":%s,"ok":%s}\n' \
        "$1" "$2" "$3" "$4" "$5" | tee -a "$OUT"
}

step() { # step <name> <method> <path> <json-body-or-empty>
    local name="$1" method="$2" path="$3" body="${4:-}"
    local args=(-X "$method" "repos/$REPO/$path" -H "Accept: application/vnd.github+json")
    [ -n "$body" ] && args+=(--input -)
    local status ok=false
    if [ -n "$body" ]; then
        resp="$(printf '%s' "$body" | gh api "${args[@]}" --include 2>&1)" && ok=true
    else
        resp="$(gh api "${args[@]}" --include 2>&1)" && ok=true
    fi
    status="$(printf '%s' "$resp" | grep -oE '^HTTP/[0-9.]+ [0-9]+' | head -1 | awk '{print $2}')"
    [ -n "$status" ] || status=0
    record "$name" "$method" "$path" "$status" "$ok"
    printf '%s' "$resp"
}

echo "== github-live-validation against $REPO (branch $BR) — results: $OUT"

# Prerequisites (not among the four recorded mutations, but the real flow
# needs them): the label conduit ensures, a branch with one commit to PR.
sha="$(gh api "repos/$REPO/git/refs/heads/main" --jq .object.sha 2>/dev/null)" \
    || sha="$(gh api "repos/$REPO/git/refs/heads/master" --jq .object.sha)"
step "prereq-ensure-label" POST "labels" \
    '{"name":"conduit:validation","color":"1d76db","description":"ADR-0012 one-time validation"}' >/dev/null || true
step "prereq-create-branch" POST "git/refs" \
    "{\"ref\":\"refs/heads/$BR\",\"sha\":\"$sha\"}" >/dev/null
step "prereq-branch-commit" PUT "contents/conduit-validation-$TS.md" \
    "{\"message\":\"conduit validation probe $TS\",\"content\":\"$(printf 'ADR-0012 one-time mutation validation, %s\n' "$TS" | base64 -w0)\",\"branch\":\"$BR\"}" >/dev/null

# The four recorded mutations, with src/forge/github.rs's exact body shapes.
pr_resp="$(step "1-create-pr" POST "pulls" \
    "{\"title\":\"conduit validation $TS\",\"body\":\"ADR-0012 one-time mutation validation. Close and delete freely.\",\"head\":\"$BR\",\"base\":\"main\"}")"
PR_NUM="$(printf '%s' "$pr_resp" | grep -oE '"number": ?[0-9]+' | head -1 | grep -oE '[0-9]+')"
[ -n "$PR_NUM" ] || { echo "create-pr returned no number — see $OUT" >&2; exit 1; }
echo "   PR number: $PR_NUM"

step "2-set-labels" PUT "issues/$PR_NUM/labels" \
    '{"labels":["conduit:validation"]}' >/dev/null

step "3-close-pr" PATCH "issues/$PR_NUM" \
    '{"state":"closed"}' >/dev/null

step "4-delete-branch" DELETE "git/refs/heads/$BR" '' >/dev/null

echo
fails="$(grep -c '"ok":false' "$OUT" || true)"
if [ "$fails" = 0 ]; then
    echo "VALIDATION PASS — all mutations accepted by GitHub. Record: $OUT"
    echo "Per ADR-0012, a decision to lift DryRun may now be PROPOSED (superseding it)."
else
    echo "VALIDATION FAIL — $fails step(s) rejected. Record: $OUT" >&2
    exit 1
fi
