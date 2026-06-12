//! In-memory Forge: scripted snapshot sequences + action recording
//! (spec §Implementations). Interior mutability via Mutex (trait takes &self).
//!
//! FakeForge is the third full Forge implementation (alongside Gitea and
//! GitHub). It is used by machine/e2e tests and the default demo path.
//!
//! Snapshot derivation (no script): when `scripted` is empty and no `last`
//! snapshot exists, `fetch_snapshot` derives a snapshot from the stored
//! in-memory issues and PRs, applying the same normalization filter as a real
//! adapter: only issues with a `conduit:`/`adr:`-prefixed label appear; only
//! PRs whose head_branch starts with `"conduit/"` appear.

use std::collections::VecDeque;
use std::sync::Mutex;
use time::OffsetDateTime;

use crate::task::{IssueId, PrId};

use super::{
    Forge, ForgeError, IssueSnapshot, LabelSpec, NewIssue, PrDraft, PrSnapshot, RepoSnapshot,
};

// ---------------------------------------------------------------------------
// Recorded actions
// ---------------------------------------------------------------------------

/// Every mutation an adapter performed, for assertions.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordedAction {
    EnsureLabels(Vec<LabelSpec>),
    CreateIssue(NewIssue),
    UpsertIssueComment {
        id: IssueId,
        marker: String,
        body: String,
    },
    SetIssueLabels {
        id: IssueId,
        labels: Vec<String>,
    },
    CloseIssue(IssueId),
    OpenPr(PrDraft),
    UpsertPrComment {
        id: PrId,
        marker: String,
        body: String,
    },
    SetPrLabels {
        id: PrId,
        labels: Vec<String>,
    },
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

struct FakeState {
    /// Queued scripted fetch results: pop front. When one remains, keep
    /// returning it (stable tail). Empty + no last → derive from stored state.
    scripted: VecDeque<RepoSnapshot>,
    /// Last scripted snapshot returned (enables stable-tail repeat).
    last: Option<RepoSnapshot>,
    /// Ensured labels (unioned by name).
    labels: Vec<LabelSpec>,
    /// Stored issues: (id, issue, closed).
    issues: Vec<(IssueId, NewIssue, bool)>,
    /// Stored issue comments: (issue_id, marker, body).
    issue_comments: Vec<(IssueId, String, String)>,
    /// Stored PRs: (id, draft, open).
    prs: Vec<(PrId, PrDraft, bool)>,
    /// Stored PR comments: (pr_id, marker, body).
    pr_comments: Vec<(PrId, String, String)>,
    /// All recorded actions in call order.
    actions: Vec<RecordedAction>,
    /// Next issue id counter (starts at 1).
    next_issue: u64,
    /// Next PR id counter (starts at 1).
    next_pr: u64,
    /// Configurable git remote URL (default "/dev/null/fake.git").
    remote_url: String,
    /// Single-shot fault injection: the named mutator fails ONCE with a 500,
    /// then the slot clears (Task 12 cursor-ordering test).
    fail_next: Option<String>,
}

impl FakeState {
    fn new() -> FakeState {
        FakeState {
            scripted: VecDeque::new(),
            last: None,
            labels: Vec::new(),
            issues: Vec::new(),
            issue_comments: Vec::new(),
            prs: Vec::new(),
            pr_comments: Vec::new(),
            actions: Vec::new(),
            next_issue: 1,
            next_pr: 1,
            remote_url: "/dev/null/fake.git".into(),
            fail_next: None,
        }
    }

    /// Consume the single-shot fault if it names `method`: error once, then
    /// succeed forever after. Failed calls are NOT recorded — the effect
    /// never happened.
    fn check_fail(&mut self, method: &str) -> Result<(), ForgeError> {
        if self.fail_next.as_deref() == Some(method) {
            self.fail_next = None;
            return Err(ForgeError::Api {
                status: 500,
                message: format!("injected failure: {method}"),
            });
        }
        Ok(())
    }

