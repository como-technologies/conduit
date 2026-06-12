//! GitLab REST v4 adapter (the N=3 forge-neutrality proof; ADR-0016: GitLab
//! is record-only — the spike NEVER mutates a GitLab instance). The only
//! public constructors, [`open_gitlab`] and [`fixture_forge`], hand out a
//! [`DryRunForge`]`<GitLabForge>`: reads delegate to this adapter, every
//! mutation is recorded to the transcript and never sent. The mutation
//! methods below exist so the payload builders are unit-testable (and so
//! DryRun *could* delegate one day), but nothing outside this module can
//! reach an unwrapped `GitLabForge`.
//!
//! Adapter obligations from the `forge` module header, honored here:
//! - Disappearance rule: snapshots fetch `state=all` for BOTH issues and
//!   merge requests (`all` is a documented value on both list endpoints) —
//!   merged/closed items stay visible until their terminal events have been
//!   observed.
//! - Explicit pagination: every list call loops `?page=N&per_page=100` and
//!   stops on a short page (`HttpResponse` carries no `x-next-page` header
//!   by design).
//! - Review identity (the third shape — dismissal-by-REMOVAL): GitLab has no
//!   review-submission objects at all. The closest documented analog is MR
//!   approvals: `approved_by` rows carry `{user, approved_at}` and NO
//!   forge-native id, and revoking an approval REMOVES the row from the API
//!   rather than keeping it (Gitea) or overwriting its state in place
//!   (GitHub). This adapter therefore filters NOTHING and synthesizes the
//!   review id from two documented fields, `{user.id}@{approved_at}`: a
//!   standing approval keeps a stable id across polls (dedupe holds), a
//!   revocation just removes a row (the diff fires nothing on review
//!   disappearance), and a re-approval mints a fresh `approved_at` — a NEW
//!   id that correctly fires. A vanished id can never reappear bearing the
//!   same verdict instance.
//!   LIMITATION (documented; ADR-0016): approvals only map to `Approved`.
//!   GitLab's "request changes" is a mutable MR-level status
//!   (`detailed_merge_status: "requested_changes"`) with no per-event
//!   identity, author timestamp, or body, so it cannot honestly satisfy the
//!   diff's dedupe-by-id contract — `ChangesRequested`/`Commented` are not
//!   derivable from a GitLab snapshot. Acceptable while GitLab is
//!   record-only: the demo transcript scripts its review events, and no live
//!   lifecycle runs on GitLab (see the ADR for the promotion path).
//! - Id-uniqueness: `iid` (the project-scoped number) is unique per resource
//!   kind. GitLab QUIRK vs Gitea/GitHub: issues and MRs are SEPARATE
//!   resources with SEPARATE iid sequences, and the label/comment endpoints
//!   do NOT cross over — PR-side calls must ride `merge_requests/{iid}`,
//!   never `issues/{iid}`.
//!
//! More GitLab quirks vs the other adapters, all from the documented API:
//! - The project path is URL-encoded into one segment:
//!   `/api/v4/projects/{owner}%2F{repo}/...`.
//! - Auth rides the `PRIVATE-TOKEN` header.
//! - Issue/MR bodies live in `description` (not `body`); the issues listing
//!   never includes MRs (no `pull_request`-row filtering needed).
//! - Labels are arrays of plain STRINGS in responses; label writes take a
//!   comma-separated name string (conduit-owned labels never contain commas)
//!   and auto-create missing labels; `POST labels` wants a `#`-prefixed
//!   color and answers 409 on conflict.
//! - `merged` is `state == "merged"` — a distinct state, NOT closed (the
//!   inverse of Gitea/GitHub, which mark merged PRs closed too; the shared
//!   diff handles both shapes). `merge_commit_sha` is documented "null until
//!   merged" (the inverse hazard of GitHub's test-merge sha): read it only
//!   when merged, falling back to `squash_commit_sha`, then the head `sha`.
//! - Closing an issue is `PUT issues/{iid}` with `state_event: "close"`,
//!   not a `state` field.
//! - CI: the latest pipeline for the head sha (`GET pipelines?sha=...`),
//!   newest first; no pipelines for the sha = `CiState::None`.

use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::dry_run::DryRunForge;
use super::{
    CiState, Forge, ForgeError, HttpResponse, HttpTransport, IssueSnapshot, LabelSpec, NewIssue,
    PrDraft, PrSnapshot, RepoSnapshot, Review, UreqTransport, rest_call,
};
use crate::task::{IssueId, PrId, ReviewId, ReviewVerdict};

/// Page size for every list call; a page shorter than this ends the loop.
const PAGE_LIMIT: usize = 100;

/// The repo slug the authored fixtures under tests/fixtures/gitlab assume.
/// Unlike the GitHub fixtures these are AUTHORED from the documented REST v4
/// response shapes, not recorded from a live instance — no sanctioned GitLab
/// host exists to record from (ADR-0016 records the trade and the upgrade
/// path: a recorder mirroring github::record_fixtures when one does).
pub const FIXTURE_OWNER: &str = "fixture-owner";
pub const FIXTURE_REPO: &str = "fixture-repo";

pub struct GitLabForge {
    transport: Box<dyn HttpTransport>,
    base_url: String, // e.g. "https://gitlab.com" (no trailing slash)
    owner: String,
    repo: String,
    token: String, // env GITLAB_TOKEN — reads only
}

/// The ONLY public way to construct a GitLab forge in production: always
/// DryRun-wrapped (ADR-0016 hard constraint — no mutation of any GitLab
/// instance, ever; same posture as GitHub's ADR-0012).
pub fn open_gitlab(cfg: &crate::config::GitlabConfig, token: String) -> DryRunForge<GitLabForge> {
    let slug = format!("{}/{}", cfg.owner, cfg.repo);
    DryRunForge::with_repo_slug(
        GitLabForge::new(
            Box::new(UreqTransport),
            &cfg.base_url,
            &cfg.owner,
            &cfg.repo,
            &token,
        ),
        &slug,
    )
}

/// DryRun-wrapped GitLab forge whose reads are served from authored fixture
/// files in `dir` (no network; mutations would panic in the transport, but
/// the DryRun wrapper never lets one through). Test support for the
/// always-on conformance leg.
pub fn fixture_forge(dir: &str) -> DryRunForge<GitLabForge> {
    let slug = format!("{FIXTURE_OWNER}/{FIXTURE_REPO}");
    DryRunForge::with_repo_slug(
        GitLabForge::new(
            Box::new(DirFixtureTransport { dir: dir.into() }),
            "https://gitlab.example.test",
            FIXTURE_OWNER,
            FIXTURE_REPO,
            "fixture-token",
        ),
        &slug,
    )
}

/// Token resolution: env GITLAB_TOKEN (trimmed), else None. The token is
/// never printed or logged. No CLI fallback — reads-only, like GitHub's env
/// leg.
pub fn resolve_token() -> Option<String> {
    match std::env::var("GITLAB_TOKEN") {
        Ok(token) if !token.trim().is_empty() => Some(token.trim().to_string()),
        _ => None,
    }
}

