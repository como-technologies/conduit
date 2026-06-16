# ADR-0007: Namespace-scoped label convergence

> State: Accepted

## Status

Accepted

## Stakeholders

conduit maintainers (the label writers), reviewers who hand-apply labels in
the forge UI, and the Measure-stage tooling that reads `effort:*`/`adr:*`
labels off merged PRs.

## Context and Problem Statement

conduit applies labels to issues and PRs as absolute sets: `set_issue_labels`
and `set_pr_labels` replace whatever is on the object. That was correct while
conduit was the only writer, but the forge label namespace is shared with
humans — a reviewer who tags a conduit-managed issue `priority-high` or
`discuss` loses that label the next time conduit converges its own labels
(e.g. the `conduit:run` → `conduit:failed` swap, or the effort relabel after
a revision round). The machine-owned labels already live in three prefixes —
`effort:*`, `adr:*`, `conduit:*` — but nothing scopes conduit's writes to
them. Before real-world use the convergence semantics must be settled,
because they affect every forge adapter and every label write site.

## Decision Drivers

- Human labels are human property: conduit must never add, remove, or rewrite
  a label it does not own
- Convergence must stay idempotent and crash-replayable (the router re-runs
  label actions behind probes; same result every time)
- The Measure-stage contract is absolute within its namespaces: exactly one
  `effort:*` label, the `adr:*` label present, stale owned labels gone
- One implementation, not one per adapter: adapter drift is the standing
  hazard the conformance suite exists to catch
- The forge APIs only offer absolute label replacement (PUT) as the
  convergent primitive — read-modify-write is unavoidable somewhere

## Considered Options

- Keep absolute label sets (status quo): simplest, but silently destroys
  human labels on every convergence — unacceptable for real-world use
- Additive-only writes (never remove): preserves human labels but leaks
  stale machine labels (two `effort:*` labels after a relabel violates the
  Measure-stage contract)
- Namespace-scoped convergence in one shared layer: conduit owns exactly the
  `effort:*`, `adr:*`, `conduit:*` prefixes; a label write replaces the owned
  subset (add missing, remove stale) and passes every unprefixed label
  through verbatim
- Per-adapter convergence (each adapter reads current labels and merges):
  same semantics but three implementations that can drift apart

## Decision Outcome

Chosen: **namespace-scoped convergence in one shared layer**, because it is
the only option that protects human labels, keeps the owned namespaces
absolute (the Measure-stage contract), and exists in exactly one place.

Mechanics: the machine state computes the DESIRED OWNED set (labels in the
three owned prefixes only). At write time the router/transcript reads the
object's current labels through a new adapter read (`get_issue_labels` /
`get_pr_labels`), and a pure shared function computes the final absolute set:
current labels outside the owned prefixes (preserved verbatim, in order) +
the desired owned set. The adapters keep their absolute-set write semantics;
ownership scoping lives in the shared normalization layer, unit-tested there,
and the conformance suite proves the composed semantics (preserve unprefixed,
remove stale owned, add missing owned) on every adapter.

The owned prefixes are exactly `effort:`, `adr:`, `conduit:` — declared once
next to the convergence function. A label equal to a bare prefix-less name a
human applied is never touched, even if it collides with a future machine
vocabulary; widening ownership requires superseding this decision.

### Positive Consequences

- Human labels survive every conduit convergence — reviewers can annotate
  conduit-managed issues and PRs freely
- The owned namespaces stay absolute: exactly one `effort:*` label, stale
  `conduit:run`/`conduit:failed` swaps converge, replay-safe
- One pure, exhaustively unit-testable function defines ownership; adapters
  cannot drift (conformance-proven on each)
- The new label reads double as cheap probes (no full snapshot fetch needed
  at label-write time)

### Negative Consequences

- Every label write becomes read-modify-write: one extra forge read per
  label action, and a write race with a human relabeling in the same instant
  can still drop the human's change (last-writer-wins at the forge; accepted
  — the window is one poll tick and the next human action restores it)
- Two new methods on the Forge trait that every adapter (and fake) must
  implement
- The owned-prefix list is a contract frozen here: adding a future machine
  namespace requires a superseding decision and a migration pass

## Implementation

1. Shared layer: `labels::OWNED_PREFIXES` + pure `labels::converge(current,
   desired_owned)` with unit tests (preserve unprefixed order, drop stale
   owned, append desired owned, dedupe).
2. Forge trait: `get_issue_labels` / `get_pr_labels` reads on every adapter
   (REST GET on the real forges, stored state on the fake, overlay + delegate
   on the dry-run wrapper).
3. Route every label write site (router `ApplyPrLabels` / `SetIssueLabels`,
   transcript twin) through read → converge → absolute write.
4. Conformance: every adapter leg proves a pre-existing unprefixed label
   survives convergence while stale owned labels are replaced.
