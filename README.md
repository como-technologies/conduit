# conduit

conduit is the **Adopt**-stage engine of the Como TAPS portfolio loop. It reads
accepted Architecture Decision Records via adroit's machine-readable seam and
drives a commodity coding engine to turn each decision into issues and reviewable
pull requests — on the team's own forge, cloud, and AI model, with nothing
locked to a vendor. Spike complete as of 2026-06-12.

- **Working agreements and build commands:** [CLAUDE.md](./CLAUDE.md)
- **Full documentation (mdbook):** `just book` or `docs/src/`
- **Demo walkthrough:** [docs/src/usage/demo.md](./docs/src/usage/demo.md)

**Never push to a real remote; never open a PR on a public forge.**
All push targets are the throwaway localhost Gitea container or local bare repos
in tests. See CLAUDE.md for the full working agreement.
