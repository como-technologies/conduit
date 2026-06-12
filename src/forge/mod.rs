//! THE KEYSTONE (spec §The forge adapter): one trait both forges implement
//! identically, proven by tests/conformance.rs. Events are NEVER produced by
//! adapters — only by the shared pure [`diff`].
//!
//! Diff semantics (the contract):
//! - `IssueLabeled`: fires for each label present on an issue in `next` but
//!   not on the same issue in `prev`. An issue absent from `prev` fires for
//!   ALL its labels. Label removals fire nothing.
//! - `ReviewSubmitted`: fires for each review whose `id` is not present on
//!   the same PR in `prev` (dedupe by forge-native id). A PR absent from
//!   `prev` fires for all its reviews.
//! - `CiChanged`: fires when a PR exists in both and `ci` differs. New PRs
//!   fire nothing.
//! - `PrMerged`: fires when `!prev.merged && next.merged` (a PR absent from
//!   `prev` counts as not merged). `merge_sha` is required — the adapter
//!   guarantees it when merged; if absent the event carries an empty string
//!   and the conformance suite catches it.
//! - `PrClosed`: fires when `!prev.closed && next.closed && !next.merged` —
//!   a merged PR emits ONLY `PrMerged` (forges mark merged PRs closed too).
//! - Within-poll flaps (state that appears and reverts between two snapshots,
//!   e.g. a review submitted then dismissed) are invisible by design
//!   (spec §Review identity).
//!
//! Disappearance rule (adapter obligation):
//! A PR or issue present in `prev` but ABSENT from `next` produces no events.
//! Adapters MUST therefore keep merged/closed PRs and closed issues in the
//! snapshot until their terminal events have been observed (i.e. never fetch
//! only state=open); a merged PR that vanishes from the snapshot loses its
//! `PrMerged` forever and wedges the task. The conformance suite asserts this
//! adapter obligation.
//!
//! Reviews are never filtered from a PR's snapshot when the forge can make a
//! filtered review reappear: keep dismissed reviews with their original
//! verdict so ids stay stable in `prev` (Gitea does this) — filtering one out
//! and letting it reappear would re-fire `ReviewSubmitted`. The narrow
//! exception: a forge where dismissal is a ONE-WAY in-place state mutation on
//! the same id (GitHub's DISMISSED) may skip those rows, because a skipped id
//! can never reappear with a submitted verdict — see each adapter's module
//! header. Either way a dismissal fires nothing; that is fine because merge
//! is a human gate. A resubmission gets a new forge-native id on both forges
//! and correctly fires.
//!
//! Snapshots must be id-unique: duplicate issue/PR ids in `prev` are
//! last-wins; duplicates in `next` fire duplicate events. Uniqueness is the
//! adapter's obligation.
//!
//! Note: `IssueSnapshot.closed` is carried for adapters/probes but is not read
//! by `diff()` — no issue-closed event exists by design.

