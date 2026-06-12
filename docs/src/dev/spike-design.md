# Spike design — the forge-adapter keystone

> Status: spec for the 1–2 week spike. Written 2026-06-11, synthesized from a
> three-architecture judge panel. This page is the normative design; the
> decisions it locks are also recorded as ADRs in the in-repo `adr/` corpus.

## One line

conduit is the Adopt-stage engine of the TAPS loop: a thin, forge-neutral
harness that turns accepted ADRs (from adroit) into driven work inside a team's
existing issue tracker and pull requests — on their forge, their cloud, their
model — while humans keep every gate.

## What the spike must prove

The pitch's net-new IP is exactly three things, and the spike builds only them:

1. **A forge-neutral event router** — forge state in, normalized events out.
2. **A PR lifecycle state machine** — pure, restart-safe, humans at every gate.
3. **The forge adapter** — one trait that GitHub and a self-hosted forge
   (Gitea) implement *identically*, proven by a shared conformance suite and a
   transcript-diff demo.

Everything else is commodity behind a seam: the coding engine is a subprocess
(Claude Code today, OpenHands later), the planner is adroit, the model is
whatever the engine/adroit are configured with (free local Ollama by default).

**Hard constraints.** All work stays under `~/repos/como-tech/**`. Nothing is
ever pushed to a real remote; no PR is opened on any public forge. The
self-hosted forge is a throwaway Gitea container on localhost. The GitHub
adapter is exercised live for **reads only**; all mutations go through a
dry-run transcript decorator. conduit never authors, edits, or transitions an
ADR — that is adroit's lane.

## Stack

Rust, single crate (`bin` + `lib`), **fully synchronous** — no tokio. Rust is
the house stack (adroit, tuesday, assessments), so the justfile / mdbook /
thiserror conventions and adroit's `HttpTransport` fake-injection pattern
transfer directly. Sync because conduit is a poll-tick loop driving one task at
a time; blocking `ureq` behind a transport seam keeps the layer thin. The
commodity pieces are *process* boundaries (`claude -p`, `adroit ... -o json`),
so an async runtime and Python interop buy nothing.

Dependencies: `clap`, `serde`/`serde_json`, `ureq` (rustls), `thiserror` (typed
core) + `anyhow` (binary), `time`, `sha2`. No database — state is files you can
`cat` under `.conduit/`. The running postgres/valkey containers are
deliberately not used.

## Module layout

```
conduit/
├── Cargo.toml             single crate `conduit`, bin+lib
├── adroit.rev             the single adroit pin location (git rev, read by `just init-adroit`)
├── justfile               init / init-adroit / ci / forge-up / forge-down / demo / conformance
├── CLAUDE.md              working agreements (no publishing, docs in mdbook, no client names)
├── adr/                   conduit's OWN adroit corpus — the dogfood input
├── docs/                  this mdbook
├── demo/docker-compose.yml  throwaway Gitea (localhost:3000, named volume, disposable)
├── demo/gitea-init.sh     two-user bootstrap (see Dogfood), labels, seeded repo
├── src/main.rs            thin binary: clap marshalling + human rendering
├── src/cli.rs             init | plan <address> | run [--once] | status | verify <address> | demo-transcript
├── src/config.rs          conduit.toml + env overlay
├── src/forge/mod.rs       THE KEYSTONE: trait Forge, snapshot types, shared pure diff()
├── src/forge/github.rs    GitHub REST v3 (live reads; mutations always decorated by dry_run)
├── src/forge/gitea.rs     Gitea REST v1 (localhost, full read-write lifecycle host)
├── src/forge/fake.rs      in-memory FakeForge (scripted snapshots, records actions)
├── src/forge/dry_run.rs   DryRunForge decorator: reads delegate, mutations → transcript JSONL
├── src/engine/mod.rs      trait Engine + TaskSpec/EngineOutcome — the subprocess contract
├── src/engine/claude_code.rs  sandboxed `claude -p` runner
├── src/engine/fake.rs     FakeEngine (deterministic) + scripted fail/hang modes
├── src/adroit.rs          AdrSource: handshake, list/show/plan, allowlist, plan snapshot
├── src/contract.rs        ALL tuesday-contract emission, pure and exhaustively tested
├── src/task.rs            Task model (id, adr_reference, state, branch, ids, attempt, work_ms)
├── src/machine.rs         pure step(&TaskRecord, &Event) -> Transition { next, actions }
├── src/router.rs          tick loop: fetch → diff → step → execute actions → persist
├── src/store.rs           .conduit/ file store, atomic tmp+rename, write-ahead intents
├── src/git.rs             local bare cache, workspaces, branch/stage/commit/push
└── tests/
    ├── machine.rs         table tests over every (state, event) pair incl. must-ignore cells
    ├── conformance.rs     ONE suite vs FakeForge always; vs live Gitea / GitHub-reads behind env flags
    ├── e2e_fake.rs        full lifecycle + kill/restart at every state + crash-replay per action kind
    └── adroit_contract.rs handshake, Accepted enforcement, superseded skip, snapshot-verbatim
```

