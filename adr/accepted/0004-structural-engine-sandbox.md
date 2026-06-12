# ADR-0004: Structural engine sandbox

> State: Accepted

## Status

Accepted

## Stakeholders

conduit maintainers. Affected: anyone plugging a new coding engine into the
seam, and security reviewers assessing what a misbehaving engine could do.

## Context and Problem Statement

The coding engine is an untrusted commodity subprocess — an AI agent that
edits files. If the engine's workspace `origin` pointed at an authenticated
remote, or its environment carried forge/AI tokens, then "the engine never
pushes and humans hold the merge gate" would be a convention the engine is
asked to follow, not a property of the system. Prompt injection, agent bugs,
or plain misconfiguration would be one `git push` away from bypassing every
human gate.

## Decision Drivers

- Humans hold every gate: the engine must be *unable* to publish, not just
  instructed not to
- Credentials must never reach a subprocess that runs model-directed code
- Engines are disposable and replaceable; the sandbox must not depend on any
  one engine's flags or goodwill
- The spike's hard constraint: nothing is ever pushed to a real remote

## Considered Options

- Trust engine-level restrictions (disallowed-tools flags, prompt rules)
- Full container isolation per engine run
- Structural sandbox: credential-free clone + scrubbed subprocess environment

## Decision Outcome

Chosen: **the structural sandbox**, because it makes the unsafe action
unrepresentable at the layer conduit controls, for every engine equally.

Workspaces are cloned from a local bare cache (`.conduit/cache/<forge>.git`),
so the engine's `origin` is a filesystem path containing no credentials. The
engine subprocess gets a constructed environment — `env_clear()` plus an
allowlist (`PATH`, `HOME`, `TERM`, `LANG`) — never a blocklist, so forge and
AI tokens are absent by construction. Only conduit's `git.rs` ever touches an
authenticated remote URL (cache fetch and final push), and its push helper
refuses non-local remotes outright as the spike's belt-and-braces. Engine-level
restrictions (disallowed tools, prompt rules) remain as defence in depth, not
as the boundary.

### Positive Consequences

- A compromised or confused engine cannot push, cannot reach the forge API,
  and cannot read tokens — there is nothing to leak in its world
- The property holds for any engine behind the seam, present or future,
  because it lives in workspace preparation rather than engine configuration
- Disposability falls out: workspaces are cheap clones, deleted and recreated
  on restart from the immutable plan snapshot

### Negative Consequences

- Not OS-level isolation: the engine can still read the checkout it is given,
  write inside its workspace, and burn CPU/time (bounded by the conduit-side
  hard timeout)
- Container-grade isolation is deferred and named as future work, not solved
  here

## Implementation

`src/git.rs` is the single authenticated-remote call site (cache fetch,
workspace creation, push with a non-local refusal); engine runners construct
the child environment with `env_clear()` + allowlist. Tests assert the
scrubbed-env list, the push refusal, and that engines receive a
credential-free `origin`.