impl GitLabForge {
    /// Private on purpose: `open_gitlab`/`fixture_forge` are the only ways
    /// out of this module, and both wrap in DryRun.
    fn new(
        transport: Box<dyn HttpTransport>,
        base_url: &str,
        owner: &str,
        repo: &str,
        token: &str,
    ) -> GitLabForge {
        GitLabForge {
            transport,
            base_url: base_url.trim_end_matches('/').to_string(),
            owner: owner.to_string(),
            repo: repo.to_string(),
            token: token.to_string(),
        }
    }

    #[cfg(test)]
    pub(crate) fn raw_for_tests(
        transport: Box<dyn HttpTransport>,
        base_url: &str,
        owner: &str,
        repo: &str,
    ) -> GitLabForge {
        GitLabForge::new(transport, base_url, owner, repo, "test-token")
    }

    // -- wire plumbing --------------------------------------------------

    /// One project-scoped REST call: `path` is relative to
    /// `{base_url}/api/v4/projects/{owner}%2F{repo}/` (the project path is
    /// URL-encoded into a single segment — the documented addressing form).
    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, ForgeError> {
        let url = format!(
            "{}/api/v4/projects/{}%2F{}/{}",
            self.base_url, self.owner, self.repo, path
        );
        rest_call(
            self.transport.as_ref(),
            method,
            &url,
            &[
                ("PRIVATE-TOKEN", &self.token),
                ("Content-Type", "application/json"),
            ],
            body,
            "gitlab",
        )
    }

    /// Paginated GET (module-header obligation: EXPLICIT `?page=N&per_page=100`
    /// loop, stop on a short page — `HttpResponse` carries no `x-next-page`
    /// header, and a page-1-only fetch silently truncates at the server
    /// default (20!) and breaks the disappearance rule).
    fn get_paginated(&self, path_and_query: &str) -> Result<Vec<Value>, ForgeError> {
        let sep = if path_and_query.contains('?') {
            '&'
        } else {
            '?'
        };
        let mut out = Vec::new();
        let mut page = 1usize;
        loop {
            let path = format!("{path_and_query}{sep}page={page}&per_page={PAGE_LIMIT}");
            let Value::Array(items) = self.call("GET", &path, None)? else {
                return Err(ForgeError::Api {
                    status: 200,
                    message: format!("gitlab: expected a JSON array from {path}"),
                });
            };
            let n = items.len();
            out.extend(items);
            if n < PAGE_LIMIT {
                return Ok(out);
            }
            page += 1;
        }
    }

    /// Raw issue listing. The project issues endpoint never includes MRs
    /// (separate resource — no `pull_request`-row filtering needed) and
    /// documents `state=all`. Shared by fetch_snapshot and the marker probe —
    /// the probe must see ALL issues, not just conduit-labeled ones.
    fn list_all_issues(&self) -> Result<Vec<Value>, ForgeError> {
        self.get_paginated("issues?state=all")
    }

    // -- comments (notes) ---------------------------------------------------

    /// Find the id of the note on `kind` ("issues"/"merge_requests") `iid`
    /// whose body carries `marker` (the upsert identity).
    fn find_marker_note(
        &self,
        kind: &str,
        iid: u64,
        marker: &str,
    ) -> Result<Option<u64>, ForgeError> {
        for n in self.get_paginated(&format!("{kind}/{iid}/notes"))? {
            if n.get("body")
                .and_then(|b| b.as_str())
                .is_some_and(|b| b.contains(marker))
            {
                return Ok(Some(field_u64(&n, "id")?));
            }
        }
        Ok(None)
    }

    /// Marker-note upsert: edit the existing marker note in place, or create
    /// one with the marker embedded. GitLab QUIRK: notes are addressed under
    /// their noteable (`{kind}/{iid}/notes/{id}`) — there is no global
    /// comment id endpoint, and issue/MR notes do NOT cross over.
    fn upsert_note(
        &self,
        kind: &str,
        iid: u64,
        marker: &str,
        body: &str,
    ) -> Result<(), ForgeError> {
        let text = format!("{marker}\n\n{body}");
        match self.find_marker_note(kind, iid, marker)? {
            Some(id) => {
                self.call(
                    "PUT",
                    &format!("{kind}/{iid}/notes/{id}"),
                    Some(json!({"body": text})),
                )?;
            }
            None => {
                self.call(
                    "POST",
                    &format!("{kind}/{iid}/notes"),
                    Some(json!({"body": text})),
                )?;
            }
        }
        Ok(())
    }

    // -- snapshot pieces ----------------------------------------------------

    /// All standing approvals of one MR, as `Approved` reviews — the module
    /// header documents why this is the whole honest mapping (GitLab has no
    /// review-submission objects; dismissal is removal; ids are synthesized
    /// from the documented `{user.id}` + `{approved_at}` pair). A 404 means
    /// the approvals feature is unavailable on the instance — no approvals,
    /// not an error.
    fn fetch_reviews(&self, iid: u64) -> Result<Vec<Review>, ForgeError> {
        let v = match self.call("GET", &format!("merge_requests/{iid}/approvals"), None) {
            Ok(v) => v,
            Err(ForgeError::Api { status: 404, .. }) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut reviews = Vec::new();
        for raw in v
            .get("approved_by")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
        {
            let user_id = raw.pointer("/user/id").and_then(|n| n.as_u64());
            let approved_at = raw.get("approved_at").and_then(|s| s.as_str());
            let (Some(user_id), Some(approved_at)) = (user_id, approved_at) else {
                // A row without the documented identity fields cannot dedupe
                // soundly — loud, not silently skipped.
                return Err(ForgeError::Api {
                    status: 200,
                    message: format!(
                        "gitlab: approved_by row missing user.id/approved_at on MR {iid}"
                    ),
                });
            };
            reviews.push(Review {
                id: ReviewId(format!("{user_id}@{approved_at}")),
                author: raw
                    .pointer("/user/username")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string(),
                verdict: ReviewVerdict::Approved,
                body: String::new(), // approvals carry no body
                submitted_at: OffsetDateTime::parse(approved_at, &Rfc3339)
                    .unwrap_or(OffsetDateTime::UNIX_EPOCH),
            });
        }
        Ok(reviews)
    }

    /// Latest pipeline for the head sha -> CiState. No pipelines = None.
    fn fetch_ci(&self, head_sha: &str) -> Result<CiState, ForgeError> {
        if head_sha.is_empty() {
            return Ok(CiState::None);
        }
        // Newest-first by default; one row decides.
        let v = self.call("GET", &format!("pipelines?sha={head_sha}&per_page=1"), None)?;
        let Some(latest) = v.as_array().and_then(|a| a.first()) else {
            return Ok(CiState::None);
        };
        Ok(match latest.get("status").and_then(|s| s.as_str()) {
            Some("success") => CiState::Success,
            Some("failed") | Some("canceled") => CiState::Failure,
            // created / waiting_for_resource / preparing / pending / running
            // / scheduled / manual: in flight or awaiting action.
            Some(_) => CiState::Pending,
            None => CiState::None,
        })
    }
}

impl Forge for GitLabForge {
    fn describe(&self) -> String {
        format!("gitlab {}/{} at {}", self.owner, self.repo, self.base_url)
    }

