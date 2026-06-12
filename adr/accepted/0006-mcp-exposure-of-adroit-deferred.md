# ADR-0006: MCP exposure of adroit deferred

> State: Accepted

## Status

Accepted

## Stakeholders

conduit maintainers and the ADR-tooling maintainers who own the MCP server
this decision declines to wire in — the lane boundary is theirs too.

## Context and Problem Statement

The coding engine needs decision context: the ADR body and the implementation
plan it is executing. The ADR tool already ships an MCP server exposing read
verbs to agents, and the engine speaks MCP natively — wiring them together is
the obvious move. But the engine is the least-trusted process in the system,
its sandbox was just defined as "no credentials, no channels beyond the
workspace", and several of the ADR tool's verbs (`review`, `plan`,
`summarize`) can trigger writes or AI calls whose arguments an agent controls.
Should the spike connect the engine to the corpus over MCP?

## Decision Drivers

- The engine sandbox is structural; every extra channel into the engine is
  surface area that must be allowlisted, audited, and tested
- conduit's own lane is enforced in code: it may invoke only `{manifest,
  list, show, plan}` — an engine-side MCP channel would bypass that chokepoint
- The plan snapshot is immutable and persisted verbatim; live corpus reads
  from inside a run would reintroduce nondeterminism
- Spike thinness: the demo needs context delivery, not corpus browsing

## Considered Options

- Wire the ADR tool's MCP server into the engine now
- Build a read-only MCP proxy in front of it now
- Inline all context into the task document and defer MCP exposure

## Decision Outcome

Chosen: **inline the context and defer MCP**, because the engine needs the
decision and the plan — both of which conduit already holds as immutable
snapshots — and nothing else.

The engine receives `TaskSpec` with the ADR body, the verbatim plan snapshot,
and current-round review feedback, rendered into a task document in the
workspace. No MCP server is started, and the engine gets no channel to the
corpus. The future shape is recorded now: when engines do get corpus access,
it will be the ADR tool's MCP server behind a *read-only allowlist proxy* —
never `review`, `plan`, or `summarize`, which can leak file or forge writes
via agent-controlled arguments.

### Positive Consequences

- The sandbox stays whole: workspace in, outcome out, no sockets or stdio
  servers in between
- Engine runs are reproducible from the persisted snapshot — the same task
  document yields the same context every retry
- One less moving part in the spike demo

### Negative Consequences

- The engine cannot consult related or superseding ADRs mid-run; cross-ADR
  context arrives only if a human puts it in the plan
- Revisiting this means building the allowlist proxy and its tests —
  deferred work, not avoided work

## Implementation

No wiring, by decision. The task-document renderer in the engine seam carries
the inlined context; the recorded future shape (read-only allowlisted MCP
proxy) lives here so the eventual implementation starts from this boundary
instead of relitigating it.
