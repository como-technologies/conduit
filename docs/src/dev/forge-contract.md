# Forge contract

The keystone (`src/forge/mod.rs`): one trait that GitHub, Gitea, and GitLab
implement *identically* (N=3), proven by a parameterized conformance suite
rather than promised. Adapters never produce events — they produce
**snapshots**, and a single shared pure `diff` derives every event, so event
semantics exist in exactly one place.

## The `Forge` trait

```rust
pub trait Forge {
    fn describe(&self) -> String;
    /// Used ONLY by src/git.rs, never by engines (the sandbox is structural).
    /// ALWAYS credential-free — no token ever rides a process argv.
    fn git_remote_url(&self) -> Result<String, ForgeError>;
    /// Credentials for git against git_remote_url, supplied via the git
    /// layer's env-only credential helper (None: local paths / never pushed).
    fn git_auth(&self) -> Result<Option<GitAuth>, ForgeError> { Ok(None) }
    // events in: one read, normalized
    fn fetch_snapshot(&self) -> Result<RepoSnapshot, ForgeError>;
    // idempotency probes (reads)
    fn find_open_pr_by_head(&self, branch: &str) -> Result<Option<PrId>, ForgeError>;
    fn find_issue_by_marker(&self, marker: &str) -> Result<Option<IssueId>, ForgeError>;
    // actions out — NO merge method exists: humans merge in the forge UI
    fn ensure_labels(&self, labels: &[LabelSpec]) -> Result<(), ForgeError>;
    fn create_issue(&self, new: &NewIssue) -> Result<IssueId, ForgeError>;
    fn upsert_issue_comment(&self, id: &IssueId, marker: &str, body: &str) -> Result<(), ForgeError>;
    fn set_issue_labels(&self, id: &IssueId, labels: &[String]) -> Result<(), ForgeError>;
    fn close_issue(&self, id: &IssueId) -> Result<(), ForgeError>;
    fn open_pr(&self, draft: &PrDraft) -> Result<PrId, ForgeError>;
    fn upsert_pr_comment(&self, id: &PrId, marker: &str, body: &str) -> Result<(), ForgeError>;
    fn set_pr_labels(&self, id: &PrId, labels: &[String]) -> Result<(), ForgeError>;
}
```

There is deliberately no `merge` method: the merge gate is human and
unrepresentable in the adapter surface. Errors are typed: `Offline`
(connection-level), `Auth` (401/403 — loud, never swallowed), `Api`
(any other non-2xx or unparseable body).

## Snapshot normalization

`fetch_snapshot()` returns one normalized read of the repo —
**conduit-labeled issues** (any `conduit:*` or `adr:*` label) and
**`conduit/*`-branch PRs** only. Adapter obligations, asserted by the
conformance suite:

- **Disappearance rule.** An item present in `prev` but absent from `next`
  produces no events, so adapters must fetch `state=all` and keep
  merged/closed PRs and closed issues visible until their terminal events
  have been observed — a merged PR that vanishes loses its `PrMerged`
  forever and wedges the task.
- **Explicit pagination.** `HttpResponse` carries no headers by design, so
  Link-header (and GitLab `x-next-page`) pagination is unreachable; adapters
  loop `?page=N&limit=50` (Gitea) / `?page=N&per_page=100` (GitHub, GitLab)
  and stop on a short page. A page-1-only fetch would silently truncate at
  the forge's default page size and violate the disappearance rule.