    /// No API call. The spike never pushes here: src/git.rs refuses any
    /// non-localhost push URL (Task 11 guard) — and the gitlab transcript
    /// leg is record-only, so it never even clones.
    fn git_remote_url(&self) -> Result<String, ForgeError> {
        Ok(format!(
            "{}/{}/{}.git",
            self.base_url, self.owner, self.repo
        ))
    }

    fn fetch_snapshot(&self) -> Result<RepoSnapshot, ForgeError> {
        // Normalization filter (RepoSnapshot doc): conduit-labeled issues +
        // conduit/*-branch MRs only. state=all on BOTH lists — terminal items
        // must stay until their events are observed (disappearance rule).
        let mut issues = Vec::new();
        for raw in self.list_all_issues()? {
            let labels = label_strings(&raw);
            if !labels
                .iter()
                .any(|l| l.starts_with("conduit:") || l.starts_with("adr:"))
            {
                continue;
            }
            issues.push(IssueSnapshot {
                id: IssueId(field_u64(&raw, "iid")?),
                labels,
                closed: raw.get("state").and_then(|s| s.as_str()) == Some("closed"),
            });
        }

        let mut prs = Vec::new();
        for raw in self.get_paginated("merge_requests?state=all")? {
            let head_branch = raw
                .get("source_branch")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if !head_branch.starts_with("conduit/") {
                continue;
            }
            let iid = field_u64(&raw, "iid")?;
            let head_sha = raw
                .get("sha")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let state = raw
                .get("state")
                .and_then(|s| s.as_str())
                .unwrap_or_default();
            // GitLab: merged is a DISTINCT state (not closed). The documented
            // merge_commit_sha is "null until merged"; squash merges populate
            // squash_commit_sha instead; fast-forward merges may leave both
            // null — the head sha then IS the merged commit.
            let merged = state == "merged";
            let merge_sha = if merged {
                ["merge_commit_sha", "squash_commit_sha"]
                    .iter()
                    .find_map(|k| raw.get(*k).and_then(|v| v.as_str()))
                    .map(str::to_string)
                    .or_else(|| Some(head_sha.clone()).filter(|s| !s.is_empty()))
            } else {
                None
            };
            prs.push(PrSnapshot {
                id: PrId(iid),
                title: field_str(&raw, "title"),
                body: field_str(&raw, "description"),
                head_branch,
                labels: label_strings(&raw),
                reviews: self.fetch_reviews(iid)?,
                ci: self.fetch_ci(&head_sha)?,
                merged,
                merge_sha,
                closed: state == "closed",
            });
        }

        Ok(RepoSnapshot {
            issues,
            prs,
            fetched_at: OffsetDateTime::now_utc(),
        })
    }

    /// GitLab has a server-side source-branch filter (GitHub-like, not
    /// Gitea-like): one query, at most one open MR can match a head branch.
    fn find_open_pr_by_head(&self, branch: &str) -> Result<Option<PrId>, ForgeError> {
        let v = self.call(
            "GET",
            &format!("merge_requests?state=opened&source_branch={branch}"),
            None,
        )?;
        match v.as_array().and_then(|a| a.first()) {
            Some(raw) => Ok(Some(PrId(field_u64(raw, "iid")?))),
            None => Ok(None),
        }
    }

    /// Identical semantics to the other adapters — body (`description`) scan
    /// first, then the marker-note fallback. The fallback is O(issues) x
    /// O(notes/issue); see the Gitea twin for the cost note.
    fn find_issue_by_marker(&self, marker: &str) -> Result<Option<IssueId>, ForgeError> {
        let all = self.list_all_issues()?;
        // Body scan first — create_issue embeds the marker there.
        for raw in &all {
            if raw
                .get("description")
                .and_then(|b| b.as_str())
                .is_some_and(|b| b.contains(marker))
            {
                return Ok(Some(IssueId(field_u64(raw, "iid")?)));
            }
        }
        // The marker may instead live in an upserted status note.
        for raw in &all {
            let iid = field_u64(raw, "iid")?;
            if self.find_marker_note("issues", iid, marker)?.is_some() {
                return Ok(Some(IssueId(iid)));
            }
        }
        Ok(None)
    }

