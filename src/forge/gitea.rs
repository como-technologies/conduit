//! Gitea REST v1 adapter (spec §Implementations: Gitea is the real lifecycle
//! host — full read-write). Sits on the [`HttpTransport`] seam: unit tests
//! below run against a `FixtureTransport` and never touch the network; the
//! live conformance leg (`CONDUIT_E2E_GITEA=1`, tests/conformance.rs) runs
//! against the throwaway demo container (`just forge-up`).
//!
//! Adapter obligations from the `forge` module header, honored here:
//! - Disappearance rule: snapshots fetch `state=all` for BOTH issues and
//!   pulls — merged/closed items stay visible until their terminal events
//!   have been observed.
//! - Explicit pagination: every list call loops `?page=N&limit=50` and stops
//!   on a short page (a page-1-only fetch silently truncates).
//! - Reviews are never filtered: only rows whose `state` is not a submitted
//!   verdict (e.g. `PENDING` drafts) are skipped; a dismissed review keeps
//!   its original state string on Gitea, so it stays with a stable id.
//! - Id-uniqueness: issue/PR numbers are unique per repo on the forge side.
//!
//! Gitea quirk: label endpoints take label IDs (i64), not names — every
//! label write resolves names through the repo's label list first.

use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::{
    CiState, Forge, ForgeError, HttpTransport, IssueSnapshot, LabelSpec, NewIssue, PrDraft,
    PrSnapshot, RepoSnapshot, Review, rest_call,
};
use crate::task::{IssueId, PrId, ReviewId, ReviewVerdict};

/// Page size for every list call; a page shorter than this ends the loop.
const PAGE_LIMIT: usize = 50;

pub struct GiteaForge {
    transport: Box<dyn HttpTransport>,
    base_url: String, // e.g. "http://localhost:3000" (no trailing slash)
    owner: String,
    repo: String,
    token: String,
}

impl GiteaForge {
    pub fn new(
        transport: Box<dyn HttpTransport>,
        base_url: &str,
        owner: &str,
        repo: &str,
        token: &str,
    ) -> GiteaForge {
        GiteaForge {
            transport,
            base_url: base_url.trim_end_matches('/').to_string(),
            owner: owner.to_string(),
            repo: repo.to_string(),
            token: token.to_string(),
        }
    }

    /// Production constructor: ureq transport + config + resolved token.
    pub fn open(cfg: &crate::config::GiteaConfig, token: String) -> GiteaForge {
        GiteaForge::new(
            Box::new(super::UreqTransport),
            &cfg.base_url,
            &cfg.owner,
            &cfg.repo,
            &token,
        )
    }

    // -- wire plumbing --------------------------------------------------

    /// One repo-scoped REST call: `path` is relative to
    /// `{base_url}/api/v1/repos/{owner}/{repo}/`.
    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, ForgeError> {
        let url = format!(
            "{}/api/v1/repos/{}/{}/{}",
            self.base_url, self.owner, self.repo, path
        );
        let auth = format!("token {}", self.token);
        rest_call(
            self.transport.as_ref(),
            method,
            &url,
            &[
                ("Authorization", &auth),
                ("Content-Type", "application/json"),
            ],
            body,
            "gitea",
        )
    }