## The forge adapter (net-new IP)

Adapters do **not** implement per-forge event APIs — that is where forges
diverge worst. Each adapter implements `fetch_snapshot()`; a single **pure
shared** `diff(prev, next) -> Vec<ForgeEvent>` in `forge/mod.rs` derives
events. Event semantics are defined once, so GitHub and Gitea behave
identically *by construction*.

```rust
pub trait Forge {
    fn describe(&self) -> String;
    fn git_remote_url(&self) -> Result<String, ForgeError>; // used ONLY by src/git.rs, never by engines
    // events in: one read, normalized (conduit-labeled issues + conduit/* -branch PRs only)
    fn fetch_snapshot(&self) -> Result<RepoSnapshot, ForgeError>;
    // idempotency probes (reads)
    fn find_open_pr_by_head(&self, branch: &str) -> Result<Option<PrId>, ForgeError>;
    fn find_issue_by_marker(&self, marker: &str) -> Result<Option<IssueId>, ForgeError>;
    // actions out — NO merge method exists: humans merge in the forge UI; the gate is unrepresentable
    fn ensure_labels(&self, labels: &[LabelSpec]) -> Result<(), ForgeError>;
    fn create_issue(&self, new: &NewIssue) -> Result<IssueId, ForgeError>;   // body carries a hidden marker
    fn upsert_issue_comment(&self, id: &IssueId, marker: &str, body: &str) -> Result<(), ForgeError>;
    fn set_issue_labels(&self, id: &IssueId, labels: &[String]) -> Result<(), ForgeError>;
    fn close_issue(&self, id: &IssueId) -> Result<(), ForgeError>;
    fn open_pr(&self, draft: &PrDraft) -> Result<PrId, ForgeError>;          // head branch already pushed by conduit
    fn upsert_pr_comment(&self, id: &PrId, marker: &str, body: &str) -> Result<(), ForgeError>;
    fn set_pr_labels(&self, id: &PrId, labels: &[String]) -> Result<(), ForgeError>;
}

pub struct RepoSnapshot { pub issues: Vec<IssueSnapshot>, pub prs: Vec<PrSnapshot>, pub fetched_at: OffsetDateTime }
pub struct IssueSnapshot { pub id: IssueId, pub labels: Vec<String>, pub closed: bool }
pub struct PrSnapshot {
    pub id: PrId, pub head_branch: String, pub labels: Vec<String>,
    pub reviews: Vec<Review>,   // Review { id: ReviewId (forge-native), author, verdict, body, submitted_at }
    pub ci: CiState,            // Pending | Success | Failure | None — consumed, never configured
    pub merged: bool, pub merge_sha: Option<String>, pub closed: bool,
}

pub enum ForgeEvent {            // produced ONLY by the shared diff
    IssueLabeled   { issue: IssueId, label: String },     // `conduit:run` = the human trigger
    ReviewSubmitted{ pr: PrId, review: Review },          // deduped on forge-native Review.id
    CiChanged      { pr: PrId, state: CiState },
    PrMerged       { pr: PrId, merge_sha: String },
    PrClosed       { pr: PrId },
}
```

**Review identity.** `Review` carries the forge-native review id and
`submitted_at`; the diff dedupes on id, so an *edited* review never re-fires
and repeated `ChangesRequested` rounds from the same reviewer are distinct
events. (Snapshot-diff semantics: state that flaps within one poll interval —
submitted-then-dismissed — is invisible by design; documented contract.)

