//! GitHub REST v3 adapter (spec §Implementations: GitHub is reads-live only —
//! the spike NEVER mutates github.com). The only public constructors,
//! [`open_github`] and [`fixture_forge`], hand out a
//! [`DryRunForge`]`<GitHubForge>`: reads delegate to this adapter, every
//! mutation is recorded to the transcript and never sent. The mutation
//! methods below exist so the payload builders are unit-testable (and so
//! DryRun *could* delegate one day), but nothing outside this module can
//! reach an unwrapped `GitHubForge`.
//!
//! Adapter obligations from the `forge` module header, honored here:
//! - Disappearance rule: snapshots fetch `state=all` for BOTH issues and
//!   pulls — merged/closed items stay visible until their terminal events
//!   have been observed.
//! - Explicit pagination: every list call loops `?page=N&per_page=100` and
//!   stops on a short page (`HttpResponse` carries no Link header by design).
//! - Review stability: rows whose `state` is not a submitted verdict are
//!   skipped — `PENDING` (draft) and `DISMISSED`. GitHub overwrites a
//!   dismissed review's state in place (the original verdict is lost) and a
//!   dismissal can never revert, so a skipped `DISMISSED` id can never
//!   reappear and re-fire `ReviewSubmitted` — the hazard the never-filter
//!   obligation guards against. A resubmission gets a new id and correctly
//!   fires.
//! - Id-uniqueness: issue/PR numbers are unique per repo on the forge side.
//!
//! GitHub quirks vs Gitea: the issues listing includes PRs (skip rows with a
//! `pull_request` key); label writes take NAMES, not ids; `merged` is
//! `merged_at != null` (`merge_commit_sha` is populated with a test-merge sha
//! even for unmerged PRs — read it only when merged).

use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::dry_run::DryRunForge;
use super::{
    CiState, Forge, ForgeError, HttpResponse, HttpTransport, IssueSnapshot, LabelSpec, NewIssue,
    PrDraft, PrSnapshot, RepoSnapshot, Review, UreqTransport, rest_call,
};
use crate::task::{IssueId, PrId, ReviewId, ReviewVerdict};

const API_BASE: &str = "https://api.github.com";

/// Page size for every list call; a page shorter than this ends the loop.
const PAGE_LIMIT: usize = 100;

/// The repo the fixtures under tests/fixtures/github were recorded from
/// (public; reads only). The live-reads conformance leg targets it too.
pub const FIXTURE_OWNER: &str = "bfowle";
pub const FIXTURE_REPO: &str = "mdbook-gruvbox";

pub struct GitHubForge {
    transport: Box<dyn HttpTransport>,
    owner: String,
    repo: String,
    token: String, // env GITHUB_TOKEN / `gh auth token` — reads only
}

/// The ONLY public way to construct a GitHub forge in the spike:
/// always DryRun-wrapped (spec hard constraint — no mutation of github.com,
/// ever).
pub fn open_github(cfg: &crate::config::GithubConfig, token: String) -> DryRunForge<GitHubForge> {
    let slug = format!("{}/{}", cfg.owner, cfg.repo);
    DryRunForge::with_repo_slug(
        GitHubForge::new(Box::new(UreqTransport), &cfg.owner, &cfg.repo, &token),
        &slug,
    )
}

/// DryRun-wrapped GitHub forge whose reads are served from recorded fixture
/// files in `dir` (no network; mutations would panic in the transport, but
/// the DryRun wrapper never lets one through). Test support for the
/// always-on conformance leg.
pub fn fixture_forge(dir: &str) -> DryRunForge<GitHubForge> {
    let slug = format!("{FIXTURE_OWNER}/{FIXTURE_REPO}");
    DryRunForge::with_repo_slug(
        GitHubForge::new(
            Box::new(DirFixtureTransport { dir: dir.into() }),
            FIXTURE_OWNER,
            FIXTURE_REPO,
            "fixture-token",
        ),
        &slug,
    )
}

/// Token resolution: env GITHUB_TOKEN, else `gh auth token` subprocess output
/// (trimmed), else None. The token is never printed or logged.
pub fn resolve_token() -> Option<String> {
    if let Ok(token) = std::env::var("GITHUB_TOKEN")
        && !token.is_empty()
    {
        return Some(token);
    }
    let out = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let token = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if token.is_empty() { None } else { Some(token) }
}

impl GitHubForge {
    /// Private on purpose: `open_github`/`fixture_forge` are the only ways
    /// out of this module, and both wrap in DryRun.
    fn new(transport: Box<dyn HttpTransport>, owner: &str, repo: &str, token: &str) -> GitHubForge {
        GitHubForge {
            transport,
            owner: owner.to_string(),
            repo: repo.to_string(),
            token: token.to_string(),
        }
    }

    #[cfg(test)]
    pub(crate) fn raw_for_tests(
        transport: Box<dyn HttpTransport>,
        owner: &str,
        repo: &str,
    ) -> GitHubForge {
        GitHubForge::new(transport, owner, repo, "test-token")
    }

    // -- wire plumbing --------------------------------------------------