    fn ensure_labels(&self, labels: &[LabelSpec]) -> Result<(), ForgeError> {
        let existing: Vec<String> = self
            .get_paginated("labels")?
            .iter()
            .filter_map(|l| l.get("name").and_then(|n| n.as_str()).map(str::to_string))
            .collect();
        for spec in labels {
            if existing.contains(&spec.name) {
                continue;
            }
            match self.call(
                "POST",
                "labels",
                Some(json!({
                    "name": spec.name,
                    // GitLab wants the leading '#'; LabelSpec carries bare hex.
                    "color": format!("#{}", spec.color),
                    "description": spec.description,
                })),
            ) {
                Ok(_) => {}
                // Already exists (create race) — GitLab answers 409:
                // converged, not an error.
                Err(ForgeError::Api { status: 409, .. }) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Labels ride the create as a comma-separated NAME string (documented;
    /// missing labels are auto-created project-side — no id resolution and
    /// no second call). Conduit-owned label names never contain commas.
    fn create_issue(&self, new: &NewIssue) -> Result<IssueId, ForgeError> {
        let v = self.call(
            "POST",
            "issues",
            Some(json!({
                "title": new.title,
                "description": new.body,
                "labels": new.labels.join(","),
            })),
        )?;
        Ok(IssueId(field_u64(&v, "iid")?))
    }

    fn upsert_issue_comment(
        &self,
        id: &IssueId,
        marker: &str,
        body: &str,
    ) -> Result<(), ForgeError> {
        self.upsert_note("issues", id.0, marker, body)
    }

    /// ADR-0007 convergence probe: current label names from the single-issue
    /// read (labels are plain strings on GitLab).
    fn get_issue_labels(&self, id: &IssueId) -> Result<Vec<String>, ForgeError> {
        Ok(label_strings(&self.call(
            "GET",
            &format!("issues/{}", id.0),
            None,
        )?))
    }

    /// GitLab QUIRK: MRs are NOT issues — the probe must read the MR.
    fn get_pr_labels(&self, id: &PrId) -> Result<Vec<String>, ForgeError> {
        Ok(label_strings(&self.call(
            "GET",
            &format!("merge_requests/{}", id.0),
            None,
        )?))
    }

    /// `labels` on update is an absolute replacement set (comma-separated
    /// names; empty string unassigns all) — convergent by the documented
    /// contract.
    fn set_issue_labels(&self, id: &IssueId, labels: &[String]) -> Result<(), ForgeError> {
        self.call(
            "PUT",
            &format!("issues/{}", id.0),
            Some(json!({"labels": labels.join(",")})),
        )?;
        Ok(())
    }

    /// Closing is a `state_event`, not a `state` field (GitLab difference).
    fn close_issue(&self, id: &IssueId) -> Result<(), ForgeError> {
        self.call(
            "PUT",
            &format!("issues/{}", id.0),
            Some(json!({"state_event": "close"})),
        )?;
        Ok(())
    }

    fn open_pr(&self, draft: &PrDraft) -> Result<PrId, ForgeError> {
        // Labels ride the create (documented `labels` param) — no second
        // call, unlike Gitea (id resolution) and GitHub (labels via PUT).
        let mut body = json!({
            "title": draft.title,
            "description": draft.body,
            "source_branch": draft.head,
            "target_branch": draft.base,
        });
        if !draft.labels.is_empty() {
            body["labels"] = json!(draft.labels.join(","));
        }
        let v = self.call("POST", "merge_requests", Some(body))?;
        Ok(PrId(field_u64(&v, "iid")?))
    }

    fn upsert_pr_comment(&self, id: &PrId, marker: &str, body: &str) -> Result<(), ForgeError> {
        // GitLab QUIRK: MR notes live under merge_requests/{iid}/notes — the
        // issue-note endpoints do NOT accept MR iids (separate sequences).
        self.upsert_note("merge_requests", id.0, marker, body)
    }

    fn set_pr_labels(&self, id: &PrId, labels: &[String]) -> Result<(), ForgeError> {
        self.call(
            "PUT",
            &format!("merge_requests/{}", id.0),
            Some(json!({"labels": labels.join(",")})),
        )?;
        Ok(())
    }
}

/// Optional string field, defaulting to "" (an MR with a null description is
/// legal).
fn field_str(v: &Value, name: &str) -> String {
    v.get(name)
        .and_then(|s| s.as_str())
        .unwrap_or_default()
        .to_string()
}

/// GitLab labels are arrays of plain strings (objects only appear under
/// `with_labels_details=true`, which this adapter never requests).
fn label_strings(v: &Value) -> Vec<String> {
    v.get("labels")
        .and_then(|l| l.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Required numeric field — a missing/odd-typed one is a loud Api error, not
/// a silent skip (snapshot identity depends on these).
fn field_u64(v: &Value, name: &str) -> Result<u64, ForgeError> {
    v.get(name)
        .and_then(|n| n.as_u64())
        .ok_or_else(|| ForgeError::Api {
            status: 200,
            message: format!("gitlab: response missing numeric `{name}`"),
        })
}

// ---------------------------------------------------------------------------
// Authored-fixture transport — serves tests/fixtures/gitlab/* by URL pattern
// (test support for the always-on conformance leg; no network, ever). The
// bodies are authored from the documented REST v4 shapes — see the module
// header and ADR-0016 for why no recorder exists yet.
// ---------------------------------------------------------------------------

struct DirFixtureTransport {
    dir: std::path::PathBuf,
}

impl DirFixtureTransport {
    fn file(&self, name: &str) -> Option<HttpResponse> {
        std::fs::read(self.dir.join(name))
            .ok()
            .map(|body| HttpResponse { status: 200, body })
    }

    fn empty_array() -> HttpResponse {
        HttpResponse {
            status: 200,
            body: b"[]".to_vec(),
        }
    }
}

/// `page` query parameter (exact key — `per_page` must not match), default 1.
fn page_of(url: &str) -> u64 {
    url.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("page=").and_then(|v| v.parse().ok()))
        .unwrap_or(1)
}

impl HttpTransport for DirFixtureTransport {
    fn request(
        &self,
        method: &str,
        url: &str,
        _headers: &[(&str, &str)],
        _body: Option<&[u8]>,
    ) -> Result<HttpResponse, ForgeError> {
        // The DryRun wrapper records mutations instead of delegating — a
        // non-GET arriving here means the hard constraint was violated.
        assert_eq!(
            method, "GET",
            "DryRun must never let a mutation reach GitLab: {method} {url}"
        );
        let (path, query) = url.split_once('?').unwrap_or((url, ""));
        // path: {base}/api/v4/projects/{owner}%2F{repo}/rest...
        let rest = path
            .split_once("/api/v4/projects/")
            .map(|(_, r)| r)
            .unwrap_or("");
        let segments: Vec<&str> = rest.split('/').collect();
        // segments: [encoded-project-path, rest...]
        let resp = match segments.get(1..).unwrap_or(&[]) {
            ["issues"] if page_of(url) == 1 => {
                Some(self.file("issues.json").unwrap_or_else(|| {
                    panic!("missing fixture issues.json under {}", self.dir.display())
                }))
            }
            ["issues"] => Some(Self::empty_array()),
            ["merge_requests"] if query.contains("source_branch=") => Some(Self::empty_array()),
            ["merge_requests"] if page_of(url) == 1 => {
                Some(self.file("merge_requests.json").unwrap_or_else(|| {
                    panic!(
                        "missing fixture merge_requests.json under {}",
                        self.dir.display()
                    )
                }))
            }
            ["merge_requests"] => Some(Self::empty_array()),
            ["merge_requests", n, "approvals"] => Some(
                self.file(&format!("approvals_{n}.json"))
                    .unwrap_or(HttpResponse {
                        status: 200,
                        body: br#"{"approved_by": []}"#.to_vec(),
                    }),
            ),
            ["merge_requests", n, "notes"] => Some(
                self.file(&format!("notes_mr_{n}.json"))
                    .unwrap_or_else(Self::empty_array),
            ),
            ["issues", n, "notes"] => Some(
                self.file(&format!("notes_issue_{n}.json"))
                    .unwrap_or_else(Self::empty_array),
            ),
            ["pipelines"] => {
                let sha = query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("sha="))
                    .unwrap_or("");
                Some(
                    self.file(&format!("pipelines_{sha}.json"))
                        .unwrap_or_else(Self::empty_array),
                )
            }
            ["labels"] => Some(self.file("labels.json").unwrap_or_else(Self::empty_array)),
            _ => None,
        };
        Ok(resp.unwrap_or_else(|| panic!("no fixture route for GET {url}")))
    }
}

// ---------------------------------------------------------------------------
// Fixture-based unit tests — every test names its exact wire traffic; an
// unexpected request panics. Fixture bodies mirror the documented GitLab
// REST v4 shapes (api/issues, api/merge_requests, api/merge_request_approvals,
// api/notes, api/pipelines, api/labels — see the module header).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::HttpResponse;
    use std::sync::{Arc, Mutex};
    use time::macros::datetime;

    /// One request as the transport saw it, for payload/header assertions.
    #[derive(Debug, Clone)]
    struct Recorded {
        method: String,
        url: String,
        headers: Vec<(String, String)>,
        body: Option<String>,
    }

    type Seen = Arc<Mutex<Vec<Recorded>>>;

    /// (method, url fragment) -> (status, body); consumed in order per match.
    /// Any request that matches no remaining route panics.
    struct FixtureTransport {
        routes: Mutex<Vec<(String, String, u16, String)>>,
        seen: Seen,
    }

    impl HttpTransport for FixtureTransport {
        fn request(
            &self,
            method: &str,
            url: &str,
            headers: &[(&str, &str)],
            body: Option<&[u8]>,
        ) -> Result<HttpResponse, ForgeError> {
            self.seen.lock().unwrap().push(Recorded {
                method: method.to_string(),
                url: url.to_string(),
                headers: headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                body: body.map(|b| String::from_utf8_lossy(b).into_owned()),
            });
            let mut routes = self.routes.lock().unwrap();
            let pos = routes
                .iter()
                .position(|(m, frag, _, _)| m == method && url.contains(frag.as_str()))
                .unwrap_or_else(|| panic!("unexpected request: {method} {url}"));
            let (_, _, status, body) = routes.remove(pos);
            Ok(HttpResponse {
                status,
                body: body.into_bytes(),
            })
        }
    }

    fn forge_with(routes: Vec<(&str, &str, u16, String)>) -> (GitLabForge, Seen) {
        let seen: Seen = Arc::default();
        let transport = FixtureTransport {
            routes: Mutex::new(
                routes
                    .into_iter()
                    .map(|(m, u, s, b)| (m.into(), u.into(), s, b))
                    .collect(),
            ),
            seen: seen.clone(),
        };
        (
            GitLabForge::raw_for_tests(
                Box::new(transport),
                "https://gitlab.example.test",
                "octo",
                "example",
            ),
            seen,
        )
    }

    fn last_body(seen: &Seen) -> Value {
        let recorded = seen.lock().unwrap();
        let body = recorded
            .last()
            .and_then(|r| r.body.clone())
            .expect("a request body");
        serde_json::from_str(&body).expect("JSON request body")
    }

    #[test]
    fn snapshot_filters_to_conduit_items_with_string_labels() {
        let (f, _) = forge_with(vec![
            (
                "GET",
                "/issues?state=all&page=1",
                200,
                // GitLab labels are plain STRINGS; bodies live in description.
                r#"[
                    {"iid": 1, "state": "opened", "description": "x",
                     "labels": ["adr:ADR-0003"]},
                    {"iid": 2, "state": "opened", "description": "y",
                     "labels": ["bug"]}
                ]"#
                .into(),
            ),
            (
                "GET",
                "/merge_requests?state=all&page=1",
                200,
                // MR 7: open — merge_commit_sha is null until merged
                // (documented), unlike GitHub's test-merge sha.
                r#"[
                    {"iid": 7, "state": "opened",
                     "merge_commit_sha": null, "squash_commit_sha": null,
                     "title": "[ADR-0003] adopt snapshot router",
                     "description": "Implements the decision.\n\nAdr-Reference: ADR-0003",
                     "source_branch": "conduit/adr-0003/x", "sha": "abc",
                     "labels": ["adr:ADR-0003"]},
                    {"iid": 8, "state": "opened",
                     "merge_commit_sha": null, "squash_commit_sha": null,
                     "title": "unrelated", "description": "",
                     "source_branch": "feature/other", "sha": "def",
                     "labels": []}
                ]"#
                .into(),
            ),
            (
                "GET",
                "/merge_requests/7/approvals",
                200,
                // approved_by rows: {user, approved_at} — no forge-native id.
                r#"{"approved_by": [
                    {"user": {"id": 5, "username": "reviewer"},
                     "approved_at": "2026-06-11T10:00:00Z"}
                ]}"#
                .into(),
            ),
            (
                "GET",
                "/pipelines?sha=abc",
                200,
                r#"[{"id": 31, "sha": "abc", "status": "running"}]"#.into(),
            ),
        ]);
        let snap = f.fetch_snapshot().unwrap();
        assert_eq!(snap.issues.len(), 1, "non-conduit issue filtered");
        assert_eq!(snap.issues[0].id, IssueId(1));
        assert_eq!(snap.prs.len(), 1, "non-conduit/* MR filtered");
        let pr = &snap.prs[0];
        // GAP A twin: title and description must be parsed verbatim.
        assert_eq!(
            pr.title, "[ADR-0003] adopt snapshot router",
            "PrSnapshot.title must carry the forge title verbatim"
        );
        assert_eq!(
            pr.body, "Implements the decision.\n\nAdr-Reference: ADR-0003",
            "PrSnapshot.body must carry the MR description verbatim"
        );
        assert!(!pr.merged, "state opened means unmerged");
        assert_eq!(pr.merge_sha, None, "merge_sha only when merged");
        assert_eq!(pr.reviews.len(), 1);
        assert_eq!(
            pr.reviews[0].id,
            ReviewId("5@2026-06-11T10:00:00Z".into()),
            "review identity synthesized from user.id + approved_at"
        );
        assert_eq!(pr.reviews[0].verdict, ReviewVerdict::Approved);
        assert_eq!(pr.reviews[0].author, "reviewer");
        assert_eq!(pr.reviews[0].submitted_at, datetime!(2026-06-11 10:00 UTC));
        assert_eq!(pr.ci, CiState::Pending, "running pipeline -> Pending");
    }

