# Demo walkthrough

The spike's acceptance run, validated end-to-end against the throwaway forge
on 2026-06-12. Every output below is real, captured from that run. The
sequence: conduit reads its **own** accepted ADRs (the in-repo `adr/` corpus,
authored with the pinned adroit) and drives work on its **own** repo via a
disposable localhost Gitea — the portfolio feeding itself. Humans hold every
gate; here the human is scripted as the second Gitea user (`reviewer`).

Prerequisites: docker, `just init && just init-adroit`, and ollama serving
`llama3.2` locally (ADR-0002 carries a *stored* plan that `conduit plan`
reads back deterministically; the other ADRs planned below generate fresh).
`conduit` in the listings below is the built binary — run it as
`cargo run --` or `./target/debug/conduit`.

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

Six accepted decisions — the corpus authored with the pinned adroit
(captured, trimmed to the contract fields):

```json
[
  {"reference": "ADR-0001", "address": "1", "title": "Rust single crate, fully synchronous",            "status": "Accepted"},
  {"reference": "ADR-0002", "address": "2", "title": "Snapshot-diff event router, polling not webhooks", "status": "Accepted"},
  {"reference": "ADR-0003", "address": "3", "title": "Filesystem store with write-ahead intents",        "status": "Accepted"},
  {"reference": "ADR-0004", "address": "4", "title": "Structural engine sandbox",                        "status": "Accepted"},
  {"reference": "ADR-0005", "address": "5", "title": "Effort labels from cumulative wall-clock",         "status": "Accepted"},
  {"reference": "ADR-0006", "address": "6", "title": "MCP exposure of adroit deferred",                  "status": "Accepted"}
]
```

## 3. Plan an accepted ADR

```sh
conduit plan 2 --forge gitea
```

`plan` runs the adroit handshake, `show 2`, enforces **Accepted** itself,
calls `adroit plan 2` — which returns the plan **stored** in ADR-0002, a
deterministic provider-free read — persists the snapshot verbatim
(sha256 onto the record), and opens the issue on Gitea with the plan as body
and the `adr:ADR-0002` label. The task is `Scoped`.

Captured:

```text
plan for ADR-0002: stored plan (deterministic read from the ADR document)
conduit: auto-creating missing label "adr:ADR-0002" (neutral grey) — add it to ensure_labels if this is a standing label
planned ADR-0002 as task adr-0002 — issue 1 on gitea como/conduit-dogfood at http://localhost:3000: label it conduit:run to start
```

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

Captured — one tick took the task Scoped → Coding → InReview (the engine
runs synchronously inside the tick; Coding persists only as a crash state):

```text
$ conduit run --forge gitea --once
conduit run: single tick via gitea como/conduit-dogfood at http://localhost:3000 (engine: fake (complete))
$ conduit status
id        state     attempt   branch
adr-0002  InReview  1         conduit/adr-0002/snapshot-diff-event-router-polling-not-w
```

The PR as the forge holds it (number 2 — issues and PRs share Gitea's index
space):

```text
2 [ADR-0002] Snapshot-diff event router, polling not webhooks ['adr:ADR-0002', 'effort:1-super-quick'] conduit/adr-0002/snapshot-diff-event-router-polling-not-w
```

## 6. Review round: Request changes → Revising → InReview

The reviewer requests changes through the API (in real life: the Gitea UI):

```sh
TOK=$(cat .secrets/reviewer.token)
API=http://localhost:3000/api/v1/repos/como/conduit-dogfood
curl -sS -X POST -H "Authorization: token $TOK" -H "Content-Type: application/json" \
  -d '{"event":"REQUEST_CHANGES","body":"Please tighten the implementation notes."}' \
  "$API/pulls/2/reviews"

conduit run --forge gitea --once   # ReviewSubmitted -> Revising -> InReview
```

The engine re-runs on the same branch with the review feedback in its spec;
conduit pushes and recomputes the effort label (swap — still exactly one).

## 7. Approve, merge: InReview → Merged

```sh
curl -sS -X POST -H "Authorization: token $TOK" -H "Content-Type: application/json" \
  -d '{"event":"APPROVED","body":"LGTM"}' "$API/pulls/2/reviews"
curl -sS -X POST -H "Authorization: token $TOK" -H "Content-Type: application/json" \
  -d '{"Do":"merge"}' "$API/pulls/2/merge"           # merge HTTP 200

conduit run --forge gitea --once   # observes PrMerged -> Merged (terminal)
```

