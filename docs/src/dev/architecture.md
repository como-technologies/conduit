# Architecture

One crate, `conduit` (`bin` + `lib`), fully synchronous — no tokio, no
database, no framework. The binary is clap marshalling over a library whose
core is pure functions and whose effects are funneled through one module.
The founding decisions live in the in-repo `adr/` corpus (authored with the
pinned adroit; see [Spike design](./spike-design.md)).

## Module map

```
conduit/
├── Cargo.toml             single crate `conduit`, bin+lib
├── adroit.rev             the single adroit pin location (read by `just init-adroit`)
├── justfile               init / init-adroit / ci / adr-check / forge-up / forge-down / demo-trigger / conformance
├── CLAUDE.md              working agreements (no publishing, docs in mdbook, no client names)
├── adr/                   conduit's OWN adroit corpus (ADR-0001..0013, accepted: 7 founding + 6 retirement)
├── docs/                  this mdbook
├── demo/docker-compose.yml  throwaway Gitea (localhost:3000, named volume, disposable)
├── demo/gitea-init.sh     two-user bootstrap, labels, seeded repo (SEED_REPO_DIR/REPO_NAME parameterize)
├── demo/demo-trigger.sh   scripted human gate: reviewer labels the issue conduit:run (REPO_NAME)
├── demo/playbook.conduit.toml   the documented config of the playbook-corpus demo
├── demo/playbook-demo-init.sh   per-run unique demo workdir (conduit.toml + .secrets/bin links)
├── src/main.rs            thin binary: clap parse + dispatch (anyhow only here and in cli.rs)
├── src/cli.rs             init | plan <address> | run [--once] | status | verify <address> | demo-transcript <address>
├── src/config.rs          conduit.toml + env overlay (CONDUIT_FORGE/ENGINE/TIMEOUT_SECS/POLL_SECS)
├── src/forge/mod.rs       THE KEYSTONE: trait Forge, snapshot types, the shared pure diff(), HttpTransport
├── src/forge/github.rs    GitHub REST v3 — reads live; the only constructors hand out DryRun(GitHubForge)
├── src/forge/gitea.rs     Gitea REST v1 — localhost, the full read-write lifecycle host
├── src/forge/fake.rs      in-memory FakeForge (scripted snapshots, records actions)
├── src/forge/dry_run.rs   DryRunForge decorator: reads delegate, mutations → normalized transcript
├── src/engine/mod.rs      trait Engine + TaskSpec/EngineOutcome — the subprocess contract + run_timed
├── src/engine/claude_code.rs  sandboxed `claude -p` runner (constructed env, hard timeout)
├── src/engine/fake.rs     FakeEngine (deterministic) + scripted fail/hang modes
├── src/adroit.rs          AdrSource: handshake, list/show/plan, subcommand allowlist
├── src/contract.rs        ALL tuesday-contract emission — pure, exhaustively tested
├── src/task.rs            TaskRecord (id, adr fields, state, branch, ids, attempt, work_ms, pending intents)
├── src/machine.rs         pure step(&TaskRecord, &Event) -> Transition { next, actions, feedback, bump_attempt }
├── src/router.rs          the tick loop: fetch → diff → step → execute actions → persist; owns ALL effects
├── src/store.rs           .conduit/ file store: atomic tmp+rename+fsync, write-ahead intents, cursors
├── src/git.rs             local bare cache, workspaces, commit/push — the ONLY authenticated-remote call site
├── src/transcript.rs      demo-transcript machinery + THE shared action normalization (used by dry_run too)
└── tests/
    ├── machine.rs         table tests over every (state, event, has_pr) cell incl. must-ignore
    ├── conformance.rs     ONE suite vs FakeForge + GitHub fixtures (always); live legs behind env flags
    ├── e2e_fake.rs        full lifecycle, kill/restart at every state, crash-replay per action kind
    ├── cli.rs             binary-level: help, status, env validation, typed errors, stub-adroit plan
    └── adroit_contract.rs handshake gate, Accepted-only, superseded skip, allowlist, pinned-binary leg
```

