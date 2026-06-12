# Demo walkthrough

The spike's acceptance run, validated end-to-end against the throwaway forge
on 2026-06-12. Every output below is real, captured from that run. The
sequence: conduit reads its **own** accepted ADRs (the in-repo `adr/` corpus,
authored with the pinned adroit) and drives work on its **own** repo via a
disposable localhost Gitea — the portfolio feeding itself. Humans hold every
gate; here the human is scripted as the second Gitea user (`reviewer`).

Prerequisites: docker, `just init && just init-adroit`, and ollama serving
`llama3.2` locally (used once — ADR-0002 carries a *stored* plan, which
`conduit plan` reads back deterministically).

## 1. Forge up

```sh
just forge-up
```

Starts the `conduit-gitea` container on `localhost:3000`, provisions two
users (`conduit-bot` = the actor, `reviewer` = the human gate — Gitea blocks
self-approval), mints tokens into gitignored `.secrets/`, creates org `como`
and repo `conduit-dogfood` seeded from this repo, and pre-creates the
`effort:*` / `conduit:*` labels.

```sh
conduit init                  # .conduit store + the standing label set
```

## 2. The dogfood input

```sh
ADROIT_DIR=adr .conduit/bin/adroit list --status accepted -o json
```

<!-- EVIDENCE:adr-list -->

## 3. Plan an accepted ADR

```sh
conduit plan 2 --forge gitea
```

`plan` runs the adroit handshake, `show 2`, enforces **Accepted** itself,
calls `adroit plan 2` — which returns the plan **stored** in ADR-0002, a
deterministic provider-free read — persists the snapshot verbatim
(sha256 onto the record), and opens the issue on Gitea with the plan as body
and the `adr:ADR-0002` label. The task is `Scoped`.

<!-- EVIDENCE:plan -->

## 4. The human gate, scripted

```sh
just demo-trigger             # reviewer labels the issue conduit:run
```

## 5. Run: Scoped → Coding → InReview

```sh
conduit run --forge gitea --once
```

The tick diffs the snapshot against the cursor, sees `IssueLabeled
conduit:run`, and drives Coding: clone from the local bare cache, branch,
FakeEngine writes its deterministic artifact, conduit commits and pushes,
opens the PR with full tagging, applies labels, links it on the issue.

<!-- EVIDENCE:inreview -->

## 6. Review round: Request changes → Revising → InReview

The reviewer requests changes through the API (in real life: the Gitea UI):

```sh
TOK=$(cat .secrets/reviewer.token)
API=http://localhost:3000/api/v1/repos/como/conduit-dogfood
curl -sS -X POST -H "Authorization: token $TOK" -H "Content-Type: application/json" \
  -d '{"event":"REQUEST_CHANGES","body":"Please tighten the implementation notes."}' \
  "$API/pulls/1/reviews"

conduit run --forge gitea --once   # ReviewSubmitted -> Revising -> InReview
```

The engine re-runs on the same branch with the review feedback in its spec;
conduit pushes and recomputes the effort label (swap — still exactly one).

## 7. Approve, merge: InReview → Merged

```sh
curl -sS -X POST -H "Authorization: token $TOK" -H "Content-Type: application/json" \
  -d '{"event":"APPROVED","body":"LGTM"}' "$API/pulls/1/reviews"
curl -sS -X POST -H "Authorization: token $TOK" -H "Content-Type: application/json" \
  -d '{"Do":"merge"}' "$API/pulls/1/merge"

conduit run --forge gitea --once   # observes PrMerged -> Merged (terminal)
```

Conduit observes `PrMerged`, closes the issue with the completion comment
carrying the merge sha. The whole lifecycle is inspectable as files:

```sh
cat .conduit/tasks/adr-0002.json
```

<!-- EVIDENCE:merged -->

## 8. The closing beat: verify

```sh
conduit verify 2 --forge gitea -o json
```

Re-reads the merged PR live from the forge and machine-asserts every element
of the tagging contract — the executable spec the Measure-stage consumer is
built against:

<!-- EVIDENCE:verify -->

The PR's labels on the forge, verbatim:

<!-- EVIDENCE:labels -->

## 9. The forge-neutrality money shot

```sh
conduit demo-transcript 2 --forge gitea  > /tmp/t-gitea.jsonl
conduit demo-transcript 2 --forge github > /tmp/t-github.jsonl
diff /tmp/t-gitea.jsonl /tmp/t-github.jsonl && echo "FORGE-NEUTRAL: identical"
```

Both legs feed the same fixture event sequence through the real state
machine. The gitea leg **executes** (live adapter, real git push against the
throwaway forge); the github leg is `DryRun(GitHubForge)` — record-only by
construction. Identical normalized output proves the action side; the read
side is the conformance suite's job.

<!-- EVIDENCE:diff -->

## 10. The restart beat

Kill conduit mid-Coding; rerun; no duplicate issue, no duplicate PR:

```sh
conduit plan 1 --forge gitea && just demo-trigger
CONDUIT_FAKE_ENGINE_MODE=hang:300 conduit run --forge gitea --once &
sleep 8 && kill -9 $!                  # task persisted as Coding, engine dead
conduit run --forge gitea --once       # fresh workspace, resumes from the snapshot
```

<!-- EVIDENCE:restart -->

## 11. Encore: the real engine

```sh
conduit plan 6 --forge gitea && just demo-trigger
CONDUIT_ENGINE=claude-code conduit run --forge gitea --once
```

<!-- EVIDENCE:encore -->

## 12. Forge down

```sh
just forge-down                # container + volume destroyed; nothing left localhost
```
