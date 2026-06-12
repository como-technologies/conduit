# Testing

The gate is `just ci` = `fmt-check` + `clippy --all-targets -- -D warnings` +
`cargo test` + `mdbook build`. Everything in `just ci` is hermetic — no
network, no docker, no secrets. The live legs are env-gated extras that never
block CI but must each be shown passing in their task.

## Test inventory

### Unit tests (in `src/`)

- **`contract.rs`** — every tagging element: the closed effort-label set, the
  threshold table (boundaries are exclusive upper bounds), title/trailer/
  commit-message builders, slug normalization, and the anti-`adr/` guard
  (the branch builder can never emit adroit's namespace, proven against
  adversarial inputs).
- **`forge/mod.rs`** — the diff semantics: label add fires once / removal
  fires nothing, review dedupe by forge-native id (an edited review never
  re-fires), CI transitions, merged-emits-only-`PrMerged`, fresh-cursor
  replays, deterministic event ordering; plus `rest_call` status
  classification (2xx/401/403/other) through an injected fake transport.
- **`forge/gitea.rs`, `forge/github.rs`, `forge/gitlab.rs`** — adapter wire
  tests against fixture transports that panic on unexpected requests:
  snapshot filtering, terminal-item visibility,
  pagination-until-short-page, label id-vs-name(-vs-comma-string) quirks,
  CI status mapping, GitLab's synthesized approval-review identity.
- **`forge/dry_run.rs`** — reads delegate, mutations only reach the
  transcript, synthesized ids, the open-PR probe overlay.
- **`engine/fake.rs`, `engine/claude_code.rs`** — deterministic artifact
  bytes, scripted fail/hang, the task-document renderer, argv construction,
  the scrubbed-env allowlist, timeout-kills-the-child (a stub binary that
  sleeps), result-envelope parsing.
- **`git.rs`** — cache/workspace lifecycle against local bare repos, pathspec
  staging excluding the task document, the non-local push refusal.
- **`store.rs`** — atomic write round-trips, stale-tmp survival, intent
  marking, verbatim plan snapshots with sha256, per-forge cursors.
- **`adroit.rs`** — handshake accept/reject, tolerant serde, the subcommand
  allowlist (refused before spawn), and a source-walking test asserting no
  other module invokes the adroit binary.
- **`transcript.rs`** — the hermetic money shot: the execute leg (FakeForge +
  real git remote) and the record-only leg (DryRun, no git) produce
  byte-identical normalized streams; rerun adoption via the PR probe.
- **`cli.rs`** — the six `verify` check names are fixed; each contract
  violation fails exactly its named check; branch-shape and address parsing.

### `tests/machine.rs` — the transition table

The must-act table (every cell from
[State machine](./state-machine.md)) asserted cell-for-cell — next state,
action kinds, feedback op, attempt bump — for `has_pr` in {false, true}; then
an exhaustive sweep asserting every other (state, event, has-PR) combination
is the identity transition. `CiChanged`-everywhere, terminal-states-ignore-
everything, and the open-PR guard each get a named test.

### `tests/conformance.rs` — one suite, every adapter

One suite body (`run_conformance`) with a `Mutations` parameter:

- `Mutations::Real` — mutations execute and read-backs observe them.
- `Mutations::DryRun` — every mutation is still *called* (asserting `Ok`),
  but lands in the transcript; forge read-back assertions are skipped.

| Leg | When | What |
|---|---|---|
| `fake_forge_conforms` | always | full lifecycle, `Mutations::Real` |
| `github_recorded_fixtures_conform` | always, no network | reads from `tests/fixtures/github/`, mutations → transcript; asserts all 17 normalized lines |
| `gitlab_authored_fixtures_conform` | always, no network | reads from `tests/fixtures/gitlab/` (authored from the documented REST v4 shapes — ADR-0016), mutations → transcript; the same 17-line assertions as the GitHub leg, plus the merged-MR/closed-issue disappearance check |
| `gitea_live_conforms` | `CONDUIT_E2E_GITEA=1` after `just forge-up` | `Mutations::Real` against the throwaway container; seeds the PR head branch via the contents API first |
| `github_live_reads_conform` | `CONDUIT_E2E_GITHUB=1` (+ `GITHUB_TOKEN` or `gh auth login`) | live reads of the public fixture repo; mutations still DryRun-only |