    /// Disappearance rule (module-header obligation): state=all keeps a
    /// merged MR and a closed issue in the snapshot, with merge_sha. GitLab
    /// nuance: "merged" is a DISTINCT state — the MR is merged but NOT
    /// closed in the raw state field (the shared diff handles both shapes).
    #[test]
    fn snapshot_keeps_terminal_mrs_and_closed_issues() {
        let (f, _) = forge_with(vec![
            (
                "GET",
                "/issues?state=all&page=1",
                200,
                r#"[{"iid": 4, "state": "closed", "description": "",
                     "labels": ["conduit:run"]}]"#
                    .into(),
            ),
            (
                "GET",
                "/merge_requests?state=all&page=1",
                200,
                r#"[
                    {"iid": 7, "state": "merged",
                     "merge_commit_sha": "cafe42", "squash_commit_sha": null,
                     "source_branch": "conduit/adr-0001/x", "sha": "abc",
                     "labels": []},
                    {"iid": 9, "state": "closed",
                     "merge_commit_sha": null, "squash_commit_sha": null,
                     "source_branch": "conduit/adr-0002/y", "sha": "def",
                     "labels": []}
                ]"#
                .into(),
            ),
            (
                "GET",
                "/merge_requests/7/approvals",
                200,
                r#"{"approved_by": []}"#.into(),
            ),
            ("GET", "/pipelines?sha=abc", 200, "[]".into()),
            (
                "GET",
                "/merge_requests/9/approvals",
                200,
                r#"{"approved_by": []}"#.into(),
            ),
            ("GET", "/pipelines?sha=def", 200, "[]".into()),
        ]);
        let snap = f.fetch_snapshot().unwrap();
        assert_eq!(snap.issues.len(), 1, "closed issue must stay");
        assert!(snap.issues[0].closed);
        assert_eq!(snap.prs.len(), 2, "terminal MRs must stay");
        let merged = &snap.prs[0];
        assert!(merged.merged);
        assert!(
            !merged.closed,
            "GitLab merged is a distinct state, not closed"
        );
        assert_eq!(merged.merge_sha.as_deref(), Some("cafe42"));
        assert_eq!(merged.ci, CiState::None, "no pipelines -> None");
        let closed = &snap.prs[1];
        assert!(!closed.merged && closed.closed);
        assert_eq!(closed.merge_sha, None);
    }

    /// Squash and fast-forward merges: merge_commit_sha may be null even
    /// when merged — fall back to squash_commit_sha, then the head sha, so
    /// the adapter keeps the contract's "merge_sha required when merged".
    #[test]
    fn merged_mr_merge_sha_falls_back_to_squash_then_head_sha() {
        let mrs = |merge: &str, squash: &str| {
            format!(
                r#"[{{"iid": 7, "state": "merged",
                     "merge_commit_sha": {merge}, "squash_commit_sha": {squash},
                     "source_branch": "conduit/adr-0001/x", "sha": "headsha",
                     "labels": []}}]"#
            )
        };
        for (merge, squash, want) in [
            ("null", "\"squashsha\"", "squashsha"),
            ("null", "null", "headsha"),
        ] {
            let (f, _) = forge_with(vec![
                ("GET", "/issues?state=all&page=1", 200, "[]".into()),
                (
                    "GET",
                    "/merge_requests?state=all&page=1",
                    200,
                    mrs(merge, squash),
                ),
                (
                    "GET",
                    "/merge_requests/7/approvals",
                    200,
                    r#"{"approved_by": []}"#.into(),
                ),
                ("GET", "/pipelines?sha=headsha", 200, "[]".into()),
            ]);
            let snap = f.fetch_snapshot().unwrap();
            assert_eq!(snap.prs[0].merge_sha.as_deref(), Some(want));
        }
    }

    /// Dismissal-by-removal (module header): a revoked approval is simply
    /// absent from approved_by — the remaining rows map, nothing is filtered
    /// by the adapter, and a re-approval carries a NEW approved_at, hence a
    /// new id (the diff then correctly fires a fresh ReviewSubmitted).
    #[test]
    fn re_approval_after_revoke_mints_a_new_review_id() {
        let approvals = |at: &str| {
            format!(
                r#"{{"approved_by": [
                    {{"user": {{"id": 5, "username": "reviewer"}},
                      "approved_at": "{at}"}}
                ]}}"#
            )
        };
        let fetch = |body: String| {
            let (f, _) = forge_with(vec![("GET", "/merge_requests/7/approvals", 200, body)]);
            f.fetch_reviews(7).unwrap()
        };
        let first = fetch(approvals("2026-06-11T10:00:00Z"));
        let again = fetch(approvals("2026-06-12T08:30:00Z"));
        assert_ne!(
            first[0].id, again[0].id,
            "re-approval must mint a new synthesized id"
        );
        // And while standing, the id is stable poll-to-poll.
        let repeat = fetch(approvals("2026-06-11T10:00:00Z"));
        assert_eq!(first[0].id, repeat[0].id, "standing approval id is stable");
    }

    /// Approvals unavailable on the instance (404) = no reviews, not an
    /// error; an identity-less row is LOUD (dedupe would be unsound).
    #[test]
    fn approvals_404_is_empty_and_missing_identity_is_loud() {
        let (f, _) = forge_with(vec![(
            "GET",
            "/merge_requests/7/approvals",
            404,
            r#"{"message": "404 Not Found"}"#.into(),
        )]);
        assert!(f.fetch_reviews(7).unwrap().is_empty());

        let (f, _) = forge_with(vec![(
            "GET",
            "/merge_requests/7/approvals",
            200,
            r#"{"approved_by": [{"user": {"username": "no-id"}}]}"#.into(),
        )]);
        let err = f.fetch_reviews(7).unwrap_err();
        assert!(
            matches!(err, ForgeError::Api { .. }),
            "identity-less approval row must be a loud Api error, got {err:?}"
        );
    }

    /// Pipeline status mapping: success/failed/canceled/in-flight/none.
    #[test]
    fn pipeline_statuses_map_to_ci_states() {
        for (status_json, want) in [
            (r#"[{"id": 1, "status": "success"}]"#, CiState::Success),
            (r#"[{"id": 1, "status": "failed"}]"#, CiState::Failure),
            (r#"[{"id": 1, "status": "canceled"}]"#, CiState::Failure),
            (r#"[{"id": 1, "status": "pending"}]"#, CiState::Pending),
            (r#"[{"id": 1, "status": "manual"}]"#, CiState::Pending),
            ("[]", CiState::None),
        ] {
            let (f, _) = forge_with(vec![("GET", "/pipelines?sha=abc", 200, status_json.into())]);
            assert_eq!(f.fetch_ci("abc").unwrap(), want, "for {status_json}");
        }
        // No head sha at all: no call (a route would panic if hit).
        let (f, _) = forge_with(vec![]);
        assert_eq!(f.fetch_ci("").unwrap(), CiState::None);
    }

    /// Explicit-pagination obligation: a full page (PAGE_LIMIT items) MUST
    /// trigger a page-2 fetch; the short page ends the loop.
    #[test]
    fn snapshot_paginates_until_short_page() {
        let page1: Vec<Value> = (1..=PAGE_LIMIT as u64)
            .map(|n| {
                json!({"iid": n, "state": "opened", "description": "",
                       "labels": ["adr:ADR-0001"]})
            })
            .collect();
        let page2 = json!([{"iid": 101, "state": "opened", "description": "",
                            "labels": ["adr:ADR-0001"]}]);
        let (f, _) = forge_with(vec![
            (
                "GET",
                "/issues?state=all&page=1&per_page=100",
                200,
                serde_json::to_string(&page1).unwrap(),
            ),
            (
                "GET",
                "/issues?state=all&page=2&per_page=100",
                200,
                page2.to_string(),
            ),
            ("GET", "/merge_requests?state=all&page=1", 200, "[]".into()),
        ]);
        let snap = f.fetch_snapshot().unwrap();
        assert_eq!(snap.issues.len(), 101, "page 2 must be fetched and merged");
    }

    /// GitLab HAS a server-side source-branch filter (GitHub-like): one
    /// query, no pagination loop.
    #[test]
    fn find_open_pr_by_head_uses_server_side_source_branch_filter() {
        let (f, seen) = forge_with(vec![(
            "GET",
            "/merge_requests?state=opened&source_branch=conduit/adr-0002/b",
            200,
            r#"[{"iid": 9, "state": "opened",
                 "source_branch": "conduit/adr-0002/b", "sha": "def"}]"#
                .into(),
        )]);
        assert_eq!(
            f.find_open_pr_by_head("conduit/adr-0002/b").unwrap(),
            Some(PrId(9))
        );
        assert_eq!(seen.lock().unwrap().len(), 1, "exactly one request");

        let (f, _) = forge_with(vec![(
            "GET",
            "/merge_requests?state=opened&source_branch=conduit/none/missing",
            200,
            "[]".into(),
        )]);
        assert_eq!(
            f.find_open_pr_by_head("conduit/none/missing").unwrap(),
            None
        );
    }

    #[test]
    fn find_issue_by_marker_hits_description_without_fetching_notes() {
        let marker = "<!-- conduit:task:adr-0007 -->";
        let (f, _) = forge_with(vec![(
            "GET",
            "/issues?state=all&page=1",
            200,
            r#"[{"iid": 3, "state": "opened",
                 "description": "intro\n\n<!-- conduit:task:adr-0007 -->",
                 "labels": []}]"#
                .into(),
        )]);
        // No note routes exist — a notes fetch would panic.
        assert_eq!(f.find_issue_by_marker(marker).unwrap(), Some(IssueId(3)));
    }

    #[test]
    fn find_issue_by_marker_falls_back_to_notes() {
        let marker = "<!-- conduit:task:adr-0007 -->";
        let (f, _) = forge_with(vec![
            (
                "GET",
                "/issues?state=all&page=1",
                200,
                r#"[{"iid": 1, "state": "opened", "description": "no marker", "labels": []},
                    {"iid": 2, "state": "opened", "description": "none here", "labels": []}]"#
                    .into(),
            ),
            ("GET", "/issues/1/notes?page=1", 200, "[]".into()),
            (
                "GET",
                "/issues/2/notes?page=1",
                200,
                r#"[{"id": 9, "body": "<!-- conduit:task:adr-0007 -->\n\nstatus"}]"#.into(),
            ),
        ]);
        assert_eq!(f.find_issue_by_marker(marker).unwrap(), Some(IssueId(2)));
    }

    /// The label create payload carries the '#'-prefixed color (GitLab
    /// difference: LabelSpec holds bare hex) and only missing labels post.
    #[test]
    fn ensure_labels_creates_only_missing_labels_with_hash_color() {
        let (f, seen) = forge_with(vec![
            (
                "GET",
                "/labels?page=1",
                200,
                r##"[{"id": 1, "name": "conduit:run", "color": "#1d76db"}]"##.into(),
            ),
            // Exactly ONE create — posting conduit:run too would panic.
            (
                "POST",
                "/labels",
                201,
                r##"{"id": 2, "name": "conduit:failed", "color": "#d73a4a"}"##.into(),
            ),
        ]);
        f.ensure_labels(&[
            LabelSpec {
                name: "conduit:run".into(),
                color: "1d76db".into(),
                description: "trigger".into(),
            },
            LabelSpec {
                name: "conduit:failed".into(),
                color: "d73a4a".into(),
                description: "failed".into(),
            },
        ])
        .unwrap();
        let body = last_body(&seen);
        assert_eq!(body["name"], "conduit:failed");
        assert_eq!(body["color"], "#d73a4a", "GitLab wants the leading '#'");
        assert_eq!(body["description"], "failed");
    }

    /// GitLab answers 409 when the label already exists — converged, not an
    /// error.
    #[test]
    fn ensure_labels_treats_409_conflict_as_converged() {
        let (f, _) = forge_with(vec![
            ("GET", "/labels?page=1", 200, "[]".into()),
            (
                "POST",
                "/labels",
                409,
                r#"{"message": "Label already exists"}"#.into(),
            ),
        ]);
        f.ensure_labels(&[LabelSpec {
            name: "conduit:run".into(),
            color: "1d76db".into(),
            description: "trigger".into(),
        }])
        .unwrap();
    }

    /// GitLab difference from both housemates: labels ride create/update as
    /// ONE comma-separated name string, and the body field is `description`.
    #[test]
    fn create_issue_posts_description_and_comma_separated_labels() {
        let (f, seen) = forge_with(vec![("POST", "/issues", 201, r#"{"iid": 5}"#.into())]);
        let id = f
            .create_issue(&NewIssue {
                title: "t".into(),
                body: "b".into(),
                labels: vec!["adr:ADR-0003".into(), "conduit:run".into()],
            })
            .unwrap();
        assert_eq!(id, IssueId(5));
        let body = last_body(&seen);
        assert_eq!(body["title"], "t");
        assert_eq!(body["description"], "b", "GitLab bodies are `description`");
        assert_eq!(body["labels"], "adr:ADR-0003,conduit:run");
    }

    #[test]
    fn set_issue_labels_puts_replacement_comma_set() {
        let (f, seen) = forge_with(vec![("PUT", "/issues/5", 200, r#"{"iid": 5}"#.into())]);
        f.set_issue_labels(&IssueId(5), &["adr:ADR-0003".into(), "conduit:run".into()])
            .unwrap();
        assert_eq!(last_body(&seen)["labels"], "adr:ADR-0003,conduit:run");
    }

    /// ADR-0007 convergence probes: label reads return current names —
    /// issues from the issue read, MRs from the MERGE REQUEST read (the
    /// endpoints do not cross over on GitLab).
    #[test]
    fn label_reads_return_current_names_for_issue_and_mr() {
        let (f, _) = forge_with(vec![
            (
                "GET",
                "/issues/5",
                200,
                r#"{"iid": 5, "labels": ["adr:ADR-0003", "discuss"]}"#.into(),
            ),
            (
                "GET",
                "/merge_requests/9",
                200,
                r#"{"iid": 9, "labels": ["effort:1-super-quick"]}"#.into(),
            ),
        ]);
        assert_eq!(
            f.get_issue_labels(&IssueId(5)).unwrap(),
            vec!["adr:ADR-0003".to_string(), "discuss".to_string()]
        );
        assert_eq!(
            f.get_pr_labels(&PrId(9)).unwrap(),
            vec!["effort:1-super-quick".to_string()]
        );
    }

    /// Closing is a state EVENT on GitLab, not a state field.
    #[test]
    fn close_issue_puts_state_event_close() {
        let (f, seen) = forge_with(vec![(
            "PUT",
            "/issues/5",
            200,
            r#"{"iid": 5, "state": "closed"}"#.into(),
        )]);
        f.close_issue(&IssueId(5)).unwrap();
        let body = last_body(&seen);
        assert_eq!(body["state_event"], "close");
        assert!(body.get("state").is_none(), "no raw state field");
    }

    #[test]
    fn close_unknown_issue_is_api_404() {
        let (f, _) = forge_with(vec![(
            "PUT",
            "/issues/999",
            404,
            r#"{"message": "404 Issue Not Found"}"#.into(),
        )]);
        let err = f.close_issue(&IssueId(999)).unwrap_err();
        let ForgeError::Api { status, .. } = err else {
            panic!("expected Api error, got {err:?}");
        };
        assert_eq!(status, 404);
    }

    /// One POST carries everything: source/target branch naming and the
    /// comma-separated labels (no second label call, unlike Gitea/GitHub).
    #[test]
    fn open_pr_posts_source_target_and_labels_in_one_call() {
        let (f, seen) = forge_with(vec![(
            "POST",
            "/merge_requests",
            201,
            r#"{"iid": 9}"#.into(),
        )]);
        let id = f
            .open_pr(&PrDraft {
                title: "t".into(),
                body: "b".into(),
                head: "conduit/adr-0001/x".into(),
                base: "main".into(),
                labels: vec!["effort:1-super-quick".into(), "adr:ADR-0001".into()],
            })
            .unwrap();
        assert_eq!(id, PrId(9));
        let recorded = seen.lock().unwrap();
        assert_eq!(recorded.len(), 1, "exactly one call");
        let post: Value = serde_json::from_str(recorded[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(post["source_branch"], "conduit/adr-0001/x");
        assert_eq!(post["target_branch"], "main");
        assert_eq!(post["description"], "b");
        assert_eq!(post["labels"], "effort:1-super-quick,adr:ADR-0001");
    }

    #[test]
    fn open_pr_without_labels_omits_the_labels_key() {
        let (f, seen) = forge_with(vec![(
            "POST",
            "/merge_requests",
            201,
            r#"{"iid": 9}"#.into(),
        )]);
        f.open_pr(&PrDraft {
            title: "t".into(),
            body: "b".into(),
            head: "conduit/adr-0001/x".into(),
            base: "main".into(),
            labels: vec![],
        })
        .unwrap();
        assert!(last_body(&seen).get("labels").is_none());
    }

    #[test]
    fn comment_upsert_edits_existing_marker_note_in_place() {
        let marker = "<!-- conduit:task:adr-0003 -->";
        let (f, seen) = forge_with(vec![
            (
                "GET",
                "/issues/5/notes?page=1",
                200,
                r#"[{"id": 42, "body": "<!-- conduit:task:adr-0003 -->\n\nold"}]"#.into(),
            ),
            ("PUT", "/issues/5/notes/42", 200, r#"{"id": 42}"#.into()),
        ]);
        f.upsert_issue_comment(&IssueId(5), marker, "new").unwrap();
        // FixtureTransport panics on a POST — reaching here proves PUT path.
        let body = last_body(&seen);
        let text = body["body"].as_str().unwrap();
        assert!(text.starts_with(marker), "marker embedded in note body");
        assert!(text.ends_with("new"));
    }

    #[test]
    fn comment_upsert_creates_when_marker_absent() {
        let marker = "<!-- conduit:task:adr-0003 -->";
        let (f, _) = forge_with(vec![
            ("GET", "/issues/5/notes?page=1", 200, "[]".into()),
            ("POST", "/issues/5/notes", 201, r#"{"id": 43}"#.into()),
        ]);
        f.upsert_issue_comment(&IssueId(5), marker, "first")
            .unwrap();
    }

    /// MR notes live under merge_requests/{iid}/notes — NOT the issue
    /// endpoints (separate iid sequences; the GitLab quirk).
    #[test]
    fn pr_comment_upsert_uses_merge_request_note_endpoints() {
        let marker = "<!-- conduit:pr:adr-0003 -->";
        let (f, _) = forge_with(vec![
            ("GET", "/merge_requests/9/notes?page=1", 200, "[]".into()),
            (
                "POST",
                "/merge_requests/9/notes",
                201,
                r#"{"id": 44}"#.into(),
            ),
        ]);
        f.upsert_pr_comment(&PrId(9), marker, "pr status").unwrap();
    }

    #[test]
    fn set_pr_labels_puts_to_the_merge_request_not_the_issue() {
        let (f, seen) = forge_with(vec![(
            "PUT",
            "/merge_requests/9",
            200,
            r#"{"iid": 9}"#.into(),
        )]);
        f.set_pr_labels(
            &PrId(9),
            &["effort:2-not-long".into(), "adr:ADR-0001".into()],
        )
        .unwrap();
        let recorded = seen.lock().unwrap();
        assert!(
            recorded[0].url.contains("/merge_requests/9"),
            "PR labels must ride the MR endpoint: {}",
            recorded[0].url
        );
        let body: Value = serde_json::from_str(recorded[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["labels"], "effort:2-not-long,adr:ADR-0001");
    }

    /// No API call — and src/git.rs refuses to push to any non-localhost URL
    /// (Task 11 guard), so this URL is never pushed to.
    #[test]
    fn git_remote_url_is_plain_https_clone_url() {
        let (f, seen) = forge_with(vec![]);
        assert_eq!(
            f.git_remote_url().unwrap(),
            "https://gitlab.example.test/octo/example.git"
        );
        assert!(seen.lock().unwrap().is_empty(), "no API call");
        assert_eq!(
            f.describe(),
            "gitlab octo/example at https://gitlab.example.test"
        );
    }

    #[test]
    fn auth_errors_map_to_forge_auth() {
        let (f, _) = forge_with(vec![(
            "GET",
            "/labels?page=1",
            401,
            r#"{"message": "401 Unauthorized"}"#.into(),
        )]);
        let err = f
            .ensure_labels(&[LabelSpec {
                name: "conduit:run".into(),
                color: "1d76db".into(),
                description: "trigger".into(),
            }])
            .unwrap_err();
        assert!(
            matches!(err, ForgeError::Auth(_)),
            "401 must map to Auth, got {err:?}"
        );
    }

    /// Requests carry PRIVATE-TOKEN auth against the URL-encoded project
    /// path (the documented addressing form).
    #[test]
    fn requests_carry_private_token_against_the_encoded_project_path() {
        let (f, seen) = forge_with(vec![("PUT", "/issues/5", 200, r#"{"iid": 5}"#.into())]);
        f.close_issue(&IssueId(5)).unwrap();
        let recorded = seen.lock().unwrap();
        assert_eq!(recorded[0].method, "PUT");
        assert_eq!(
            recorded[0].url, "https://gitlab.example.test/api/v4/projects/octo%2Fexample/issues/5",
            "project path URL-encoded into one segment"
        );
        let auth = recorded[0]
            .headers
            .iter()
            .find(|(k, _)| k == "PRIVATE-TOKEN")
            .map(|(_, v)| v.as_str());
        assert_eq!(auth, Some("test-token"));
    }

    /// The hard constraint, structurally: the public constructor only hands
    /// out a DryRun wrapper — a mutation is recorded, never sent (the
    /// UreqTransport inside would hit the real network if it were).
    #[test]
    fn open_gitlab_only_hands_out_dry_run() {
        let cfg = crate::config::GitlabConfig {
            base_url: "https://gitlab.example.test".into(),
            owner: "octo".into(),
            repo: "example".into(),
        };
        let forge: DryRunForge<GitLabForge> = open_gitlab(&cfg, "tok".into());
        let id = forge
            .create_issue(&NewIssue {
                title: "t".into(),
                body: "see octo/example".into(),
                labels: vec![],
            })
            .unwrap();
        let _ = id; // synthetic — nothing was sent anywhere
        let t = forge.transcript();
        assert_eq!(t.len(), 1);
        assert!(t[0].contains("\"action\":\"create_issue\""));
        assert!(
            t[0].contains("$REPO") && !t[0].contains("octo/example"),
            "open_gitlab wires the repo-slug redaction: {}",
            t[0]
        );
    }

    #[test]
    fn resolve_token_reads_env_only() {
        // Hermetic: only assert when the runner has no GITLAB_TOKEN set
        // (env mutation in parallel tests is racy — house rule).
        if std::env::var("GITLAB_TOKEN").is_err() {
            assert_eq!(resolve_token(), None);
        }
    }
}
