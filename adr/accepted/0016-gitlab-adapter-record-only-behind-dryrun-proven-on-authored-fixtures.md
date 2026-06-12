# ADR-0016: GitLab adapter: record-only behind DryRun, proven on authored fixtures

> State: Accepted

## Status

Accepted
Created: 2026-06-12

## Stakeholders

conduit maintainers (the forge-adapter keystone and its conformance
obligations live here) and the Como portfolio owner, whose forge-neutrality
claim — proven at N=2, GitLab queued, pinned two-sided by the portfolio's
verify-claims — this adapter widens to N=3.

## Context and Problem Statement

The keystone contract (`src/forge/mod.rs`) requires every adapter to satisfy
the same obligations identically: the disappearance rule (state=all
equivalence), explicit pagination, normalized snapshots, and a sound review
identity for the shared diff's dedupe-by-id semantics. Gitea and GitHub
already conform — Gitea as the live-write lifecycle host (sanctioned only as
the throwaway local container), GitHub record-only behind `DryRun`
(ADR-0012). The third adapter must land without breaking the suite's hard
constraint: no real remote is ever mutated. Three questions had to be
settled before writing it: (1) does GitLab get gitea-style live-write or
github-style DryRun-only? (2) what proves it — recorded fixtures need a live
instance to record from, and none is sanctioned; (3) which side of the
Gitea/GitHub review-dismissal split does GitLab fall on?

The research answer to (3) is that GitLab falls on NEITHER side: it has no
review-submission objects at all. The closest documented analog is MR
approvals — `approved_by` rows of `{user, approved_at}` with no forge-native
id — and revoking an approval REMOVES the row from the API (Gitea keeps
dismissed rows with their original verdict; GitHub overwrites the state
in place, one-way). GitLab's "request changes" is a mutable MR-level status
(`detailed_merge_status: "requested_changes"`) with no per-event identity,
author timestamp, or body.

## Decision Drivers

- The suite mandate is absolute: no real remote mutation, ever; the only
  sanctioned live-write host is a throwaway LOCAL container
- A local `gitlab-ce` container is disproportionately heavy for this rig
  (multi-GB image, minutes-long boot, gigabytes of RAM) versus the
  sub-second Gitea container the kit stands up — and the gates must stay
  hermetic: no network, no docker in `just ci`
- The diff's review semantics demand dedupe on a stable id, no re-fire for
  an edited row, a fresh id (and a fire) for a resubmission, and silence on
  dismissal — whatever shape the forge's native objects take
- The N=3 proof must ride the exact same conformance suite body and the
  same shared transcript normalization as N=2 did — a third adapter that
  needed its own assertions would prove nothing about neutrality
- The portfolio's verify-claims pins the gap two-sided by design: landing
  the adapter flips its forge-list assertion red, forcing the book claim to
  widen in the same motion

## Considered Options

- **Gitea-style live-write against a local GitLab container**: maximal
  fidelity, but `gitlab-ce` is far too heavy for the kit and the gates, and
  no owner-sanctioned GitLab instance exists; the container would exist
  solely to flatter the adapter
- **GitHub-style record-only behind `DryRun`, proven on fixtures**: the
  constructor only hands out `DryRun(GitLabForge)`; reads delegate,
  mutations are recorded and never sent; the conformance suite runs the
  fixtures leg always-on with `Mutations::DryRun`
- **Defer the adapter until a live instance is sanctioned**: keeps the
  matrix cell red and the book claim at N=2 for no architectural reason —
  the fixtures path proves everything the GitHub leg proves today

For the fixture provenance, two sub-options: record from a real GitLab
project (none exists under the mandate) or author the fixture bodies from
the documented REST v4 response shapes, exactly as the per-adapter unit
fixtures for Gitea and GitHub already do.

## Decision Outcome

Chosen: **record-only behind DryRun, proven on authored fixtures** — the
GitHub posture, because the suite constraint (no real remote mutations) plus
the disproportionate weight of a local `gitlab-ce` container leave no
sanctioned live-write target, and the fixtures path carries the full
conformance burden the GitHub leg already carries.

