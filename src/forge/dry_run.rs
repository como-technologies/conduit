//! DryRunForge: reads delegate to the inner forge; mutations are serialized to
//! a transcript (JSONL) in normalized form and NEVER executed
//! (spec §Transcript-diff semantics). Synthesized ids keep callers working.
//!
//! Normalization rules (the demo's forge-neutrality money shot — both
//! transcript legs must serialize identically):
//! - Forge-assigned ids → `$ISSUE_1`/`$PR_1`… placeholders in first-seen
//!   order. Synthesized ids from mutations AND ids passed back in by the
//!   caller map through the same table; an id never seen before through a
//!   mutation gets the next placeholder.
//! - Timestamps and durations: omitted entirely.
//! - Effort label VALUES redacted: any label matching `effort:*` →
//!   `effort:$REDACTED` (transcript-only; real PRs always carry the real
//!   label — it derives from wall-clock, which would break the diff).
//! - Repo slug → `$REPO` (replace `{owner}/{repo}` occurrences in bodies).
//! - Line shape: `{"action":"<kind>", ...}` with stable key order
//!   (serde_json's default `Map` is a BTreeMap, so keys serialize sorted).
//!
//! Probe overlay: `find_open_pr_by_head` consults the recorded open PRs
//! BEFORE delegating to live reads — the open_pr → probe replay round-trip
//! must keep working even though the PR never reached the real forge. The
//! issue-marker probe deliberately has NO overlay (the conformance suite's
//! DryRun mode skips that read-back; spec §Risks: dry-run proves the stream,
//! not GitHub's acceptance).

use std::sync::Mutex;

use serde_json::{Value, json};

use crate::task::{IssueId, PrId};

use super::{Forge, ForgeError, LabelSpec, NewIssue, PrDraft, RepoSnapshot};

/// Synthesized ids start above any plausible real forge number so an id
/// handed out here can never collide with one the caller read live and
/// passed back in (both route through the same placeholder table either way,
/// but a collision would silently merge two distinct objects).
const SYNTHETIC_ID_BASE: u64 = 9_000_000_000;

struct DryRunState {
    /// Normalized transcript, one compact JSON object per line.
    lines: Vec<String>,
    /// Issue ids in first-seen order: index i ⇒ placeholder `$ISSUE_{i+1}`.
    issue_ids: Vec<u64>,
    /// PR ids in first-seen order: index i ⇒ placeholder `$PR_{i+1}`.
    pr_ids: Vec<u64>,
    /// Probe overlay: (head branch, synthetic id) per recorded open_pr.
    open_prs: Vec<(String, PrId)>,
    next_issue: u64,
    next_pr: u64,
}

pub struct DryRunForge<F: Forge> {
    inner: F,
    /// `{owner}/{repo}` to rewrite as `$REPO` in bodies (None = no rewrite).
    repo_slug: Option<String>,
    state: Mutex<DryRunState>,
}