Conduit observes `PrMerged`, closes the issue with the completion comment
carrying the merge sha. The whole lifecycle is inspectable as files:

```sh
cat .conduit/tasks/adr-0002.json
```

Captured — the final record, verbatim (the close comment carries the merge
sha; every write-ahead intent marked done):

```json
{
  "id": "adr-0002",
  "adr_reference": "ADR-0002",
  "adr_address": "2",
  "title": "Snapshot-diff event router, polling not webhooks",
  "state": "Merged",
  "branch": "conduit/adr-0002/snapshot-diff-event-router-polling-not-w",
  "issue": 1,
  "pr": 2,
  "attempt": 1,
  "work_ms": 0,
  "plan_sha256": "09c825920cd7d48fa18ebf9b5c376627da2d0849512849e7d1558aeb6c34dbef",
  "review_feedback": [],
  "pending": [
    {
      "action": {
        "CloseIssue": {
          "comment": "Merged as efe73ffac0c09f37daf8a2537f54ae1b3190e9ea.\n\n<!-- conduit:task:adr-0002 -->"
        }
      },
      "done": true
    }
  ]
}
```

## 8. The closing beat: verify

```sh
conduit verify 2 --forge gitea -o json
```

Re-reads the merged PR live from the forge and machine-asserts every element
of the tagging contract — the executable spec the Measure-stage consumer is
built against:

Captured — ALL SIX CHECKS PASS, exit 0:

```json
{
  "checks": [
    {
      "detail": "title \"[ADR-0002] Snapshot-diff event router, polling not webhooks\" (want ^\\[ADR-dddd\\] )",
      "name": "title_prefix",
      "pass": true
    },
    {
      "detail": "final body line \"Adr-Reference: ADR-0002\" (want \"Adr-Reference: ADR-0002\")",
      "name": "trailer_final_line",
      "pass": true
    },
    {
      "detail": "effort labels [\"effort:1-super-quick\"] (want exactly one from the closed set)",
      "name": "exactly_one_effort_label",
      "pass": true
    },
    {
      "detail": "labels [\"adr:ADR-0002\", \"effort:1-super-quick\"] (want \"adr:ADR-0002\")",
      "name": "adr_label_present",
      "pass": true
    },
    {
      "detail": "head branch \"conduit/adr-0002/snapshot-diff-event-router-polling-not-w\" (want conduit/adr-dddd/<slug>)",
      "name": "branch_shape",
      "pass": true
    },
    {
      "detail": "head branch \"conduit/adr-0002/snapshot-diff-event-router-polling-not-w\" (must never start adr/)",
      "name": "never_adr_namespace",
      "pass": true
    }
  ],
  "pass": true,
  "pr": 2,
  "task": "adr-0002"
}
```

The PR's labels on the forge, verbatim:

```json
{
  "number": 2,
  "title": "[ADR-0002] Snapshot-diff event router, polling not webhooks",
  "labels": ["adr:ADR-0002", "effort:1-super-quick"],
  "merged": true,
  "merge_commit_sha": "efe73ffac0c09f37daf8a2537f54ae1b3190e9ea",
  "head": "conduit/adr-0002/snapshot-diff-event-router-polling-not-w"
}
```

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

Captured — the diff is empty:

```text
$ diff /tmp/t-gitea.jsonl /tmp/t-github.jsonl && echo "FORGE-NEUTRAL: identical"
FORGE-NEUTRAL: identical
```

The identical 7-line normalized stream (both legs, byte for byte —
`create_issue`, `open_pr`, `set_pr_labels`, link comment, effort recompute,
close comment, `close_issue`; ids are first-seen placeholders, effort values
redacted because they derive from wall-clock):