    /// Derive a normalized snapshot from stored in-memory state.
    ///
    /// This simulates the POST-filter state a correct adapter produces:
    /// - Issues: only those with at least one label starting with `"conduit:"`
    ///   or `"adr:"`. Closed issues are included intentionally — the
    ///   disappearance rule requires terminal items stay visible.
    /// - PRs: only those whose `head` starts with `"conduit/"`. Closed and
    ///   merged PRs are retained with `closed: true` per the disappearance
    ///   rule; this is not a raw `?state=open` API filter.
    fn derive_snapshot(&self) -> RepoSnapshot {
        let issues: Vec<IssueSnapshot> = self
            .issues
            .iter()
            .filter(|(_, new_issue, _)| {
                new_issue
                    .labels
                    .iter()
                    .any(|l| l.starts_with("conduit:") || l.starts_with("adr:"))
            })
            .map(|(id, new_issue, closed)| IssueSnapshot {
                id: *id,
                labels: new_issue.labels.clone(),
                closed: *closed,
            })
            .collect();

        let prs: Vec<PrSnapshot> = self
            .prs
            .iter()
            .filter(|(_, draft, _)| draft.head.starts_with("conduit/"))
            .map(|(id, draft, open)| PrSnapshot {
                id: *id,
                title: draft.title.clone(),
                body: draft.body.clone(),
                head_branch: draft.head.clone(),
                labels: draft.labels.clone(),
                reviews: vec![],
                ci: super::CiState::None,
                merged: false,
                merge_sha: None,
                closed: !open,
            })
            .collect();

        RepoSnapshot {
            issues,
            prs,
            fetched_at: OffsetDateTime::now_utc(),
        }
    }
}

// ---------------------------------------------------------------------------
// FakeForge
// ---------------------------------------------------------------------------

pub struct FakeForge {
    state: Mutex<FakeState>,
}

impl Default for FakeForge {
    fn default() -> Self {
        FakeForge::new()
    }
}

impl FakeForge {
    /// Create a new FakeForge. Issue and PR ids start at 1.
    pub fn new() -> FakeForge {
        FakeForge {
            state: Mutex::new(FakeState::new()),
        }
    }

    /// Queue scripted snapshot results. `fetch_snapshot` pops from the front;
    /// when one snapshot remains it is returned repeatedly (stable tail).
    /// Calling `script` replaces the queue (allowing re-scripting between test
    /// phases).
    pub fn script(&self, snapshots: Vec<RepoSnapshot>) {
        let mut s = self.state.lock().expect("FakeForge lock");
        s.scripted = VecDeque::from(snapshots);
        s.last = None;
    }

    /// Return all recorded actions in call order.
    pub fn actions(&self) -> Vec<RecordedAction> {
        self.state.lock().expect("FakeForge lock").actions.clone()
    }

    /// Count of recorded actions matching `f`.
    pub fn count<F: Fn(&RecordedAction) -> bool>(&self, f: F) -> usize {
        self.state
            .lock()
            .expect("FakeForge lock")
            .actions
            .iter()
            .filter(|a| f(a))
            .count()
    }

    /// Set the URL returned by `git_remote_url`. Defaults to
    /// `"/dev/null/fake.git"`. Task 12's e2e rig points this at a seeded local
    /// bare repo so CommitAndPush works.
    pub fn set_remote_url(&self, url: &str) {
        self.state.lock().expect("FakeForge lock").remote_url = url.to_string();
    }

    /// Arm single-shot fault injection: the NEXT call to the named mutator
    /// (e.g. `"open_pr"`) fails with a 500, then the slot clears. Drives the
    /// cursor-ordering e2e (a failed action must leave the cursor unadvanced).
    ///
    /// Panics immediately if `method` is not one of the known mutator names —
    /// fail fast on typos in test infra rather than silently injecting nothing.
    pub fn fail_next(&self, method: &str) {
        const VALID: &[&str] = &[
            "ensure_labels",
            "create_issue",
            "upsert_issue_comment",
            "set_issue_labels",
            "close_issue",
            "open_pr",
            "upsert_pr_comment",
            "set_pr_labels",
        ];
        assert!(
            VALID.contains(&method),
            "FakeForge::fail_next: unknown mutator {:?}; valid names are: {:?}",
            method,
            VALID,
        );
        self.state.lock().expect("FakeForge lock").fail_next = Some(method.to_string());
    }

