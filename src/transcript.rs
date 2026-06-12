//! Demo-transcript machinery (spec §Transcript-diff semantics) — the
//! forge-neutrality money shot. `conduit demo-transcript` does NOT poll: it
//! feeds a fixture event sequence ([`fixture_events`]) through the REAL
//! [`machine::step`] with FakeEngine and an in-memory record, emitting every
//! resulting forge action through the chosen adapter (live Gitea, or
//! `DryRun(GitHubForge)` which records instead of executing) wrapped in a
//! transcript emitter. Both legs serialize each action with the SAME
//! normalization ([`normalize_action`] + [`Redactor`], shared with
//! `dry_run.rs`), so the two JSONL outputs are byte-identical.
//!
//! Normalization rules (one place, used by both DryRunForge and this module):
//! - Forge-assigned ids → `$ISSUE_1`/`$PR_1`… placeholders in first-seen
//!   order (synthesized DryRun ids and live forge ids route through the same
//!   table — that is WHY the legs diff clean).
//! - Timestamps and durations: omitted entirely.
//! - `effort:*` label VALUES → `effort:$REDACTED` (they derive from
//!   wall-clock; transcript-only — real PRs always carry the real label).
//! - Repo slug → `$REPO` in body fields (the legs target different repos).
//! - Line shape: `{"action":"<kind>", ...}`, keys sorted (serde_json's
//!   default map is a BTreeMap).

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::config::Config;
use crate::contract;
use crate::engine::fake::{FakeEngine, FakeMode};
use crate::engine::{EngineOutcome, TaskSpec, run_timed};
use crate::forge::{Forge, LabelSpec, NewIssue, PrDraft};
use crate::machine::{self, Action, Event, FeedbackOp};
use crate::task::{EngineResult, IssueId, PrId, ReviewVerdict, TaskRecord};

// ---------------------------------------------------------------------------
// Shared normalization — dry_run.rs and the transcript emitter both call this.
// ---------------------------------------------------------------------------

/// Id-placeholder table + slug redaction. First-seen order: the first issue id
/// that passes through becomes `$ISSUE_1`, and so on; synthesized ids and ids
/// passed back in by callers map through the same table.
pub struct Redactor {
    issue_ids: Vec<u64>,
    pr_ids: Vec<u64>,
    /// `{owner}/{repo}` to rewrite as `$REPO` in bodies (None = no rewrite).
    repo_slug: Option<String>,
}

impl Redactor {
    pub fn new(repo_slug: Option<String>) -> Redactor {
        Redactor {
            issue_ids: Vec::new(),
            pr_ids: Vec::new(),
            repo_slug,
        }
    }

    /// `$ISSUE_n` for this id (registering it on first sight).
    pub fn issue(&mut self, id: IssueId) -> String {
        placeholder(&mut self.issue_ids, "ISSUE", id.0)
    }

    /// `$PR_n` for this id (registering it on first sight).
    pub fn pr(&mut self, id: PrId) -> String {
        placeholder(&mut self.pr_ids, "PR", id.0)
    }

    /// `$REPO`-redact a free-text field. LITERAL substring replace, not
    /// word-boundary aware (dry_run.rs precedent — fine for generated bodies).
    pub fn text(&self, text: &str) -> String {
        match &self.repo_slug {
            Some(slug) => text.replace(slug.as_str(), "$REPO"),
            None => text.to_string(),
        }
    }
}

fn placeholder(seen: &mut Vec<u64>, prefix: &str, id: u64) -> String {
    let index = seen.iter().position(|&s| s == id).unwrap_or_else(|| {
        seen.push(id);
        seen.len() - 1
    });
    format!("${prefix}_{}", index + 1)
}

/// `effort:*` label VALUES are redacted — they derive from wall-clock, which
/// must never make two otherwise-identical transcripts differ.
pub fn redact_label(label: &str) -> String {
    if label.starts_with("effort:") {
        "effort:$REDACTED".to_string()
    } else {
        label.to_string()
    }
}

fn redact_labels(labels: &[String]) -> Vec<String> {
    labels.iter().map(|l| redact_label(l)).collect()
}