## Pure core, effectful shell

The pure core — no I/O, exhaustively unit-tested:

- **`contract.rs`** — every tagging element the Measure stage reads: the
  closed effort-label set, `adr:<reference>`, `[ADR-NNNN] ` titles, the
  `Adr-Reference:` trailer, branch names (structurally unable to emit
  adroit's `adr/` namespace), the hidden task marker, the effort-bucket map.
- **`machine.rs`** — `step(&TaskRecord, &Event) -> Transition`: the whole
  lifecycle as one exhaustive match. See [State machine](./state-machine.md).
- **`forge::diff`** — `diff(prev, next) -> Vec<ForgeEvent>`: all event
  semantics, defined once for every forge. See
  [Forge contract](./forge-contract.md).
- **`transcript::normalize_action`** — the one normalization both transcript
  producers share.

The effectful shell:

- **`router.rs`** owns every effect: forge calls, engine runs, git
  operations, store writes. Per transition it persists write-ahead intents
  *before* executing, executes probe-first, marks each intent done, and
  advances the poll cursor only after the whole tick's actions complete —
  crash anywhere converges on restart (at-least-once execution, exactly-once
  effect).
- **`store.rs`** is plain files under `.conduit/` (`tasks/`, `plans/`,
  `cursor/`, `cache/`, `workspaces/`, `bin/`), every write atomic via
  tmp + fsync + rename + parent-dir fsync. `cat .conduit/tasks/*.json` shows
  the whole lifecycle.
- **`git.rs`** is the only module that ever sees an authenticated remote URL,
  and its push helper refuses non-local remotes (spike hard constraint).

## The seams

Every external dependency sits behind a trait with an injectable fake:

| Seam | Trait / type | Production | Test double |
|---|---|---|---|
| Forge API | `forge::Forge` | `GiteaForge`, `DryRun(GitHubForge)` | `FakeForge`, `DryRunForge` |
| HTTP wire | `forge::HttpTransport` | `UreqTransport` (blocking, rustls, bounded timeouts) | fixture/fake transports per adapter test |
| Coding engine | `engine::Engine` | `ClaudeCodeEngine` (`claude -p`, sandboxed) | `FakeEngine` (`complete`/`fail`/`hang` modes) |
| Planner | `adroit::AdrSource` (subprocess) | pinned `.conduit/bin/adroit` | stub binaries + `CONDUIT_ADROIT_BIN` env seam |

Seam rules that hold everywhere:

- Adapters never produce events; they implement `fetch_snapshot()` and the
  shared `diff` derives events — neutrality by construction (ADR-0002).
- Engines never see credentials or authenticated remotes: workspaces are
  cloned from the local bare cache, and engine/adroit/git subprocesses all
  get **constructed** environments (`env_clear()` + allowlist), never
  inherited ones (ADR-0004).
- `src/adroit.rs` is the only adroit call site, hardcoded to
  `{manifest, list, show, plan}`; a source-walking test asserts no other
  module invokes the binary (the conduit/adroit lane boundary, in code).
- The GitHub adapter's only public constructors return
  `DryRun(GitHubForge)` — mutating github.com is unrepresentable in the
  spike.

## Layering rules

- Typed errors (`thiserror`) in lib modules; `anyhow` only at the binary
  edge (`main.rs`/`cli.rs`/`router.rs` orchestration).
- Pure modules (`contract`, `machine`, `forge::diff`) may not perform I/O;
  the router may not embed contract knowledge — it calls `contract::*`
  builders for every emitted string.
- No test-only state in production types: behavior is injected via the fakes
  above plus documented env overrides (`CONDUIT_FAKE_ENGINE_MODE`,
  `CONDUIT_ADROIT_BIN`).
- Dependencies are the short earned list: `clap`, `serde`/`serde_json`,
  `ureq`, `thiserror`/`anyhow`, `time`, `sha2`, `toml` (ADR-0001).
