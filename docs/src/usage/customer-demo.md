# The customer demo (the engagement, end to end)

The suite's north-star deliverable: sit a customer down and run, live and
repeatably, a complete fictional engagement — **"Como modernizes a client's
engineering practice, end to end, with humans at every gate."** The client
is fully generic (a small-to-mid-size product team and its decision
playbook); every claim shown is machine evidence produced in front of the
audience; nothing ever leaves localhost.

The kit lives in `demo/kit/`: one `demo-up`, five beat scripts, one
`demo-down`. Each beat prints its talking point, the exact commands it
runs, and the machine evidence it just produced. Both full rehearsals are
committed verbatim under `demo/kit/rehearsals/` — every output quoted on
this page is from those transcripts.

**Design rules** (ADR-0015): a *pre-baked/live split* — every AI lane ships
a pre-authored artifact for the fast path and a `--live` flag that
recomputes it on local ollama; *kit-owns-no-state* — all run state lives in
a per-`demo-up` workdir under gitignored `demo/runs/`, and the kit never
writes to a sibling repo; *evidence-per-beat* — no beat ends on narration.

## Stand up: `demo/kit/demo-up`

One command from a checkout. It resolves every cross-repo dependency
through the suite resolution chain (ADR-0014) and prints where each came
from, builds whatever is missing (conduit, the pinned adroit, the
`assessments` / `tuesday-report` / `pulse-simulate` binaries), stands up
the throwaway Gitea seeded with the client's playbook corpus, creates the
per-run workdir with the standing label set, pre-warms the local model for
the live lanes, and prints the beat menu:

```text
 demo/kit/beat-1-measure-prior      Measure (prior): pulse's verified-anonymous team signal
 demo/kit/beat-2-assess [--live]    Assess: brief + signals -> schema-valid assessment
 demo/kit/beat-3-prescribe [--live] Prescribe: assessment -> ADRs; accept; stored plan
 demo/kit/beat-4-adopt [--restart]  Adopt: stored plan -> human-gated PR -> merged -> verify 6/6
 demo/kit/beat-5-measure            Measure: tuesday --strict + Adopt<->Measure cross-check
 demo/kit/demo-down                 Tear down: forge destroyed, workdir removed, nothing left
```

Idempotent: re-running keeps a live forge and workdir. Rehearsed: 13s with
sibling binaries already built (the first-ever run additionally pays the
cargo builds — minutes, and that is setup, not a beat).

## Timing (both rehearsals, 2026-06-12)

| Step | Rehearsal 1 (pre-baked) | Rehearsal 2 (live + restart) |
|---|---|---|
| demo-up | 13s | 6s |
| beat 1 — measure prior | 2s | 0s |
| beat 2 — assess | 0s | **321s** (`--live`, llama3.2) |
| beat 3 — prescribe | 0s | **296s** (`--live`, llama3.2) |
| beat 4 — adopt | 4s (re-run: 2s) | 5s (`--restart`) |
| beat 5 — measure | 0s | 0s |
| demo-down | 1s | 2s |

