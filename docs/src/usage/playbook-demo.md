# Playbook-corpus demo

The iteration-2 dogfood beat: conduit drives work on the **playbook** — the
generic Como product-template corpus — instead of its own repo. Same loop,
different corpus, proving the demo machinery is parameterized rather than
hardwired to conduit's self-dogfood. Validated end-to-end against the
throwaway forge on 2026-06-12; every output below is captured from that run.

Two demo-shape changes over the [original walkthrough](./demo.md):

1. **Parameterized seeding.** `demo/gitea-init.sh` takes `SEED_REPO_DIR`
   (which local repo's `main` seeds the forge) and `REPO_NAME` (the forge
   repo under org `como`). Defaults preserve the self-dogfood demo
   (`.`/`conduit-dogfood`); token filenames stay pinned at
   `.secrets/conduit-bot.token` and `.secrets/reviewer.token` either way.
   `demo/demo-trigger.sh` takes the same `REPO_NAME`.
2. **Per-run unique workdirs.** Run 1 taught that the repo's shared
   `.conduit/` store is not single-writer: two flows interleaving in one
   store stomp each other's cursors and task records. The demo flow now
   writes ALL its state under a caller-supplied or timestamped workdir
   created by `demo/playbook-demo-init.sh` — never a shared fixed path.

## 1. Forge up, seeded with the playbook

```sh
SEED_REPO_DIR=../playbook REPO_NAME=playbook just forge-up
```

Captured:

```text
created user conduit-bot
created user reviewer
minted token for conduit-bot -> .secrets/conduit-bot.token
minted token for reviewer -> .secrets/reviewer.token
To http://localhost:3000/como/playbook.git
 * [new branch]      main -> main
forge ready: http://localhost:3000 (org como, repo playbook; tokens in .secrets/)
```

## 2. The per-run workdir

```sh
RUN_DIR=$(bash demo/playbook-demo-init.sh)
cd "$RUN_DIR"
```

The script refuses to reuse an existing dir (unique per run; default
`demo/runs/<UTC timestamp>`, gitignored) and stocks the workdir with
everything `conduit` resolves from its cwd:

- `conduit.toml` — `demo/playbook.conduit.toml` (the demo's exact,
  fully-documented config: gitea `como/playbook`, fake engine, `[adroit]
  dir` resolved to the playbook checkout's `src/adrs`)
- `.secrets` — symlink to the repo's gitignored token dir
- `.conduit/bin` — symlink to the pinned adroit install

Captured:

```text
demo workdir ready: .../conduit/demo/runs/20260612T134927Z (adroit corpus: .../playbook/src/adrs)
```

Task records, plan snapshots, cursors, the git cache, and engine workspaces
all land under `<workdir>/.conduit/` — inspectable, disposable, and never
shared with another run. (This one-workdir-per-corpus shape is also the
supported multi-repo answer per ADR-0011.)

## 3. The dogfood input

```sh
conduit init
.conduit/bin/adroit --dir <playbook>/src/adrs list --status accepted -o json
```

Four accepted generic decisions; ADR-0001 and ADR-0004 carry **stored**
plans:

```text
ADR-0001 Adopt trunk-based development with short-lived branches
ADR-0002 Require ADRs for cross-team architectural decisions
ADR-0003 Pin and audit third-party dependencies in CI
ADR-0004 Maintain a glossary of shared engineering terms in the playbook
```

## 4. Plan → trigger → run → review → merge → verify

```sh
conduit plan 1                       # stored plan: deterministic, no AI env
REPO_NAME=playbook just demo-trigger # reviewer labels issue 1 conduit:run
conduit run --once                   # Scoped -> Coding -> InReview
# reviewer approves + merges PR 2 via the API (in real life: the Gitea UI)
conduit run --once                   # observes PrMerged -> Merged
conduit verify 1 -o json             # the executable tuesday contract
```

Captured — the plan read was stored (no ollama, no AI env anywhere in the
run):

```text
plan for ADR-0001: stored plan (deterministic read from the ADR document)
planned ADR-0001 as task adr-0001 — issue 1 on gitea como/playbook at http://localhost:3000: label it conduit:run to start
```

Captured — one tick to InReview, merge observed on the next:

```text
conduit run: single tick via gitea como/playbook at http://localhost:3000 (engine: fake (complete))
adr-0001  InReview  1  conduit/adr-0001/adopt-trunk-based-development-with-short
# PR 2: [ADR-0001] Adopt trunk-based development with short-lived branches
#   labels ['adr:ADR-0001', 'effort:1-super-quick']
# reviewer: review HTTP 200, merge HTTP 200
adr-0001  Merged    1  conduit/adr-0001/adopt-trunk-based-development-with-short
```

Captured — `conduit verify 1 -o json`, ALL SIX CHECKS PASS, exit 0:

```json
{
  "checks": [
    {"name": "title_prefix",            "pass": true},
    {"name": "trailer_final_line",      "pass": true},
    {"name": "exactly_one_effort_label","pass": true},
    {"name": "adr_label_present",       "pass": true},
    {"name": "branch_shape",            "pass": true},
    {"name": "never_adr_namespace",     "pass": true}
  ],
  "pass": true,
  "pr": 2,
  "task": "adr-0001"
}
```

(Full per-check `detail` strings omitted here for width; the shape and
semantics are the [demo walkthrough's](./demo.md) section 8.)

## 5. Forge neutrality on the playbook corpus

```sh
conduit demo-transcript 1 --forge gitea  > t-gitea.jsonl
conduit demo-transcript 1 --forge github > t-github.jsonl
diff t-gitea.jsonl t-github.jsonl && echo "FORGE-NEUTRAL: identical"
```

Captured — the 7-line normalized streams are byte-identical (same sha256):

```text
FORGE-NEUTRAL: identical
b119003e0d6d2809debd259f9f14871e53cb11b61170229a6775d4b75fbba865  t-gitea.jsonl
b119003e0d6d2809debd259f9f14871e53cb11b61170229a6775d4b75fbba865  t-github.jsonl
```

## 6. The harvest rule, and forge down

The merged PR's diff was the FakeEngine's deterministic artifact
(`docs/impl/adr-0001.md`) — demo evidence, not playbook content. The
playbook's working agreement is explicit: work merged on the throwaway forge
returns to the real playbook repo ONLY via an explicit `git fetch` by URL,
executed before teardown, and the throwaway forge is never added as a
remote. A demo artifact is deliberately **not** harvested; a real run (e.g.
the live-engine encore producing corpus-worthy content) would fetch first.

```sh
just forge-down   # container + volume destroyed; the real playbook repo untouched
```

The real playbook checkout stayed clean throughout — seeding pushes *from*
it; nothing ever pushes *to* it.
