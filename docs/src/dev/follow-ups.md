# Post-spike follow-ups

Seven residual items identified during the spike. None block the demo; all
are worth doing before production use.

## 1. Move off token-in-URL (GIT\_ASKPASS / credential helper)

Credential redaction landed in `src/git.rs`: any `scheme://user:secret@host`
occurrence in git argv or stderr is replaced with `scheme://$REDACTED@host`
before it enters a `GitError::Command`. The URL itself, however, still appears
in the process argument list (visible to `ps`, `/proc/<pid>/cmdline`, and any
process-auditing daemon on the host). The right fix is to switch
`GiteaForge::git_remote_url` to return a credential-free URL and supply the
token via `GIT_ASKPASS` or a temporary per-invocation git credential helper —
eliminating the secret from argv entirely. That change touches the `Forge`
trait contract and the git subprocess harness, so it was deferred to keep the
spike diff minimal.

## 2. Adroit plan timeout

`src/adroit.rs` carries a `TODO` comment marking where the `adroit plan`
subprocess should inherit the engine's deadline mechanism (the same
`timeout_secs` the engine uses). Without it, a slow or hung adroit invocation
blocks the daemon indefinitely. The fix reuses the existing
`EngineConfig::timeout_secs` field: pass it as a `--timeout` argument or
wrap the subprocess in the same deadline wrapper the engine uses.

## 3. Extract shared action-payload builders

`router.rs` and `transcript.rs` both construct forge action payloads
independently. If either drifts — a field name changes, a new required key
is added — the other silently produces the wrong payload and the tests may
not catch it because they exercise only one path. The fix is to extract the
builders into a shared module (e.g. `src/payload.rs`) and add a
cross-assertion test that both call-sites produce identical output for the
same inputs.

## 4. Namespace-scoped label convergence

Labels applied by conduit currently share the flat forge namespace with human
labels. The planned scheme — `effort:*/adr:*/conduit:*` prefixes — avoids
collision and makes conduit labels machine-queryable, but it must preserve
any human-applied labels that happen to share a prefix. The convergence
semantics (add missing prefixed labels, remove stale prefixed labels, never
touch unprefixed labels) are non-trivial enough to warrant an ADR before the
first real-world use: the decision affects every forge adapter and the
snapshot normalisation filter in `router.rs`.

## 5. Mechanical enforcement of the ForgeCall trio obligation

Every `ForgeCall` variant must be handled by exactly three things: the
router, the transcript recorder, and the fake forge. At present this is
enforced only by convention and code-review. A compile-time approach —
a procedural macro or a sealed trait with exhaustive match arms in a
dedicated test module — would make it impossible to add a new variant
without also updating all three sites.

## 6. Sacrificial private GitHub repo for mutation acceptance

The spike's GitHub adapter always wraps mutations in `DryRun`. The spec
names one residual gap: we have never actually sent a mutation payload to
the GitHub API and verified it was accepted. A one-time validation against a
throwaway private repo (create PR, set labels, close PR, delete branch) would
close that gap and give confidence that the payload shapes match GitHub's
current API before the `DryRun` wrapper is lifted for real use.

## 7. sha256\_hex dedup and README stub

`sha256_hex` is hand-rolled in at least three places in the codebase. The
copies should be consolidated into a single `src/hash.rs` (or a small
inline utility) and covered by one set of tests. Separately, the repo root
has no `README.md` — a ten-line stub pointing at `CLAUDE.md`, the mdbook,
and the demo page is sufficient to orient a new contributor.
