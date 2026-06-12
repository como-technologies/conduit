#!/usr/bin/env bash
# Scripted human gate (spec §Demo script): as `reviewer`, add the
# `conduit:run` label to the newest open issue on the throwaway forge.
# POST (not PUT) so the issue's adr:* label survives. Localhost only.
set -euo pipefail
cd "$(dirname "$0")/.."
TOK="$(cat .secrets/reviewer.token)"
API="http://localhost:3000/api/v1/repos/como/conduit-dogfood"
issue=$(curl -fsS -H "Authorization: token $TOK" "$API/issues?type=issues&state=open&limit=1" |
  grep -o '"number":[0-9]*' | head -1 | cut -d: -f2)
label_id=$(curl -fsS -H "Authorization: token $TOK" "$API/labels?limit=50" |
  tr '{' '\n' | grep '"name":"conduit:run"' | grep -o '"id":[0-9]*' | head -1 | cut -d: -f2)
curl -fsS -X POST -H "Authorization: token $TOK" -H "Content-Type: application/json" \
  -d "{\"labels\":[$label_id]}" "$API/issues/$issue/labels" >/dev/null
echo "labeled issue $issue with conduit:run (as reviewer)"
