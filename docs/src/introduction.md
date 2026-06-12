# conduit

A development harness where the entire build loop — scope, code, review,
merge — happens inside a team's existing issue tracker and pull requests, on
*their* forge, *their* cloud, and *their* AI model, with nothing locked to a
vendor.

conduit is the **Adopt**-stage engine of the Como TAPS portfolio loop
(Assess → Prescribe → **Adopt** → Measure). It reads accepted Architecture
Decision Records and their implementation plans over
[adroit](https://github.com/como-technologies/adroit)'s machine-readable seam
(`manifest` / `-o json`), and drives a commodity coding engine to turn each
decision into issues and reviewable pull requests — tagged so the Measure
stage (tuesday) can trace effort back to the decision that prompted it.
Humans keep every gate: scope, review, and merge.

conduit is **not** an agent. It stands on existing engines and builds only the
thin layer they don't have:

1. a **forge-neutral event router**,
2. a **PR/MR lifecycle state machine**, and
3. the **forge adapter** — the net-new IP.

conduit **drives GitHub and self-hosted Gitea identically today** — proven by a
shared conformance suite and a byte-identical transcript diff; GitLab is the
planned third implementation.

Status: spike complete. See the [demo walkthrough](./usage/demo.md) for
evidence, and the [spike design](./dev/spike-design.md) for the historical
normative architecture. The suite's end-to-end engagement demo — the full
TAPS loop as a presenter-paced kit (`demo/kit/`) — is the
[customer demo](./usage/customer-demo.md).
