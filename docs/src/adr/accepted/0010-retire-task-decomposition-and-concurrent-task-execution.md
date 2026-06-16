# ADR-0010: Retire task decomposition and concurrent task execution

> State: Accepted

## Status

Accepted
Created: 2026-06-12

## Stakeholders

conduit maintainers (who would own the scheduling and isolation machinery)
and the human reviewers whose gate capacity is the loop's actual throughput
limit.

## Context and Problem Statement

The spike spec deferred two related capabilities on the OUT-list: task
decomposition (splitting one ADR into multiple tasks/PRs) and concurrent
task execution (multiple tasks in flight at once). Today the invariant is
one ADR = one task = one PR, executed serially — the state machine, the
write-ahead store, the workspace layout, and the verify contract all assume
it. The suite-done bar requires the deferred items settled: build the
parallel machinery or retire it by ADR.

## Decision Drivers

- One ADR = one task = one PR is the human-gate safety story the product
  sells: a reviewer approves exactly the scope an accepted decision named,
  and `conduit verify` machine-asserts that one-to-one chain
- Parallelism is a throughput optimization, and the throughput bottleneck is
  human review capacity — no rung below self-serve saturates a serial loop
- Decomposition moves scoping judgment from the accepted ADR (a human
  decision) into the harness (a machine guess), exactly the agent-framework
  drift the thin-layer pitch guards against
- Concurrency would force per-task store isolation, cursor sharding, and
  workspace locking — a large correctness surface against the run-1 learning
  that even the demo artifact dir wasn't single-writer

## Considered Options

- Build decomposition + concurrency: higher theoretical throughput, at the
  cost of scheduler/locking machinery and a weakened one-to-one
  reviewability contract
- Retire both by ADR, keeping serial one-ADR-one-task-one-PR: preserves the
  reviewable safety story and the simple crash-recovery proof; throughput
  scales by running the loop more often, not wider
- Build concurrency only (no decomposition): keeps scope judgment human but
  still buys the locking surface for a throughput win the human gate cannot
  consume

## Decision Outcome

Chosen: **retire task decomposition and concurrent task execution by ADR**,
because one ADR = one task = one PR is the human-gate safety story the
product sells, and parallelism is a throughput optimization no rung below
self-serve needs.

The serial invariant is now contract, not accident: the store, machine, and
verify chain may continue to assume exactly one in-flight task per ADR and
serial tick execution. Widening it requires superseding this decision with
the isolation design in hand.

### Positive Consequences

- The reviewability story stays crisp: every merged PR maps to exactly one
  accepted decision, machine-asserted by `conduit verify`
- No scheduler, lock files, or cursor sharding; crash recovery keeps its
  current straight-line proof
- The per-run artifact-dir learning from run 1 stays solved by uniqueness
  (one workdir per run) instead of locking

### Negative Consequences

- A backlog of accepted ADRs drains serially; wall-clock to clear N
  decisions is N times the loop latency (accepted: the human gate was always
  the slower stage)
- Genuinely large ADRs land as one large PR; the recourse is authoring
  smaller decisions upstream in adroit, not splitting downstream in conduit

## Implementation

Nothing to build. The serial invariant is already what the code implements;
this ADR records it as a decision rather than an accident of spike scope.