    /// One repo-scoped REST call: `path` is relative to
    /// `https://api.github.com/repos/{owner}/{repo}/`.
    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, ForgeError> {
        let url = format!("{API_BASE}/repos/{}/{}/{path}", self.owner, self.repo);
        let auth = format!("Bearer {}", self.token);
        rest_call(
            self.transport.as_ref(),
            method,
            &url,
            &[
                ("Authorization", &auth),
                ("Accept", "application/vnd.github+json"),
                ("User-Agent", "conduit-spike"),
                ("Content-Type", "application/json"),
            ],
            body,
            "github",
        )
    }

    /// Paginated GET (module-header obligation: EXPLICIT `?page=N&per_page=100`
    /// loop, stop on a short page — `HttpResponse` carries no Link header, and
    /// a page-1-only fetch silently truncates at the server default and
    /// breaks the disappearance rule).
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
                    message: format!("github: expected a JSON array from {path}"),
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

    /// Raw issue listing with PR rows removed (GitHub lists PRs as issues —
    /// any row carrying a `pull_request` key is one). The skip happens AFTER
    /// pagination so a PR-heavy page never reads as a short page. state=all
    /// per the disappearance rule. Shared by fetch_snapshot and the marker
    /// probe — the probe must see ALL issues, not just conduit-labeled ones.
    fn list_all_issues(&self) -> Result<Vec<Value>, ForgeError> {
        Ok(self
            .get_paginated("issues?state=all")?
            .into_iter()
            .filter(|raw| raw.get("pull_request").is_none())
            .collect())
    }

    // -- comments ----------------------------------------------------------

    /// Find the id of the comment on issue/PR `number` whose body carries
    /// `marker` (the upsert identity).
    fn find_marker_comment(&self, number: u64, marker: &str) -> Result<Option<u64>, ForgeError> {
        for c in self.get_paginated(&format!("issues/{number}/comments"))? {
            if c.get("body")
                .and_then(|b| b.as_str())
                .is_some_and(|b| b.contains(marker))
            {
                return Ok(Some(field_u64(&c, "id")?));
            }
        }
        Ok(None)
    }

    /// Marker-comment upsert: edit the existing marker comment in place, or
    /// create one with the marker embedded. PR comments use the same
    /// issue-comment endpoints (PR number works).
    fn upsert_comment(&self, number: u64, marker: &str, body: &str) -> Result<(), ForgeError> {
        let text = format!("{marker}\n\n{body}");
        match self.find_marker_comment(number, marker)? {
            Some(id) => {
                self.call(
                    "PATCH",
                    &format!("issues/comments/{id}"),
                    Some(json!({"body": text})),
                )?;
            }
            None => {
                self.call(
                    "POST",
                    &format!("issues/{number}/comments"),
                    Some(json!({"body": text})),
                )?;
            }
        }
        Ok(())
    }

    // -- snapshot pieces ----------------------------------------------------

    /// All submitted reviews of one PR. `PENDING` (draft) and `DISMISSED`
    /// rows are skipped — see the module header for why skipping `DISMISSED`
    /// cannot re-fire `ReviewSubmitted` on GitHub.
    fn fetch_reviews(&self, number: u64) -> Result<Vec<Review>, ForgeError> {
        let mut reviews = Vec::new();
        for raw in self.get_paginated(&format!("pulls/{number}/reviews"))? {
            let verdict = match raw.get("state").and_then(|s| s.as_str()) {
                Some("APPROVED") => ReviewVerdict::Approved,
                Some("CHANGES_REQUESTED") => ReviewVerdict::ChangesRequested,
                Some("COMMENTED") => ReviewVerdict::Commented,
                _ => continue, // PENDING / DISMISSED: not submitted verdicts
            };
            reviews.push(Review {
                id: ReviewId(field_u64(&raw, "id")?.to_string()),
                author: raw
                    .pointer("/user/login")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                verdict,
                body: raw
                    .get("body")
                    .and_then(|b| b.as_str())
                    .unwrap_or_default()
                    .to_string(),
                submitted_at: raw
                    .get("submitted_at")
                    .and_then(|s| s.as_str())
                    .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
                    .unwrap_or(OffsetDateTime::UNIX_EPOCH),
            });
        }
        Ok(reviews)
    }

    /// Combined commit status -> CiState. `total_count` 0 means no statuses
    /// were ever reported (GitHub still says state "pending" then) — that is
    /// CiState::None, as is a 404 for an unknown sha.
    fn fetch_ci(&self, head_sha: &str) -> Result<CiState, ForgeError> {
        if head_sha.is_empty() {
            return Ok(CiState::None);
        }
        match self.call("GET", &format!("commits/{head_sha}/status"), None) {
            Ok(v) => {
                if v.get("total_count").and_then(|n| n.as_u64()) == Some(0) {
                    return Ok(CiState::None);
                }
                Ok(match v.get("state").and_then(|s| s.as_str()) {
                    Some("pending") => CiState::Pending,
                    Some("success") => CiState::Success,
                    Some("failure") | Some("error") => CiState::Failure,
                    _ => CiState::None,
                })
            }
            Err(ForgeError::Api { status: 404, .. }) => Ok(CiState::None),
            Err(e) => Err(e),
        }
    }
}

impl Forge for GitHubForge {
    fn describe(&self) -> String {
        format!("github {}/{}", self.owner, self.repo)
    }

    /// No API call. The spike never pushes here: src/git.rs refuses any
    /// non-localhost push URL (Task 11 guard).
    fn git_remote_url(&self) -> Result<String, ForgeError> {
        Ok(format!(
            "https://github.com/{}/{}.git",
            self.owner, self.repo
        ))
    }

