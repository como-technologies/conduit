# ADR-0008: Retire webhook ingestion, reaffirm snapshot-diff polling

> State: Accepted

## Status

Accepted
Created: 2026-06-12

## Stakeholders

conduit maintainers (who would own a webhook receiver's operational surface)
and the Como portfolio owner, whose suite-done bar requires every deferred
spec item to be either built or retired by an accepted ADR.

## Context and Problem Statement

Accepted ADR-0002 chose snapshot-diff polling as conduit's only event
producer and named webhook ingestion as the candidate second producer feeding
the same normalized event stream. The spike's OUT-list deferred it; the
question now is whether to build it or retire it. Run 1 of the dogfood loop
completed the full lifecycle — plan, trigger, code, review rounds, merge,
verify — on polling alone, with the 15-second default interval never the
bottleneck (engine wall-clock dominates every tick by orders of magnitude).
A decision is forced now because the iteration-2 suite-done bar requires the
deferred list to be formally settled, not left as spec prose.

## Decision Drivers

- Run-1 evidence: polling proved sufficient for the complete loop; no
  latency-driven failure or operator complaint surfaced
- A webhook receiver adds a listening server, forge-side delivery
  configuration, secret rotation, and replay/dedup handling — pure
  operational surface, on a tool whose state model is files under `.conduit/`
- Low-latency event delivery only pays off at hosted, many-tenant scale —
  self-serve territory conduit is explicitly not targeting this suite
  (its target rung is SME-usable, with adoption enablement covering the gap)
- ADR-0002's crash story (re-poll converges from the unadvanced cursor) is
  load-bearing; a second producer would need an equivalent proof

## Considered Options

- Build webhook ingestion as the second producer: lower event latency, but
  a standing server + delivery configuration + a second convergence proof,
  for a latency win nothing in the target rung needs
- Retire webhook ingestion by ADR, reaffirming ADR-0002 polling: keeps one
  producer, one convergence story, zero new operational surface; revisit only
  via a superseding decision if a hosted rung is ever targeted
- Leave it deferred in spec prose: costs nothing today but fails the
  suite-done bar (deferred items must be built or retired by ADR) and leaves
  the scope question to be re-litigated every iteration

## Decision Outcome

Chosen: **retire webhook ingestion by ADR, reaffirming ADR-0002 polling**,
because run 1 proved polling sufficient for the loop conduit actually runs,
and webhooks only pay off for hosted low-latency scale — self-serve territory
conduit is explicitly not targeting this suite.

Snapshot-diff polling remains the only event producer. The normalized event
stream and the cursor-convergence semantics of ADR-0002 are unchanged and
reaffirmed. Reopening this decision requires superseding this ADR, with a
hosted/low-latency requirement in hand.

### Positive Consequences

- One event producer, one crash-convergence story — the property run 1
  actually exercised stays the only one to maintain
- No listening server, webhook secrets, delivery retries, or dedup logic
  enters a codebase whose pitch is a thin, inspectable layer
- The deferred-list entry is formally closed; iteration grading can cite an
  accepted ADR instead of spec prose

### Negative Consequences

- Event latency stays bounded below by the poll interval; a future
  low-latency requirement reopens this decision via supersession rather than
  finding a receiver half-built
- Forge API quota is consumed by polling even when idle (accepted: one
  snapshot fetch per interval is well inside every forge's limits at
  single-repo scale)

## Implementation

Nothing to build. The retirement is recorded by accepting this ADR; the
follow-ups page and the spike spec's OUT-list entry now point here.