    /// Stored comments on one issue as `(marker, body)` pairs — the
    /// exactly-once-effect assertion surface for marker-upsert replay.
    pub fn issue_comments(&self, id: &IssueId) -> Vec<(String, String)> {
        self.state
            .lock()
            .expect("FakeForge lock")
            .issue_comments
            .iter()
            .filter(|(cid, _, _)| cid == id)
            .map(|(_, marker, body)| (marker.clone(), body.clone()))
            .collect()
    }
}

impl Forge for FakeForge {
    fn describe(&self) -> String {
        "fake".into()
    }

    fn git_remote_url(&self) -> Result<String, ForgeError> {
        Ok(self
            .state
            .lock()
            .expect("FakeForge lock")
            .remote_url
            .clone())
    }

    /// Pop the front of the scripted queue. When one snapshot remains, keep
    /// returning it (stable-tail repeat). When the queue is empty and there is
    /// no `last`, derive a snapshot from stored in-memory state.
    fn fetch_snapshot(&self) -> Result<RepoSnapshot, ForgeError> {
        let mut s = self.state.lock().expect("FakeForge lock");
        if s.scripted.len() > 1 {
            let snap = s.scripted.pop_front().unwrap();
            s.last = Some(snap.clone());
            return Ok(snap);
        }
        if s.scripted.len() == 1 {
            // Stable tail: keep it in the queue AND record as last.
            let snap = s.scripted.front().unwrap().clone();
            s.last = Some(snap.clone());
            return Ok(snap);
        }
        // No script queued — use last or derive from stored state.
        if let Some(last) = &s.last {
            return Ok(last.clone());
        }
        Ok(s.derive_snapshot())
    }

    fn find_open_pr_by_head(&self, branch: &str) -> Result<Option<PrId>, ForgeError> {
        let s = self.state.lock().expect("FakeForge lock");
        let found = s
            .prs
            .iter()
            .find(|(_, draft, open)| *open && draft.head == branch)
            .map(|(id, _, _)| *id);
        Ok(found)
    }

    /// Scan stored issue bodies AND issue comments for `marker` as a substring.
    fn find_issue_by_marker(&self, marker: &str) -> Result<Option<IssueId>, ForgeError> {
        let s = self.state.lock().expect("FakeForge lock");
        // Check issue bodies first.
        for (id, new_issue, _closed) in &s.issues {
            if new_issue.body.contains(marker) {
                return Ok(Some(*id));
            }
        }
        // Check issue comments.
        for (issue_id, _comment_marker, body) in &s.issue_comments {
            if body.contains(marker) {
                return Ok(Some(*issue_id));
            }
        }
        Ok(None)
    }

    /// ADR-0007 convergence probe: stored label state, 404 on unknown ids
    /// (real-adapter parity).
    fn get_issue_labels(&self, id: &IssueId) -> Result<Vec<String>, ForgeError> {
        let s = self.state.lock().expect("FakeForge lock");
        s.issues
            .iter()
            .find(|(iid, _, _)| iid == id)
            .map(|(_, new_issue, _)| new_issue.labels.clone())
            .ok_or_else(|| ForgeError::Api {
                status: 404,
                message: format!("issue {} not found", id.0),
            })
    }

    fn get_pr_labels(&self, id: &PrId) -> Result<Vec<String>, ForgeError> {
        let s = self.state.lock().expect("FakeForge lock");
        s.prs
            .iter()
            .find(|(pid, _, _)| pid == id)
            .map(|(_, draft, _)| draft.labels.clone())
            .ok_or_else(|| ForgeError::Api {
                status: 404,
                message: format!("PR {} not found", id.0),
            })
    }

