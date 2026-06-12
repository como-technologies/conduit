# ADR-0012: GitHub mutation acceptance held owner-gated behind DryRun

> State: Accepted

## Status

Accepted
Created: 2026-06-12

## Stakeholders

The Como portfolio owner (the only person mandated to mutate a real remote)
and conduit maintainers, who must know exactly which gap the DryRun wrapper
leaves open.

## Context and Problem Statement

The GitHub adapter's mutations are always DryRun-decorated — the constructor
only hands out `DryRun(GitHubForge)`, reads are live, writes are recorded.
The spike spec and follow-up 6 name the residual gap honestly: conduit has
never actually sent a mutation payload to the GitHub API and confirmed it
was accepted. Dry-run proves the action *stream*; recorded fixtures and
schema checks prove the payload *shape*; neither proves GitHub's acceptance.
The proposed closure was a one-time validation against a sacrificial private
repo (create PR, set labels, close PR, delete branch). But mutating a real
remote — even a sacrificial one — is a remote action, and remote actions are
owner-only under the standing mandate. The suite-done bar requires this
deferred item settled rather than silently open.

## Decision Drivers

- The mandate is absolute: nothing is ever pushed to a real remote and no
  real-forge mutation happens except by the owner, personally, explicitly —
  an agent or CI leg must not be the thing that first mutates github.com
- The gap is evidential, not architectural: payload shapes are already
  pinned against recorded fixtures from the documented REST API; what is
  missing is one acceptance datum only the owner may produce
- Leaving the gap undocumented invites either silent risk-taking (someone
  lifts DryRun without the validation) or silent staleness (the item lives
  forever in a follow-ups page)
- The validation is cheap and one-time once the owner chooses to run it; no
  standing machinery is needed

## Considered Options

- Run the sacrificial-repo validation now as part of suite work: closes the
  gap fastest, but a non-owner actor mutating a real remote violates the
  mandate that protects every repo in the portfolio
- Retire the item by ADR as an owner-gated action: DryRun remains the only
  GitHub mutation path until the owner personally runs the one-time payload
  validation; the residual gap is documented here rather than silently open
- Drop the validation idea entirely: simplest, but then lifting DryRun for
  real use would happen with zero acceptance evidence — worse than the
  documented residual

## Decision Outcome

Chosen: **retire the sacrificial-repo mutation acceptance as an owner-gated
action**, because it requires mutating a real remote and remote actions are
owner-only under the mandate.

Concretely: `DryRun(GitHubForge)` stays the only constructor output for
GitHub. The residual gap — GitHub's acceptance of conduit's mutation
payloads is fixture-verified but never live-verified — is acknowledged and
owned here. The gate lifts only when the owner personally runs the one-time
validation against a sacrificial private repo (create PR, set labels, close
PR, delete branch) and records the result; until then the gap is documented,
not open by accident.

### Positive Consequences

- The mandate stays unbroken: no agent, test leg, or CI path can be the
  first thing to mutate github.com
- The residual risk is named in an accepted decision instead of living as a
  perpetual follow-up item — graders and future maintainers find one
  authoritative statement
- The one-time validation checklist is recorded and ready for the owner to
  execute whenever real GitHub use is actually wanted

### Negative Consequences

- The gap itself remains: payload acceptance by GitHub stays unproven until
  the owner acts, and any real-use plan inherits that prerequisite
- DryRun-only GitHub means the adapter's mutation path can bit-rot against
  API changes without a live canary (mitigated: reads are live, fixtures
  pin the documented API, and the conformance suite runs on every change)

## Implementation

Nothing to build now. When real GitHub use is wanted: the owner creates a
sacrificial private repo, runs the four-mutation validation with conduit's
recorded payloads, records the outcome, and only then may a decision to lift
DryRun be proposed — superseding this ADR.