pub mod dry_run;
pub mod fake;
pub mod gitea;
pub mod github;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::task::{IssueId, PrId, ReviewId, ReviewVerdict};

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    /// Network / connectivity failure (connection refused, DNS, TLS, timeout).
    #[error("forge unreachable: {0}")]
    Offline(String),
    /// 401/403 — loud misconfiguration, never swallowed.
    #[error("forge auth failed (check the token env var): {0}")]
    Auth(String),
    /// Any other non-2xx, or unparseable response.
    #[error("forge API error {status}: {message}")]
    Api { status: u16, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CiState {
    Pending,
    Success,
    Failure,
    None,
}

/// Forge-native review identity + submitted_at (spec §Review identity): the
/// diff dedupes on `id`, so an EDITED review never re-fires and repeated
/// ChangesRequested rounds from the same reviewer are distinct events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub id: ReviewId,
    pub author: String,
    pub verdict: ReviewVerdict,
    pub body: String,
    #[serde(with = "time::serde::rfc3339")]
    pub submitted_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueSnapshot {
    pub id: IssueId,
    pub labels: Vec<String>,
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrSnapshot {
    pub id: PrId,
    /// PR title as the forge holds it — `conduit verify` asserts the
    /// tuesday-contract prefix on it (spec §The tuesday contract).
    pub title: String,
    /// PR body as the forge holds it — `conduit verify` asserts the
    /// trailer-as-final-line on it.
    pub body: String,
    pub head_branch: String,
    pub labels: Vec<String>,
    pub reviews: Vec<Review>,
    /// Consumed, never configured (spec §Out of scope: CI provisioning).
    pub ci: CiState,
    pub merged: bool,
    pub merge_sha: Option<String>,
    pub closed: bool,
}

/// One normalized read of the repo: conduit-labeled issues + conduit/*-branch
/// PRs ONLY (each adapter filters; asserted by the conformance suite).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoSnapshot {
    pub issues: Vec<IssueSnapshot>,
    pub prs: Vec<PrSnapshot>,
    #[serde(with = "time::serde::rfc3339")]
    pub fetched_at: OffsetDateTime,
}

/// Produced ONLY by the shared diff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ForgeEvent {
    IssueLabeled { issue: IssueId, label: String },
    ReviewSubmitted { pr: PrId, review: Review },
    CiChanged { pr: PrId, state: CiState },
    PrMerged { pr: PrId, merge_sha: String },
    PrClosed { pr: PrId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelSpec {
    pub name: String,
    /// Hex without '#', e.g. "00aabb".
    pub color: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewIssue {
    pub title: String,
    /// Carries the hidden task marker (contract::task_marker) for the
    /// find_issue_by_marker probe.
    pub body: String,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrDraft {
    pub title: String,
    /// Final line is the Adr-Reference trailer (contract::pr_body).
    pub body: String,
    /// Head branch — already pushed by conduit's git.rs before open_pr runs.
    pub head: String,
    pub base: String,
    pub labels: Vec<String>,
}

pub trait Forge {
    fn describe(&self) -> String;
    /// Used ONLY by src/git.rs, never by engines (spec: sandbox is structural).
    /// ALWAYS credential-free (follow-up 1) — no token ever rides a process
    /// argv; authentication goes through [`Forge::git_auth`].
    fn git_remote_url(&self) -> Result<String, ForgeError>;
    /// Credentials for git operations against [`Forge::git_remote_url`],
    /// supplied to the git layer's env-only credential helper. `None` = the
    /// remote needs no auth (local paths; GitHub, which is never pushed).
    fn git_auth(&self) -> Result<Option<crate::git::GitAuth>, ForgeError> {
        Ok(None)
    }
    // events in: one read, normalized
    fn fetch_snapshot(&self) -> Result<RepoSnapshot, ForgeError>;
    // idempotency probes (reads)
    fn find_open_pr_by_head(&self, branch: &str) -> Result<Option<PrId>, ForgeError>;
    fn find_issue_by_marker(&self, marker: &str) -> Result<Option<IssueId>, ForgeError>;
    // label-convergence probes (reads, ADR-0007): current label names of one
    // object — the read half of read→converge→absolute-write. Label writes
    // stay absolute; ownership scoping lives in crate::labels.
    fn get_issue_labels(&self, id: &IssueId) -> Result<Vec<String>, ForgeError>;
    fn get_pr_labels(&self, id: &PrId) -> Result<Vec<String>, ForgeError>;
    // actions out — NO merge method exists: humans merge in the forge UI
    fn ensure_labels(&self, labels: &[LabelSpec]) -> Result<(), ForgeError>;
    fn create_issue(&self, new: &NewIssue) -> Result<IssueId, ForgeError>;
    fn upsert_issue_comment(
        &self,
        id: &IssueId,
        marker: &str,
        body: &str,
    ) -> Result<(), ForgeError>;
    fn set_issue_labels(&self, id: &IssueId, labels: &[String]) -> Result<(), ForgeError>;
    fn close_issue(&self, id: &IssueId) -> Result<(), ForgeError>;
    fn open_pr(&self, draft: &PrDraft) -> Result<PrId, ForgeError>;
    fn upsert_pr_comment(&self, id: &PrId, marker: &str, body: &str) -> Result<(), ForgeError>;
    fn set_pr_labels(&self, id: &PrId, labels: &[String]) -> Result<(), ForgeError>;
}

/// THE shared pure diff — event semantics defined once (spec §The forge
/// adapter); the full contract is in the module header. Deterministic event
/// order: issues in `next` order, then PRs in `next` order; per PR:
/// ReviewSubmitted (snapshot order), CiChanged, then PrMerged/PrClosed.
pub fn diff(prev: &RepoSnapshot, next: &RepoSnapshot) -> Vec<ForgeEvent> {
    let mut events = Vec::new();

    let prev_issues: HashMap<IssueId, &IssueSnapshot> =
        prev.issues.iter().map(|i| (i.id, i)).collect();
    for issue in &next.issues {
        // An issue absent from `prev` has an empty previous label set, so ALL
        // its labels fire; removals fire nothing.
        let prev_labels = prev_issues
            .get(&issue.id)
            .map(|i| i.labels.as_slice())
            .unwrap_or(&[]);
        for label in &issue.labels {
            if !prev_labels.contains(label) {
                events.push(ForgeEvent::IssueLabeled {
                    issue: issue.id,
                    label: label.clone(),
                });
            }
        }
    }

    let prev_prs: HashMap<PrId, &PrSnapshot> = prev.prs.iter().map(|p| (p.id, p)).collect();
    for pr in &next.prs {
        let old = prev_prs.get(&pr.id).copied();
        // ReviewSubmitted: dedupe on forge-native id — an edited review keeps
        // its id and never re-fires; a PR absent from `prev` fires for all.
        for review in &pr.reviews {
            let seen = old.is_some_and(|o| o.reviews.iter().any(|r| r.id == review.id));
            if !seen {
                events.push(ForgeEvent::ReviewSubmitted {
                    pr: pr.id,
                    review: review.clone(),
                });
            }
        }
        // CiChanged: a transition needs both sides — new PRs fire nothing.
        if let Some(o) = old
            && o.ci != pr.ci
        {
            events.push(ForgeEvent::CiChanged {
                pr: pr.id,
                state: pr.ci,
            });
        }
        // PrMerged / PrClosed are mutually exclusive: a merged PR emits ONLY
        // PrMerged even though forges mark it closed too. A PR absent from
        // `prev` counts as not-merged/not-closed.
        let was_merged = old.is_some_and(|o| o.merged);
        let was_closed = old.is_some_and(|o| o.closed);
        if !was_merged && pr.merged {
            events.push(ForgeEvent::PrMerged {
                pr: pr.id,
                merge_sha: pr.merge_sha.clone().unwrap_or_default(),
            });
        } else if !was_closed && pr.closed && !pr.merged {
            events.push(ForgeEvent::PrClosed { pr: pr.id });
        }
    }

    events
}

// ---------------------------------------------------------------------------
// HTTP transport seam — adapters depend on this, not on ureq directly, so
// unit tests inject a fake transport (or recorded fixtures) and never hit the
// network.
// ---------------------------------------------------------------------------

/// A minimal blocking HTTP response (status + raw body).
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Blocking HTTP, abstracted so adapters are testable with a fake / recorded
/// fixtures and never hit the network in unit tests.
pub trait HttpTransport: Send + Sync {
    fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, ForgeError>;
}

/// Production transport over `ureq` (blocking, rustls). A non-2xx status is
/// returned as a normal [`HttpResponse`] (so adapters can map 401/403 → `Auth`,
/// else → `Api`); only a connection-level failure becomes
/// [`ForgeError::Offline`].
pub struct UreqTransport;

impl HttpTransport for UreqTransport {
    fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, ForgeError> {
        // ureq 3 reports 4xx/5xx as `Err` by default; disable that so a non-2xx
        // still comes back as a normal response. Bound every request (connect +
        // overall) so a network hang surfaces as a clean `Offline` error
        // instead of freezing the poll loop.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(std::time::Duration::from_secs(20)))
            .timeout_global(Some(std::time::Duration::from_secs(60)))
            .build()
            .into();
        let mut builder = ureq::http::Request::builder().method(method).uri(url);
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        let request = builder
            .body(body.unwrap_or(&[]).to_vec())
            .map_err(|e| ForgeError::Offline(e.to_string()))?;
        match agent.run(request) {
            Ok(resp) => Ok(read_response(resp)),
            // Connection refused / DNS / TLS / timeout → offline.
            Err(e) => Err(ForgeError::Offline(e.to_string())),
        }
    }
}

fn read_response(resp: ureq::http::Response<ureq::Body>) -> HttpResponse {
    use std::io::Read;
    let status = resp.status().as_u16();
    let mut body = Vec::new();
    // Best-effort body read; an unreadable body is just empty bytes.
    let _ = resp.into_body().into_reader().read_to_end(&mut body);
    HttpResponse { status, body }
}

/// Run one REST call: serialize `body`, send via `transport`, classify the
/// status (2xx ok; 401/403 → Auth; else Api), return `Value::Null` for an
/// empty 2xx body, else parse JSON. `label` names the provider in the
/// parse-error message. Error text extraction: the JSON `message` field if
/// present, else the lossy body string.
///
/// Pagination: `HttpResponse` carries no headers, so Link-header pagination
/// is unreachable by design. Adapters MUST paginate with explicit
/// `?page=N&per_page=...` query loops, stopping on a short page — a naive
/// page-1-only fetch silently truncates at the forges' default per_page=30
/// and violates the snapshot disappearance rule above.
pub(crate) fn rest_call(
    transport: &dyn HttpTransport,
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<serde_json::Value>,
    label: &str,
) -> Result<serde_json::Value, ForgeError> {
    let bytes = body.map(|b| serde_json::to_vec(&b).expect("serialize JSON body"));
    let resp = transport.request(method, url, headers, bytes.as_deref())?;
    match resp.status {
        200..=299 => {}
        401 | 403 => return Err(ForgeError::Auth(extract_error(&resp.body))),
        status => {
            return Err(ForgeError::Api {
                status,
                message: extract_error(&resp.body),
            });
        }
    }
    if resp.body.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_slice(&resp.body).map_err(|e| ForgeError::Api {
        status: resp.status,
        message: format!("invalid JSON from {label}: {e}"),
    })
}

/// Pull the forge's error text from a failed body: the JSON `message` field
/// (both GitHub and Gitea use it), else the lossy body string.
fn extract_error(body: &[u8]) -> String {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body)
        && let Some(msg) = v.get("message").and_then(|m| m.as_str())
    {
        return msg.to_string();
    }
    String::from_utf8_lossy(body).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{IssueId, PrId, ReviewId, ReviewVerdict};
    use time::macros::datetime;

    fn snap(issues: Vec<IssueSnapshot>, prs: Vec<PrSnapshot>) -> RepoSnapshot {
        RepoSnapshot {
            issues,
            prs,
            fetched_at: datetime!(2026-06-11 00:00 UTC),
        }
    }
    fn issue(id: u64, labels: &[&str]) -> IssueSnapshot {
        IssueSnapshot {
            id: IssueId(id),
            labels: labels.iter().map(|s| s.to_string()).collect(),
            closed: false,
        }
    }
    fn pr(id: u64) -> PrSnapshot {
        PrSnapshot {
            id: PrId(id),
            title: "[ADR-0003] x".into(),
            body: "b\n\nAdr-Reference: ADR-0003".into(),
            head_branch: "conduit/adr-0003/x".into(),
            labels: vec![],
            reviews: vec![],
            ci: CiState::None,
            merged: false,
            merge_sha: None,
            closed: false,
        }
    }
    fn review(id: &str, verdict: ReviewVerdict, body: &str) -> Review {
        Review {
            id: ReviewId(id.into()),
            author: "reviewer".into(),
            verdict,
            body: body.into(),
            submitted_at: datetime!(2026-06-11 00:00 UTC),
        }
    }

    #[test]
    fn unchanged_snapshots_produce_no_events() {
        let a = snap(vec![issue(1, &["conduit:run"])], vec![pr(7)]);
        assert!(diff(&a, &a.clone()).is_empty());
    }

    #[test]
    fn added_label_fires_once_removed_fires_nothing() {
        let prev = snap(vec![issue(1, &["adr:ADR-0003"])], vec![]);
        let next = snap(vec![issue(1, &["adr:ADR-0003", "conduit:run"])], vec![]);
        assert_eq!(
            diff(&prev, &next),
            vec![ForgeEvent::IssueLabeled {
                issue: IssueId(1),
                label: "conduit:run".into()
            }]
        );
        // removal: nothing
        assert!(diff(&next, &prev).is_empty());
    }

    #[test]
    fn new_issue_fires_all_its_labels() {
        let prev = snap(vec![], vec![]);
        let next = snap(vec![issue(1, &["adr:ADR-0003", "conduit:run"])], vec![]);
        let events = diff(&prev, &next);
        assert_eq!(events.len(), 2);
        assert!(
            events
                .iter()
                .all(|e| matches!(e, ForgeEvent::IssueLabeled { .. }))
        );
    }

    #[test]
    fn reviews_dedupe_by_forge_native_id() {
        let mut p_prev = pr(7);
        p_prev.reviews = vec![review("r1", ReviewVerdict::ChangesRequested, "fix x")];
        let mut p_next = p_prev.clone();
        // r1 EDITED (same id, new body) must NOT re-fire; r2 is new.
        p_next.reviews = vec![
            review("r1", ReviewVerdict::ChangesRequested, "fix x (edited)"),
            review("r2", ReviewVerdict::ChangesRequested, "fix y"),
        ];
        let events = diff(&snap(vec![], vec![p_prev]), &snap(vec![], vec![p_next]));
        assert_eq!(events.len(), 1);
        let ForgeEvent::ReviewSubmitted { pr, review } = &events[0] else {
            panic!()
        };
        assert_eq!(*pr, PrId(7));
        assert_eq!(review.id, ReviewId("r2".into()));
    }

    #[test]
    fn repeated_changes_requested_rounds_are_distinct_events() {
        // Same reviewer, new round = new forge-native id = new event.
        let mut p1 = pr(7);
        p1.reviews = vec![review("r1", ReviewVerdict::ChangesRequested, "round 1")];
        let mut p2 = p1.clone();
        p2.reviews
            .push(review("r9", ReviewVerdict::ChangesRequested, "round 2"));
        let events = diff(&snap(vec![], vec![p1]), &snap(vec![], vec![p2]));
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn ci_transition_fires_new_pr_ci_does_not() {
        let mut prev_pr = pr(7);
        prev_pr.ci = CiState::Pending;
        let mut next_pr = pr(7);
        next_pr.ci = CiState::Failure;
        let events = diff(
            &snap(vec![], vec![prev_pr]),
            &snap(vec![], vec![next_pr.clone()]),
        );
        assert_eq!(
            events,
            vec![ForgeEvent::CiChanged {
                pr: PrId(7),
                state: CiState::Failure
            }]
        );
        // brand-new PR with CI state: no CiChanged
        assert!(
            diff(&snap(vec![], vec![]), &snap(vec![], vec![next_pr]))
                .iter()
                .all(|e| !matches!(e, ForgeEvent::CiChanged { .. }))
        );
    }

    #[test]
    fn merged_pr_emits_only_pr_merged_never_pr_closed() {
        let prev_pr = pr(7);
        let mut next_pr = pr(7);
        next_pr.merged = true;
        next_pr.closed = true; // forges mark merged PRs closed
        next_pr.merge_sha = Some("cafe42".into());
        let events = diff(&snap(vec![], vec![prev_pr]), &snap(vec![], vec![next_pr]));
        assert_eq!(
            events,
            vec![ForgeEvent::PrMerged {
                pr: PrId(7),
                merge_sha: "cafe42".into()
            }]
        );
    }

    #[test]
    fn closed_without_merge_emits_pr_closed() {
        let prev_pr = pr(7);
        let mut next_pr = pr(7);
        next_pr.closed = true;
        let events = diff(&snap(vec![], vec![prev_pr]), &snap(vec![], vec![next_pr]));
        assert_eq!(events, vec![ForgeEvent::PrClosed { pr: PrId(7) }]);
    }

    #[test]
    fn already_terminal_prs_do_not_refire() {
        let mut p = pr(7);
        p.merged = true;
        p.closed = true;
        p.merge_sha = Some("cafe42".into());
        assert!(diff(&snap(vec![], vec![p.clone()]), &snap(vec![], vec![p])).is_empty());
    }

    // -- fresh-cursor / full-replay cases --

    /// Lost/fresh-cursor case: cursor file gone → first tick sees everything as
    /// new. A PR absent from `prev` that already shows merged=true (and a
    /// merge_sha) MUST fire exactly one PrMerged — it must not be swallowed.
    #[test]
    fn pr_absent_from_prev_appearing_merged_fires_pr_merged() {
        let mut next_pr = pr(7);
        next_pr.merged = true;
        next_pr.closed = true;
        next_pr.merge_sha = Some("abc123".into());
        let events = diff(&snap(vec![], vec![]), &snap(vec![], vec![next_pr]));
        assert_eq!(
            events,
            vec![ForgeEvent::PrMerged {
                pr: PrId(7),
                merge_sha: "abc123".into()
            }]
        );
    }

    /// PR-side analog of `new_issue_fires_all_its_labels`: a PR absent from
    /// `prev` fires `ReviewSubmitted` for ALL reviews it carries in `next`.
    #[test]
    fn pr_absent_from_prev_fires_all_its_reviews() {
        let mut next_pr = pr(7);
        next_pr.reviews = vec![
            review("r1", ReviewVerdict::ChangesRequested, "first pass"),
            review("r2", ReviewVerdict::Approved, "lgtm"),
        ];
        let events = diff(&snap(vec![], vec![]), &snap(vec![], vec![next_pr]));
        assert_eq!(events.len(), 2);
        let ids: Vec<&ReviewId> = events
            .iter()
            .map(|e| {
                let ForgeEvent::ReviewSubmitted { review, .. } = e else {
                    panic!("expected ReviewSubmitted, got {e:?}")
                };
                &review.id
            })
            .collect();
        assert_eq!(ids, vec![&ReviewId("r1".into()), &ReviewId("r2".into())]);
    }

    /// Deterministic event ordering: issues (in `next` order) first, then per
    /// PR in `next` order — within each PR: ReviewSubmitted (snapshot order),
    /// CiChanged, then PrMerged/PrClosed.
    #[test]
    fn event_order_is_deterministic_issues_then_per_pr() {
        // Issue 1 gets a new label.
        let prev_issue = issue(1, &["adr:ADR-0003"]);
        let next_issue = issue(1, &["adr:ADR-0003", "conduit:run"]);

        // PR 10: two new reviews + CI transition — appears in both prev/next.
        let mut prev_pr10 = pr(10);
        prev_pr10.ci = CiState::Pending;
        let mut next_pr10 = pr(10);
        next_pr10.ci = CiState::Success;
        next_pr10.reviews = vec![
            review("r1", ReviewVerdict::ChangesRequested, "nit"),
            review("r2", ReviewVerdict::Approved, "lgtm"),
        ];

        // PR 20: absent from prev, appears merged — fires PrMerged.
        let mut next_pr20 = pr(20);
        next_pr20.merged = true;
        next_pr20.closed = true;
        next_pr20.merge_sha = Some("deadbeef".into());

        let prev = snap(vec![prev_issue], vec![prev_pr10]);
        let next = snap(vec![next_issue], vec![next_pr10, next_pr20]);
        let events = diff(&prev, &next);

        assert_eq!(
            events,
            vec![
                // issues first
                ForgeEvent::IssueLabeled {
                    issue: IssueId(1),
                    label: "conduit:run".into()
                },
                // PR 10: reviews in snapshot order, then CiChanged
                ForgeEvent::ReviewSubmitted {
                    pr: PrId(10),
                    review: review("r1", ReviewVerdict::ChangesRequested, "nit"),
                },
                ForgeEvent::ReviewSubmitted {
                    pr: PrId(10),
                    review: review("r2", ReviewVerdict::Approved, "lgtm"),
                },
                ForgeEvent::CiChanged {
                    pr: PrId(10),
                    state: CiState::Success
                },
                // PR 20: PrMerged
                ForgeEvent::PrMerged {
                    pr: PrId(20),
                    merge_sha: "deadbeef".into()
                },
            ]
        );
    }

    // -- rest_call: status classification through an injected fake transport --

    /// Always answers with one canned response; never touches the network.
    struct FakeTransport {
        status: u16,
        body: Vec<u8>,
    }

    impl HttpTransport for FakeTransport {
        fn request(
            &self,
            _method: &str,
            _url: &str,
            _headers: &[(&str, &str)],
            _body: Option<&[u8]>,
        ) -> Result<HttpResponse, ForgeError> {
            Ok(HttpResponse {
                status: self.status,
                body: self.body.clone(),
            })
        }
    }

    fn call(status: u16, body: &[u8]) -> Result<serde_json::Value, ForgeError> {
        let t = FakeTransport {
            status,
            body: body.to_vec(),
        };
        rest_call(&t, "GET", "http://x/api", &[], None, "testforge")
    }

    #[test]
    fn rest_call_parses_2xx_json_body() {
        let v = call(200, br#"{"ok": true}"#).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true));
    }

    #[test]
    fn rest_call_empty_2xx_body_is_null() {
        assert_eq!(call(204, b"").unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn rest_call_401_and_403_are_auth_with_json_message() {
        for status in [401u16, 403] {
            let err = call(status, br#"{"message": "bad token"}"#).unwrap_err();
            let ForgeError::Auth(msg) = err else {
                panic!("expected Auth, got {err:?}")
            };
            assert_eq!(msg, "bad token");
        }
    }

    #[test]
    fn rest_call_other_non_2xx_is_api_with_lossy_body_fallback() {
        let err = call(500, b"plain text oops").unwrap_err();
        let ForgeError::Api { status, message } = err else {
            panic!("expected Api, got {err:?}")
        };
        assert_eq!(status, 500);
        assert_eq!(message, "plain text oops");
    }

    #[test]
    fn rest_call_unparseable_2xx_body_is_api_error_naming_the_provider() {
        let err = call(200, b"<html>not json</html>").unwrap_err();
        let ForgeError::Api { status, message } = err else {
            panic!("expected Api, got {err:?}")
        };
        assert_eq!(status, 200);
        assert!(message.contains("testforge"), "message: {message}");
    }
}