    /// Paginated GET (module-header obligation: EXPLICIT `?page=N&limit=50`
    /// loop, stop on a short page — a page-1-only fetch silently truncates
    /// at the server default and breaks the disappearance rule).
    fn get_paginated(&self, path_and_query: &str) -> Result<Vec<Value>, ForgeError> {
        let sep = if path_and_query.contains('?') {
            '&'
        } else {
            '?'
        };
        let mut out = Vec::new();
        let mut page = 1usize;
        loop {
            let path = format!("{path_and_query}{sep}page={page}&limit={PAGE_LIMIT}");
            let Value::Array(items) = self.call("GET", &path, None)? else {
                return Err(ForgeError::Api {
                    status: 200,
                    message: format!("gitea: expected a JSON array from {path}"),
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

    // -- labels ----------------------------------------------------------

    /// All repo labels as (id, name) — Gitea's label write endpoints take
    /// IDs, never names.
    fn list_labels(&self) -> Result<Vec<(i64, String)>, ForgeError> {
        Ok(self
            .get_paginated("labels")?
            .iter()
            .filter_map(|l| Some((l.get("id")?.as_i64()?, l.get("name")?.as_str()?.to_string())))
            .collect())
    }

    /// Resolve label names to Gitea ids. A name missing from the repo is
    /// created on the fly (neutral grey) so label writes converge even for
    /// labels outside the `ensure_labels` standard set (e.g. `adr:ADR-NNNN`).
    fn resolve_label_ids(&self, names: &[String]) -> Result<Vec<i64>, ForgeError> {
        let mut known = self.list_labels()?;
        let mut ids = Vec::with_capacity(names.len());
        for name in names {
            if let Some((id, _)) = known.iter().find(|(_, n)| n == name) {
                ids.push(*id);
                continue;
            }
            eprintln!(
                "conduit: auto-creating missing label {name:?} (neutral grey) — \
                 add it to ensure_labels if this is a standing label"
            );
            match self.call(
                "POST",
                "labels",
                Some(json!({"name": name, "color": "ededed", "description": ""})),
            ) {
                Ok(v) => {
                    let id = field_u64(&v, "id")? as i64;
                    known.push((id, name.clone()));
                    ids.push(id);
                }
                // Lost a create race — re-list and take the winner's id.
                Err(ForgeError::Api { status: 409, .. }) => {
                    known = self.list_labels()?;
                    let id = known
                        .iter()
                        .find(|(_, n)| n == name)
                        .map(|(id, _)| *id)
                        .ok_or_else(|| ForgeError::Api {
                            status: 409,
                            message: format!("gitea: label {name} conflicts but is not listed"),
                        })?;
                    ids.push(id);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(ids)
    }

    /// Current label names of an issue OR a PR — the ADR-0007 convergence
    /// probe (Gitea's label endpoints live under /issues/{n} and accept PR
    /// numbers).
    fn get_labels(&self, number: u64) -> Result<Vec<String>, ForgeError> {
        Ok(self
            .get_paginated(&format!("issues/{number}/labels"))?
            .iter()
            .filter_map(|l| l.get("name").and_then(|n| n.as_str()).map(str::to_string))
            .collect())
    }

    /// Absolute, convergent label set on an issue OR a PR (Gitea's label
    /// endpoints live under /issues/{n} and accept PR numbers).
    fn put_labels(&self, number: u64, labels: &[String]) -> Result<(), ForgeError> {
        let ids = self.resolve_label_ids(labels)?;
        self.call(
            "PUT",
            &format!("issues/{number}/labels"),
            Some(json!({"labels": ids})),
        )?;
        Ok(())
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

    /// All submitted reviews of one PR. Never filtered (module-header
    /// obligation) — only rows whose `state` is not a submitted verdict
    /// (`PENDING` drafts etc.) are skipped; Gitea keeps a dismissed review's
    /// state string, so it stays with a stable id.
    fn fetch_reviews(&self, number: u64) -> Result<Vec<Review>, ForgeError> {
        let mut reviews = Vec::new();
        for raw in self.get_paginated(&format!("pulls/{number}/reviews"))? {
            let verdict = match raw.get("state").and_then(|s| s.as_str()) {
                Some("APPROVED") => ReviewVerdict::Approved,
                Some("REQUEST_CHANGES") => ReviewVerdict::ChangesRequested,
                Some("COMMENT") => ReviewVerdict::Commented,
                _ => continue, // PENDING / REQUEST_REVIEW / UNKNOWN: not submitted verdicts
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

    /// Combined commit status -> CiState. 404 (no statuses ever reported for
    /// the sha) and the empty-string state both mean "no CI".
    fn fetch_ci(&self, head_sha: &str) -> Result<CiState, ForgeError> {
        if head_sha.is_empty() {
            return Ok(CiState::None);
        }
        match self.call("GET", &format!("commits/{head_sha}/status"), None) {
            Ok(v) => Ok(match v.get("state").and_then(|s| s.as_str()) {
                Some("pending") => CiState::Pending,
                Some("success") => CiState::Success,
                Some("failure") | Some("error") => CiState::Failure,
                _ => CiState::None, // "" = no statuses reported
            }),
            Err(ForgeError::Api { status: 404, .. }) => Ok(CiState::None),
            Err(e) => Err(e),
        }
    }

    /// Raw issue listing (PRs excluded via type=issues; state=all per the
    /// disappearance rule). Shared by fetch_snapshot and the marker probe —
    /// the probe must see ALL issues, not just conduit-labeled ones.
    fn list_all_issues(&self) -> Result<Vec<Value>, ForgeError> {
        self.get_paginated("issues?type=issues&state=all")
    }
}

impl Forge for GiteaForge {
    fn describe(&self) -> String {
        format!("gitea {}/{} at {}", self.owner, self.repo, self.base_url)
    }

    /// Credential-free (follow-up 1): the token never enters a git argv — it
    /// reaches git through [`GiteaForge::git_auth`] + the env-only helper.
    fn git_remote_url(&self) -> Result<String, ForgeError> {
        Ok(format!(
            "{}/{}/{}.git",
            self.base_url, self.owner, self.repo
        ))
    }

    fn git_auth(&self) -> Result<Option<crate::git::GitAuth>, ForgeError> {
        Ok(Some(crate::git::GitAuth {
            username: "conduit-bot".to_string(),
            token: self.token.clone(),
        }))
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
            let merged = raw.get("merged").and_then(|v| v.as_bool()).unwrap_or(false);
            // Confirmed against live Gitea 1.24 (Task 8 step 6): the PR
            // object carries BOTH "merge_commit_sha" (populated when merged)
            // and "merged_commit_id" (null) — read the former.
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

    fn find_open_pr_by_head(&self, branch: &str) -> Result<Option<PrId>, ForgeError> {
        // Gitea has no head= query filter — filter client-side.
        for raw in self.get_paginated("pulls?state=open")? {
            if raw.pointer("/head/ref").and_then(|v| v.as_str()) == Some(branch) {
                return Ok(Some(PrId(field_u64(&raw, "number")?)));
            }
        }
        Ok(None)
    }

    /// Note: the comment-fallback leg is O(issues) x O(comments/issue). It
    /// fires only when the marker is absent from every issue body (conduit
    /// always embeds the marker at create time, so in practice: pre-existing
    /// issues not created by conduit). Repos conduit manages are small
    /// (dozens of issues), so the cost is acceptable for the spike.
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
        let existing: Vec<String> = self.list_labels()?.into_iter().map(|(_, n)| n).collect();
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
                // Already exists (create race) — converged, not an error.
                Err(ForgeError::Api { status: 409, .. }) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn create_issue(&self, new: &NewIssue) -> Result<IssueId, ForgeError> {
        let ids = self.resolve_label_ids(&new.labels)?;
        let v = self.call(
            "POST",
            "issues",
            Some(json!({"title": new.title, "body": new.body, "labels": ids})),
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

    fn get_issue_labels(&self, id: &IssueId) -> Result<Vec<String>, ForgeError> {
        self.get_labels(id.0)
    }

    fn get_pr_labels(&self, id: &PrId) -> Result<Vec<String>, ForgeError> {
        self.get_labels(id.0)
    }

    fn set_issue_labels(&self, id: &IssueId, labels: &[String]) -> Result<(), ForgeError> {
        self.put_labels(id.0, labels)
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
            self.put_labels(id.0, &draft.labels)?;
        }
        Ok(id)
    }

    fn upsert_pr_comment(&self, id: &PrId, marker: &str, body: &str) -> Result<(), ForgeError> {
        self.upsert_comment(id.0, marker, body)
    }

    fn set_pr_labels(&self, id: &PrId, labels: &[String]) -> Result<(), ForgeError> {
        self.put_labels(id.0, labels)
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
            message: format!("gitea: response missing numeric `{name}`"),
        })
}

// ---------------------------------------------------------------------------
// Fixture-based unit tests — every test names its exact wire traffic; an
// unexpected request panics. Fixture bodies mirror live gitea/gitea:1.24
// responses (Task 8 live-verification note).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::HttpResponse;
    use std::sync::Mutex;

    /// (method, url fragment) -> (status, body); consumed in order per match.
    /// Any request that matches no remaining route panics.
    struct FixtureTransport {
        routes: Mutex<Vec<(String, String, u16, String)>>,
    }

    impl HttpTransport for FixtureTransport {
        fn request(
            &self,
            method: &str,
            url: &str,
            _headers: &[(&str, &str)],
            _body: Option<&[u8]>,
        ) -> Result<HttpResponse, ForgeError> {
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

    fn forge_with(routes: Vec<(&str, &str, u16, String)>) -> GiteaForge {
        GiteaForge::new(
            Box::new(FixtureTransport {
                routes: Mutex::new(
                    routes
                        .into_iter()
                        .map(|(m, u, s, b)| (m.into(), u.into(), s, b))
                        .collect(),
                ),
            }),
            "http://localhost:3000",
            "como",
            "conduit-dogfood",
            "tok",
        )
    }

    #[test]
    fn create_issue_resolves_label_names_to_ids() {
        let f = forge_with(vec![
            (
                "GET",
                "/labels",
                200,
                r#"[{"id": 11, "name": "adr:ADR-0003"}]"#.into(),
            ),
            ("POST", "/issues", 201, r#"{"number": 5}"#.into()),
        ]);
        let id = f
            .create_issue(&NewIssue {
                title: "t".into(),
                body: "b".into(),
                labels: vec!["adr:ADR-0003".into()],
            })
            .unwrap();
        assert_eq!(id, IssueId(5));
    }

    #[test]
    fn snapshot_filters_to_conduit_issues_and_branches() {
        let f = forge_with(vec![
            (
                "GET",
                "/issues?type=issues&state=all",
                200,
                r#"[
                    {"number": 1, "state": "open", "body": "x",
                     "labels": [{"name": "adr:ADR-0003"}]},
                    {"number": 2, "state": "open", "body": "y",
                     "labels": [{"name": "bug"}]}
                ]"#
                .into(),
            ),
            (
                "GET",
                "/pulls?state=all",
                200,
                r#"[
                    {"number": 7, "state": "open", "merged": false,
                     "title": "[ADR-0003] adopt snapshot router",
                     "body": "Implements the decision.\n\nAdr-Reference: ADR-0003",
                     "head": {"ref": "conduit/adr-0003/x", "sha": "abc"},
                     "labels": []},
                    {"number": 8, "state": "open", "merged": false,
                     "title": "unrelated", "body": "",
                     "head": {"ref": "feature/other", "sha": "def"},
                     "labels": []}
                ]"#
                .into(),
            ),
            (
                "GET",
                "/pulls/7/reviews",
                200,
                // Mirrors live Gitea 1.24: a superseded REQUEST_CHANGES review
                // comes back dismissed=true with its original state — it must
                // be KEPT (reviews are never filtered; ids stay stable).
                r#"[
                    {"id": 31, "user": {"login": "reviewer"}, "state": "REQUEST_CHANGES",
                     "dismissed": true, "official": false,
                     "body": "fix x", "submitted_at": "2026-06-11T10:00:00Z"},
                    {"id": 32, "user": {"login": "reviewer"}, "state": "PENDING",
                     "dismissed": false, "official": false,
                     "body": "draft", "submitted_at": null}
                ]"#
                .into(),
            ),
            (
                "GET",
                "/commits/abc/status",
                200,
                r#"{"state": "success"}"#.into(),
            ),
        ]);
        let snap = f.fetch_snapshot().unwrap();
        assert_eq!(snap.issues.len(), 1, "non-conduit issue filtered");
        assert_eq!(snap.prs.len(), 1, "non-conduit/* PR filtered");
        // GAP A: title and body must be parsed verbatim — a field-name typo in
        // the adapter (e.g. "Title" vs "title") fails here before conformance.
        assert_eq!(
            snap.prs[0].title, "[ADR-0003] adopt snapshot router",
            "PrSnapshot.title must carry the forge title verbatim"
        );
        assert_eq!(
            snap.prs[0].body, "Implements the decision.\n\nAdr-Reference: ADR-0003",
            "PrSnapshot.body must carry the forge body verbatim"
        );
        assert_eq!(snap.prs[0].reviews.len(), 1, "PENDING draft skipped");
        assert_eq!(
            snap.prs[0].reviews[0].verdict,
            ReviewVerdict::ChangesRequested
        );
        assert_eq!(snap.prs[0].reviews[0].id, ReviewId("31".into()));
        assert_eq!(snap.prs[0].ci, CiState::Success);
    }

    /// Disappearance rule (module-header obligation): state=all keeps a
    /// merged+closed PR and a closed issue in the snapshot, with merge_sha.
    #[test]
    fn snapshot_keeps_terminal_prs_and_closed_issues() {
        let f = forge_with(vec![
            (
                "GET",
                "/issues?type=issues&state=all",
                200,
                r#"[{"number": 4, "state": "closed", "body": "",
                     "labels": [{"name": "conduit:run"}]}]"#
                    .into(),
            ),
            (
                "GET",
                "/pulls?state=all",
                200,
                // Mirrors live Gitea 1.24: the populated field is
                // merge_commit_sha; merged_commit_id also exists but is null.
                r#"[{"number": 7, "state": "closed", "merged": true,
                     "merge_commit_sha": "cafe42", "merged_commit_id": null,
                     "head": {"ref": "conduit/adr-0001/x", "sha": "abc"},
                     "labels": []}]"#
                    .into(),
            ),
            ("GET", "/pulls/7/reviews", 200, "[]".into()),
            ("GET", "/commits/abc/status", 200, r#"{"state": ""}"#.into()),
        ]);
        let snap = f.fetch_snapshot().unwrap();
        assert_eq!(snap.issues.len(), 1, "closed issue must stay");
        assert!(snap.issues[0].closed);
        assert_eq!(snap.prs.len(), 1, "merged PR must stay");
        let pr = &snap.prs[0];
        assert!(pr.merged);
        assert!(pr.closed);
        assert_eq!(pr.merge_sha.as_deref(), Some("cafe42"));
        assert_eq!(pr.ci, CiState::None, "empty combined status is None");
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
        let page2 = json!([{"number": 51, "state": "open", "body": "",
                            "labels": [{"name": "adr:ADR-0001"}]}]);
        let f = forge_with(vec![
            (
                "GET",
                "/issues?type=issues&state=all&page=1&limit=50",
                200,
                serde_json::to_string(&page1).unwrap(),
            ),
            (
                "GET",
                "/issues?type=issues&state=all&page=2&limit=50",
                200,
                page2.to_string(),
            ),
            ("GET", "/pulls?state=all&page=1", 200, "[]".into()),
        ]);
        let snap = f.fetch_snapshot().unwrap();
        assert_eq!(snap.issues.len(), 51, "page 2 must be fetched and merged");
    }

    #[test]
    fn ci_status_404_is_none_and_error_is_failure() {
        let f = forge_with(vec![
            ("GET", "/issues?type=issues&state=all", 200, "[]".into()),
            (
                "GET",
                "/pulls?state=all",
                200,
                r#"[
                    {"number": 7, "state": "open", "merged": false,
                     "head": {"ref": "conduit/adr-0001/a", "sha": "abc"}, "labels": []},
                    {"number": 9, "state": "open", "merged": false,
                     "head": {"ref": "conduit/adr-0002/b", "sha": "def"}, "labels": []}
                ]"#
                .into(),
            ),
            ("GET", "/pulls/7/reviews", 200, "[]".into()),
            (
                "GET",
                "/commits/abc/status",
                404,
                r#"{"message": "no commit status"}"#.into(),
            ),
            ("GET", "/pulls/9/reviews", 200, "[]".into()),
            (
                "GET",
                "/commits/def/status",
                200,
                r#"{"state": "error"}"#.into(),
            ),
        ]);
        let snap = f.fetch_snapshot().unwrap();
        assert_eq!(snap.prs[0].ci, CiState::None, "404 combined status -> None");
        assert_eq!(snap.prs[1].ci, CiState::Failure, "error -> Failure");
    }

    #[test]
    fn comment_upsert_edits_existing_marker_comment() {
        let marker = "<!-- conduit:task:adr-0003 -->";
        let f = forge_with(vec![
            (
                "GET",
                "/issues/5/comments",
                200,
                r#"[{"id": 42, "body": "<!-- conduit:task:adr-0003 -->\n\nold"}]"#.into(),
            ),
            ("PATCH", "/issues/comments/42", 200, r#"{}"#.into()),
        ]);
        f.upsert_issue_comment(&IssueId(5), marker, "new").unwrap();
        // FixtureTransport panics on a POST — reaching here proves PATCH path.
    }

    #[test]
    fn comment_upsert_creates_when_marker_absent() {
        let marker = "<!-- conduit:task:adr-0003 -->";
        let f = forge_with(vec![
            ("GET", "/issues/5/comments", 200, "[]".into()),
            ("POST", "/issues/5/comments", 201, r#"{"id": 43}"#.into()),
        ]);
        f.upsert_issue_comment(&IssueId(5), marker, "first")
            .unwrap();
    }

    #[test]
    fn find_open_pr_by_head_filters_client_side() {
        let pulls = r#"[
            {"number": 7, "state": "open", "merged": false,
             "head": {"ref": "conduit/adr-0001/a", "sha": "abc"}, "labels": []},
            {"number": 9, "state": "open", "merged": false,
             "head": {"ref": "conduit/adr-0002/b", "sha": "def"}, "labels": []}
        ]"#;
        let f = forge_with(vec![("GET", "/pulls?state=open", 200, pulls.into())]);
        assert_eq!(
            f.find_open_pr_by_head("conduit/adr-0002/b").unwrap(),
            Some(PrId(9))
        );
        let f = forge_with(vec![("GET", "/pulls?state=open", 200, pulls.into())]);
        assert_eq!(
            f.find_open_pr_by_head("conduit/none/missing").unwrap(),
            None
        );
    }

    #[test]
    fn find_issue_by_marker_hits_body_without_fetching_comments() {
        let marker = "<!-- conduit:task:adr-0007 -->";
        let f = forge_with(vec![(
            "GET",
            "/issues?type=issues&state=all",
            200,
            r#"[{"number": 3, "state": "open",
                 "body": "intro\n\n<!-- conduit:task:adr-0007 -->", "labels": []}]"#
                .into(),
        )]);
        // No comment routes exist — a comment fetch would panic.
        assert_eq!(f.find_issue_by_marker(marker).unwrap(), Some(IssueId(3)));
    }

    #[test]
    fn find_issue_by_marker_falls_back_to_comments() {
        let marker = "<!-- conduit:task:adr-0007 -->";
        let f = forge_with(vec![
            (
                "GET",
                "/issues?type=issues&state=all",
                200,
                r#"[{"number": 1, "state": "open", "body": "no marker", "labels": []},
                    {"number": 2, "state": "open", "body": "none here", "labels": []}]"#
                    .into(),
            ),
            ("GET", "/issues/1/comments", 200, "[]".into()),
            (
                "GET",
                "/issues/2/comments",
                200,
                r#"[{"id": 9, "body": "<!-- conduit:task:adr-0007 -->\n\nstatus"}]"#.into(),
            ),
        ]);
        assert_eq!(f.find_issue_by_marker(marker).unwrap(), Some(IssueId(2)));
    }

    #[test]
    fn ensure_labels_creates_only_missing_labels() {
        let f = forge_with(vec![
            (
                "GET",
                "/labels",
                200,
                r#"[{"id": 1, "name": "conduit:run"}]"#.into(),
            ),
            // Exactly ONE create — posting conduit:run too would panic.
            (
                "POST",
                "/labels",
                201,
                r#"{"id": 2, "name": "conduit:failed"}"#.into(),
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
    }

    #[test]
    fn ensure_labels_treats_409_conflict_as_converged() {
        let f = forge_with(vec![
            ("GET", "/labels", 200, "[]".into()),
            (
                "POST",
                "/labels",
                409,
                r#"{"message": "label already exists"}"#.into(),
            ),
        ]);
        f.ensure_labels(&[LabelSpec {
            name: "conduit:run".into(),
            color: "1d76db".into(),
            description: "trigger".into(),
        }])
        .unwrap();
    }

    /// ADR-0007 convergence probes: label reads return the forge's current
    /// label names for issues AND PRs (Gitea's label endpoints live under
    /// /issues/{n} and accept PR numbers).
    #[test]
    fn label_reads_return_current_names_for_issue_and_pr() {
        let f = forge_with(vec![
            (
                "GET",
                "/issues/5/labels",
                200,
                r#"[{"id": 1, "name": "adr:ADR-0003"}, {"id": 2, "name": "discuss"}]"#.into(),
            ),
            (
                "GET",
                "/issues/9/labels",
                200,
                r#"[{"id": 3, "name": "effort:1-super-quick"}]"#.into(),
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

    #[test]
    fn auth_errors_map_to_forge_auth() {
        let f = forge_with(vec![(
            "GET",
            "/labels",
            401,
            r#"{"message": "bad token"}"#.into(),
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
    fn close_unknown_issue_is_api_404() {
        let f = forge_with(vec![(
            "PATCH",
            "/issues/999",
            404,
            r#"{"message": "issue does not exist"}"#.into(),
        )]);
        let err = f.close_issue(&IssueId(999)).unwrap_err();
        let ForgeError::Api { status, .. } = err else {
            panic!("expected Api error, got {err:?}");
        };
        assert_eq!(status, 404);
    }

    #[test]
    fn open_pr_creates_then_sets_labels() {
        let f = forge_with(vec![
            ("POST", "/pulls", 201, r#"{"number": 9}"#.into()),
            (
                "GET",
                "/labels",
                200,
                r#"[{"id": 1, "name": "effort:1-super-quick"},
                    {"id": 2, "name": "adr:ADR-0001"}]"#
                    .into(),
            ),
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
    }

    /// Follow-up 1: the remote URL carries NO credential — argv is
    /// world-readable (`ps`, /proc/<pid>/cmdline); the token rides the git
    /// child env via [`Forge::git_auth`] instead.
    #[test]
    fn git_remote_url_is_credential_free() {
        let f = forge_with(vec![]);
        let url = f.git_remote_url().unwrap();
        assert_eq!(url, "http://localhost:3000/como/conduit-dogfood.git");
        assert!(!url.contains("tok"), "token must not appear in the URL");
        assert!(
            crate::git::is_local_remote(&url),
            "demo URL stays inside the local-push guard"
        );
    }

    #[test]
    fn git_auth_supplies_the_bot_credential_for_the_git_layer() {
        let f = forge_with(vec![]);
        let auth = f.git_auth().unwrap().expect("gitea pushes need auth");
        assert_eq!(auth.username, "conduit-bot");
        assert_eq!(auth.token, "tok");
    }
}