Concretely:

- `open_gitlab` and `fixture_forge` are the only public constructors; both
  return `DryRun(GitLabForge)`. An unwrapped `GitLabForge` is unreachable
  outside the module. NO merge method exists, as everywhere.
- `tests/fixtures/gitlab/` is AUTHORED from the documented REST v4 shapes
  (issues, merge_requests, approvals, pipelines, labels, notes) — not
  recorded; the always-on conformance leg
  (`gitlab_authored_fixtures_conform`) runs the shared suite body with
  `Mutations::DryRun` and asserts the same 17-line normalized transcript as
  the GitHub leg.
- Review identity is SYNTHESIZED from documented fields:
  `{user.id}@{approved_at}` per approval row. GitLab's dismissal shape is
  dismissal-by-REMOVAL — a third shape beside Gitea (keep-with-verdict) and
  GitHub (overwrite-in-place): the adapter filters nothing, a standing
  approval keeps a stable id, a revocation silently removes a row (fires
  nothing), and a re-approval mints a fresh timestamp — a new id that
  correctly fires. Documented limitation: approvals map only to `Approved`;
  `ChangesRequested`/`Commented` are not derivable from a GitLab snapshot
  (no per-event identity exists for them) — acceptable while GitLab is
  record-only, because no live lifecycle polls GitLab and the demo
  transcript scripts its review events.
- Other documented mappings: `state=all` on both lists (disappearance
  rule); explicit `?page=N&per_page=100` loops; `iid` identity with
  SEPARATE issue/MR sequences (label/comment endpoints never cross over);
  `merged` is the distinct `state == "merged"`; `merge_commit_sha` is null
  until merged (fallbacks: `squash_commit_sha`, head `sha`); issues close
  via `state_event`; labels are plain strings read-side and one
  comma-separated name string write-side; CI is the latest pipeline for the
  head sha.

The evidence: `demo-transcript --forge gitlab` is record-only (no git
context, like GitHub) and byte-identical to both other legs — the kit's
beat-4 three-way diff is the N=3 money shot, and a CLI test pins the gitlab
and github legs byte-identical hermetically.

### Positive Consequences

- N=3 forge-neutrality with zero new normalization code: the third adapter
  rides the same shared diff, the same `normalize_action`, the same
  conformance body — neutrality stays a CI assertion, not a slogan
- The mandate stays unbroken: no path exists from conduit to a mutated
  GitLab instance, structurally
- The keystone's review-identity contract got STRONGER: documenting the
  third dismissal shape (removal) forced the module header to state the
  general rule any future adapter must satisfy
- The gates stay hermetic and fast — no heavyweight container rides CI or
  the kit

### Negative Consequences

- The fixtures are authored, not recorded: a divergence between GitLab's
  documented and actual behavior would not surface until a live leg exists
  (mitigated: the same risk held for GitHub's documented-shape unit
  fixtures, and every fixture body cites the documented field semantics)
- GitLab review rounds cannot drive a live lifecycle (`ChangesRequested` is
  not derivable) — promotion to lifecycle host would need the notes-based
  event log or a future per-event reviewer API, decided then
- Like GitHub (ADR-0012), GitLab's acceptance of conduit's mutation
  payloads is unproven live; the same owner-gated posture applies

## Implementation

Landed with this decision: `src/forge/gitlab.rs` (adapter + authored-fixture
transport + wire tests), `ForgeKind::Gitlab` through config and CLI
(`--forge gitlab`), the always-on conformance leg, the kit's beat-4
three-way diff, and the book updates (forge contract, testing, intro).
Upgrade path when an owner sanctions a GitLab instance: add a recorder
mirroring `github::record_fixtures`, an env-gated `CONDUIT_E2E_GITLAB=1`
live-reads leg, and only then revisit live-write — by a new ADR superseding
this one's record-only constraint.