Every beat except the two opt-in ollama lanes lands far under the 60s bar.
The live lanes are the slow ones by design — that is what the pre-baked
variants are for. Pre-warm note: `demo-up` loads the model into ollama's
memory, so a live beat pays no cold start; run-2 of the full dogfood loop
measured the same lanes at 355.9s (assess, zero retries) and ~5.2 min
(import `--ai` + `plan --save`), consistent with rehearsal 2. (Clock
footnote: rehearsal 2's WALL-CLOCK lines used the realtime clock, which
WSL2 stepped ~23s against the assessments binary's monotonic internal
timer — both numbers are preserved in that transcript. The kit now times
beats on the monotonic clock (`/proc/uptime`), so the narration and the
binaries' own elapsed marks can no longer disagree.)

## Beat 1 — Measure the prior period (pulse)

**Say.** Before Como prescribes anything, we measure. pulse collects team
sentiment with verified anonymity — k-anonymity suppression is tested on
both sides of the threshold. The report is deterministic by design: same
seed, byte-identical bytes. We prove that, live, by running it twice.

**Run.** `demo/kit/beat-1-measure-prior` — pulse's own `just dogfood`,
twice, then sha-compare.

**The audience sees** (rehearsal 1):

```text
   run 1 sha256: a3e94c63d52b825febf4e17249eb299bbb4d171f10b40abdcd66b3d0f3283e1c
   run 2 sha256: a3e94c63d52b825febf4e17249eb299bbb4d171f10b40abdcd66b3d0f3283e1c
   BYTE-IDENTICAL: yes
   schema pulse.measure-report/v1  seed 42  flows 10/10 passed, 0 failed
   signal: "How confident are you that this iteration's changes improved the portfolio?" -> avg 4.2 (10 unique pseudonyms, suppressed: false)
```

The weakest signal (dogfood-loop support, avg 2.6) is exactly the kind of
finding the next beat's assessment takes as context — the loop's return
edge, shown before the loop even starts.

## Beat 2 — Assess (assessments)

**Say.** An assessment that used to take a consultant weeks is authored in
minutes from the client's own brief, on a 3B model running on this laptop —
no cloud, nothing leaves the room. The output is schema-validated YAML
behind mechanical quality gates (degeneracy, dedupe, leakage), and the
client's architect reviews every question before it is asked.

**Run.** `demo/kit/beat-2-assess` — fast path: the kit's pre-baked
assessment (authored 2026-06-12 by this same pipeline, 355.9s, zero
retries) is copied in and **re-validated live**; the audience watches the
gate pass, not a slide. `--live` recomputes the whole thing on ollama
(~5.5 min) with the beat-1 pulse report as `--context`.

**The audience sees** (rehearsal 1 fast path):

```text
   valid: 'Software Engineering Maturity Assessment' — 4 domains, 8 practices, 96 questions
   exit code: 0
```

Rehearsal 2's `--live` lane authored a fresh 4-domain / 8-practice /
89-question assessment in 321s and passed the same validation — the
pre-baked artifact is a cache, not a fake.

## Beat 3 — Prescribe (adroit + the playbook)

**Say.** Findings become decisions. adroit ingests the assessment and
seeds one proposed ADR per practice — a governed, machine-readable
decision corpus. The human gate: nothing is prescribed until the client's
architect moves a decision to Accepted. Then the trick that controls AI
cost and risk: each accepted decision carries a **stored** implementation
plan inside the document itself. AI is paid once, at authoring time; every
read after that is deterministic and provider-free.

**Run.** `demo/kit/beat-3-prescribe` — the mechanical import runs live
into a scratch corpus inside the workdir (the kit never writes to the real
playbook), an accept transition runs live, and the stored plan is read
twice with the AI environment scrubbed. `--live` adds the ollama
flesh-out of all eight ADRs plus `plan --save` (~5 min).

**The audience sees** (rehearsal 1):

```text
   seeded 8 proposed ADR(s), 0 skipped (dedupe guard)
   ...
   Updated ADR-0001 status to Accepted (.../prescribe/adrs/accepted/0001-automated-testing.md)
   ...
   ADR-0005: stored = true
   read 1 sha256: aaa56e54efbd94d598107d4604c20595d93d903cf1710aa72a2251023a81c19e
   read 2 sha256: aaa56e54efbd94d598107d4604c20595d93d903cf1710aa72a2251023a81c19e
   SHA-IDENTICAL: yes — no AI was configured for either read
```

That sha is the same value the full dogfood run recorded for this plan —
the stored plan has been byte-stable across machines, days, and runs.

## Beat 4 — Adopt (conduit): the flagship

**Say.** The pitch, verbatim from the portfolio's agentic-delivery page:
the human gates aren't a safety disclaimer bolted onto an agent — they're
what you're buying. **You never have to trust an agent; you have to review
a pull request, which your team already knows how to do.** Three gates by
name: the *scope* gate (nothing runs until a reviewer labels the issue
`conduit:run` — you read the plan before any code exists), the *review*
gate (every change arrives as a PR in your own forge), and the *merge*
gate (conduit has no merge method — structurally unrepresentable, and the
actor account cannot even approve its own PRs). And we don't claim
success — we machine-verify it.

**Run.** `demo/kit/beat-4-adopt` — plan (stored, no AI env) → scripted
reviewer labels `conduit:run` → one tick to InReview → reviewer approves
and merges through the API (in real life: the forge UI) → next tick
observes Merged → `verify 5 -o json` → the forge-neutrality transcript
diff, **three-way** since the GitLab adapter landed (ADR-0016): gitea
executes live, github and gitlab are dry-run by construction. `--restart`
inserts the crash sub-beat: `kill -9` mid-Coding, recover, audit the live
forge for duplicates.

**The audience sees** (rehearsal 1; restart evidence from rehearsal 2; the
`FORGE-NEUTRAL` block re-captured from the N=3 beat — same sha as the
rehearsal's two-way capture, now three ways):

```text
   plan for ADR-0005: stored plan (deterministic read from the ADR document)
   planned ADR-0005 as task adr-0005 — issue 1 on gitea como/playbook ...: label it conduit:run to start
   labeled issue 1 with conduit:run (as reviewer)
   ...
   PR 2: [ADR-0005] Automated Testing
   labels: adr:ADR-0005, effort:1-super-quick
   ...
   review APPROVED: HTTP 200
   merge: HTTP 200
   adr-0005   Merged   1   conduit/adr-0005/automated-testing

   PASS  title_prefix / trailer_final_line / exactly_one_effort_label /
         adr_label_present / branch_shape / never_adr_namespace
   overall: pass=true  pr=2  task=adr-0005

   FORGE-NEUTRAL (N=3): identical (7 lines)
   9cf0b8d8...c7a6e  t-gitea.jsonl
   9cf0b8d8...c7a6e  t-github.jsonl
   9cf0b8d8...c7a6e  t-gitlab.jsonl
```

```text
   state: Coding | pending: RunEngine done=false        <- the kill -9 crash record
   issues carrying the adr-0005 task marker: [1] (want exactly one)
   PRs with head conduit/adr-0005/*:          [2] (want exactly one)
```

Timing note: 4–5s wall-clock for the whole lifecycle including the crash
sub-beat (deterministic FakeEngine — the engine seam is the demo's subject,
not the model). A live-engine encore (real coding agent, ~5.5 min,
producing the playbook's actual glossary page) was proven in the full
dogfood run-2; the kit keeps it out of the default path because its output
is nondeterministic.

## Beat 5 — Measure (tuesday) and the loop closes

**Say.** Adoption you can count: tuesday reads the merged PRs off the
client's forge and attributes measured hours to the decision that caused
them. Strict mode exits nonzero if any merged PR is unaccounted for. Then
the double-entry bookkeeping — conduit's `verify` and tuesday's report are
two independent codebases, and the cross-check asserts they agree on the
same PR, effort, and decision. This report plus the pulse signal are
exactly what the next assessment consumes: the loop closes on camera.

**Run.** `demo/kit/beat-5-measure`.

**The audience sees** (rehearsal 1):

```text
   exit code: 0 (strict mode satisfied)
   como 2026-June: 1 allocation(s), 0 unallocated PR(s) [strict requires 0]
   PR 2 "[ADR-0005] Automated Testing" -> ADR-0005 (SuperQuick, 160.0h)
   adr_totals: ADR-0005 = 160.0h
   pr:     conduit=2 tuesday=2
   effort: conduit=effort:1-super-quick tuesday=effort:1-super-quick (SuperQuick)
   adr:    conduit=ADR-0005 tuesday=ADR-0005 (adr_totals: 160.0h)
   CROSS-CHECK PASS: PR 2, effort:1-super-quick, ADR-0005 — Adopt and Measure agree
```

## Tear down: `demo/kit/demo-down`

**Say.** The whole engagement ran on a throwaway forge on this machine.
Nothing was ever pushed anywhere but localhost.

```text
   forge container: gone
   forge volume: gone
   workdir: gone
   remotes touched: none (localhost was the only push target, ever)
```

## Appendix: run it yourself

The customer's engineer can replay everything above from this repo:

```sh
git clone <conduit> && cd conduit
just init                      # toolchain + mdbook (rust, docker, jq required)
demo/kit/preflight             # verify docker is up (+ pull llama3.2 for --live)
demo/kit/demo-up               # resolves, builds, seeds, prints the menu
demo/kit/beat-1-measure-prior
demo/kit/beat-2-assess         # add --live to recompute on your ollama
demo/kit/beat-3-prescribe      # add --live likewise
demo/kit/beat-4-adopt          # add --restart for the crash sub-beat
demo/kit/beat-5-measure
demo/kit/demo-down
```

Dependencies resolve per the suite convention (ADR-0014), in order: env
override (`COMO_<REPO>_DIR`) → sibling checkout (`../playbook`, `../pulse`,
`../assessments`, `../tuesday`) → the gitignored `.como/deps/` clone cache
from `${COMO_<REPO>_GIT:-${COMO_GIT_BASE:-https://github.com/como-technologies}/<repo>.git}`
→ skip-with-notice (only the playbook corpus is a hard requirement; a beat
whose repo did not resolve says so and names the knobs). The pinned adroit
installs via `just init-adroit` from `adroit.rev`.

**The honest note.** The suite repos are now published — adroit (including its
`v0.2.0` tag), tuesday, pulse, conduit, portfolio, and assessments all have a
remote `main` — so on a fresh machine the clone-cache legs resolve remotely
with no kit change. The one exception is **playbook**, which has no remote yet:
its corpus leg still needs a sibling checkout (`COMO_PLAYBOOK_DIR`, or a
`COMO_GIT_BASE=file:///path/to/mirrors` mirror) until the owner publishes it.
What needs what:

| Leg | Status today |
|---|---|
| playbook corpus (hard) | **not yet published** — sibling / `COMO_PLAYBOOK_DIR` / file:// base |
| adroit pin (`just init-adroit`) | published — `adroit.rev` pins adroit `main`, reachable on the remote (cold clone needs no sibling) |
| pulse, assessments, tuesday (beats 1/2/5) | published — resolve remotely |
| ollama `llama3.2` (only for `--live`) | local install, any machine — never remote |

Requirements (run `demo/kit/preflight` to check them): docker with its daemon up
(the throwaway forge), the rust toolchain + `just`, `jq`, `curl`; ollama with
`llama3.2` only for the `--live` variants. The pre-baked fast path runs with no
model installed at all.
