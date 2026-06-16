# ADR-0011: Retire multi-repo, multi-tenant, hosting, and auth machinery

> State: Accepted

## Status

Accepted
Created: 2026-06-12

## Stakeholders

conduit maintainers, the Como portfolio owner (whose adoption-enablement
service is the designed alternative to self-serve hosting), and any future
operator who would otherwise expect a hosted control plane.

## Context and Problem Statement

The spike spec deferred the platform tier on the OUT-list: driving multiple
repositories from one conduit, multi-tenant operation, hosted deployment,
and an authentication/authorization layer. conduit today is a single-binary,
single-repo, files-under-`.conduit/` tool an operator runs next to the work.
The suite-done bar forces the question: is the platform tier a missing
feature or a different product? conduit's target rung this suite is
SME-usable — an operator-driven tool with the adoption-enablement service
covering the gap — not self-serve.

## Decision Drivers

- Multi-repo/tenant/hosting/auth is self-serve-rung machinery; conduit's
  target rung is SME-usable, and the adoption-enablement service covers the
  gap by design (an engineer alongside, not a control plane)
- Every platform element multiplies the security surface: tenant isolation,
  credential custody for many forges, a network-listening service — against
  a mandate where remote mutations are owner-gated and state is inspectable
  files
- The single-repo assumption is load-bearing in the store layout (one cursor
  per forge, task ids from ADR references) and in the demo/verify loop; a
  multi-repo abstraction would touch every layer
- Running one conduit per repo is a zero-code answer to the multi-repo need
  at SME scale, now proven by per-run unique workdirs (each run carries its
  own config and store)

## Considered Options

- Build the platform tier (multi-repo routing, tenancy, hosting, auth):
  enables self-serve scale, at the cost of a service architecture, a
  credential-custody story, and an isolation proof — none of which the
  target rung needs
- Retire it by ADR: conduit stays a single-repo operator tool; scale-out at
  SME size is one workdir (config + store) per repo; self-serve demand, if
  it ever materializes, supersedes this with a real tenancy design
- Build a thin multi-repo loop only (no tenancy/hosting/auth): the smallest
  platform slice, but it still breaks the one-store-one-repo invariant and
  delivers nothing the per-repo-workdir pattern doesn't already

## Decision Outcome

Chosen: **retire multi-repo, multi-tenant, hosting, and auth machinery by
ADR**, because this is self-serve-rung machinery, conduit's target rung is
SME-usable, and the adoption-enablement service covers the gap by design.

conduit remains a local, single-repo, operator-run binary. The supported
multi-repo answer is one workdir per repo (the per-run workdir pattern:
each carries its own `conduit.toml`, `.secrets`, and `.conduit/` store).
Reopening the platform tier requires superseding this decision with a
tenancy and credential-custody design attached.

### Positive Consequences

- No listening service, no tenant isolation proof, no credential custody for
  third parties — the security posture stays "files on the operator's
  machine, tokens in gitignored `.secrets/`"
- The store and contract layers keep their single-repo invariants;
  correctness work this suite stays focused on the loop itself
- The product boundary matches the business design: enablement engagements,
  not hosted seats

### Negative Consequences

- An SME with many repos runs many workdirs and processes — manageable at
  that scale but undeniably manual; the convenience of one daemon over N
  repos is forgone
- Any future self-serve pivot starts the platform tier from zero, with this
  ADR as the recorded reason

## Implementation

Nothing to build. The per-run workdir pattern (the playbook demo's
`demo/playbook-demo-init.sh`) documents the supported one-workdir-per-corpus
shape this decision leans on.