    fn fetch_snapshot(&self) -> Result<RepoSnapshot, ForgeError> {
        // Normalization filter (RepoSnapshot doc): conduit-labeled issues +
        // conduit/*-branch PRs only. state=all on BOTH lists — terminal items
        // must stay until their events are observed (disappearance rule).
        let mut issues = Vec::new();
        for raw in self.list_all_issues()? {
            let labels = label_names(&raw);
            if !labels
                .iter()
                .any(|l| l.starts_with("conduit:") || l.starts_with("adr:"))
            {
                continue;
            }
            issues.push(IssueSnapshot {
                id: IssueId(field_u64(&raw, "number")?),
                labels,
                closed: raw.get("state").and_then(|s| s.as_str()) == Some("closed"),
            });
        }

        let mut prs = Vec::new();
        for raw in self.get_paginated("pulls?state=all")? {
            let head_branch = raw
                .pointer("/head/ref")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if !head_branch.starts_with("conduit/") {
                continue;
            }
            let number = field_u64(&raw, "number")?;
            let head_sha = raw
                .pointer("/head/sha")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            // GitHub: merged == merged_at is non-null. merge_commit_sha is
            // populated with a TEST-merge sha even for unmerged PRs — read it
            // only when merged.
            let merged = raw.get("merged_at").is_some_and(|v| !v.is_null());
            let merge_sha = raw
                .get("merge_commit_sha")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            prs.push(PrSnapshot {
                id: PrId(number),
                title: field_str(&raw, "title"),
                body: field_str(&raw, "body"),
                head_branch,
                labels: label_names(&raw),
                reviews: self.fetch_reviews(number)?,
                ci: self.fetch_ci(&head_sha)?,
                merged,
                merge_sha: if merged { merge_sha } else { None },
                closed: raw.get("state").and_then(|s| s.as_str()) == Some("closed"),
            });
        }

        Ok(RepoSnapshot {
            issues,
            prs,
            fetched_at: OffsetDateTime::now_utc(),
        })
    }

    /// GitHub has a server-side head filter (Gitea difference): one query,
    /// `head={owner}:{branch}`, at most one open PR can match.
    fn find_open_pr_by_head(&self, branch: &str) -> Result<Option<PrId>, ForgeError> {
        let v = self.call(
            "GET",
            &format!("pulls?state=open&head={}:{branch}", self.owner),
            None,
        )?;
        match v.as_array().and_then(|a| a.first()) {
            Some(raw) => Ok(Some(PrId(field_u64(raw, "number")?))),
            None => Ok(None),
        }
    }

    /// Identical semantics to Gitea — deliberately NOT the search API (its
    /// indexing lag would break the crash-replay probe). The comment-fallback
    /// leg is O(issues) x O(comments/issue); see the Gitea twin for the cost
    /// note.
    fn find_issue_by_marker(&self, marker: &str) -> Result<Option<IssueId>, ForgeError> {
        let all = self.list_all_issues()?;
        // Body scan first — create_issue embeds the marker there.
        for raw in &all {
            if raw
                .get("body")
                .and_then(|b| b.as_str())
                .is_some_and(|b| b.contains(marker))
            {
                return Ok(Some(IssueId(field_u64(raw, "number")?)));
            }
        }
        // The marker may instead live in an upserted status comment.
        for raw in &all {
            let number = field_u64(raw, "number")?;
            if self.find_marker_comment(number, marker)?.is_some() {
                return Ok(Some(IssueId(number)));
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
                    "color": spec.color,
                    "description": spec.description,
                })),
            ) {
                Ok(_) => {}
                // Already exists (create race) — GitHub answers 422
                // already_exists: converged, not an error.
                Err(ForgeError::Api { status: 422, .. }) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// GitHub difference from Gitea: labels are NAMES in the payload — no
    /// id-resolution round-trip exists or is needed.
    fn create_issue(&self, new: &NewIssue) -> Result<IssueId, ForgeError> {
        let v = self.call(
            "POST",
            "issues",
            Some(json!({"title": new.title, "body": new.body, "labels": new.labels})),
        )?;
        Ok(IssueId(field_u64(&v, "number")?))
    }

    fn upsert_issue_comment(
        &self,
        id: &IssueId,
        marker: &str,
        body: &str,
    ) -> Result<(), ForgeError> {
        self.upsert_comment(id.0, marker, body)
    }

    /// PUT replaces the whole label set (absolute, convergent).
    fn set_issue_labels(&self, id: &IssueId, labels: &[String]) -> Result<(), ForgeError> {
        self.call(
            "PUT",
            &format!("issues/{}/labels", id.0),
            Some(json!({"labels": labels})),
        )?;
        Ok(())
    }

    fn close_issue(&self, id: &IssueId) -> Result<(), ForgeError> {
        self.call(
            "PATCH",
            &format!("issues/{}", id.0),
            Some(json!({"state": "closed"})),
        )?;
        Ok(())
    }

    fn open_pr(&self, draft: &PrDraft) -> Result<PrId, ForgeError> {
        let v = self.call(
            "POST",
            "pulls",
            Some(json!({
                "title": draft.title,
                "body": draft.body,
                "head": draft.head,
                "base": draft.base,
            })),
        )?;
        let id = PrId(field_u64(&v, "number")?);
        if !draft.labels.is_empty() {
            // PR labels ride the issues endpoint (PR number works).
            self.set_issue_labels(&IssueId(id.0), &draft.labels.to_vec())?;
        }
        Ok(id)
    }

    fn upsert_pr_comment(&self, id: &PrId, marker: &str, body: &str) -> Result<(), ForgeError> {
        self.upsert_comment(id.0, marker, body)
    }

    fn set_pr_labels(&self, id: &PrId, labels: &[String]) -> Result<(), ForgeError> {
        self.call(
            "PUT",
            &format!("issues/{}/labels", id.0),
            Some(json!({"labels": labels})),
        )?;
        Ok(())
    }
}