```json
{"action":"create_issue","body":"# Plan: forge-neutrality transcript for ADR-0002\n\n1. Fixture events drive the real state machine (no polling).\n2. Every resulting forge action is emitted through the chosen adapter.\n3. The two normalized streams must be byte-identical.\n\n<!-- conduit:task:adr-0002-transcript -->","labels":["adr:ADR-0002"],"title":"[ADR-0002] Forge neutrality transcript"}
{"action":"open_pr","base":"main","body":"# Plan: forge-neutrality transcript for ADR-0002\n\n1. Fixture events drive the real state machine (no polling).\n2. Every resulting forge action is emitted through the chosen adapter.\n3. The two normalized streams must be byte-identical.\n\nAdr-Reference: ADR-0002","head":"conduit/adr-0002/forge-neutrality-transcript","labels":["adr:ADR-0002","effort:$REDACTED"],"title":"[ADR-0002] Forge neutrality transcript"}
{"action":"set_pr_labels","labels":["effort:$REDACTED","adr:ADR-0002"],"pr":"$PR_1"}
{"action":"upsert_issue_comment","body":"Opened PR $PR_1 for ADR-0002: Forge neutrality transcript.\n\n<!-- conduit:task:adr-0002-transcript -->","issue":"$ISSUE_1","marker":"<!-- conduit:task:adr-0002-transcript -->"}
{"action":"set_pr_labels","labels":["effort:$REDACTED","adr:ADR-0002"],"pr":"$PR_1"}
{"action":"upsert_issue_comment","body":"Merged as cafe42cafe42cafe42cafe42cafe42cafe42cafe.\n\n<!-- conduit:task:adr-0002-transcript -->","issue":"$ISSUE_1","marker":"<!-- conduit:task:adr-0002-transcript -->"}
{"action":"close_issue","issue":"$ISSUE_1"}
```

## 10. The restart beat

Kill conduit mid-Coding; rerun; no duplicate issue, no duplicate PR:

```sh
conduit plan 1 --forge gitea           # fresh ollama generation this time
just demo-trigger
CONDUIT_FAKE_ENGINE_MODE=hang:300 conduit run --forge gitea --once &
# wait for the tick to enter Coding, then kill the process group hard:
kill -9 <conduit-pid>
conduit run --forge gitea --once       # fresh workspace, resumes from the snapshot
```

Captured — the crash state persisted exactly as designed (write-ahead intent
undone), and the rerun converged with no duplicates:

```text
$ conduit plan 1 --forge gitea
plan for ADR-0001: freshly generated (nondeterministic; snapshot is now canonical)
planned ADR-0001 as task adr-0001 — issue 5 on gitea ...: label it conduit:run to start

# killed -9 conduit (pid 3392539) mid-Coding; the record on disk:
state: Coding | pending: [('RunEngine', False)]

$ conduit run --forge gitea --once     # recovery
$ conduit status
adr-0002  Merged    1  conduit/adr-0002/snapshot-diff-event-router-polling-not-w
adr-0001  InReview  1  conduit/adr-0001/rust-single-crate-fully-synchronous

# duplicate audit against the live forge:
issues with marker <!-- conduit:task:adr-0001 -->: [5]
PRs with head conduit/adr-0001/*: [(6, 'conduit/adr-0001/rust-single-crate-fully-synchronous')]
```

Exactly one issue, exactly one PR: the undone `RunEngine` intent re-executed
in a fresh workspace from the immutable plan snapshot, and the open-PR /
marker probes absorbed the replayed `IssueLabeled` event (the cursor had not
advanced past the killed tick).

## 11. Encore: the real engine

```sh
conduit plan 6 --forge gitea && just demo-trigger
CONDUIT_ENGINE=claude-code conduit run --forge gitea --once
```

Captured — the run took ~6.3 minutes of engine wall-clock and reached
InReview with a real, coherent PR (three files: an implementation artifact,
a new book page recording the deferral boundary, and the SUMMARY wiring):

```text
conduit run: single tick via gitea como/conduit-dogfood at http://localhost:3000 (engine: claude-code (claude, timeout 900s))

$ conduit status
adr-0006  InReview  1  conduit/adr-0006/mcp-exposure-of-adroit-deferred

$ python3 - < .conduit/tasks/adr-0006.json   # work_ms feeds the effort bucket
{"id": "adr-0006", "state": "InReview", "branch": "conduit/adr-0006/mcp-exposure-of-adroit-deferred",
 "issue": 7, "pr": 8, "attempt": 1, "work_ms": 375233}

# PR 8 on the forge:
8 [ADR-0006] MCP exposure of adroit deferred ['adr:ADR-0006', 'effort:1-super-quick']
# diff: 3 files — docs/impl/adr-0006.md, docs/src/dev/mcp-deferral.md, docs/src/SUMMARY.md
```

Honest notes: the plan it implemented was a fresh llama3.2 generation
(nondeterministic — snapshot persisted verbatim before the gate, as always),
and the engine interpreted the "defer, don't build" decision correctly,
recording the boundary as documentation instead of wiring an MCP server. The
sandbox held: the subprocess saw a credential-free `origin` and a scrubbed
environment; conduit committed and pushed.

## 12. Forge down

```sh
just forge-down                # container + volume destroyed; nothing left localhost
```
