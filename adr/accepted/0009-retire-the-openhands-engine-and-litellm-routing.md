# ADR-0009: Retire the OpenHands engine and LiteLLM routing

> State: Accepted

## Status

Accepted
Created: 2026-06-12

## Stakeholders

conduit maintainers (who would carry a second engine adapter and its docker
runtime) and the Como portfolio owner, whose vendor-neutrality pitch the
Engine seam exists to honor.

## Context and Problem Statement

The spike spec named OpenHands (behind LiteLLM routing) as the candidate
second engine implementation, deferred on the OUT-list. The Engine seam it
would plug into is already proven at the contract level: the deterministic
FakeEngine is the default demo path (the referee's ruling for run 1), and the
sandboxed live ClaudeCodeEngine ran the demo encore — two implementations,
one of them real, both behind the same trait with the same sandbox and
timeout semantics. The decision to settle now, with the suite-done bar
requiring every deferred item built or retired by ADR, is whether a second
*real* engine buys correctness evidence or only breadth.

## Decision Drivers

- The seam is the product claim, and it is already exercised by two
  implementations (deterministic fake + sandboxed live claude); a third
  would re-prove the same contract
- OpenHands costs a docker runtime dependency, and LiteLLM-routed local
  models inherit the ledger's dominant pain — roughly 30 seconds of ollama
  wall-clock per call — multiplied across a real coding session
- No SME-usable scenario with Como alongside requires a non-Claude engine:
  the engagement model puts an operator next to the tool, not a fleet of
  interchangeable engines
- Engine breadth is the agent-framework treadmill the thin-layer pitch
  explicitly avoids (the spike's named existential risk)

## Considered Options

- Build the OpenHands + LiteLLM engine: demonstrates a second real engine,
  but adds docker and model-routing operational weight and re-proves a
  contract the FakeEngine tests and the live claude encore already pin
- Retire it by ADR: the Engine seam stays open (trait + sandbox + timeout
  are engine-agnostic), the runtime cost is never paid, and a future engine
  need arrives via supersession with a concrete requirement attached
- Keep it deferred in spec prose: fails the suite-done bar's
  built-or-retired requirement and invites re-litigation each iteration

## Decision Outcome

Chosen: **retire the OpenHands engine and LiteLLM routing by ADR**, because a
second real engine adds breadth, not correctness — the seam is proven by the
deterministic FakeEngine plus the sandboxed live ClaudeCodeEngine — and no
scenario on conduit's target rung requires a non-Claude engine.

The Engine trait, the structural sandbox (ADR-0004), and the timeout
semantics remain engine-agnostic; nothing in this retirement narrows the
seam. What is retired is the obligation to implement and operate a second
real engine this suite.

### Positive Consequences

- No docker runtime or model-routing layer enters the dependency set; the
  default demo path stays deterministic and offline
- The ollama wall-clock cost (the ledger's dominant pain) is not multiplied
  into the engine loop
- Engine-vendor neutrality remains an architectural property (the seam)
  rather than a maintenance treadmill (N adapters)

### Negative Consequences

- The vendor-neutrality claim rests on seam design plus one real engine, not
  a demonstrated second real engine — that story must lean on the FakeEngine
  contract evidence
- If the claude CLI's availability or licensing shifts, standing up a
  replacement engine starts from the trait contract, not from a half-built
  alternative

## Implementation

Nothing to build. The engine seam's documentation already records the
contract a future implementation must satisfy; this ADR is the formal
retirement the deferred list points to.