/// Optional string field, defaulting to "" (a PR with a null body is legal).
fn field_str(v: &Value, name: &str) -> String {
    v.get(name)
        .and_then(|s| s.as_str())
        .unwrap_or_default()
        .to_string()
}

/// The `name` of every entry in a response's `labels` array.
fn label_names(v: &Value) -> Vec<String> {
    v.get("labels")
        .and_then(|l| l.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l.get("name").and_then(|n| n.as_str()).map(str::to_string))
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
            message: format!("github: response missing numeric `{name}`"),
        })
}

// ---------------------------------------------------------------------------
// Recorded-fixture transport — serves tests/fixtures/github/* by URL pattern
// (test support for the always-on conformance leg; no network, ever).
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
            "DryRun must never let a mutation reach GitHub: {method} {url}"
        );
        let (path, query) = url.split_once('?').unwrap_or((url, ""));
        let segments: Vec<&str> = path
            .trim_start_matches(&format!("{API_BASE}/repos/"))
            .split('/')
            .collect();
        // segments: [owner, repo, rest...]
        let resp = match segments.get(2..).unwrap_or(&[]) {
            ["issues"] if page_of(url) == 1 => {
                Some(self.file("issues.json").unwrap_or_else(|| {
                    panic!("missing fixture issues.json — run the record_fixtures recorder")
                }))
            }
            ["issues"] => Some(Self::empty_array()),
            ["pulls"] if query.contains("head=") => Some(Self::empty_array()),
            ["pulls"] if page_of(url) == 1 => Some(self.file("pulls.json").unwrap_or_else(|| {
                panic!("missing fixture pulls.json — run the record_fixtures recorder")
            })),
            ["pulls"] => Some(Self::empty_array()),
            ["pulls", n, "reviews"] => Some(
                self.file(&format!("reviews_{n}.json"))
                    .unwrap_or_else(Self::empty_array),
            ),
            ["commits", sha, "status"] => Some(self.file(&format!("status_{sha}.json")).unwrap_or(
                HttpResponse {
                    status: 404,
                    body: br#"{"message": "no recorded status"}"#.to_vec(),
                },
            )),
            ["issues", n, "comments"] => Some(
                self.file(&format!("comments_{n}.json"))
                    .unwrap_or_else(Self::empty_array),
            ),
            ["labels"] => Some(self.file("labels.json").unwrap_or_else(Self::empty_array)),
            _ => None,
        };
        Ok(resp.unwrap_or_else(|| panic!("no fixture route for GET {url}")))
    }
}

