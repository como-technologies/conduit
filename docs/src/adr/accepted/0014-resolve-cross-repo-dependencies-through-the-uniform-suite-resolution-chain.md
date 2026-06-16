# ADR-0014: Resolve cross-repo dependencies through the uniform suite resolution chain

> State: Accepted

## Status

Accepted
Created: 2026-06-12

## Stakeholders

conduit maintainers and the Como portfolio owner. The convention is
suite-wide: every TAPS repo records the same decision in its own corpus
(the playbook — the corpus this repo's demo drives — records it for its
side of the seam).

## Context and Problem Statement

conduit's hard dependency on adroit was installed from a hardcoded absolute
path (`cargo install --git file:///home/brett/repos/como-tech/adroit`), and
the demo assumed a sibling `../playbook` checkout. Both bake one developer
machine's directory layout into the repo: a fresh clone anywhere else cannot
init, and the failure says nothing about how to fix it. The layout
assumption also hides a real pin-reachability problem — `adroit.rev` pins
the v0.2.0 tag rev, which exists only in the local adroit checkout; the
public remote's main has never been pushed past an older commit, so a
remote-only install of the pin cannot succeed today. Every repo in the
suite had grown its own variant of the same sibling assumption. One uniform
resolution convention has to replace them all.

## Decision Drivers

- The pin discipline must survive intact: `adroit.rev` holds one exact rev
  and bumps stay explicit reviewed edits
- A fresh clone on another machine must either resolve its dependencies or
  fail with an actionable error naming every knob
- Resolvers must be self-contained — no repo ever sources helper code from
  a sibling checkout
- Secrets never travel via git or clone URLs; resolvers never push and
  never register the cache as a remote
- Repos with no public remote today (conduit, playbook, docs) must keep
  working exactly as they do now via the sibling leg, and start resolving
  remotely the day the owner pushes them — with no further change
- Offline operation must be possible (`COMO_OFFLINE=1`): use populated
  caches as-is, never fetch

## Considered Options

- The uniform suite resolution chain: explicit env override → sibling
  checkout → PATH (binaries only) → gitignored `.como/` clone cache from
  `${COMO_GIT_BASE:-https://github.com/como-technologies}/<repo>.git` →
  skip-with-notice (advisory) or actionable error (hard)
- Status quo: per-repo hardcoded absolute or relative paths — breaks every
  off-machine clone and drifts differently in each repo
- A shared resolver script sourced from a sibling repo — couples checkouts
  to bootstrap the very thing that resolves checkouts
- Vendoring binaries or corpora into each consumer — heavy, goes stale, and
  defeats the rev-pin discipline

## Decision Outcome

Chosen: **the uniform suite resolution chain**, because it keeps the pin
discipline, names its knobs, and degrades to exactly today's behavior where
remotes do not exist yet.

Cross-repo references in the Como TAPS suite resolve through one uniform,
self-contained chain instead of assuming sibling checkouts: (1) an explicit
environment override (`ADROIT_BIN` for the adroit binary, `COMO_<REPO>_DIR`
for a checkout directory), (2) the sibling checkout `../<repo>`, (3) for
binaries, an installed binary on PATH, (4) a gitignored git-clone cache
under `.como/` in the consuming repo, populated read-only from
`${COMO_GIT_BASE:-https://github.com/como-technologies}/<repo>.git`, and
(5) the existing skip-with-notice for advisory gates or an actionable error
naming the knobs for hard dependencies. Each repo embeds its own copy of
the resolver — no repo ever sources helper code from a sibling.
Contract-grade dependencies stay pinned: conduit installs adroit at the
exact rev in `adroit.rev` (remote URL by default, sibling `file://` only as
the local-dev override), and any script that reads another repo's source as
a contract (portfolio's verify-claims) declares the rev it clones and
prints which source it actually resolved. Runtime secrets and live-forge
artifacts are never resolved via git — they are env-first with documented
local-path fallbacks — and the docs evidence repo is local-only by policy,
so references to it stop at skip-with-notice. Resolvers only clone and
fetch: they never push, never add the cache as a remote, and never carry
credentials in URLs. Repos without a public remote today (conduit,
playbook, docs) degrade to exactly the skip-with-notice behavior they
produce now, so nothing breaks before the owner pushes them.

### Positive Consequences

- A conduit clone next to any adroit source that carries the pin — remote
  or sibling — can init; failures name `COMO_ADROIT_GIT`, `COMO_GIT_BASE`,
  and the sibling path instead of silently assuming a layout
- The pin-reachability gap is surfaced, not hidden: the resolver verifies
  the rev exists after the probe clone and prints a notice when it falls
  back to the sibling, so "the tag was never pushed" is visible at init
  time instead of breaking a future user
- The demo's playbook corpus resolves through the same chain
  (`COMO_PLAYBOOK_DIR` → sibling → `.como/deps/playbook` cache), so the
  coupled conduit/playbook demo works from any layout that can reach a
  playbook

### Negative Consequences

- The remote leg of the adroit install is dead until the owner pushes the
  v0.2.0 tag — every fresh standalone conduit clone still fails (now with
  an actionable error); only sibling or `COMO_ADROIT_GIT`/`COMO_GIT_BASE`
  layouts work today
- Suite repos each carry a copy of the same resolver shape, which can
  drift; the convention text in each corpus is the only anchor
- First population of the `.como/` cache costs network and (for binaries) a
  full `cargo install` build

## Implementation

`just init-adroit` resolves
`${COMO_ADROIT_GIT:-${COMO_GIT_BASE:-https://github.com/como-technologies}/adroit.git}`,
probe-clones it (bare, blob-less) into the gitignored `.como/deps/adroit`
cache, verifies the `adroit.rev` rev exists there, and only then installs
with `cargo install --git <url> --rev <pin> --locked --root .conduit`; on
any remote failure (rev absent, no network, `COMO_OFFLINE=1`) it falls back
with a printed notice to `file://$(realpath ../adroit)` — the local-dev
override — and otherwise fails with the knob-naming error. Demo seeding
(`demo/gitea-init.sh`, `demo/playbook-demo-init.sh`) resolves the playbook
checkout as `PLAYBOOK_DIR`/`SEED_REPO_DIR` → `COMO_PLAYBOOK_DIR` → sibling
`../playbook` → `.como/deps/playbook` clone cache → hard error noting that
playbook has no public remote yet. A populated cache is never auto-updated;
refreshing it is an explicit `rm -rf .como/...` or a pin bump.
`CONDUIT_ADROIT_BIN` remains conduit's internal test seam for the pinned
install; `src/adroit.rs` and the token chain are untouched (already
env-first seams).
