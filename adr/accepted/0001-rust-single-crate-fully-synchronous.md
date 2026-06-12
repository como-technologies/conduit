# ADR-0001: Rust single crate, fully synchronous

> State: Accepted

## Status

Accepted

## Stakeholders

conduit maintainers (Como portfolio engineering). Downstream: the Measure-stage
consumer that reads conduit's PR tagging, and the other portfolio tools whose
conventions this decision inherits.

## Context and Problem Statement

conduit is the Adopt-stage engine of the TAPS loop: a thin harness that turns
accepted ADRs into driven work on a team's existing forge. The spike must
prove exactly three pieces of net-new IP — a forge-neutral event router, a PR
lifecycle state machine, and a forge adapter trait two forges implement
identically. Everything else (the coding engine, the planner, the model) is a
commodity behind a process boundary. We need an implementation stack that
keeps the layer thin, transfers the house conventions, and does not smuggle in
infrastructure the spike does not need.

## Decision Drivers

- The house stack is Rust: the justfile / mdbook / thiserror conventions and
  the `HttpTransport` fake-injection pattern transfer directly
- conduit is a poll-tick loop driving one task at a time — there is no
  concurrency to schedule
- The commodity pieces are subprocess boundaries (`claude -p`,
  `adroit ... -o json`), so async runtimes and language interop buy nothing
- The "thin layer" pitch is existential: every dependency must earn its place

## Considered Options

- Rust, single crate (bin + lib), fully synchronous
- Rust with tokio and async adapters
- A multi-crate workspace split by seam
- Python on an existing agent framework

## Decision Outcome

Chosen: **Rust, single crate (bin + lib), fully synchronous — no tokio**,
because a one-task-at-a-time poll loop gains nothing from an async runtime,
and the house conventions transfer wholesale.

Blocking HTTP goes through `ureq` (rustls) behind an `HttpTransport` seam so
unit tests inject a fake transport and never touch the network. Dependencies
are pinned to the short list that earns its place: `clap`, `serde`/
`serde_json`, `ureq`, `thiserror` (typed errors in lib modules) plus `anyhow`
(binary only), `time`, `sha2`. There is no database — state is files you can
`cat` under `.conduit/`; the running postgres/valkey containers are
deliberately not used.

### Positive Consequences

- The layer stays thin and debuggable: every effect is a plain blocking call
  in a single binary
- Conventions, CI shape, and the transport fake-injection pattern arrive
  pre-proven from the sibling tools
- Bounded timeouts on the blocking transport surface network hangs as clean
  typed errors instead of freezing a runtime

### Negative Consequences

- Concurrent task execution (explicitly out of scope for the spike) would
  require revisiting the loop, though the pure core would survive a rework
- Forge HTTP calls serialize within a tick; latency adds up linearly with
  task count

## Implementation

Single `conduit` crate with `src/main.rs` as thin clap marshalling over
`src/cli.rs`; pure modules (`contract.rs`, `machine.rs`, `forge::diff`) carry
no I/O while `router.rs` owns all effects. The gate is `just ci` =
fmt-check + clippy (`-D warnings`) + tests + mdbook build.
