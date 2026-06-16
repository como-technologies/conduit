# ADR-0003: Filesystem store with write-ahead intents

> State: Accepted

## Status

Accepted

## Stakeholders

conduit maintainers. Affected: operators who debug a stuck task (the store is
the debugging surface) and anyone reasoning about crash behaviour.

## Context and Problem Statement

conduit mutates two worlds that cannot share a transaction: its own task state
and the remote forge (issues, PRs, comments, labels). A crash between "decided
to open a PR" and "recorded that the PR is open" must not duplicate the PR on
restart, and a crash before recording a state change must not lose the
transition. The spike has postgres and valkey containers available, but a
database would add infrastructure to what is pitched as a thin harness — and
would not solve the cross-world atomicity problem anyway.

## Decision Drivers

- Crash anywhere must converge on restart: at-least-once execution,
  exactly-once effect
- State must be inspectable with `cat` — the demo's "the whole lifecycle as
  files" beat is a feature
- No infrastructure dependencies: the harness runs anywhere a checkout runs
- Forge mutations are remote and non-transactional; the design must absorb
  that, not hide it

## Considered Options

- Postgres (already running) with transactional task state
- SQLite embedded in the binary
- Flat JSON files with atomic writes and write-ahead action intents

## Decision Outcome

Chosen: **flat files under `.conduit/` with atomic writes and write-ahead
intents**, because the consistency problem is ordering against a remote forge,
not local transactionality — and files solve it with zero infrastructure.

Every record/plan/cursor write is atomic: bytes to `<path>.tmp`, fsync,
rename, fsync the parent dir. Per transition the ordering is fixed: (1)
persist the new state plus pending action intents *before* executing anything;
(2) execute each action probe-first (marker comments, `find_open_pr_by_head`,
`ls-remote` compare); (3) mark the intent done; (4) advance the forge cursor
only after the tick's actions complete. On restart, undone intents re-execute
behind their probes and converge.

### Positive Consequences

- `cat .conduit/tasks/*.json` shows the whole lifecycle, pending intents
  included — debugging needs no tooling
- Crash-replay behaviour is testable per mutating action kind, with a
  dedicated exactly-once-effect test for each
- The plan snapshot persisted at `plans/<task-id>.md` is verbatim and
  immutable — restart recovery re-reads it instead of regenerating
  nondeterministically

### Negative Consequences

- Single-writer by design: no concurrent daemons against one store
- No queries; anything beyond list-and-filter means reading every record
- Load-modify-save invariants (intent indices, single-threaded ticks) rest on
  the caller and are documented rather than enforced by a storage engine

## Implementation

`src/store.rs` owns the layout (`tasks/`, `plans/`, `cursor/`, `cache/`,
`workspaces/`, `bin/`) and the atomic write helper; `src/router.rs` owns the
four-step transition ordering and probe-first replay. Both are exercised by
crash-replay tests that kill the rig at every state and after every mutating
action kind.