Plus FakeForge-only contract tests: the disappearance rule, snapshot id
uniqueness, closing an unknown issue is a 404-shaped `Api` error.

### `tests/e2e_fake.rs` — lifecycle and crash behaviour

Router e2e over FakeForge + FakeEngine: the full Scoped → Merged lifecycle;
engine failure → Failed → relabel retry (attempt+1); hang → timeout →
Failed; PrClosed → Abandoned; a Revising task whose PR merges mid-run
discards the engine result; **kill/restart at every state converges**; a
**crash-replay test per mutating action kind** (issue, PR, push, comment,
labels) asserting exactly-once effect; and cursor-advances-only-after-actions.

### `tests/cli.rs` — the binary surface

`assert_cmd` against the real binary in a temp dir: help lists all
subcommands, `status -o json` on an empty store, invalid env overrides fail
loudly, `run --once` surfaces the typed offline error, `init` on an
unreachable forge fails but opens the store, `verify` refuses unknown and
unmerged tasks with non-zero exits, `plan` bails on terminal tasks without
invoking adroit, `plan` via a stub adroit creates a Scoped record (the
`CONDUIT_ADROIT_BIN` seam), the github demo-transcript leg emits
deterministic normalized JSONL twice over, and the gitlab leg is asserted
byte-identical to the github leg (both record-only — the hermetic two
thirds of the N=3 proof; the live gitea third is the demo-kit beat).

### `tests/adroit_contract.rs` — the planner seam

Hermetic by default via self-contained stub binaries (the `AdrSource` child
env is constructed — `env_clear` + allowlist — so canned JSON is embedded in
the stubs): handshake gate, plan-persisted-verbatim with sha, Accepted-only
enforcement, superseded-row skip, and the crate-wide allowlist. The pinned
binary runs the same assertions against `tests/fixtures/corpus/` behind
`CONDUIT_E2E_ADROIT=1`, and `src/adroit.rs` carries a live end-to-end test
(author a throwaway corpus via the binary, stored-plan read is deterministic
and provider-free) behind the same flag.

## Running things

```sh
just ci            # the full hermetic gate
just test          # cargo test (all hermetic suites)
just conformance   # the conformance suite only
just adr-check     # validate the in-repo adr/ corpus (needs `just init-adroit`)

# Env-gated live legs (each needs its prerequisite):
just forge-up && CONDUIT_E2E_GITEA=1  cargo test --test conformance gitea_live
CONDUIT_E2E_GITHUB=1 cargo test --test conformance github_live   # GITHUB_TOKEN / gh auth
just init-adroit && CONDUIT_E2E_ADROIT=1 cargo test adroit
CONDUIT_E2E_CLAUDE=1 cargo test claude   # live `claude` CLI smoke
```

## Recording the GitHub fixtures

The recorded-fixture leg reads canned API bodies from
`tests/fixtures/github/` (`issues.json`, `pulls.json`, `reviews_<n>.json`,
`status_<sha>.json`, …), recorded from a small public repo — reads only,
never mutations. To re-record after the fixture repo changes:

```sh
GITHUB_TOKEN=$(gh auth token) cargo test --lib github::record_fixtures -- --ignored
```

The recorder (`src/forge/github.rs`) fetches each endpoint the snapshot path
uses and writes the raw bodies into the fixture dir; commit the changed
files. The fixture transport panics on any request without a recorded route,
so adapter drift surfaces as a loud test failure, not silent staleness.

## The GitLab fixtures are authored, not recorded

`tests/fixtures/gitlab/` (`issues.json`, `merge_requests.json`,
`approvals_<iid>.json`, `pipelines_<sha>.json`, `labels.json`) is authored
from the **documented** REST v4 response shapes — no sanctioned GitLab host
exists to record from: a local `gitlab-ce` container is far too heavy for
the demo rig, and recording from gitlab.com would require a real project
there (ADR-0016 records the trade and the upgrade path — a recorder
mirroring `github::record_fixtures` plus an env-gated `CONDUIT_E2E_GITLAB=1`
live leg, when an owner sanctions an instance). The fixture transport panics
on unknown routes here too, so the same drift-detection property holds.