// ---------------------------------------------------------------------------
// Fixture-based unit tests — every test names its exact wire traffic; an
// unexpected request panics. Fixture bodies mirror documented GitHub REST v3
// shapes (live-verified by the recorded fixtures + CONDUIT_E2E_GITHUB leg).
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

    fn forge_with(routes: Vec<(&str, &str, u16, String)>) -> (GitHubForge, Seen) {
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
            GitHubForge::raw_for_tests(Box::new(transport), "octo", "example"),
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
    fn snapshot_filters_to_conduit_items_and_skips_pull_request_rows() {
        let (f, _) = forge_with(vec![
            (
                "GET",
                "/issues?state=all&page=1",
                200,
                // Row 3 carries a pull_request key — GitHub lists PRs as
                // issues; it must be skipped even though it is adr-labeled.
                r#"[
                    {"number": 1, "state": "open", "body": "x",
                     "labels": [{"name": "adr:ADR-0003"}]},
                    {"number": 2, "state": "open", "body": "y",
                     "labels": [{"name": "bug"}]},
                    {"number": 3, "state": "open", "body": "z",
                     "labels": [{"name": "adr:ADR-0003"}],
                     "pull_request": {"url": "https://api.github.com/repos/octo/example/pulls/3"}}
                ]"#
                .into(),
            ),
            (
                "GET",
                "/pulls?state=all&page=1",
                200,
                // PR 7: open, unmerged — merge_commit_sha holds GitHub's
                // test-merge sha and MUST be ignored while merged_at is null.
                r#"[
                    {"number": 7, "state": "open", "merged_at": null,
                     "merge_commit_sha": "feedface",
                     "head": {"ref": "conduit/adr-0003/x", "sha": "abc"},
                     "labels": [{"name": "adr:ADR-0003"}]},
                    {"number": 8, "state": "open", "merged_at": null,
                     "merge_commit_sha": null,
                     "head": {"ref": "feature/other", "sha": "def"},
                     "labels": []}
                ]"#
                .into(),
            ),
            (
                "GET",
                "/pulls/7/reviews?page=1",
                200,
                // PENDING (draft) and DISMISSED are skipped; the rest map.
                r#"[
                    {"id": 100, "user": {"login": "reviewer"}, "state": "APPROVED",
                     "body": "lgtm", "submitted_at": "2026-06-11T10:00:00Z"},
                    {"id": 101, "user": {"login": "reviewer"}, "state": "PENDING",
                     "body": "draft", "submitted_at": null},
                    {"id": 102, "user": {"login": "reviewer"}, "state": "DISMISSED",
                     "body": "stale", "submitted_at": "2026-06-10T09:00:00Z"},
                    {"id": 103, "user": {"login": "reviewer"}, "state": "CHANGES_REQUESTED",
                     "body": "fix x", "submitted_at": "2026-06-11T11:00:00Z"}
                ]"#
                .into(),
            ),
            (
                "GET",
                "/commits/abc/status",
                200,
                r#"{"state": "pending", "total_count": 2}"#.into(),
            ),
        ]);
        let snap = f.fetch_snapshot().unwrap();
        assert_eq!(snap.issues.len(), 1, "non-conduit issue + PR row filtered");
        assert_eq!(snap.issues[0].id, IssueId(1));
        assert_eq!(snap.prs.len(), 1, "non-conduit/* PR filtered");
        let pr = &snap.prs[0];
        assert!(!pr.merged, "merged_at null means unmerged");
        assert_eq!(
            pr.merge_sha, None,
            "test-merge sha must be ignored while unmerged"
        );
        assert_eq!(pr.reviews.len(), 2, "PENDING and DISMISSED skipped");
        assert_eq!(pr.reviews[0].id, ReviewId("100".into()));
        assert_eq!(pr.reviews[0].verdict, ReviewVerdict::Approved);
        assert_eq!(pr.reviews[0].author, "reviewer");
        assert_eq!(pr.reviews[0].submitted_at, datetime!(2026-06-11 10:00 UTC));
        assert_eq!(pr.reviews[1].id, ReviewId("103".into()));
        assert_eq!(pr.reviews[1].verdict, ReviewVerdict::ChangesRequested);
        assert_eq!(pr.ci, CiState::Pending);
    }

    /// Disappearance rule (module-header obligation): state=all keeps a
    /// merged+closed PR and a closed issue in the snapshot, with merge_sha.
    #[test]
    fn snapshot_keeps_terminal_prs_and_closed_issues() {
        let (f, _) = forge_with(vec![
            (
                "GET",
                "/issues?state=all&page=1",
                200,
                r#"[{"number": 4, "state": "closed", "body": "",
                     "labels": [{"name": "conduit:run"}]}]"#
                    .into(),
            ),
            (
                "GET",
                "/pulls?state=all&page=1",
                200,
                r#"[
                    {"number": 7, "state": "closed",
                     "merged_at": "2026-06-11T12:00:00Z",
                     "merge_commit_sha": "cafe42",
                     "head": {"ref": "conduit/adr-0001/x", "sha": "abc"},
                     "labels": []},
                    {"number": 9, "state": "closed", "merged_at": null,
                     "merge_commit_sha": "feedface",
                     "head": {"ref": "conduit/adr-0002/y", "sha": "def"},
                     "labels": []}
                ]"#
                .into(),
            ),
            ("GET", "/pulls/7/reviews?page=1", 200, "[]".into()),
            (
                "GET",
                "/commits/abc/status",
                200,
                r#"{"state": "success", "total_count": 1}"#.into(),
            ),
            ("GET", "/pulls/9/reviews?page=1", 200, "[]".into()),
            (
                "GET",
                "/commits/def/status",
                404,
                r#"{"message": "No commit found"}"#.into(),
            ),
        ]);
        let snap = f.fetch_snapshot().unwrap();
        assert_eq!(snap.issues.len(), 1, "closed issue must stay");
        assert!(snap.issues[0].closed);
        assert_eq!(snap.prs.len(), 2, "terminal PRs must stay");
        let merged = &snap.prs[0];
        assert!(merged.merged && merged.closed);
        assert_eq!(merged.merge_sha.as_deref(), Some("cafe42"));
        assert_eq!(merged.ci, CiState::Success);
        let closed = &snap.prs[1];
        assert!(!closed.merged, "closed-without-merge stays unmerged");
        assert!(closed.closed);
        assert_eq!(closed.merge_sha, None);
        assert_eq!(closed.ci, CiState::None, "404 combined status -> None");
    }

    /// Explicit-pagination obligation: a full page (PAGE_LIMIT items) MUST
    /// trigger a page-2 fetch; the short page ends the loop.
    #[test]
    fn snapshot_paginates_until_short_page() {
        let page1: Vec<Value> = (1..=PAGE_LIMIT as u64)
            .map(|n| {
                json!({"number": n, "state": "open", "body": "",
                       "labels": [{"name": "adr:ADR-0001"}]})
            })
            .collect();
        let page2 = json!([{"number": 101, "state": "open", "body": "",
                            "labels": [{"name": "adr:ADR-0001"}]}]);
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
            ("GET", "/pulls?state=all&page=1", 200, "[]".into()),
        ]);
        let snap = f.fetch_snapshot().unwrap();
        assert_eq!(snap.issues.len(), 101, "page 2 must be fetched and merged");
    }

    /// GitHub's combined status reports state "pending" with total_count 0
    /// when NO statuses were ever reported — that is CiState::None.
    #[test]
    fn combined_status_total_count_zero_is_none() {
        let (f, _) = forge_with(vec![
            ("GET", "/issues?state=all&page=1", 200, "[]".into()),
            (
                "GET",
                "/pulls?state=all&page=1",
                200,
                r#"[
                    {"number": 7, "state": "open", "merged_at": null,
                     "head": {"ref": "conduit/adr-0001/a", "sha": "abc"}, "labels": []},
                    {"number": 9, "state": "open", "merged_at": null,
                     "head": {"ref": "conduit/adr-0002/b", "sha": "def"}, "labels": []}
                ]"#
                .into(),
            ),
            ("GET", "/pulls/7/reviews?page=1", 200, "[]".into()),
            (
                "GET",
                "/commits/abc/status",
                200,
                r#"{"state": "pending", "total_count": 0}"#.into(),
            ),
            ("GET", "/pulls/9/reviews?page=1", 200, "[]".into()),
            (
                "GET",
                "/commits/def/status",
                200,
                r#"{"state": "failure", "total_count": 3}"#.into(),
            ),
        ]);
        let snap = f.fetch_snapshot().unwrap();
        assert_eq!(snap.prs[0].ci, CiState::None, "zero total_count -> None");
        assert_eq!(snap.prs[1].ci, CiState::Failure);
    }

    /// GitHub HAS a server-side head filter (Gitea difference): one query,
    /// no pagination loop, `head={owner}:{branch}`.
    #[test]
    fn find_open_pr_by_head_uses_server_side_head_filter() {
        let (f, seen) = forge_with(vec![(
            "GET",
            "/pulls?state=open&head=octo:conduit/adr-0002/b",
            200,
            r#"[{"number": 9, "state": "open",
                 "head": {"ref": "conduit/adr-0002/b", "sha": "def"}}]"#
                .into(),
        )]);
        assert_eq!(
            f.find_open_pr_by_head("conduit/adr-0002/b").unwrap(),
            Some(PrId(9))
        );
        assert_eq!(seen.lock().unwrap().len(), 1, "exactly one request");

        let (f, _) = forge_with(vec![(
            "GET",
            "/pulls?state=open&head=octo:conduit/none/missing",
            200,
            "[]".into(),
        )]);
        assert_eq!(
            f.find_open_pr_by_head("conduit/none/missing").unwrap(),
            None
        );
    }

    #[test]
    fn find_issue_by_marker_hits_body_without_fetching_comments() {
        let marker = "<!-- conduit:task:adr-0007 -->";
        let (f, _) = forge_with(vec![(
            "GET",
            "/issues?state=all&page=1",
            200,
            r#"[{"number": 3, "state": "open",
                 "body": "intro\n\n<!-- conduit:task:adr-0007 -->", "labels": []}]"#
                .into(),
        )]);
        // No comment routes exist — a comment fetch would panic.
        assert_eq!(f.find_issue_by_marker(marker).unwrap(), Some(IssueId(3)));
    }

    /// A PR row in the issues listing must not match the marker probe — by
    /// body OR by comment fallback (no comments fetch for PR rows).
    #[test]
    fn find_issue_by_marker_skips_pull_request_rows() {
        let marker = "<!-- conduit:task:adr-0007 -->";
        let (f, _) = forge_with(vec![
            (
                "GET",
                "/issues?state=all&page=1",
                200,
                r#"[{"number": 9, "state": "open",
                     "body": "<!-- conduit:task:adr-0007 -->",
                     "labels": [],
                     "pull_request": {"url": "https://api.github.com/repos/octo/example/pulls/9"}},
                    {"number": 1, "state": "open", "body": "no marker", "labels": []}]"#
                    .into(),
            ),
            // Fallback fetches comments for issue 1 ONLY — a fetch for the
            // PR row (9) would panic.
            ("GET", "/issues/1/comments?page=1", 200, "[]".into()),
        ]);
        assert_eq!(f.find_issue_by_marker(marker).unwrap(), None);
    }

    #[test]
    fn find_issue_by_marker_falls_back_to_comments() {
        let marker = "<!-- conduit:task:adr-0007 -->";
        let (f, _) = forge_with(vec![
            (
                "GET",
                "/issues?state=all&page=1",
                200,
                r#"[{"number": 1, "state": "open", "body": "no marker", "labels": []},
                    {"number": 2, "state": "open", "body": "none here", "labels": []}]"#
                    .into(),
            ),
            ("GET", "/issues/1/comments?page=1", 200, "[]".into()),
            (
                "GET",
                "/issues/2/comments?page=1",
                200,
                r#"[{"id": 9, "body": "<!-- conduit:task:adr-0007 -->\n\nstatus"}]"#.into(),
            ),
        ]);
        assert_eq!(f.find_issue_by_marker(marker).unwrap(), Some(IssueId(2)));
    }

    #[test]
    fn ensure_labels_creates_only_missing_labels() {
        let (f, seen) = forge_with(vec![
            (
                "GET",
                "/labels?page=1",
                200,
                r#"[{"id": 1, "name": "conduit:run", "color": "1d76db"}]"#.into(),
            ),
            // Exactly ONE create — posting conduit:run too would panic.
            (
                "POST",
                "/labels",
                201,
                r#"{"id": 2, "name": "conduit:failed", "color": "d73a4a"}"#.into(),
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
        assert_eq!(body["color"], "d73a4a");
        assert_eq!(body["description"], "failed");
    }

    /// GitHub answers 422 (not Gitea's 409) when the label already exists —
    /// converged, not an error.
    #[test]
    fn ensure_labels_treats_422_already_exists_as_converged() {
        let (f, _) = forge_with(vec![
            ("GET", "/labels?page=1", 200, "[]".into()),
            (
                "POST",
                "/labels",
                422,
                r#"{"message": "Validation Failed",
                    "errors": [{"resource": "Label", "code": "already_exists"}]}"#
                    .into(),
            ),
        ]);
        f.ensure_labels(&[LabelSpec {
            name: "conduit:run".into(),
            color: "1d76db".into(),
            description: "trigger".into(),
        }])
        .unwrap();
    }

    /// GitHub difference from Gitea: issue/PR label payloads carry NAMES,
    /// never ids — no labels-list lookup happens before the write.
    #[test]
    fn create_issue_posts_label_names_not_ids() {
        let (f, seen) = forge_with(vec![("POST", "/issues", 201, r#"{"number": 5}"#.into())]);
        let id = f
            .create_issue(&NewIssue {
                title: "t".into(),
                body: "b".into(),
                labels: vec!["adr:ADR-0003".into()],
            })
            .unwrap();
        assert_eq!(id, IssueId(5));
        let body = last_body(&seen);
        assert_eq!(body["title"], "t");
        assert_eq!(body["body"], "b");
        assert_eq!(body["labels"], json!(["adr:ADR-0003"]));
    }

    #[test]
    fn set_issue_labels_puts_replacement_name_set() {
        let (f, seen) = forge_with(vec![("PUT", "/issues/5/labels", 200, "[]".into())]);
        f.set_issue_labels(&IssueId(5), &["adr:ADR-0003".into(), "conduit:run".into()])
            .unwrap();
        let body = last_body(&seen);
        assert_eq!(body["labels"], json!(["adr:ADR-0003", "conduit:run"]));
    }

    #[test]
    fn close_issue_patches_state_closed() {
        let (f, seen) = forge_with(vec![(
            "PATCH",
            "/issues/5",
            200,
            r#"{"number": 5, "state": "closed"}"#.into(),
        )]);
        f.close_issue(&IssueId(5)).unwrap();
        assert_eq!(last_body(&seen)["state"], "closed");
    }

    #[test]
    fn close_unknown_issue_is_api_404() {
        let (f, _) = forge_with(vec![(
            "PATCH",
            "/issues/999",
            404,
            r#"{"message": "Not Found"}"#.into(),
        )]);
        let err = f.close_issue(&IssueId(999)).unwrap_err();
        let ForgeError::Api { status, .. } = err else {
            panic!("expected Api error, got {err:?}");
        };
        assert_eq!(status, 404);
    }

    #[test]
    fn open_pr_creates_then_sets_labels_by_name() {
        let (f, seen) = forge_with(vec![
            ("POST", "/pulls", 201, r#"{"number": 9}"#.into()),
            ("PUT", "/issues/9/labels", 200, "[]".into()),
        ]);
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
        let post: Value = serde_json::from_str(recorded[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(post["head"], "conduit/adr-0001/x");
        assert_eq!(post["base"], "main");
        assert!(post.get("labels").is_none(), "labels go via the PUT");
        let put: Value = serde_json::from_str(recorded[1].body.as_deref().unwrap()).unwrap();
        assert_eq!(
            put["labels"],
            json!(["effort:1-super-quick", "adr:ADR-0001"])
        );
    }

    #[test]
    fn open_pr_without_labels_skips_the_label_put() {
        let (f, _) = forge_with(vec![("POST", "/pulls", 201, r#"{"number": 9}"#.into())]);
        // A PUT would panic (no route for it).
        let id = f
            .open_pr(&PrDraft {
                title: "t".into(),
                body: "b".into(),
                head: "conduit/adr-0001/x".into(),
                base: "main".into(),
                labels: vec![],
            })
            .unwrap();
        assert_eq!(id, PrId(9));
    }

    #[test]
    fn comment_upsert_edits_existing_marker_comment() {
        let marker = "<!-- conduit:task:adr-0003 -->";
        let (f, seen) = forge_with(vec![
            (
                "GET",
                "/issues/5/comments?page=1",
                200,
                r#"[{"id": 42, "body": "<!-- conduit:task:adr-0003 -->\n\nold"}]"#.into(),
            ),
            ("PATCH", "/issues/comments/42", 200, r#"{"id": 42}"#.into()),
        ]);
        f.upsert_issue_comment(&IssueId(5), marker, "new").unwrap();
        // FixtureTransport panics on a POST — reaching here proves PATCH path.
        let body = last_body(&seen);
        let text = body["body"].as_str().unwrap();
        assert!(text.starts_with(marker), "marker embedded in comment body");
        assert!(text.ends_with("new"));
    }

    #[test]
    fn comment_upsert_creates_when_marker_absent() {
        let marker = "<!-- conduit:task:adr-0003 -->";
        let (f, _) = forge_with(vec![
            ("GET", "/issues/5/comments?page=1", 200, "[]".into()),
            ("POST", "/issues/5/comments", 201, r#"{"id": 43}"#.into()),
        ]);
        f.upsert_issue_comment(&IssueId(5), marker, "first")
            .unwrap();
    }

    /// PR comments ride the SAME issue-comment endpoints (PR number works).
    #[test]
    fn pr_comment_upsert_uses_issue_comment_endpoints() {
        let marker = "<!-- conduit:pr:adr-0003 -->";
        let (f, _) = forge_with(vec![
            ("GET", "/issues/9/comments?page=1", 200, "[]".into()),
            ("POST", "/issues/9/comments", 201, r#"{"id": 44}"#.into()),
        ]);
        f.upsert_pr_comment(&PrId(9), marker, "pr status").unwrap();
    }

    /// No API call — and src/git.rs refuses to push to any non-localhost URL
    /// (Task 11 guard), so this URL is never pushed to in the spike.
    #[test]
    fn git_remote_url_is_plain_https_clone_url() {
        let (f, seen) = forge_with(vec![]);
        assert_eq!(
            f.git_remote_url().unwrap(),
            "https://github.com/octo/example.git"
        );
        assert!(seen.lock().unwrap().is_empty(), "no API call");
        assert_eq!(f.describe(), "github octo/example");
    }

    #[test]
    fn auth_errors_map_to_forge_auth() {
        let (f, _) = forge_with(vec![(
            "GET",
            "/labels?page=1",
            401,
            r#"{"message": "Bad credentials"}"#.into(),
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

    #[test]
    fn requests_carry_bearer_auth_accept_and_user_agent() {
        let (f, seen) = forge_with(vec![("PATCH", "/issues/5", 200, r#"{"number": 5}"#.into())]);
        f.close_issue(&IssueId(5)).unwrap();
        let recorded = seen.lock().unwrap();
        assert_eq!(recorded[0].method, "PATCH");
        assert_eq!(
            recorded[0].url, "https://api.github.com/repos/octo/example/issues/5",
            "repo-scoped URL built from the documented base"
        );
        let headers = &recorded[0].headers;
        let get = |k: &str| {
            headers
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("Authorization"), Some("Bearer test-token"));
        assert_eq!(get("Accept"), Some("application/vnd.github+json"));
        assert_eq!(get("User-Agent"), Some("conduit-spike"));
    }

    /// The spike's hard constraint, structurally: the public constructor only
    /// hands out a DryRun wrapper — a mutation is recorded, never sent (the
    /// UreqTransport inside would hit the real network if it were).
    #[test]
    fn open_github_only_hands_out_dry_run() {
        let cfg = crate::config::GithubConfig {
            owner: "octo".into(),
            repo: "example".into(),
        };
        let forge: DryRunForge<GitHubForge> = open_github(&cfg, "tok".into());
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
            "open_github wires the repo-slug redaction: {}",
            t[0]
        );
    }

    // -----------------------------------------------------------------------
    // Manual fixture recorder — READS ONLY against the public fixture repo.
    // Run: cargo test --lib github::record_fixtures -- --ignored
    // -----------------------------------------------------------------------

    #[test]
    #[ignore = "manual recorder: needs GITHUB_TOKEN / gh login; reads only"]
    fn record_fixtures() {
        let token = resolve_token().expect("GITHUB_TOKEN or `gh auth login` first");
        let dir = std::path::Path::new("tests/fixtures/github");
        std::fs::create_dir_all(dir).unwrap();
        let auth = format!("Bearer {token}");
        let get = |path_and_query: &str| -> Vec<u8> {
            let url = format!("{API_BASE}/repos/{FIXTURE_OWNER}/{FIXTURE_REPO}/{path_and_query}");
            let resp = UreqTransport
                .request(
                    "GET",
                    &url,
                    &[
                        ("Authorization", &auth),
                        ("Accept", "application/vnd.github+json"),
                        ("User-Agent", "conduit-spike"),
                    ],
                    None,
                )
                .expect("fixture read");
            assert!(
                (200..300).contains(&resp.status),
                "GET {path_and_query}: HTTP {}",
                resp.status
            );
            resp.body
        };

        let issues = get("issues?state=all&page=1&per_page=100");
        std::fs::write(dir.join("issues.json"), &issues).unwrap();
        let pulls = get("pulls?state=all&page=1&per_page=100");
        std::fs::write(dir.join("pulls.json"), &pulls).unwrap();

        // Reviews + combined status for every recorded PR (real shapes for
        // the fixture transport, whether or not the snapshot path uses them).
        let parsed: Value = serde_json::from_slice(&pulls).unwrap();
        for pr in parsed.as_array().unwrap() {
            let number = pr["number"].as_u64().unwrap();
            let sha = pr.pointer("/head/sha").and_then(|v| v.as_str()).unwrap();
            let reviews = get(&format!("pulls/{number}/reviews?page=1&per_page=100"));
            std::fs::write(dir.join(format!("reviews_{number}.json")), &reviews).unwrap();
            let status = get(&format!("commits/{sha}/status"));
            std::fs::write(dir.join(format!("status_{sha}.json")), &status).unwrap();
        }

        // The token must never appear in a recorded file.
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let contents = std::fs::read_to_string(&path).unwrap();
            assert!(
                !contents.contains(&token),
                "token leaked into {}",
                path.display()
            );
        }
    }
}
