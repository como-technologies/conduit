# ADR-0015: Package the customer demo as a kit: pre-baked/live split, stateless kit, evidence per beat

> State: Accepted

## Status

Accepted
Created: 2026-06-12

## Stakeholders

conduit maintainers (the kit lives in this repo because conduit owns the
Adopt machinery and the demo infrastructure) and the Como portfolio owner,
whose north star — the customer-showable engagement — the kit is the
deliverable for. The sibling repos (playbook, pulse, assessments, tuesday,
adroit) are consumed read-only.

## Context and Problem Statement

The suite's north star is a complete fictional engagement a presenter can
run live and repeatably in front of a customer: Measure, Assess, Prescribe,
Adopt, Measure, loop closed, humans at every gate. Every beat was proven
working in the full dogfood runs, but the proof lived as one-off shell
history across five repos: standing it up took tribal knowledge, the two
local-AI lanes take five-plus minutes each (unpresentable as a default),
and nothing guaranteed a re-run would not stomp earlier state or leave
residue behind. The demo had to become a packaged kit with a one-command
stand-up, presenter-paced beats, and machine evidence at every step —
without becoming a second copy of the machinery it demonstrates.

## Decision Drivers

- Every beat must land under ~60 seconds wall-clock except the explicitly
  opt-in local-AI lanes (the suite bar)
- Every claim a beat makes must be machine evidence produced in front of
  the audience — exit codes, shas, live forge reads — never narration
- Re-running any script must be safe (idempotent) mid-demo
- Tear-down must leave nothing: container, volume, and run state gone; no
  remote ever touched
- The kit must not duplicate or fork the proven machinery (gitea-init,
  playbook-demo-init, conduit, the sibling tools) — it may only orchestrate
- Cross-repo references must resolve per the uniform suite convention
  (ADR-0014), with the unpublished-remote reality stated honestly

## Considered Options

- A kit of thin orchestration scripts: pre-baked/live split for the AI
  lanes, all state in the per-up workdir, evidence printed per beat
- Live-only demo (no pre-baked artifacts): honest but unpresentable — two
  five-minute ollama waits in the middle of a customer meeting
- Pre-recorded outputs (slideware): fast but violates the no-slideware bar;
  evidence must be produced live
- A single monolithic demo script: fewer files, but no presenter pacing,
  no per-beat re-runs, and one failure aborts the whole show

## Decision Outcome

Chosen: **the kit of thin orchestration scripts** under `demo/kit/`
(`demo-up`, `beat-1` … `beat-5`, `demo-down` + a shared `lib.sh`), because
it is the only option that is simultaneously presentable, honest, and
repeatable. Three principles govern it:

1. **Pre-baked/live split.** Every AI lane ships a pre-authored artifact in
   `demo/kit/prebaked/` (the assessment this same pipeline authored on
   2026-06-12) or reads the client corpus's stored plan, giving a fast path
   in seconds — and every such beat takes `--live` to recompute the same
   artifact on local ollama with the same commands and gates. The pre-baked
   artifact is a cache, not a fake: the fast path still runs its validation
   live, and provenance (when, what model, how long) is printed every time.
2. **Kit-owns-no-state.** All mutable state lands in the per-`demo-up`
   workdir under gitignored `demo/runs/` (created by the existing
   `playbook-demo-init.sh`); the only kit-side file is the gitignored
   `.current` pointer. Sibling repos are never written to — beat 3 imports
   into a scratch corpus inside the workdir. `demo-down` removes the forge
   (container + volume) and the workdir; re-runs of any beat converge
   instead of duplicating (beat 4 detects a Merged task and re-asserts the
   evidence live).
3. **Evidence-per-beat.** Each beat ends with machine output it just
   produced: byte-identical shas (pulse determinism, stored-plan double
   read, forge-neutral transcripts), live forge reads (the PR, the
   duplicate audit), exit-0 gates (validate, verify 6/6, tuesday --strict,
   cross-check PASS). The talking points frame the evidence; they never
   substitute for it.

The kit resolves siblings per ADR-0014 and only orchestrates: every command
a beat runs is the same command the repos' own recipes and walkthroughs
document.

### Positive Consequences

- The north-star demo is one command away from any checkout with the
  sibling layout, and each rehearsal is reproducible: rehearsal 1 ran every
  beat in 0–13s; rehearsal 2's live lanes measured 321s (assess) and 296s
  (prescribe), both transcripts committed under `demo/kit/rehearsals/`
- The presenter chooses pacing per audience: all-fast (under a minute of
  total wall-clock), or any subset of live lanes / the crash sub-beat
- The narrated script (`usage/customer-demo.md`) and the kit cannot drift
  silently: the book page quotes the committed rehearsal transcripts

### Negative Consequences

- The pre-baked assessment is a committed artifact that will go stale as
  the assessments pipeline evolves; refreshing it is a manual `--live` run
  plus a reviewed copy into `prebaked/`
- The kit's talking points duplicate phrasing from the portfolio's
  agentic-delivery page; wording drift between the two is possible
- Pre-baked beat 3 demonstrates accept-and-stored-plan on the real corpus's
  decision while the import lands in a scratch corpus — two corpora in one
  beat, which the presenter must narrate carefully (the script does)
- The live lanes' timings depend on the host's ollama performance; the
  documented numbers are this machine's, not a guarantee

## Implementation

Landed with this decision on the `demo-kit` branch: `demo/kit/` (lib.sh,
demo-up, five beats, demo-down, prebaked/assessment.yaml), two committed
rehearsal transcripts under `demo/kit/rehearsals/`, the narrated book page
`usage/customer-demo.md`, and the `.gitignore` entry for the `.current`
pointer. Refresh procedure for the pre-baked artifact: run
`beat-2-assess --live`, review the output, copy it into `prebaked/`, and
re-run both rehearsals.
