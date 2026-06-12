# ADR-0005: Effort labels from cumulative wall-clock

> State: Accepted

## Status

Accepted

## Stakeholders

conduit maintainers and the Measure-stage consumer that reads the labels at
merge time — the contract here is built for it.

## Context and Problem Statement

The Measure stage needs to trace effort back to the decision that prompted the
work: every merged PR must carry exactly one effort label from a closed set,
plus the `adr:<reference>` tag. Something has to assign that effort value.
AI-based estimation is out of scope, human estimation defeats the point of an
automated harness, and proxy metrics like diff size measure the wrong thing.
Whatever is chosen must be cheap, objective, and final by the moment a human
merges — that is when the label is read.

## Decision Drivers

- The consumer contract: exactly ONE label from a closed five-bucket enum,
  final at merge time
- Objectivity and reproducibility — no model in the loop, nothing to tune
- The number must reflect work actually spent on the task, including retries
  and review rounds
- "Exactly one" should be enforced structurally, not by cleanup logic

## Considered Options

- AI effort estimation from the plan or the diff
- Size heuristics (files touched, lines changed, commits)
- Cumulative engine wall-clock mapped through configurable bucket thresholds

## Decision Outcome

Chosen: **cumulative engine wall-clock through bucket thresholds**, because
time spent driving the engine is the one effort signal the harness measures
directly and cannot argue with.

Every engine run is timed; the milliseconds accumulate on the task record
(`work_ms += elapsed`) across attempts and revision rounds — never reset. The
bucket map defaults to `<10m`, `<30m`, `<2h`, `<8h`, else the fifth bucket,
with thresholds overridable in `conduit.toml` `[effort]` (validated strictly
increasing). The label is applied at PR open and recomputed-and-swapped after
each revision push; "exactly one" is structural because the label set is
written as an absolute set — the chosen label present, the other four absent
by construction. The value is final at merge, which is the moment the
Measure stage reads it.

### Positive Consequences

- Zero estimation machinery: the number falls out of timing calls the router
  already makes
- Honest across the lifecycle: a task that failed, retried, and went through
  review rounds carries all of that time
- The closed enum plus absolute label writes make consumer-side validation a
  mechanical check

### Negative Consequences

- Engine wall-clock is not human effort — review latency and thinking time
  are invisible; the buckets are deliberately coarse to absorb that
- A hung engine inflates the measure up to the hard timeout (a timeout run
  still counts its wall-clock, by design)
- Thresholds are a heuristic; teams with very different engine speeds may
  need to retune them

## Implementation

`src/contract.rs` owns the closed label set, the threshold table, and the
bucket mapping as pure functions; the router times every run via one
`run_timed` chokepoint and applies labels with absolute sets. The verify
command machine-asserts exactly-one-effort on the merged PR as part of the
six-check contract report.