    fn ensure_labels(&self, labels: &[LabelSpec]) -> Result<(), ForgeError> {
        let mut s = self.state.lock().expect("FakeForge lock");
        s.check_fail("ensure_labels")?;
        // Union by name.
        for label in labels {
            if !s.labels.iter().any(|l| l.name == label.name) {
                s.labels.push(label.clone());
            }
        }
        s.actions
            .push(RecordedAction::EnsureLabels(labels.to_vec()));
        Ok(())
    }

    fn create_issue(&self, new: &NewIssue) -> Result<IssueId, ForgeError> {
        let mut s = self.state.lock().expect("FakeForge lock");
        s.check_fail("create_issue")?;
        let id = IssueId(s.next_issue);
        s.next_issue += 1;
        s.issues.push((id, new.clone(), false));
        s.actions.push(RecordedAction::CreateIssue(new.clone()));
        Ok(id)
    }

    /// Replaces an existing comment with the same marker, else appends.
    /// Records the call either way (the recording is of calls; convergence is
    /// asserted on stored state).
    fn upsert_issue_comment(
        &self,
        id: &IssueId,
        marker: &str,
        body: &str,
    ) -> Result<(), ForgeError> {
        let mut s = self.state.lock().expect("FakeForge lock");
        s.check_fail("upsert_issue_comment")?;
        let action = RecordedAction::UpsertIssueComment {
            id: *id,
            marker: marker.to_string(),
            body: body.to_string(),
        };
        // Replace existing comment with the same marker, else append.
        if let Some(entry) = s
            .issue_comments
            .iter_mut()
            .find(|(cid, m, _)| *cid == *id && m == marker)
        {
            entry.2 = body.to_string();
        } else {
            s.issue_comments
                .push((*id, marker.to_string(), body.to_string()));
        }
        s.actions.push(action);
        Ok(())
    }

    /// Stores the absolute label set (convergent).
    fn set_issue_labels(&self, id: &IssueId, labels: &[String]) -> Result<(), ForgeError> {
        let mut s = self.state.lock().expect("FakeForge lock");
        s.check_fail("set_issue_labels")?;
        // Update the stored issue's label list.
        if let Some(entry) = s.issues.iter_mut().find(|(iid, _, _)| *iid == *id) {
            entry.1.labels = labels.to_vec();
        }
        s.actions.push(RecordedAction::SetIssueLabels {
            id: *id,
            labels: labels.to_vec(),
        });
        Ok(())
    }

    fn close_issue(&self, id: &IssueId) -> Result<(), ForgeError> {
        let mut s = self.state.lock().expect("FakeForge lock");
        s.check_fail("close_issue")?;
        let entry = s.issues.iter_mut().find(|(iid, _, _)| *iid == *id);
        match entry {
            Some(e) => {
                e.2 = true;
                s.actions.push(RecordedAction::CloseIssue(*id));
                Ok(())
            }
            None => Err(ForgeError::Api {
                status: 404,
                message: format!("issue {} not found", id.0),
            }),
        }
    }

    fn open_pr(&self, draft: &PrDraft) -> Result<PrId, ForgeError> {
        let mut s = self.state.lock().expect("FakeForge lock");
        s.check_fail("open_pr")?;
        let id = PrId(s.next_pr);
        s.next_pr += 1;
        s.prs.push((id, draft.clone(), true));
        s.actions.push(RecordedAction::OpenPr(draft.clone()));
        Ok(id)
    }

    /// Replaces an existing PR comment with the same marker, else appends.
    fn upsert_pr_comment(&self, id: &PrId, marker: &str, body: &str) -> Result<(), ForgeError> {
        let mut s = self.state.lock().expect("FakeForge lock");
        s.check_fail("upsert_pr_comment")?;
        let action = RecordedAction::UpsertPrComment {
            id: *id,
            marker: marker.to_string(),
            body: body.to_string(),
        };
        if let Some(entry) = s
            .pr_comments
            .iter_mut()
            .find(|(cid, m, _)| *cid == *id && m == marker)
        {
            entry.2 = body.to_string();
        } else {
            s.pr_comments
                .push((*id, marker.to_string(), body.to_string()));
        }
        s.actions.push(action);
        Ok(())
    }

