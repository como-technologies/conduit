# Post-spike follow-ups

Seven residual items were identified during the spike. As of iteration 2
this list is **closed**: six are done, one is retired by an accepted ADR.
Each entry below names the change of record. (The broader spec OUT-list is
likewise closed — built, scheduled, or retired by ADR-0008..ADR-0013.)

## 1. Move off token-in-URL — DONE

Done in `d0fdafd` (security(git)): `GiteaForge::git_remote_url` returns a
credential-free URL across adapters; the token rides the child environment
via an env-only one-shot credential helper, never argv. A regression test
asserts no token substring appears in constructed git argv, and the
token-in-URL push in `demo/gitea-init.sh` was fixed the same way. The
spike-era redaction in `src/git.rs` is kept as defense-in-depth.

## 2. Adroit plan timeout — DONE

Done in `1b44b27` (feat(adroit)): the `adroit plan` subprocess inherits the
engine deadline (`[engine] timeout_secs`) via the same process-group-kill
pattern the claude engine uses (run-1 learning: leader-kill is not enough).
A hanging-fake-adroit test proves the call fails within the deadline; the
old `TODO(timeout)` marker in `src/adroit.rs` is gone.

## 3. Extract shared action-payload builders — DONE

Done in `d028a27` (refactor(payload)): `src/payload.rs` is the single source
for forge action payloads, with a cross-assertion test
(`tests/payload_parity.rs`) proving `router.rs` and `transcript.rs` emit
byte-identical payloads for the same inputs.

## 4. Namespace-scoped label convergence — DONE

Decided in accepted ADR-0007
(`adr/accepted/0007-namespace-scoped-label-convergence.md`, `c0251e2`) and
implemented in `83bd2e3`
(feat(labels)): conduit owns exactly the `effort:*` / `adr:*` / `conduit:*`
prefixes; convergence adds missing and removes stale owned labels and never
touches unprefixed human labels. The semantics live in one shared
normalization layer (`src/labels.rs`), unit-tested there and proven on every
adapter by the conformance suite.

## 5. Mechanical enforcement of the ForgeCall trio obligation — DONE

Done in `ee5447c` (test(transcript)): adding a `ForgeCall` variant without
router + transcript + FakeForge handling now fails mechanically
(fail-closed), not by code-review convention.

## 6. Sacrificial private GitHub repo for mutation acceptance — RETIRED

Retired by accepted ADR-0012
(`adr/accepted/0012-github-mutation-acceptance-held-owner-gated-behind-dryrun.md`)
as an **owner-gated action**: validating that
GitHub accepts conduit's mutation payloads requires mutating a real remote,
and remote actions are owner-only under the standing mandate. `DryRun` stays
the only GitHub mutation path until the owner personally runs the one-time
validation; the residual gap is documented in the ADR rather than silently
open.

## 7. sha256_hex dedup and README stub — DONE

Both halves done: `d028a27` consolidated the three hand-rolled sha256-hex
copies into the single `src/hash.rs` helper (grep-checkable), and the repo
root has carried a `README.md` since `011ee11` — the earlier claim on this
page that no README existed was stale and is hereby corrected.