/// One forge mutation, as data — the vocabulary [`normalize_action`] speaks.
/// `CreateIssue`/`OpenPr` carry the forge-assigned (or synthesized) id so the
/// placeholder table registers it in mutation order, but the id never appears
/// in the line itself.
pub enum ForgeCall<'a> {
    EnsureLabels {
        labels: &'a [LabelSpec],
    },
    CreateIssue {
        new: &'a NewIssue,
        id: IssueId,
    },
    UpsertIssueComment {
        id: IssueId,
        marker: &'a str,
        body: &'a str,
    },
    SetIssueLabels {
        id: IssueId,
        labels: &'a [String],
    },
    CloseIssue {
        id: IssueId,
    },
    OpenPr {
        draft: &'a PrDraft,
        id: PrId,
    },
    UpsertPrComment {
        id: PrId,
        marker: &'a str,
        body: &'a str,
    },
    SetPrLabels {
        id: PrId,
        labels: &'a [String],
    },
}

/// THE normalization: one forge mutation → one stable JSON value. Defined
/// once so DryRunForge's transcript and the demo-transcript emitter cannot
/// drift apart.
pub fn normalize_action(redactor: &mut Redactor, call: &ForgeCall<'_>) -> Value {
    match call {
        ForgeCall::EnsureLabels { labels } => {
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
            json!({"action": "ensure_labels", "labels": specs})
        }
        ForgeCall::CreateIssue { new, id } => {
            // Register the id now: first-seen order is mutation order.
            redactor.issue(*id);
            json!({
                "action": "create_issue",
                "body": redactor.text(&new.body),
                "labels": redact_labels(&new.labels),
                "title": new.title,
            })
        }
        ForgeCall::UpsertIssueComment { id, marker, body } => {
            let issue = redactor.issue(*id);
            json!({
                "action": "upsert_issue_comment",
                "body": redactor.text(body),
                "issue": issue,
                "marker": marker,
            })
        }
        ForgeCall::SetIssueLabels { id, labels } => {
            let issue = redactor.issue(*id);
            json!({
                "action": "set_issue_labels",
                "issue": issue,
                "labels": redact_labels(labels),
            })
        }
        ForgeCall::CloseIssue { id } => {
            let issue = redactor.issue(*id);
            json!({"action": "close_issue", "issue": issue})
        }
        ForgeCall::OpenPr { draft, id } => {
            redactor.pr(*id);
            json!({
                "action": "open_pr",
                "base": draft.base,
                "body": redactor.text(&draft.body),
                "head": draft.head,
                "labels": redact_labels(&draft.labels),
                "title": draft.title,
            })
        }
        ForgeCall::UpsertPrComment { id, marker, body } => {
            let pr = redactor.pr(*id);
            json!({
                "action": "upsert_pr_comment",
                "body": redactor.text(body),
                "marker": marker,
                "pr": pr,
            })
        }
        ForgeCall::SetPrLabels { id, labels } => {
            let pr = redactor.pr(*id);
            json!({
                "action": "set_pr_labels",
                "labels": redact_labels(labels),
                "pr": pr,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// The scripted scenario
// ---------------------------------------------------------------------------

/// Fixture title — fixed so both legs derive the same branch/slug, and so the
/// transcript task can never collide with a real planned task's title.
pub const FIXTURE_TITLE: &str = "Forge neutrality transcript";

/// Fixture merge sha — `PrMerged` is a scripted event (nobody actually merges
/// anything), so the sha is a fixed fake; it lands in the close comment and
/// must be identical on both legs.
pub const FIXTURE_MERGE_SHA: &str = "cafe42cafe42cafe42cafe42cafe42cafe42cafe";

/// Fixture plan markdown — fixed bytes per reference, so the issue body and
/// PR body are identical across legs with no store or adroit dependency.
pub fn fixture_plan(reference: &str) -> String {
    format!(
        "# Plan: forge-neutrality transcript for {reference}\n\n\
         1. Fixture events drive the real state machine (no polling).\n\
         2. Every resulting forge action is emitted through the chosen adapter.\n\
         3. The two normalized streams must be byte-identical.\n"
    )
}

/// The scripted scenario (spec §Transcript-diff semantics): label-trigger →
/// engine completes → ChangesRequested round → engine completes → merge.
/// `EngineFinished` is scripted here — the FakeEngine still runs on the
/// execute leg (RunEngine actions), but its result is discarded in favor of
/// the fixture event so both legs see the exact same event stream.
pub fn fixture_events(reference: &str) -> Vec<Event> {
    vec![
        Event::IssueLabeled {
            label: contract::LABEL_RUN.to_string(),
        },
        Event::EngineFinished(EngineResult::Completed {
            summary: format!("implemented {reference} from the plan snapshot"),
        }),
        Event::ReviewSubmitted {
            verdict: ReviewVerdict::ChangesRequested,
            body: "Please tighten the docs.".to_string(),
        },
        Event::EngineFinished(EngineResult::Completed {
            summary: format!("addressed review feedback for {reference}"),
        }),
        Event::PrMerged {
            merge_sha: FIXTURE_MERGE_SHA.to_string(),
        },
    ]
}

// ---------------------------------------------------------------------------
// The transcript runner
// ---------------------------------------------------------------------------

/// Git plumbing for the EXECUTE leg (live Gitea): the engine really runs in a
/// real workspace and the branch is really pushed, so `open_pr` can succeed
/// on the live forge. The record-only leg (`DryRun(GitHubForge)`) passes
/// `None` — github is a transcript-only demo by construction: no clone, no
/// push, no live probe.
pub struct GitContext {
    pub remote_url: String,
    pub cache_dir: PathBuf,
    /// Workspaces land at `<workspace_root>/<transcript-task-id>`.
    pub workspace_root: PathBuf,
    pub base_branch: String,
}

/// Run the scripted scenario against `forge`, returning the normalized
/// transcript lines (one JSON object per line).
///
/// Both legs run THIS function — the only asymmetry is `git` (engine/git
/// effects on the execute leg) and which adapter sits behind `forge`; every
/// serialized byte comes from the same code path, which is what makes the
/// `diff` in the demo honest.
///
/// Replay behavior on the execute leg: the issue is created fresh each run
/// (no probe — each transcript run is a new demo beat on the throwaway
/// forge), while `open_pr` probes `find_open_pr_by_head` first and ADOPTS a
/// previous run's still-open PR (the scripted `PrMerged` never really merged
/// it) — the open_pr line is emitted either way, so re-runs stay
/// byte-identical with the record-only leg.
pub fn run(
    forge: &dyn Forge,
    repo_slug: Option<String>,
    reference: &str,
    address: &str,
    config: &Config,
    git: Option<&GitContext>,
) -> anyhow::Result<Vec<String>> {
    let plan = fixture_plan(reference);
    let plan_sha = sha256_hex(plan.as_bytes());
    let mut record = TaskRecord::new(reference, address, FIXTURE_TITLE, &plan_sha);
    // Distinct identity from any real planned task for the same ADR: marker
    // and workspace must never collide with the genuine lifecycle's.
    record.id = format!("{}-transcript", record.id);

    let mut emitter = Emitter {
        forge,
        redactor: Redactor::new(repo_slug),
        lines: Vec::new(),
    };

    // The `conduit plan` beat: issue with the plan body + hidden task marker
    // (mirrors Router::ensure_issue's body/label construction).
    let marker = contract::task_marker(&record.id);
    let issue = emitter.create_issue(&NewIssue {
        title: contract::pr_title(reference, FIXTURE_TITLE),
        body: format!("{}\n\n{marker}", plan.trim_end()),
        labels: vec![contract::adr_label(reference)],
    })?;
    record.issue = Some(issue);

    for event in fixture_events(reference) {
        let transition = machine::step(&record, &event);
        record.state = transition.next;
        match transition.feedback {
            FeedbackOp::Keep => {}
            FeedbackOp::Append(body) => record.review_feedback.push(body),
            FeedbackOp::Clear => record.review_feedback.clear(),
        }
        if transition.bump_attempt {
            record.attempt += 1;
        }
        for action in &transition.actions {
            execute(&mut emitter, &mut record, action, &plan, config, git)?;
        }
    }

    // The scenario merges from InReview, so no DisposeWorkspace action fires;
    // clean the execute leg's workspace here.
    if let Some(g) = git {
        let ws = g.workspace_root.join(&record.id);
        if ws.exists() {
            std::fs::remove_dir_all(&ws)?;
        }
    }
    Ok(emitter.lines)
}

/// Execute one machine action transcript-style. Forge mutations go through
/// the emitter (delegate + normalized line); engine/git actions execute for
/// real only on the execute leg. Mirrors `Router::execute`'s payload
/// construction (contract::* throughout) — with ONE documented divergence:
/// the link comment names the PR by its PLACEHOLDER, not its raw number,
/// because the raw numbers differ between legs by construction.
fn execute(
    emitter: &mut Emitter<'_>,
    record: &mut TaskRecord,
    action: &Action,
    plan: &str,
    config: &Config,
    git: Option<&GitContext>,
) -> anyhow::Result<()> {
    let marker = contract::task_marker(&record.id);
    match action {
        Action::RunEngine { fresh_workspace } => {
            let Some(g) = git else {
                return Ok(()); // record-only leg: transcript-only by construction
            };
            let ws = g.workspace_root.join(&record.id);
            if ws.exists() {
                std::fs::remove_dir_all(&ws)?;
            }
            crate::git::ensure_cache(&g.cache_dir, &g.remote_url)?;
            crate::git::create_workspace(
                &g.cache_dir,
                &ws,
                &g.base_branch,
                &record.branch,
                *fresh_workspace,
            )?;
            let spec = TaskSpec {
                adr_reference: record.adr_reference.clone(),
                title: record.title.clone(),
                adr_body: String::new(), // fixture scenario: the plan IS the context
                plan_markdown: plan.to_string(),
                review_feedback: if record.review_feedback.is_empty() {
                    None
                } else {
                    Some(record.review_feedback.join("\n\n---\n\n"))
                },
                workspace: ws,
            };
            let engine = FakeEngine {
                mode: FakeMode::Complete,
            };
            let (outcome, elapsed_ms) = run_timed(&engine, &spec);
            record.work_ms += elapsed_ms;
            // The outcome itself is discarded — the fixture sequence carries
            // the scripted EngineFinished — but a FakeEngine that could not
            // even write its artifact is a broken rig, surface it.
            if let EngineOutcome::Failed { reason, .. } = outcome? {
                anyhow::bail!("transcript FakeEngine failed: {reason}");
            }
            Ok(())
        }
        Action::CommitAndPush => {
            let Some(g) = git else {
                return Ok(());
            };
            let ws = g.workspace_root.join(&record.id);
            let message = contract::commit_message(&record.adr_reference, &record.title);
            crate::git::commit_all_except_task_file(&ws, &message)?;
            let local = crate::git::head_sha(&ws)?;
            if crate::git::ls_remote_sha(&g.remote_url, &record.branch)?.as_deref()
                != Some(local.as_str())
            {
                crate::git::push(&ws, &g.remote_url, &record.branch)?;
            }
            Ok(())
        }
        Action::OpenPr => {
            // Probe only on the execute leg: a previous transcript run's PR is
            // still open there (PrMerged was scripted, never real). The
            // record-only leg must not touch the live read path.
            if record.pr.is_none() && git.is_some() {
                record.pr = emitter.forge.find_open_pr_by_head(&record.branch)?;
            }
            let effort = contract::effort_bucket(record.work_ms, &config.effort);
            let draft = PrDraft {
                title: contract::pr_title(&record.adr_reference, &record.title),
                body: contract::pr_body(&record.adr_reference, plan.trim_end()),
                head: record.branch.clone(),
                base: git
                    .map(|g| g.base_branch.clone())
                    .unwrap_or_else(|| "main".to_string()),
                labels: vec![
                    contract::adr_label(&record.adr_reference),
                    effort.label().to_string(),
                ],
            };
            record.pr = Some(emitter.open_pr(&draft, record.pr)?);
            Ok(())
        }
        Action::ApplyPrLabels => {
            let pr = pr_id(record)?;
            let effort = contract::effort_bucket(record.work_ms, &config.effort);
            emitter.set_pr_labels(
                pr,
                &[
                    effort.label().to_string(),
                    contract::adr_label(&record.adr_reference),
                ],
            )
        }
        Action::LinkComment => {
            let issue = issue_id(record)?;
            let pr = pr_id(record)?;
            let pr_display = emitter.redactor.pr(pr); // registered at open_pr
            let body = format!(
                "Opened PR {pr_display} for {}: {}.\n\n{marker}",
                record.adr_reference, record.title
            );
            emitter.upsert_issue_comment(issue, &marker, &body)
        }
        Action::FailureComment { reason, log_tail } => {
            let issue = issue_id(record)?;
            let body = format!(
                "Engine failed (attempt {}): {reason}\n\n```\n{log_tail}\n```\n\n{marker}",
                record.attempt
            );
            emitter.upsert_issue_comment(issue, &marker, &body)
        }
        Action::SetIssueLabels { labels } => emitter.set_issue_labels(issue_id(record)?, labels),
        Action::CloseIssue { comment } => {
            let issue = issue_id(record)?;
            emitter.upsert_issue_comment(issue, &marker, comment)?;
            emitter.close_issue(issue)
        }
        Action::DisposeWorkspace => {
            if let Some(g) = git {
                let ws = g.workspace_root.join(&record.id);
                if ws.exists() {
                    std::fs::remove_dir_all(&ws)?;
                }
            }
            Ok(())
        }
    }
}

fn issue_id(record: &TaskRecord) -> anyhow::Result<IssueId> {
    record
        .issue
        .ok_or_else(|| anyhow::anyhow!("transcript task {} has no issue", record.id))
}

fn pr_id(record: &TaskRecord) -> anyhow::Result<PrId> {
    record
        .pr
        .ok_or_else(|| anyhow::anyhow!("transcript task {} has no PR", record.id))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Delegates each mutation to the adapter AND appends its normalized line —
/// "the chosen adapter wrapped in a transcript emitter". On the gitea leg the
/// mutation really executes; on the github leg the adapter is DryRun and only
/// records (its internal transcript is redundant here and ignored — the lines
/// BOTH legs print come from this emitter, one code path).
struct Emitter<'a> {
    forge: &'a dyn Forge,
    redactor: Redactor,
    lines: Vec<String>,
}

impl Emitter<'_> {
    fn push(&mut self, call: &ForgeCall<'_>) {
        let line = normalize_action(&mut self.redactor, call).to_string();
        self.lines.push(line);
    }

    fn create_issue(&mut self, new: &NewIssue) -> anyhow::Result<IssueId> {
        let id = self.forge.create_issue(new)?;
        self.push(&ForgeCall::CreateIssue { new, id });
        Ok(id)
    }

    /// `adopted` = a probe hit from a previous run: the mutation is skipped
    /// but the line is still emitted (the action happened, logically).
    fn open_pr(&mut self, draft: &PrDraft, adopted: Option<PrId>) -> anyhow::Result<PrId> {
        let id = match adopted {
            Some(id) => id,
            None => self.forge.open_pr(draft)?,
        };
        self.push(&ForgeCall::OpenPr { draft, id });
        Ok(id)
    }

    fn set_pr_labels(&mut self, id: PrId, labels: &[String]) -> anyhow::Result<()> {
        self.forge.set_pr_labels(&id, labels)?;
        self.push(&ForgeCall::SetPrLabels { id, labels });
        Ok(())
    }

    fn upsert_issue_comment(
        &mut self,
        id: IssueId,
        marker: &str,
        body: &str,
    ) -> anyhow::Result<()> {
        self.forge.upsert_issue_comment(&id, marker, body)?;
        self.push(&ForgeCall::UpsertIssueComment { id, marker, body });
        Ok(())
    }

    fn set_issue_labels(&mut self, id: IssueId, labels: &[String]) -> anyhow::Result<()> {
        self.forge.set_issue_labels(&id, labels)?;
        self.push(&ForgeCall::SetIssueLabels { id, labels });
        Ok(())
    }

    fn close_issue(&mut self, id: IssueId) -> anyhow::Result<()> {
        self.forge.close_issue(&id)?;
        self.push(&ForgeCall::CloseIssue { id });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::dry_run::DryRunForge;
    use crate::forge::fake::{FakeForge, RecordedAction};
    use std::path::Path;

    fn sh(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Seeded local bare repo, the stand-in forge git host (e2e rig pattern).
    fn seed_remote(dir: &Path) -> String {
        let work = dir.join("seed");
        std::fs::create_dir(&work).unwrap();
        sh(&work, &["init", "-b", "main"]);
        std::fs::write(work.join("README.md"), "seed\n").unwrap();
        sh(&work, &["add", "README.md"]);
        sh(&work, &["commit", "-m", "seed"]);
        let bare = dir.join("remote.git");
        sh(
            dir,
            &[
                "clone",
                "--bare",
                work.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        bare.to_str().unwrap().to_string()
    }

    #[test]
    fn fixture_events_are_the_scripted_scenario() {
        let events = fixture_events("ADR-0003");
        assert_eq!(events.len(), 5);
        assert_eq!(
            events[0],
            Event::IssueLabeled {
                label: "conduit:run".into()
            }
        );
        assert!(matches!(
            events[2],
            Event::ReviewSubmitted {
                verdict: ReviewVerdict::ChangesRequested,
                ..
            }
        ));
        assert!(matches!(events[4], Event::PrMerged { .. }));
    }

    /// THE money-shot assertion, hermetic: the execute leg (FakeForge with a
    /// real git remote — engine runs, branch pushed, mutations executed) and
    /// the record-only leg (DryRun-wrapped, no git) produce byte-identical
    /// normalized streams.
    #[test]
    fn execute_and_record_only_legs_are_byte_identical() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = seed_remote(dir.path());

        // Execute leg.
        let live = FakeForge::new();
        live.set_remote_url(&remote);
        let git = GitContext {
            remote_url: remote.clone(),
            cache_dir: dir.path().join("cache.git"),
            workspace_root: dir.path().join("workspaces"),
            base_branch: "main".into(),
        };
        let config = Config::default();
        let executed = run(&live, None, "ADR-0003", "3", &config, Some(&git)).unwrap();

        // Record-only leg (the github shape: DryRun, no git).
        let dry = DryRunForge::new(FakeForge::new());
        let recorded = run(&dry, None, "ADR-0003", "3", &config, None).unwrap();

        assert_eq!(executed, recorded, "the two legs must diff clean");

        // The stream shape: the full lifecycle's forge actions, in order.
        let kinds: Vec<String> = executed
            .iter()
            .map(|l| {
                serde_json::from_str::<Value>(l).unwrap()["action"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "create_issue",
                "open_pr",
                "set_pr_labels",
                "upsert_issue_comment", // link comment
                "set_pr_labels",        // effort recompute after the round
                "upsert_issue_comment", // close comment ...
                "close_issue",          // ... then close
            ]
        );

        // Normalization: placeholders, no raw forge ids, effort redacted.
        let all = executed.join("\n");
        assert!(all.contains("\"$ISSUE_1\""));
        assert!(all.contains("\"$PR_1\""));
        assert!(all.contains("Opened PR $PR_1"), "ids in bodies normalized");
        assert!(all.contains("effort:$REDACTED"));
        assert!(
            !all.contains("super-quick"),
            "effort label values never appear"
        );
        assert!(!all.contains("_at\""), "no timestamps");

        // The execute leg REALLY executed: issue + PR on the fake forge, the
        // branch really pushed to the remote.
        assert_eq!(
            live.count(|a| matches!(a, RecordedAction::CreateIssue(_))),
            1
        );
        assert_eq!(live.count(|a| matches!(a, RecordedAction::OpenPr(_))), 1);
        assert_eq!(
            live.count(|a| matches!(a, RecordedAction::CloseIssue(_))),
            1
        );
        let branch = contract::branch_name("ADR-0003", FIXTURE_TITLE);
        assert!(
            crate::git::ls_remote_sha(&remote, &branch)
                .unwrap()
                .is_some(),
            "execute leg pushed the transcript branch"
        );
        // The record-only leg executed NOTHING (DryRun inner saw no mutation).
        assert!(dry.inner_ref_for_tests().actions().is_empty());
    }

    /// Execute-leg replay: a second run on the same forge adopts the
    /// still-open PR via the probe and emits the SAME stream (a fresh issue is
    /// created by design — each run is a new demo beat).
    #[test]
    fn execute_leg_rerun_is_byte_identical_and_adopts_the_open_pr() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = seed_remote(dir.path());
        let live = FakeForge::new();
        live.set_remote_url(&remote);
        let git = GitContext {
            remote_url: remote,
            cache_dir: dir.path().join("cache.git"),
            workspace_root: dir.path().join("workspaces"),
            base_branch: "main".into(),
        };
        let config = Config::default();
        let first = run(&live, None, "ADR-0003", "3", &config, Some(&git)).unwrap();
        let second = run(&live, None, "ADR-0003", "3", &config, Some(&git)).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            live.count(|a| matches!(a, RecordedAction::OpenPr(_))),
            1,
            "second run adopted the open PR instead of duplicating it"
        );
        assert_eq!(
            live.count(|a| matches!(a, RecordedAction::CreateIssue(_))),
            2,
            "each run is a fresh demo issue"
        );
    }

    #[test]
    fn repo_slug_is_redacted_in_bodies() {
        let dry = DryRunForge::new(FakeForge::new());
        let mut redactor = Redactor::new(Some("octo/example".into()));
        let _ = &dry;
        assert_eq!(
            redactor.text("see https://host/octo/example/pull/1"),
            "see https://host/$REPO/pull/1"
        );
        assert_eq!(redactor.issue(IssueId(42)), "$ISSUE_1");
        assert_eq!(redactor.issue(IssueId(42)), "$ISSUE_1", "stable mapping");
        assert_eq!(redactor.issue(IssueId(7)), "$ISSUE_2");
        assert_eq!(redactor.pr(PrId(42)), "$PR_1", "separate table per kind");
    }
}
