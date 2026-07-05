# conduit

Forge-neutral agentic development harness — the Adopt-stage engine of the Como
TAPS loop. Spike spec (normative): `docs/src/dev/spike-design.md`.

## Working agreements (IMPORTANT — read first)

- **Never push to a real remote. Never open a PR on any public forge.** The only
  push target that ever exists is the throwaway localhost Gitea container (and
  local bare repos in tests). GitHub and GitLab mutations are ALWAYS
  DryRun-decorated — the constructors only hand out `DryRun(GitHubForge)` /
  `DryRun(GitLabForge)` (ADR-0012 / ADR-0016).
- **All work stays under `~/repos/como-tech/**`.** Tokens live in gitignored
  `.secrets/`; never commit or log them.
- **Humans hold every gate.** No `Forge::merge` method exists; the `conduit:run`
  label and PR review/merge are human actions. Do not add automation that
  bypasses a gate.
- **conduit never authors, edits, or transitions an ADR** — that is adroit's
  lane. The only adroit subcommands conduit may invoke are
  `{manifest, list, show, plan}` (enforced by test).
- **All documentation lives in the mdbook** (`docs/src/**`, wired into
  `docs/src/SUMMARY.md`). No standalone Markdown docs elsewhere. Keep code and
  docs in sync; `just book` must build.
- **No client names** in docs/comments/examples — keep examples generic.
- Never write a bare `#<number>` in forge-rendered text (commits, PR/issue
  bodies) — use `task N` / plain `N`.

## Build & test

Always use `just` recipes — never raw `cargo`/`mdbook`.

```sh
just init        # toolchain components + mdbook
just init-adroit # pinned adroit -> .conduit/bin (adroit.rev; remote URL
                 # via COMO_ADROIT_GIT/COMO_GIT_BASE, sibling file:// fallback)
just ci          # fmt-check + clippy + test + adr-check + book (the gate)
just adr-check   # validate the in-repo docs/src/adr corpus with the pinned adroit
just test        # all tests
just forge-up    # throwaway Gitea on localhost:3000 (demo/)
just forge-down  # destroy it
```

The customer demo kit (`demo/kit/demo-up`, per-beat scripts, `demo-down`)
packages the full TAPS engagement demo — narrated script:
`docs/src/usage/customer-demo.md`; design: ADR-0015.

`adr-check` is a `ci` leg (every suite repo's ci carries one). It depends on
`just init-adroit` — idempotent-fast once the pin is installed; a fresh
checkout (GitHub CI included) resolves the pin from the adroit remote.
`cargo audit` runs as a separate CI job (`just crate-audit`, plus a weekly
schedule) so a fresh advisory can't mask the code gates.
The `docs/src/adr` corpus is authored ONLY with the pinned adroit
(`new` / `edit` / `set-status` / `plan --save`), never by hand, and keeps
adroit's forge integration disabled.

Env-gated test legs: `CONDUIT_E2E_GITEA=1` (live Gitea conformance),
`CONDUIT_E2E_GITHUB=1` (GitHub live reads), `CONDUIT_E2E_ADROIT=1` (pinned
adroit binary contract tests), `CONDUIT_E2E_CLAUDE=1` (live claude CLI
engine smoke).

## Design rules

- Fully synchronous — no tokio. HTTP via ureq behind the `HttpTransport` seam;
  unit tests inject `FakeTransport`, never the network.
- Typed errors (`thiserror`) in lib modules; `anyhow` only in `main.rs`.
- Pure core, effectful shell: `contract.rs`, `machine.rs`, `forge::diff` are
  pure and exhaustively unit-tested; `router.rs` owns all effects.
- State is files under `.conduit/` you can `cat` — no database.
- Never put test-only state in a production type; use injected fakes
  (`FakeForge`, `FakeEngine`, `FakeTransport`) and documented env overrides.
