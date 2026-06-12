#!/usr/bin/env bash
# Bootstrap the throwaway demo Gitea (spec §Self-dogfood): two users
# (conduit-bot = the actor; reviewer = the human gate — Gitea blocks
# self-approval, hence two users), org como, repo conduit-dogfood seeded by
# pushing THIS repo, and the tuesday-contract labels. Idempotent — safe to
# re-run. Tokens land in gitignored .secrets/ (chmod 600). Nothing here ever
# leaves localhost; the push below is the one sanctioned push target.
set -euo pipefail

cd "$(dirname "$0")/.." # repo root
COMPOSE=(docker compose -f demo/docker-compose.yml)
BASE="http://localhost:3000"
API="$BASE/api/v1"

# 1. Wait for the container to answer (60s budget).
echo "waiting for gitea at $BASE ..."
for i in $(seq 1 60); do
  if curl -fsS "$BASE/api/healthz" >/dev/null 2>&1; then
    break
  fi
  if [ "$i" -eq 60 ]; then
    echo "ERROR: gitea did not become healthy within 60s" >&2
    exit 1
  fi
  sleep 1
done
echo "gitea is up"

# 2. Two users via the container admin CLI ("already exists" is fine).
create_user() {
  local name="$1"
  shift
  local out
  if out=$("${COMPOSE[@]}" exec -T -u git gitea gitea admin user create \
    --username "$name" --password "$(openssl rand -hex 16)" \
    --email "$name@localhost" --must-change-password=false "$@" 2>&1); then
    echo "created user $name"
  elif echo "$out" | grep -qi "already exists"; then
    echo "user $name already exists"
  else
    echo "$out" >&2
    exit 1
  fi
}
create_user conduit-bot --admin
create_user reviewer

# 3. Tokens -> .secrets/<user>.token (pinned filenames; gitignored).
mkdir -p .secrets
mint_token() {
  local name="$1" file=".secrets/$1.token"
  # Keep an existing token only if it still authenticates — the container
  # volume may have been destroyed (forge-down) since it was minted.
  if [ -s "$file" ] &&
    curl -fsS -H "Authorization: token $(cat "$file")" "$API/user" >/dev/null 2>&1; then
    echo "token for $name still valid, keeping $file"
    return
  fi
  # Token names are unique per user — suffix with a timestamp so re-minting
  # never collides with a previous run's token.
  "${COMPOSE[@]}" exec -T -u git gitea gitea admin user generate-access-token \
    --username "$name" --token-name "conduit-$(date +%s)" --scopes all --raw |
    tr -d '[:space:]' >"$file"
  chmod 600 "$file"
  echo "minted token for $name -> $file"
}
mint_token conduit-bot
mint_token reviewer

TOK="$(cat .secrets/conduit-bot.token)"

# api METHOD PATH JSON OK_CODE... — curl as conduit-bot; any listed code
# (e.g. the "already exists" 409/422) counts as success.
api() {
  local method="$1" path="$2" data="$3"
  shift 3
  local args=(-sS -o /tmp/conduit-gitea-init-resp.json -w '%{http_code}'
    -X "$method" -H "Authorization: token $TOK" -H "Content-Type: application/json")
  if [ -n "$data" ]; then
    args+=(-d "$data")
  fi
  local code
  code=$(curl "${args[@]}" "$API$path")
  local ok
  for ok in "$@"; do
    if [ "$code" = "$ok" ]; then
      return 0
    fi
  done
  echo "ERROR: $method $path -> HTTP $code" >&2
  cat /tmp/conduit-gitea-init-resp.json >&2
  echo >&2
  return 1
}

# 4. Org + repo, as conduit-bot (422/409 = already there).
api POST /orgs '{"username":"como"}' 201 409 422
api POST /orgs/como/repos \
  '{"name":"conduit-dogfood","private":false,"default_branch":"main"}' 201 409

# 5. Seed by pushing THIS repo's main to the container — the one sanctioned
#    push target (throwaway, localhost-only; -f is fine here).
git push -f "http://conduit-bot:$TOK@localhost:3000/como/conduit-dogfood.git" main:main

# 6. The tuesday-contract labels (colors: bare hex, no '#').
label() {
  api POST /repos/como/conduit-dogfood/labels \
    "{\"name\":\"$1\",\"color\":\"$2\",\"description\":\"$3\"}" 201 409 422
}
label "effort:1-super-quick" "0e8a16" "under 10 minutes"
label "effort:2-not-long" "5319e7" "under 30 minutes"
label "effort:3-average" "fbca04" "under 2 hours"
label "effort:4-a-while" "d93f0b" "under 8 hours"
label "effort:5-felt-like-forever" "b60205" "8 hours or more"
label "conduit:run" "1d76db" "human trigger: start this task"
label "conduit:failed" "d73a4a" "engine failed; needs attention"

# 7. reviewer can review/approve.
api PUT /repos/como/conduit-dogfood/collaborators/reviewer \
  '{"permission":"write"}' 204

echo "forge ready: $BASE (org como, repo conduit-dogfood; tokens in .secrets/)"