    /// Stores the absolute label set for the PR (convergent).
    fn set_pr_labels(&self, id: &PrId, labels: &[String]) -> Result<(), ForgeError> {
        let mut s = self.state.lock().expect("FakeForge lock");
        s.check_fail("set_pr_labels")?;
        if let Some(entry) = s.prs.iter_mut().find(|(pid, _, _)| *pid == *id) {
            entry.1.labels = labels.to_vec();
        }
        s.actions.push(RecordedAction::SetPrLabels {
            id: *id,
            labels: labels.to_vec(),
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::PrDraft;

    /// derive_snapshot must keep closed/merged PRs with closed=true (the
    /// disappearance rule: terminal items stay visible, not filtered out).
    #[test]
    fn derive_snapshot_retains_closed_and_merged_prs() {
        let forge = FakeForge::new();

        // Open a PR, then close it by setting open=false in stored state.
        let draft = PrDraft {
            head: "conduit/adr-0001/test".into(),
            base: "main".into(),
            title: "test PR".into(),
            body: "body".into(),
            labels: vec![],
        };
        let pr_id = forge.open_pr(&draft).unwrap();

        // Mark it closed directly in state.
        {
            let mut s = forge.state.lock().expect("FakeForge lock");
            if let Some(entry) = s.prs.iter_mut().find(|(id, _, _)| *id == pr_id) {
                entry.2 = false; // open = false → closed
            }
        }

        let snap = forge.fetch_snapshot().unwrap();
        let pr = snap.prs.iter().find(|p| p.id == pr_id);
        assert!(pr.is_some(), "closed PR must appear in derived snapshot");
        assert!(pr.unwrap().closed, "closed PR must have closed=true");
    }

    /// ADR-0007 convergence probes: label reads reflect stored state and
    /// 404 on unknown ids (real-adapter parity).
    #[test]
    fn label_reads_reflect_stored_state_and_404_unknown() {
        let forge = FakeForge::new();
        let issue = forge
            .create_issue(&super::NewIssue {
                title: "t".into(),
                body: "b".into(),
                labels: vec!["adr:ADR-0001".into(), "discuss".into()],
            })
            .unwrap();
        assert_eq!(
            forge.get_issue_labels(&issue).unwrap(),
            vec!["adr:ADR-0001".to_string(), "discuss".to_string()]
        );
        forge
            .set_issue_labels(&issue, &["conduit:failed".into()])
            .unwrap();
        assert_eq!(
            forge.get_issue_labels(&issue).unwrap(),
            vec!["conduit:failed".to_string()]
        );
        let pr = forge
            .open_pr(&PrDraft {
                head: "conduit/adr-0001/x".into(),
                base: "main".into(),
                title: "p".into(),
                body: "".into(),
                labels: vec!["effort:1-super-quick".into()],
            })
            .unwrap();
        assert_eq!(
            forge.get_pr_labels(&pr).unwrap(),
            vec!["effort:1-super-quick".to_string()]
        );
        for err in [
            forge.get_issue_labels(&IssueId(999_999)).unwrap_err(),
            forge.get_pr_labels(&PrId(999_999)).unwrap_err(),
        ] {
            let ForgeError::Api { status, .. } = err else {
                panic!("expected Api 404, got {err:?}");
            };
            assert_eq!(status, 404);
        }
    }

    /// An open PR must appear with closed=false in the derived snapshot.
    #[test]
    fn derive_snapshot_open_pr_has_closed_false() {
        let forge = FakeForge::new();
        let draft = PrDraft {
            head: "conduit/adr-0002/test".into(),
            base: "main".into(),
            title: "open PR".into(),
            body: "body".into(),
            labels: vec![],
        };
        let pr_id = forge.open_pr(&draft).unwrap();
        let snap = forge.fetch_snapshot().unwrap();
        let pr = snap.prs.iter().find(|p| p.id == pr_id).unwrap();
        assert!(!pr.closed, "open PR must have closed=false");
    }
}