**Polling, not webhooks** (spike decision): works against localhost Gitea and
read-only GitHub with zero inbound exposure, matches partners whose forge sits
on a private network, and keeps adapters thin. A webhook receiver is a future
*second producer* of the same `ForgeEvent` stream feeding the unchanged
router. Cursor = the previous `RepoSnapshot`, persisted per forge.

**Idempotency: probe before reissue.** Every mutating action kind has a
defined replay guard, exercised by a dedicated crash-replay test:

| Action | Probe |
|---|---|
| `create_issue` | `find_issue_by_marker(task marker)` |
| `open_pr` | `find_open_pr_by_head(branch)` |
| push branch | `git ls-remote` compare before push |
| comments | marker upsert (hidden HTML marker, adroit's pattern) |
| labels | convergent set (`set_*_labels` is absolute, not additive) |

**Implementations.** `gitea.rs` (REST v1, full read-write — the real lifecycle
host), `github.rs` (REST v3; reads live via `GITHUB_TOKEN`; in this spike the
constructor only ever hands out `DryRun(GitHubForge)`), `fake.rs` (in-memory,
scripted snapshot sequences, records actions). Both HTTP adapters sit on the
`HttpTransport` seam (ureq in prod, fake transport in unit tests, **recorded
fixtures** for the GitHub conformance leg so CI needs no network; a live
read-only run is available behind `CONDUIT_E2E_GITHUB=1`).

`tests/conformance.rs` is one parameterized suite run against all three
implementations — "identically" is a CI assertion, not a slogan.

## Lifecycle state machine

Seven states, two terminal. Humans hold every gate (scope, review, merge):

```
Scoped ──(IssueLabeled conduit:run — HUMAN)──▶ Coding
Coding ──(EngineFinished Completed)──▶ InReview     commit, push, open PR (full tuesday tagging), link comment
Coding ──(EngineFinished Failed/Timeout)──▶ Failed  comment w/ log tail, label conduit:run → conduit:failed
Failed ──(IssueLabeled conduit:run — HUMAN)──▶ Coding (attempt+1, fresh workspace)
InReview ──(ReviewSubmitted ChangesRequested — HUMAN)──▶ Revising   engine re-runs w/ feedback, same branch
Revising ──(EngineFinished Completed)──▶ InReview   commit, push, recompute effort label (swap, still exactly one)
Revising ──(EngineFinished Failed/Timeout)──▶ Failed
InReview ──(PrMerged — HUMAN merges in forge UI)──▶ Merged (terminal)   close issue, completion comment w/ sha
InReview ──(PrClosed without merge — HUMAN)──▶ Abandoned (terminal)     close issue w/ comment
```

**`PrMerged`/`PrClosed` are must-act from *any* non-terminal state whose task
has an open PR** (`InReview`, `Revising`, and `Failed`-after-a-PR-exists) —
the diff is edge-triggered and the cursor advances, so an ignored terminal
event would wedge the task forever. A `Revising` task whose PR merges or
closes mid-engine-run transitions immediately; the in-flight engine result is
discarded and its workspace disposed. **`CiChanged` is must-ignore in every
state for the spike** — conduit consumes the event type but takes no action on
it (wiring CI failure into `Revising` would bypass the human gate); it exists
so the snapshot/diff layer is proven against CI-bearing forges.

`machine::step(&TaskRecord, &Event) -> Transition { next, actions }` is a pure
function — zero I/O, exhaustive match, table-tested over every (state, event)
pair including must-ignore cells. `Action` spans forge calls, `RunEngine`, and
git ops; `router.rs` executes them and owns all effects.

**Crash consistency (file store, defined ordering).** Per transition:
(1) persist new state + pending action intents to `.conduit/tasks/<id>.json`
via tmp+rename+fsync **before** executing anything; (2) execute each action,
probe-first; (3) mark it done in the record; (4) advance the forge cursor only
after the tick's actions complete. A crash at any point converges on restart:
pending intents re-execute behind their probes (at-least-once execution,
exactly-once effect).

**Restart recovery (auto, demoed by kill mid-Coding).** On boot: pending
intents reconcile as above; a task found in `Coding`/`Revising` with no live
engine gets its stale workspace **disposed** and the engine re-runs in a fresh
workspace from the immutable plan snapshot — engines are disposable, the
snapshot is truth. A task in `Scoped`/`InReview` simply resumes polling.
`Failed` is reserved for engine-reported failure or timeout, never for
interruption.

## The engine seam (commodity)

The contract is deliberately dumb: *given a prepared git workspace and an
instruction document, edit files; report success.* Conduit owns all git and
all forge interaction.

```rust
pub struct TaskSpec {
    pub adr_reference: String,            // "ADR-0003"
    pub title: String,
    pub adr_body: String,                 // AdrDetail body markdown
    pub plan_markdown: String,            // the VERBATIM persisted plan snapshot
    pub review_feedback: Option<String>,  // ChangesRequested bodies of the CURRENT round only:
                                          // reviews received since the task last entered InReview
    pub workspace: PathBuf,               // already on branch conduit/<ref-lower>/<slug>
}
pub enum EngineOutcome { Completed { summary: String }, Failed { reason: String, log_tail: String } }
pub trait Engine {
    fn describe(&self) -> String;
    fn run(&self, spec: &TaskSpec) -> Result<EngineOutcome, EngineError>;
}
```

**Sandbox — structural, not conventional.** The workspace is cloned from a
**local bare cache** (`.conduit/cache/<forge>.git`), so the engine's `origin`
is a filesystem path containing no credentials. The engine subprocess runs
with all forge/AI tokens **scrubbed from its environment**. Only conduit's
`git.rs` ever touches an authenticated remote URL (cache fetch + final push,
local Gitea only). `ClaudeCodeEngine` invokes:

```
claude -p "Implement the plan in .conduit-task.md. Edit files in this directory only." \
  --output-format json \
  --permission-mode acceptEdits \
  --disallowedTools "Bash(git push:*),Bash(git remote:*),WebFetch,WebSearch"
```

with `cwd = workspace`, a conduit-enforced hard timeout (timeout ⇒
`Failed`), wall-clock measured (feeds the effort bucket), and the JSON result
envelope parsed for the summary. Verify the flags against the installed CLI
during implementation; adjust to whatever the CLI actually supports.

**Committing.** Conduit deletes `.conduit-task.md` and stages by pathspec
(everything except conduit's own artifacts) — the task file never lands in a
PR. Commit message: `[ADR-0003] <title>` + blank line + `Adr-Reference:
ADR-0003` trailer. Conduit pushes; the engine cannot.

**Fakes.** `FakeEngine` is fully deterministic (writes
`docs/impl/<ref-lower>.md` containing the title + SHA-256 of the plan
snapshot) — the default demo path. Scripted modes `fail` and `hang` drive the
`Failed` and timeout transitions as first-class tested paths. OpenHands fits
the same seam later (mounted-volume container, LiteLLM any-model) — deferred,
named.

## adroit integration (read-only, allowlisted)

- **Pin:** `adroit.rev` at repo root holds the git rev. `just init-adroit`
  builds it from `file:///home/brett/repos/como-tech/adroit` at that rev
  (`cargo install --git ... --locked --root .conduit`) — no network.
- **Handshake:** at startup run `adroit manifest -o json`; require
  `tool == "adroit" && manifest_schema == 1`, else bail loudly.
- **Allowlist:** `src/adroit.rs` is the only adroit call site, hardcoded to
  `{manifest, list, show, plan}`; a test asserts no other adroit subcommand
  string exists in the crate. The Conduit/adroit lane boundary is enforced in
  code, not convention.
- **Enumerate:** `ADROIT_DIR=<corpus> adroit list --status accepted -o json`
  → `AdrSummary[]`; skip rows with `superseded_by != null`. Address ADRs by
  the `address` field; display by `reference`. Serde is tolerant: require the
  contracted fields, deny nothing (additive drift on adroit main must not
  break the pinned client).
- **Guard:** conduit enforces `status == "Accepted"` itself before planning —
  adroit does not; this guard has its own test.
- **Plan snapshot:** `adroit plan <address> -o json` (env
  `ADROIT_AI_PROVIDER=ollama ADROIT_AI_MODEL=llama3.2` supplied by conduit;
  Anthropic key optional via config). The returned markdown is persisted
  **verbatim** to `.conduit/plans/<task-id>.md` (fsync, sha256 recorded) before
  the task leaves `Scoped`-creation; it is never regenerated — regeneration is
  nondeterministic. Replanning = cancel + new task.
- **MCP:** not used in the spike. The engine gets ADR context inlined in
  `TaskSpec`. The future shape — `adroit mcp` behind a read-only allowlist
  proxy (never `review`/`plan`/`summarize`, which leak forge/file writes via
  args) — is recorded as a deferred ADR.

## The tuesday contract (Measure handoff)

All emission lives in **`src/contract.rs`** — one pure module, exhaustively
unit-tested, the single place this contract can drift:

| Element | Value |
|---|---|
| Effort label | exactly ONE of `effort:1-super-quick` `effort:2-not-long` `effort:3-average` `effort:4-a-while` `effort:5-felt-like-forever` (closed enum; set pre-created by `conduit init`) |
| ADR label | `adr:<reference>`, e.g. `adr:ADR-0003` |
| PR title | prefix `[ADR-0003] ` |
| PR body | final line is the trailer `Adr-Reference: ADR-0003` |
| Commits | same `[ADR-0003]` prefix + `Adr-Reference` trailer |
| Branch | `conduit/<reference-lower>/<task-slug>`; a unit test proves the builder can never emit the `adr/` prefix (adroit's namespace) |

Effort is mapped from cumulative engine wall-clock (defaults `<10m=1, <30m=2,
<2h=3, <8h=4, else 5`; thresholds in `conduit.toml`), applied at PR open and
recomputed-and-swapped after each `Revising` push. **The label is final at
merge time** — that is the moment tuesday reads it; "exactly one" is enforced
structurally (`set_pr_labels` with the chosen label, the other four absent).

**`conduit verify <task> -o json`** re-reads the merged PR from the live forge
API and machine-asserts every element above (title regex, trailer-as-final-line,
exactly-one-effort, adr label, branch regex, never-`adr/`). It is the
executable spec the tuesday-side consumer is built against, and the demo's
closing beat.

## Self-dogfood

conduit's own architectural decisions are authored **with adroit** into the
in-repo `adr/` corpus during the spike (Rust-single-crate, snapshot-diff
router, filesystem store, engine sandbox, effort semantics, MCP deferral …).
The demo is conduit reading its **own** accepted ADRs and driving work on its
**own** repo via the throwaway forge — the portfolio feeding itself. The
in-repo corpus keeps adroit's forge integration disabled, so adroit never
opens `adr/`-branch PRs on the demo forge.

**Two-user Gitea bootstrap** (`demo/gitea-init.sh`): `conduit-bot` authors
issues/PRs with its token; `reviewer` approves and merges with a separate
token (Gitea restricts self-approval). Tokens land in gitignored `.secrets/`.
The script provisions org `como`, repo `conduit-dogfood` seeded by pushing this
repo, and pre-creates the five `effort:*` + `conduit:*` labels.

## Demo script

```sh
just init && just init-adroit          # toolchain; pinned adroit → .conduit/bin (manifest handshake)
just forge-up                          # throwaway Gitea on localhost:3000, two users, labels, seeded repo
ADROIT_DIR=adr .conduit/bin/adroit list --status accepted -o json   # the dogfood input: conduit's own ADRs
                                       # (ADROIT_DIR is the env form of --dir; conduit always uses the env form)

conduit plan 3 --forge gitea           # handshake → show 3 → ENFORCE Accepted → adroit plan 3 (ollama)
                                       # → persist plan VERBATIM → issue on Gitea (plan body, adr: label). Scoped.
# HUMAN GATE: label the issue `conduit:run` in the Gitea UI   (scripted: just demo-trigger)

conduit run --forge gitea              # poll → diff → Coding: clone from local cache, branch
                                       # conduit/adr-0003/<slug>, engine edits (sandboxed), conduit commits
                                       # + pushes, opens PR w/ full tagging. InReview.
# HUMAN GATES in Gitea: Request changes → Revising → InReview; Approve + Merge → Merged.
cat .conduit/tasks/*.json              # the whole lifecycle, inspectable as files
# Restart beat: kill -9 conduit mid-Coding; rerun `conduit run` — fresh workspace, resumes from the
# plan snapshot; crash-replay probes mean no duplicate issue/PR/comment.

conduit verify 3 --forge gitea -o json # closing beat: machine-asserts the tuesday contract on the merged PR
                                       # (tasks are addressed by ADR address while one ADR = one task holds)

# THE FORGE-NEUTRALITY MONEY SHOT — same scripted scenario, two adapters, identical normalized stream:
conduit demo-transcript 3 --forge gitea            > /tmp/t-gitea.jsonl
conduit demo-transcript 3 --forge github           > /tmp/t-github.jsonl   # always DryRun-decorated
diff /tmp/t-gitea.jsonl /tmp/t-github.jsonl && echo "FORGE-NEUTRAL: identical"

CONDUIT_ENGINE=claude-code conduit run --forge gitea --once   # encore: the real engine
just forge-down                        # the forge is destroyed; nothing ever left localhost
```

**Transcript-diff semantics (pinned down).** `demo-transcript` does not poll:
it feeds a **fixture event sequence** (the same `ForgeEvent` stream on both
runs) through the real state machine with `FakeEngine`, emitting every
resulting action through (a) the live Gitea adapter and (b)
`DryRun(GitHubForge)`. Each emitted action is serialized in normalized form
with **stable redaction**: forge-assigned ids → `$ISSUE_1`/`$PR_1`
placeholders in first-seen order, timestamps and durations omitted, the effort
label value redacted (it derives from wall-clock; redaction is
transcript-only — the real label is always present on real PRs), repo slug →
`$REPO`. The diff being empty proves the *action-side* normalization is
identical; the *read-side* is proven by the conformance suite (live Gitea +
GitHub read-only + recorded fixtures). Honest claim, both halves covered.

## Testing

- `tests/machine.rs` — every (state, event) pair, including must-ignore cells.
- `tests/conformance.rs` — one parameterized suite vs FakeForge (always), live
  Gitea (`CONDUIT_E2E_GITEA=1`), GitHub reads live (`CONDUIT_E2E_GITHUB=1`)
  and via recorded transport fixtures (always, no network).
- `tests/e2e_fake.rs` — full lifecycle on FakeForge+FakeEngine; kill/restart
  at every state; a crash-replay test per mutating action kind (issue, PR,
  push, comment, labels) asserting exactly-once effect; engine `fail` and
  `hang` fixtures driving Failed/timeout.
- `tests/adroit_contract.rs` — handshake gate, Accepted-only, superseded skip,
  plan-snapshot-verbatim, subcommand allowlist, against a fixture corpus.
- `src/contract.rs` unit tests — every tagging element, anti-`adr/` guard.
- `just ci` = fmt-check + clippy + test + book build (the house gate).

## Out of scope (named, deferred)

GitLab adapter (the third impl is the post-spike N>2 proof) · webhook
ingestion (second producer, same events) · OpenHands engine + LiteLLM routing
· MCP exposure of adroit to engines · task decomposition (one ADR = one task =
one PR) · concurrent tasks · multi-repo/multi-tenant/hosting/auth · deploy
stage (human gate, outside the loop) · web dashboard · postgres/valkey ·
automated rebase/conflict resolution (conflict ⇒ Failed + human) · AI effort
estimation · in-flight replanning · CI provisioning (events consumed, never
configured) · any real mutation of github.com, any push to any real remote.

## Risks (accepted, mitigated)

- **Snapshot fidelity across forges** is the real keystone risk (review
  states, label timing, merge detection differ) — minimal `RepoSnapshot`, one
  shared diff, conformance suite against live Gitea; expect adapter iteration
  to dominate week 1.
- **Dry-run proves the stream, not GitHub's acceptance** of every payload —
  reads verified live; mutation payloads schema-checked + fixture-verified;
  named residual gap until a sacrificial private repo is authorized.
- **Engine nondeterminism** — FakeEngine is the default demo path; the real
  engine is the encore; timeout ⇒ Failed is first-class.
- **llama3.2 plan quality** — plans are human-visible on the issue *before*
  the `conduit:run` gate; bad plans are a content problem, not a contract
  problem; Anthropic-key upgrade path in config.
- **Scope creep into agent-framework territory** — the existential risk to
  the "thin layer" pitch; guarded by the OUT list, the engine sandbox, the
  adroit allowlist, and ADRs recording each boundary.
