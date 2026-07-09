# GitHub live-mutation validation

ADR-0012 holds GitHub mutation acceptance owner-gated behind DryRun: the
adapter's writes are fixture-verified but have never been accepted by the
real github.com, and the decision records that the gate lifts **only after
the owner personally runs a one-time validation** against a sacrificial
private repo and records the result. `demo/github-live-validation.sh` is
that validation, packaged.

```sh
# 1. Create a throwaway PRIVATE repo (any name that is not a suite repo)
# 2. Run, as yourself (uses your own `gh` login):
GITHUB_VALIDATION_REPO=<owner>/<sacrificial-repo> demo/github-live-validation.sh
```

What it does — after refusing public repos and every real suite repo by
name — is exercise the four mutations ADR-0012 lists, using the exact
request bodies `src/forge/github.rs` sends (plus the two prerequisites the
real flow needs: the ensured label and a branch with one commit):

1. create PR (`POST pulls`, the `open_pr` body shape)
2. set labels (`PUT issues/{n}/labels` — PR labels ride the issues
   endpoint, exactly as the adapter does it)
3. close PR (`PATCH issues/{n}`, the `close_issue` body shape)
4. delete branch (`DELETE git/refs/heads/{branch}`)

Every step's HTTP status lands in `demo/runs/github-validation-<ts>.jsonl`
— that file is the recorded outcome ADR-0012 asks for. On a full pass the
script says so and names the next step: a decision to lift DryRun may then
be **proposed**, superseding ADR-0012. Nothing else changes by running
this: `DryRun(GitHubForge)` remains the only constructor output until such
a decision is accepted.
