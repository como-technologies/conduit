//! DryRunForge: reads delegate to the inner forge; mutations are serialized to
//! a transcript (JSONL) in normalized form and NEVER executed
//! (spec §Transcript-diff semantics). Synthesized ids keep callers working.
//!
//! The normalization itself (id placeholders, effort/slug redaction, line
//! shape) lives in [`crate::transcript`] — defined ONCE, shared with the
//! demo-transcript emitter so the two legs of the forge-neutrality diff can
//! never drift apart. See `transcript::normalize_action` for the rules.
//!
//! Probe overlay: `find_open_pr_by_head` consults the recorded open PRs
//! BEFORE delegating to live reads — the open_pr → probe replay round-trip
//! must keep working even though the PR never reached the real forge. The
//! issue-marker probe deliberately has NO overlay (the conformance suite's
//! DryRun mode skips that read-back; spec §Risks: dry-run proves the stream,
//! not GitHub's acceptance).

use std::sync::Mutex;

use crate::task::{IssueId, PrId};
use crate::transcript::{ForgeCall, Redactor, normalize_action};

use super::{Forge, ForgeError, LabelSpec, NewIssue, PrDraft, RepoSnapshot};

/// Synthesized ids start above any plausible real forge number so an id
/// handed out here can never collide with one the caller read live and
/// passed back in (both route through the same placeholder table either way,
/// but a collision would silently merge two distinct objects).
const SYNTHETIC_ID_BASE: u64 = 9_000_000_000;

struct DryRunState {
    /// Normalized transcript, one compact JSON object per line.
    lines: Vec<String>,
    /// Shared normalization state (id placeholder tables + slug redaction).
    redactor: Redactor,
    /// Probe overlay: (head branch, synthetic id) per recorded open_pr.
    open_prs: Vec<(String, PrId)>,
    /// Label overlay (ADR-0007 convergence probes): the label state of
    /// objects this dry run created/labeled — the live forge never saw them,
    /// so the read must resolve here, exactly like the open-PR probe.
    issue_labels: Vec<(IssueId, Vec<String>)>,
    pr_labels: Vec<(PrId, Vec<String>)>,
    next_issue: u64,
    next_pr: u64,
}

impl DryRunState {
    /// Normalize one mutation through the SHARED rules and append its line.
    fn record(&mut self, call: &ForgeCall<'_>) {
        let line = normalize_action(&mut self.redactor, call).to_string();
        self.lines.push(line);
    }
}

pub struct DryRunForge<F: Forge> {
    inner: F,
    state: Mutex<DryRunState>,
}

impl<F: Forge> DryRunForge<F> {
    pub fn new(inner: F) -> DryRunForge<F> {
        DryRunForge {
            inner,
            state: Mutex::new(DryRunState {
                lines: Vec::new(),
                redactor: Redactor::new(None),
                open_prs: Vec::new(),
                issue_labels: Vec::new(),
                pr_labels: Vec::new(),
                next_issue: SYNTHETIC_ID_BASE + 1,
                next_pr: SYNTHETIC_ID_BASE + 1,
            }),
        }
    }

    /// Like [`DryRunForge::new`], additionally redacting the repo slug
    /// (`{owner}/{repo}` → `$REPO`) wherever it appears in recorded bodies.
    ///
    /// Scope of the guarantee: slug redaction applies to BODY/comment-body
    /// fields only. Titles, heads, and label descriptions pass through
    /// verbatim — transcript neutrality there rests on callers building them
    /// via `contract::*` (which never embeds a repo slug).
    pub fn with_repo_slug(inner: F, slug: &str) -> DryRunForge<F> {
        let d = DryRunForge::new(inner);
        d.state.lock().expect("DryRunForge lock").redactor = Redactor::new(Some(slug.to_string()));
        d
    }

    /// The normalized transcript so far, one JSON object per line.
    pub fn transcript(&self) -> Vec<String> {
        self.state.lock().expect("DryRunForge lock").lines.clone()
    }

    #[cfg(test)]
    pub(crate) fn inner_ref_for_tests(&self) -> &F {
        &self.inner
    }
}

