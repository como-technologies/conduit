# State machine

Seven states, two terminal. Humans hold every gate: scope (`conduit:run`
label), review (request changes / approve), and merge (the forge UI — no
`Forge::merge` method exists). `machine::step(&TaskRecord, &Event) ->
Transition` is a pure function: zero I/O, exhaustive match, table-tested over
every (state, event, has-PR) cell including the must-ignore cells
(`tests/machine.rs`).

```
Scoped ──(IssueLabeled conduit:run — HUMAN)──▶ Coding
Coding ──(EngineFinished Completed)──▶ InReview     commit+push, open PR (full tagging), link comment
Coding ──(EngineFinished Failed/Timeout)──▶ Failed  failure comment w/ log tail, labels → conduit:failed
Failed ──(IssueLabeled conduit:run — HUMAN)──▶ Coding (attempt+1, fresh workspace)
InReview ──(ReviewSubmitted ChangesRequested — HUMAN)──▶ Revising   engine re-runs w/ feedback, same branch
Revising ──(EngineFinished Completed)──▶ InReview   commit+push, recompute effort label (swap, still exactly one)
Revising ──(EngineFinished Failed/Timeout)──▶ Failed
InReview ──(PrMerged — HUMAN merges in forge UI)──▶ Merged (terminal)   close issue, completion comment w/ sha
InReview ──(PrClosed without merge — HUMAN)──▶ Abandoned (terminal)    close issue w/ comment
```

## Events

Machine-level events (`machine::Event`) are the five forge events mapped by
the router plus the internal engine completion:

`IssueLabeled { label }` · `ReviewSubmitted { verdict, body }` · `CiChanged`
· `PrMerged { merge_sha }` · `PrClosed` · `EngineFinished(Completed | Failed)`

A `Transition` carries the next state, the `Action` list the router executes
(`RunEngine`, `CommitAndPush`, `OpenPr`, `ApplyPrLabels`, `LinkComment`,
`FailureComment`, `SetIssueLabels`, `CloseIssue`, `DisposeWorkspace`), a
`FeedbackOp` (keep / append the review body / clear on round completion) and
the `bump_attempt` flag (true only on Failed → Coding retry).

## The full transition table

Must-act cells, exactly as pinned by `tests/machine.rs`. Action kinds:
`run-fresh`/`run-same` = `RunEngine { fresh_workspace }`, `push` =
`CommitAndPush`.

| State | Event | Next | Actions | Feedback | Attempt |
|---|---|---|---|---|---|
| Scoped | `IssueLabeled(conduit:run)` | Coding | `run-fresh` | keep | — |
| Coding | `EngineFinished(Completed)` | InReview | `push, open-pr, pr-labels, link` | **clear** | — |
| Coding | `EngineFinished(Failed)` | Failed | `fail-comment, issue-labels` | keep | — |
| InReview | `ReviewSubmitted(ChangesRequested)` | Revising | `run-same` | **append body** | — |
| Revising | `ReviewSubmitted(ChangesRequested)` | Revising | — | **append body** | — |
| Revising | `EngineFinished(Completed)` | InReview | `push, pr-labels` | **clear** | — |
| Revising | `EngineFinished(Failed)` | Failed | `fail-comment, issue-labels` | keep | — |
| Failed | `IssueLabeled(conduit:run)` | Coding | `run-fresh` | keep | **+1** |

These cells do not consult `record.pr`, so they behave identically with and
without a PR on the record (Coding-with-PR is real: a Failed-after-PR task
relabelled `conduit:run` retries through Coding, and `OpenPr`'s probe makes
the replay idempotent).

### Terminal PR events — must-act from ANY non-terminal state with an open PR

`PrMerged`/`PrClosed` are guarded by `record.pr.is_some()` and act from
**every** non-terminal state — the diff is edge-triggered and the cursor
advances, so an ignored terminal event would wedge the task forever:

| State (with PR) | `PrMerged` | `PrClosed` |
|---|---|---|
| Scoped | → Merged: `close-issue` | → Abandoned: `close-issue` |
| Coding | → Merged: `dispose, close-issue` | → Abandoned: `dispose, close-issue` |
| InReview | → Merged: `close-issue` | → Abandoned: `close-issue` |
| Revising | → Merged: `dispose, close-issue` | → Abandoned: `dispose, close-issue` |
| Failed | → Merged: `close-issue` | → Abandoned: `close-issue` |

Coding/Revising additionally dispose the workspace: a task whose PR merges or
closes mid-engine-run transitions immediately, the in-flight engine result is
**discarded** (the router checks for a terminal PR event later in the same
tick's batch), and the workspace is deleted. The Merged close comment carries
the merge sha.

### Must-ignore cells

Everything else is the identity transition, swept exhaustively by
`tests/machine.rs`:

- **`CiChanged` is must-ignore in every state** — conduit consumes the event
  type but takes no action (acting on CI would bypass the human review gate);
  it exists so the snapshot/diff layer is proven against CI-bearing forges.
- `PrMerged`/`PrClosed` with **no PR on the record** are ignored (the open-PR
  guard).
- Terminal states (Merged, Abandoned) ignore **all** events.
- Non-trigger labels, `Approved`/`Commented` reviews (approval itself changes
  nothing — the human still merges), and `EngineFinished` in states that are
  not Coding/Revising are all ignored.

## Crash consistency

The router executes each transition with a fixed ordering (`src/router.rs`,
`src/store.rs`):

1. **Persist write-ahead**: the new state plus all pending action intents go
   to `.conduit/tasks/<id>.json` via tmp + fsync + rename + parent-dir fsync
   *before* anything executes.
2. **Execute probe-first**: each action consults its idempotency probe
   (marker / `find_open_pr_by_head` / `ls-remote` compare / absolute label
   sets — see [Forge contract](./forge-contract.md)).
3. **Mark done**: the intent is flagged done in the record after its effect
   succeeds.
4. **Advance the cursor** only after the whole tick's actions complete; a
   failed action propagates before the cursor save, so the next tick re-diffs
   the same snapshot and converges behind the probes.

A crash at any point converges on restart: at-least-once execution,
exactly-once effect. The engine runs synchronously inside the `RunEngine`
intent and its result feeds straight back through `step` + apply, so
Coding/Revising only ever persist as *crash* states.

## Restart recovery

On boot (and at the top of every tick) the router reconciles every
non-terminal task:

- Undone intents re-execute behind their probes.
- A task found in Coding/Revising with no live engine gets its stale
  workspace **disposed** and `RunEngine` re-queued — a fresh clone from the
  local bare cache, re-reading the immutable plan snapshot
  (`.conduit/plans/<id>.md`), never regenerating it. Engines are disposable;
  the snapshot is truth.
- Scoped/InReview tasks simply resume polling.
- `Failed` is reserved for engine-reported failure or timeout, never for
  interruption: `kill -9` mid-Coding resumes as Coding.

This is demoed live (kill mid-Coding, rerun, no duplicate issue/PR) in the
[demo walkthrough](../usage/demo.md) and tested at every state in
`tests/e2e_fake.rs`.