- **Review stability.** Reviews are never filtered when the forge could make
  a filtered row reappear: Gitea keeps dismissed reviews with their original
  verdict and stable id. The narrow exception is a one-way in-place state
  mutation on the same id (GitHub's `DISMISSED`): a skipped id can never
  reappear with a submitted verdict, so skipping it cannot re-fire an event.
  `PENDING` (draft) rows are skipped on both. GitLab is a **third shape —
  dismissal-by-removal**: it has no review-submission objects at all, the
  closest analog is MR approvals (`approved_by` rows of `{user,
  approved_at}` with no forge-native id), and revoking an approval *removes*
  the row. The adapter filters nothing and synthesizes the review id from
  the documented `{user.id}@{approved_at}` pair: a standing approval is
  stable across polls, a revocation fires nothing, and a re-approval mints a
  fresh timestamp — a new id that correctly fires. Documented GitLab
  limitation (ADR-0016): approvals map only to `Approved`; GitLab's "request
  changes" is a mutable MR-level status with no per-event identity, so
  `ChangesRequested`/`Commented` are not derivable from a GitLab snapshot —
  acceptable while GitLab is record-only. A resubmission gets a new
  forge-native (or synthesized) id on every forge and correctly fires.
- **Id uniqueness.** Issue/PR ids must be unique per snapshot; duplicates in
  `prev` are last-wins, duplicates in `next` fire duplicate events.

## Diff event semantics

`diff(prev, next) -> Vec<ForgeEvent>` — pure, shared, the contract:

| Event | Fires when | Notes |
|---|---|---|
| `IssueLabeled { issue, label }` | label present on an issue in `next`, absent on the same issue in `prev` | an issue absent from `prev` fires for ALL its labels; removals fire nothing |
| `ReviewSubmitted { pr, review }` | review id not present on the same PR in `prev` | dedupe on forge-native id: an **edited** review never re-fires; repeated rounds are distinct events; a PR absent from `prev` fires for all its reviews |
| `CiChanged { pr, state }` | PR in both snapshots and `ci` differs | new PRs fire nothing; consumed, never acted on (must-ignore everywhere) |
| `PrMerged { pr, merge_sha }` | `!prev.merged && next.merged` | absent-from-`prev` counts as not merged, so a fresh cursor still fires exactly once |
| `PrClosed { pr }` | `!prev.closed && next.closed && !next.merged` | a merged PR emits ONLY `PrMerged` (forges mark merged PRs closed too) |

Event order is deterministic: issues in `next` order, then PRs in `next`
order; within a PR: `ReviewSubmitted` (snapshot order), `CiChanged`, then
`PrMerged`/`PrClosed`. Within-poll flaps (a review submitted then dismissed
between two snapshots) are invisible by design — acceptable because merge is
a human gate. The cursor is the previous `RepoSnapshot`, persisted per forge
under `.conduit/cursor/<forge>.json`, advanced only after a tick's actions
complete.

## Idempotency: probe before reissue

Every mutating action has a replay guard, each exercised by a dedicated
crash-replay test:

| Action | Probe |
|---|---|
| `create_issue` | `find_issue_by_marker(task marker)` — the hidden `<!-- conduit:task:<id> -->` HTML comment in the issue body |
| `open_pr` | `find_open_pr_by_head(branch)` — adopts an existing PR instead of duplicating |
| push branch | `git ls-remote` sha compare before push |
| comments | marker upsert: find the comment carrying the marker, PATCH it, else POST |
| labels | convergent set — `set_*_labels` is absolute, not additive; re-running converges. Writes are namespace-scoped (ADR-0007): the caller reads current labels (`get_*_labels`), converges through `labels::converge` — owned prefixes (`effort:`/`adr:`/`conduit:`) absolute, unprefixed human labels preserved — then writes |

## Endpoint maps

Both adapters sit on the `HttpTransport` seam (production `ureq`, blocking,
rustls, 20s connect / 60s global timeouts; tests inject fixture transports).

### Gitea (REST v1, `{base}/api/v1/repos/{owner}/{repo}/...`)

| Method | Endpoint(s) |
|---|---|
| `fetch_snapshot` | `GET issues?type=issues&state=all`, `GET pulls?state=all`, per PR `GET pulls/{n}/reviews` + `GET commits/{sha}/status` |
| `find_open_pr_by_head` | `GET pulls?state=open`, filtered client-side by head branch (Gitea has no `head=` query) |
| `find_issue_by_marker` | `GET issues?type=issues&state=all` + body scan, then a marker-comment fallback per issue |
| `ensure_labels` | `GET labels`, `POST labels` (409/conflict = already there) |
| `create_issue` | `POST issues` (label **ids** — Gitea takes ids, resolved via `GET labels`) |
| comments | `GET issues/{n}/comments`, `PATCH issues/comments/{id}` / `POST issues/{n}/comments` |
| `set_issue_labels` / `set_pr_labels` | `PUT issues/{n}/labels` (ids) |
| `close_issue` | `PATCH issues/{n}` `{"state":"closed"}` |
| `open_pr` | `POST pulls` |

### GitHub (REST v3, `https://api.github.com/repos/{owner}/{repo}/...`)

| Method | Endpoint(s) |
|---|---|
| `fetch_snapshot` | `GET issues?state=all` (rows with a `pull_request` key skipped), `GET pulls?state=all`, per PR `GET pulls/{n}/reviews` + `GET commits/{sha}/status` |
| `find_open_pr_by_head` | `GET pulls?head={owner}:{branch}&state=open` |
| `find_issue_by_marker` | `GET issues?state=all` + body scan |
| `ensure_labels` | `POST labels` (422 = already there) |
| `create_issue` | `POST issues` (label **names**) |
| comments | `GET issues/{n}/comments`, `PATCH issues/comments/{id}` / `POST issues/{n}/comments` |
| `set_issue_labels` / `set_pr_labels` | `PUT issues/{n}/labels` |
| `close_issue` | `PATCH issues/{n}` |
| `open_pr` | `POST pulls` |

GitHub quirks normalized away: the issues listing includes PRs (skipped by
the `pull_request` key); `merged` is `merged_at != null` —
`merge_commit_sha` is populated with a test-merge sha even for unmerged PRs,
so it is read only when merged.

### GitLab (REST v4, `{base}/api/v4/projects/{owner}%2F{repo}/...`)

| Method | Endpoint(s) |
|---|---|
| `fetch_snapshot` | `GET issues?state=all`, `GET merge_requests?state=all`, per MR `GET merge_requests/{iid}/approvals` + `GET pipelines?sha={sha}&per_page=1` |
| `find_open_pr_by_head` | `GET merge_requests?state=opened&source_branch={branch}` (server-side filter, GitHub-like) |
| `find_issue_by_marker` | `GET issues?state=all` + `description` scan, then a marker-note fallback per issue |
| `ensure_labels` | `GET labels`, `POST labels` (`#`-prefixed color; 409 = already there) |
| `create_issue` | `POST issues` (label **names**, ONE comma-separated string; missing labels auto-created) |
| comments | notes API per noteable: `GET {kind}/{iid}/notes`, `PUT {kind}/{iid}/notes/{id}` / `POST {kind}/{iid}/notes` — issue and MR notes do NOT cross over |
| `set_issue_labels` / `set_pr_labels` | `PUT issues/{iid}` / `PUT merge_requests/{iid}` (`labels` = absolute comma-separated replacement) |
| `close_issue` | `PUT issues/{iid}` `{"state_event":"close"}` |
| `open_pr` | `POST merge_requests` (labels ride the create — no second call) |

GitLab quirks normalized away: the project path is URL-encoded into one
segment; auth is the `PRIVATE-TOKEN` header; bodies are `description`;
labels come back as plain strings; issues and MRs have **separate iid
sequences** (label/comment endpoints never cross over); `merged` is the
distinct `state == "merged"` (not closed — the inverse of Gitea/GitHub, and
the shared diff handles both shapes); `merge_commit_sha` is documented "null
until merged" (the inverse hazard of GitHub's test-merge sha) with
`squash_commit_sha` / head `sha` as the merged-state fallbacks; CI is the
latest pipeline for the head sha.

**The GitHub and GitLab adapters are never handed out raw.** Their only
public constructors (`open_github` / `open_gitlab`, plus each
`fixture_forge`) return `DryRun(...)`: reads delegate live (or to fixtures),
every mutation is recorded and never sent. Mutating github.com or a GitLab
instance is unrepresentable (ADR-0012, ADR-0016). Gitea remains the one
live-write lifecycle host, sanctioned only as the throwaway local container.

## DryRun normalization rules

`DryRunForge` serializes each would-be mutation through
`transcript::normalize_action` — the SAME function the demo-transcript
emitter uses, so the two transcript producers cannot drift:

- Forge-assigned ids → `$ISSUE_1`/`$PR_1`… placeholders in first-seen order
  (synthesized DryRun ids and live forge ids route through the same table).
- Timestamps and durations: omitted entirely.
- `effort:*` label **values** → `effort:$REDACTED` (they derive from
  wall-clock; transcript-only — real PRs always carry the real label).
- Repo slug → `$REPO` in body fields.
- Line shape: one JSON object per line, `{"action":"<kind>", ...}`, keys
  sorted.

Synthesized ids start at 9 000 000 000 so they can never collide with a real
forge number passed back in. The probe overlay: `find_open_pr_by_head`
consults recorded open PRs before delegating, so the open-PR replay
round-trip works even though the PR never reached a real forge.

## Conformance

`tests/conformance.rs` is one suite body (`run_conformance`) run against
every implementation: FakeForge (always), GitHub recorded fixtures (always,
no network), GitLab authored fixtures (always, no network), live Gitea
(`CONDUIT_E2E_GITEA=1`), live GitHub reads (`CONDUIT_E2E_GITHUB=1`).
"Identically" is a CI assertion, not a slogan — see
[Testing](./testing.md).
