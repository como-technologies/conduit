# ADR-0013: Scope reaffirmation: remaining OUT-list items stay retired

> State: Accepted

## Status

Accepted
Created: 2026-06-12

## Stakeholders

conduit maintainers and the Como portfolio owner: this is the formal closure
of the spike spec's deferred list, which the suite-done bar requires to be
retired by ADR rather than carried as spec prose.

## Context and Problem Statement

The spike spec's "Out of scope (named, deferred)" list parked everything the
spike would not build. Most entries have since been settled individually:
the GitLab adapter is in the build plan (the N=3 forge proof), webhooks are
retired by ADR-0008, the OpenHands/LiteLLM engine by ADR-0009, decomposition
and concurrency by ADR-0010, multi-repo/tenant/hosting/auth by ADR-0011,
GitHub mutation acceptance is owner-gated by ADR-0012, and MCP exposure of
adroit to engines was already retired by accepted ADR-0006 (re-cited here;
not re-litigated). That leaves a tail of named items with no decision of
record: web dashboard · postgres/valkey · automated rebase/conflict
resolution · AI effort estimation · in-flight replanning · CI provisioning ·
deploy stage. One decision should retire the tail so the deferred list is
formally empty.

## Decision Drivers

- The suite-done bar: every spec OUT-list item must be built or retired by
  an accepted ADR — a deferred list living only in spec prose fails it
- Each tail item contradicts a load-bearing design rule already on the
  record: state-is-files (dashboard, postgres/valkey), conflict ⇒ Failed +
  human (auto-rebase), effort from measured wall-clock per ADR-0005 (AI
  estimation), immutable plan snapshots (in-flight replanning), events
  consumed never configured (CI provisioning), human gate outside the loop
  (deploy stage)
- One scope ADR for the tail keeps the corpus navigable; seven near-empty
  retirement ADRs would dilute it
- Re-litigating any individually would repeat rationale already accepted in
  ADR-0001..ADR-0006

## Considered Options

- One scope-reaffirmation ADR retiring the tail as a block, each item with
  its one-line reason: lowest corpus noise, single citation target for
  graders and follow-ups
- Individual ADRs per tail item: maximal granularity, but seven documents
  each restating an existing design rule in different words
- Leave the tail in spec prose: the status quo the suite-done bar explicitly
  rejects

## Decision Outcome

Chosen: **one scope-reaffirmation ADR retiring the remaining OUT-list tail
as a block**, because every item falls to a design rule already accepted,
and the suite-done bar wants a decision of record, not prose.

Retired, with reasons:

- **Web dashboard** — state is files you can `cat` (ADR-0003); `conduit
  status` and the task records are the inspection surface
- **postgres/valkey** — same decision: the filesystem store with write-ahead
  intents is the database; a server store reopens ADR-0003
- **Automated rebase/conflict resolution** — a conflict means the world
  changed under the task; the safe semantic is Failed + human, never a
  machine-guessed merge
- **AI effort estimation** — effort labels derive from measured cumulative
  wall-clock (ADR-0005); replacing a measurement with a model guess weakens
  the Measure-stage contract
- **In-flight replanning** — the persisted plan snapshot is immutable and
  replanning is cancel + new task; mid-flight plan mutation breaks replay
  and reviewability
- **CI provisioning** — conduit consumes CI events, it never configures CI;
  owning pipeline setup is platform territory (see also ADR-0011)
- **Deploy stage** — deployment is a human gate outside the loop; conduit
  ends at the merged, verified PR

MCP exposure of adroit to engines stays retired per accepted ADR-0006 —
re-cited, not re-litigated. Any individual item returns only via a
superseding ADR with a concrete requirement.

### Positive Consequences

- The spec's deferred list is formally empty: every entry is now built, in
  the build plan, or retired by an accepted decision — the suite-done cell
  is checkable by citation
- Scope pressure on the thin-layer pitch (the spike's named existential
  risk) now has a single fence document to point at
- Future "should conduit just add a dashboard?" conversations start from a
  recorded trade-off instead of a blank page

### Negative Consequences

- Block retirement is coarser than per-item ADRs: reopening one item means
  superseding a decision that names seven, and the supersession must say
  which item it revives
- A genuinely new requirement (e.g. a client-funded dashboard) pays the
  supersession overhead even when the answer would be yes

## Implementation

Nothing to build. The follow-ups page and the spike spec's OUT-list section
point here; `adroit check` keeps the citation graph valid.