impl<F: Forge> Forge for DryRunForge<F> {
    // -- reads: delegate to the inner forge untouched ----------------------

    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn git_remote_url(&self) -> Result<String, ForgeError> {
        self.inner.git_remote_url()
    }

    fn git_auth(&self) -> Result<Option<crate::git::GitAuth>, ForgeError> {
        self.inner.git_auth()
    }

    fn fetch_snapshot(&self) -> Result<RepoSnapshot, ForgeError> {
        self.inner.fetch_snapshot()
    }

    /// Overlay first: a head recorded via `open_pr` resolves to its synthetic
    /// id (the replay probe must see the PR this dry run "opened"), then the
    /// live read.
    fn find_open_pr_by_head(&self, branch: &str) -> Result<Option<PrId>, ForgeError> {
        {
            let state = self.state.lock().expect("DryRunForge lock");
            if let Some((_, id)) = state.open_prs.iter().find(|(head, _)| head == branch) {
                return Ok(Some(*id));
            }
        }
        self.inner.find_open_pr_by_head(branch)
    }

    fn find_issue_by_marker(&self, marker: &str) -> Result<Option<IssueId>, ForgeError> {
        self.inner.find_issue_by_marker(marker)
    }

    /// Overlay first (ADR-0007): labels of objects this dry run
    /// created/labeled resolve from the recorded state; unrecorded ids
    /// delegate to the live read.
    fn get_issue_labels(&self, id: &IssueId) -> Result<Vec<String>, ForgeError> {
        {
            let state = self.state.lock().expect("DryRunForge lock");
            if let Some((_, labels)) = state.issue_labels.iter().find(|(iid, _)| iid == id) {
                return Ok(labels.clone());
            }
        }
        self.inner.get_issue_labels(id)
    }

    fn get_pr_labels(&self, id: &PrId) -> Result<Vec<String>, ForgeError> {
        {
            let state = self.state.lock().expect("DryRunForge lock");
            if let Some((_, labels)) = state.pr_labels.iter().find(|(pid, _)| pid == id) {
                return Ok(labels.clone());
            }
        }
        self.inner.get_pr_labels(id)
    }

    // -- mutations: recorded, never executed --------------------------------