impl<F: Forge> DryRunForge<F> {
    pub fn new(inner: F) -> DryRunForge<F> {
        DryRunForge {
            inner,
            repo_slug: None,
            state: Mutex::new(DryRunState {
                lines: Vec::new(),
                issue_ids: Vec::new(),
                pr_ids: Vec::new(),
                open_prs: Vec::new(),
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
        let mut d = DryRunForge::new(inner);
        d.repo_slug = Some(slug.to_string());
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

    /// Serialize and append one transcript line. serde_json's default map is
    /// a BTreeMap, so keys come out alphabetically sorted — stable key order
    /// without any extra bookkeeping.
    fn record(&self, state: &mut DryRunState, line: Value) {
        state.lines.push(line.to_string());
    }

    /// `$REPO`-redact a free-text field (spec: repo slug never appears in a
    /// transcript — the two demo legs target different repos).
    ///
    /// This is a LITERAL substring replace, not word-boundary aware: a body
    /// naming `owner/repo-fork` with slug `owner/repo` becomes `$REPO-fork`.
    /// Acceptable for the spike's generated bodies; don't pass slugs that
    /// prefix other paths the caller wants preserved.
    fn redact_text(&self, text: &str) -> String {
        match &self.repo_slug {
            Some(slug) => text.replace(slug.as_str(), "$REPO"),
            None => text.to_string(),
        }
    }
}

/// `effort:*` label VALUES are redacted — they derive from wall-clock, which
/// must never make two otherwise-identical transcripts differ.
fn redact_label(label: &str) -> String {
    if label.starts_with("effort:") {
        "effort:$REDACTED".to_string()
    } else {
        label.to_string()
    }
}

fn redact_labels(labels: &[String]) -> Vec<String> {
    labels.iter().map(|l| redact_label(l)).collect()
}

/// Placeholder for `id` in `seen` first-seen order; an id never seen before
/// through a mutation gets the next placeholder (same table for synthesized
/// ids and ids the caller passed back in).
fn placeholder(seen: &mut Vec<u64>, prefix: &str, id: u64) -> String {
    let index = seen.iter().position(|&s| s == id).unwrap_or_else(|| {
        seen.push(id);
        seen.len() - 1
    });
    format!("${prefix}_{}", index + 1)
}

impl<F: Forge> Forge for DryRunForge<F> {
    // -- reads: delegate to the inner forge untouched ----------------------

    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn git_remote_url(&self) -> Result<String, ForgeError> {
        self.inner.git_remote_url()
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

    // -- mutations: recorded, never executed --------------------------------

    fn ensure_labels(&self, labels: &[LabelSpec]) -> Result<(), ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        let specs: Vec<Value> = labels
            .iter()
            .map(|l| {
                json!({
                    "color": l.color,
                    "description": l.description,
                    "name": redact_label(&l.name),
                })
            })
            .collect();
        self.record(
            &mut state,
            json!({"action": "ensure_labels", "labels": specs}),
        );
        Ok(())
    }

    fn create_issue(&self, new: &NewIssue) -> Result<IssueId, ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        let id = state.next_issue;
        state.next_issue += 1;
        // Register the synthetic id now: first-seen order is mutation order.
        placeholder(&mut state.issue_ids, "ISSUE", id);
        let line = json!({
            "action": "create_issue",
            "body": self.redact_text(&new.body),
            "labels": redact_labels(&new.labels),
            "title": new.title,
        });
        self.record(&mut state, line);
        Ok(IssueId(id))
    }

    fn upsert_issue_comment(
        &self,
        id: &IssueId,
        marker: &str,
        body: &str,
    ) -> Result<(), ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        let issue = placeholder(&mut state.issue_ids, "ISSUE", id.0);
        let line = json!({
            "action": "upsert_issue_comment",
            "body": self.redact_text(body),
            "issue": issue,
            "marker": marker,
        });
        self.record(&mut state, line);
        Ok(())
    }

    fn set_issue_labels(&self, id: &IssueId, labels: &[String]) -> Result<(), ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        let issue = placeholder(&mut state.issue_ids, "ISSUE", id.0);
        let line = json!({
            "action": "set_issue_labels",
            "issue": issue,
            "labels": redact_labels(labels),
        });
        self.record(&mut state, line);
        Ok(())
    }

    fn close_issue(&self, id: &IssueId) -> Result<(), ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        let issue = placeholder(&mut state.issue_ids, "ISSUE", id.0);
        self.record(&mut state, json!({"action": "close_issue", "issue": issue}));
        Ok(())
    }

    fn open_pr(&self, draft: &PrDraft) -> Result<PrId, ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        let id = state.next_pr;
        state.next_pr += 1;
        placeholder(&mut state.pr_ids, "PR", id);
        state.open_prs.push((draft.head.clone(), PrId(id)));
        let line = json!({
            "action": "open_pr",
            "base": draft.base,
            "body": self.redact_text(&draft.body),
            "head": draft.head,
            "labels": redact_labels(&draft.labels),
            "title": draft.title,
        });
        self.record(&mut state, line);
        Ok(PrId(id))
    }

    fn upsert_pr_comment(&self, id: &PrId, marker: &str, body: &str) -> Result<(), ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        let pr = placeholder(&mut state.pr_ids, "PR", id.0);
        let line = json!({
            "action": "upsert_pr_comment",
            "body": self.redact_text(body),
            "marker": marker,
            "pr": pr,
        });
        self.record(&mut state, line);
        Ok(())
    }

    fn set_pr_labels(&self, id: &PrId, labels: &[String]) -> Result<(), ForgeError> {
        let mut state = self.state.lock().expect("DryRunForge lock");
        let pr = placeholder(&mut state.pr_ids, "PR", id.0);
        let line = json!({
            "action": "set_pr_labels",
            "labels": redact_labels(labels),
            "pr": pr,
        });
        self.record(&mut state, line);
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