    fn ensure_labels(&self, labels: &[LabelSpec]) -> Result<(), ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        state.record(&ForgeCall::EnsureLabels { labels });
        Ok(())
    }

    fn create_issue(&self, new: &NewIssue) -> Result<IssueId, ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        let id = IssueId(state.next_issue);
        state.next_issue += 1;
        state.issue_labels.push((id, new.labels.clone()));
        // normalize_action registers the synthetic id: first-seen order is
        // mutation order.
        state.record(&ForgeCall::CreateIssue { new, id });
        Ok(id)
    }

    fn upsert_issue_comment(
        &self,
        id: &IssueId,
        marker: &str,
        body: &str,
    ) -> Result<(), ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        state.record(&ForgeCall::UpsertIssueComment {
            id: *id,
            marker,
            body,
        });
        Ok(())
    }

    fn set_issue_labels(&self, id: &IssueId, labels: &[String]) -> Result<(), ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        match state.issue_labels.iter_mut().find(|(iid, _)| iid == id) {
            Some((_, stored)) => *stored = labels.to_vec(),
            None => state.issue_labels.push((*id, labels.to_vec())),
        }
        state.record(&ForgeCall::SetIssueLabels { id: *id, labels });
        Ok(())
    }

    fn close_issue(&self, id: &IssueId) -> Result<(), ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        state.record(&ForgeCall::CloseIssue { id: *id });
        Ok(())
    }

    fn open_pr(&self, draft: &PrDraft) -> Result<PrId, ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        let id = PrId(state.next_pr);
        state.next_pr += 1;
        state.open_prs.push((draft.head.clone(), id));
        state.pr_labels.push((id, draft.labels.clone()));
        state.record(&ForgeCall::OpenPr { draft, id });
        Ok(id)
    }

    fn upsert_pr_comment(&self, id: &PrId, marker: &str, body: &str) -> Result<(), ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        state.record(&ForgeCall::UpsertPrComment {
            id: *id,
            marker,
            body,
        });
        Ok(())
    }

    fn set_pr_labels(&self, id: &PrId, labels: &[String]) -> Result<(), ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        match state.pr_labels.iter_mut().find(|(pid, _)| pid == id) {
            Some((_, stored)) => *stored = labels.to_vec(),
            None => state.pr_labels.push((*id, labels.to_vec())),
        }
        state.record(&ForgeCall::SetPrLabels { id: *id, labels });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::fake::FakeForge;
    use crate::forge::{Forge, NewIssue, PrDraft};

    fn dry() -> DryRunForge<FakeForge> {
        DryRunForge::new(FakeForge::new())
    }

    #[test]
    fn mutations_never_reach_the_inner_forge() {
        let d = dry();
        let issue = d
            .create_issue(&NewIssue {
                title: "t".into(),
                body: "b".into(),
                labels: vec![],
            })
            .unwrap();
        d.close_issue(&issue).unwrap();
        assert!(
            d.inner_ref_for_tests().actions().is_empty(),
            "DryRun must record, not execute"
        );
    }

    #[test]
    fn ids_become_placeholders_in_first_seen_order() {
        let d = dry();
        let i1 = d
            .create_issue(&NewIssue {
                title: "a".into(),
                body: "".into(),
                labels: vec![],
            })
            .unwrap();
        let p1 = d
            .open_pr(&PrDraft {
                title: "p".into(),
                body: "".into(),
                head: "conduit/adr-0003/x".into(),
                base: "main".into(),
                labels: vec![],
            })
            .unwrap();
        d.close_issue(&i1).unwrap();
        d.set_pr_labels(&p1, &["adr:ADR-0003".into()]).unwrap();
        let t = d.transcript();
        assert!(t[2].contains("\"$ISSUE_1\""));
        assert!(t[3].contains("\"$PR_1\""));
    }

    #[test]
    fn effort_label_value_is_redacted() {
        let d = dry();
        let p = d
            .open_pr(&PrDraft {
                title: "p".into(),
                body: "".into(),
                head: "conduit/adr-0003/x".into(),
                base: "main".into(),
                labels: vec![],
            })
            .unwrap();
        d.set_pr_labels(&p, &["effort:3-average".into(), "adr:ADR-0003".into()])
            .unwrap();
        let line = d.transcript().pop().unwrap();
        assert!(line.contains("effort:$REDACTED"));
        assert!(!line.contains("3-average"));
        assert!(
            line.contains("adr:ADR-0003"),
            "non-effort labels stay verbatim"
        );
    }

    #[test]
    fn transcript_lines_are_valid_json_with_action_key() {
        let d = dry();
        d.create_issue(&NewIssue {
            title: "t".into(),
            body: "x".into(),
            labels: vec![],
        })
        .unwrap();
        for line in d.transcript() {
            let v: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert!(v.get("action").is_some());
        }
    }

    #[test]
    fn no_timestamps_in_transcript() {
        let d = dry();
        d.create_issue(&NewIssue {
            title: "t".into(),
            body: "x".into(),
            labels: vec![],
        })
        .unwrap();
        for line in d.transcript() {
            assert!(
                !line.contains("_at\""),
                "timestamps must be omitted: {line}"
            );
        }
    }

    #[test]
    fn caller_supplied_id_never_seen_before_gets_next_placeholder() {
        // An id obtained from a live read (not synthesized here) routes
        // through the same first-seen table.
        let d = dry();
        let i1 = d
            .create_issue(&NewIssue {
                title: "a".into(),
                body: "".into(),
                labels: vec![],
            })
            .unwrap();
        d.close_issue(&crate::task::IssueId(42)).unwrap(); // from a live read
        d.close_issue(&i1).unwrap();
        let t = d.transcript();
        assert!(t[1].contains("\"$ISSUE_2\""), "unseen id gets next slot");
        assert!(t[2].contains("\"$ISSUE_1\""), "synthesized id keeps slot 1");
    }

    #[test]
    fn find_open_pr_by_head_consults_the_overlay_before_live_reads() {
        let d = dry();
        let p1 = d
            .open_pr(&PrDraft {
                title: "p".into(),
                body: "".into(),
                head: "conduit/adr-0003/x".into(),
                base: "main".into(),
                labels: vec![],
            })
            .unwrap();
        // The recorded PR resolves through the overlay (the inner forge never
        // saw it)…
        assert_eq!(
            d.find_open_pr_by_head("conduit/adr-0003/x").unwrap(),
            Some(p1)
        );
        // …and an unrecorded head falls through to the inner read.
        assert_eq!(
            d.find_open_pr_by_head("conduit/none/missing").unwrap(),
            None
        );
    }

    /// ADR-0007 convergence probes on the record-only leg: label reads
    /// resolve through the overlay for objects this dry run created/labeled
    /// (the live forge never saw them); unrecorded ids delegate to the inner
    /// forge.
    #[test]
    fn label_reads_consult_the_overlay_for_recorded_objects() {
        let d = dry();
        let issue = d
            .create_issue(&NewIssue {
                title: "t".into(),
                body: "".into(),
                labels: vec!["adr:ADR-0001".into()],
            })
            .unwrap();
        assert_eq!(
            d.get_issue_labels(&issue).unwrap(),
            vec!["adr:ADR-0001".to_string()]
        );
        d.set_issue_labels(&issue, &["conformance:x".into(), "conduit:run".into()])
            .unwrap();
        assert_eq!(
            d.get_issue_labels(&issue).unwrap(),
            vec!["conformance:x".to_string(), "conduit:run".to_string()]
        );
        let pr = d
            .open_pr(&PrDraft {
                title: "p".into(),
                body: "".into(),
                head: "conduit/adr-0001/x".into(),
                base: "main".into(),
                labels: vec!["effort:1-super-quick".into()],
            })
            .unwrap();
        assert_eq!(
            d.get_pr_labels(&pr).unwrap(),
            vec!["effort:1-super-quick".to_string()]
        );
        d.set_pr_labels(&pr, &["effort:2-not-long".into()]).unwrap();
        assert_eq!(
            d.get_pr_labels(&pr).unwrap(),
            vec!["effort:2-not-long".to_string()]
        );
        // Unrecorded ids fall through to the inner forge (FakeForge: 404).
        assert!(d.get_issue_labels(&crate::task::IssueId(424_242)).is_err());
    }

    #[test]
    fn repo_slug_in_bodies_becomes_repo_placeholder() {
        let d = DryRunForge::with_repo_slug(FakeForge::new(), "octo/example");
        d.create_issue(&NewIssue {
            title: "t".into(),
            body: "see https://github.com/octo/example/pull/1".into(),
            labels: vec![],
        })
        .unwrap();
        let line = d.transcript().pop().unwrap();
        assert!(line.contains("$REPO"), "slug must be redacted: {line}");
        assert!(!line.contains("octo/example"));
    }

    #[test]
    fn ensure_labels_records_specs_with_effort_names_redacted() {
        let d = dry();
        d.ensure_labels(&[
            crate::forge::LabelSpec {
                name: "effort:1-super-quick".into(),
                color: "00aa00".into(),
                description: "quick".into(),
            },
            crate::forge::LabelSpec {
                name: "conduit:run".into(),
                color: "1d76db".into(),
                description: "trigger".into(),
            },
        ])
        .unwrap();
        let line = d.transcript().pop().unwrap();
        assert!(line.contains("effort:$REDACTED"));
        assert!(!line.contains("1-super-quick"));
        assert!(line.contains("conduit:run"));
    }

    #[test]
    fn reads_delegate_to_the_inner_forge() {
        let d = dry();
        // FakeForge derives an empty snapshot from empty state — proving the
        // call reached it.
        let snap = d.fetch_snapshot().unwrap();
        assert!(snap.issues.is_empty() && snap.prs.is_empty());
        assert_eq!(d.describe(), "fake");
        assert_eq!(d.git_remote_url().unwrap(), "/dev/null/fake.git");
        assert_eq!(d.find_issue_by_marker("<!-- x -->").unwrap(), None);
    }
}
