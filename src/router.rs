//! The tick loop (spec §Module layout): fetch -> diff -> step -> execute -> persist.
//! Per-transition ordering (spec §Crash consistency):
//!   (1) persist new state + pending intents (tmp+rename+fsync) BEFORE executing
//!   (2) execute each action, probe-first
//!   (3) mark it done in the record
//!   (4) advance the forge cursor only after the tick's actions complete.
//! Crash anywhere -> restart converges: pending intents re-execute behind their
//! probes (at-least-once execution, exactly-once effect).
//!
//! The router owns ALL effects; `machine::step` is pure. The engine runs
//! synchronously inside `execute(RunEngine)`, and its `EngineFinished` result
//! feeds straight back through `step` + `apply` — so `Coding`/`Revising` only
//! ever persist as crash states.

use crate::engine::{EngineOutcome, TaskSpec};
use crate::forge::{ForgeEvent, NewIssue, PrDraft, RepoSnapshot};
use crate::machine::{self, Action, Event, FeedbackOp, Transition};
use crate::task::{ActionIntent, EngineResult, IssueId, PrId, TaskRecord, TaskState};

pub struct Router<'a> {
    pub forge: &'a dyn crate::forge::Forge,
    /// Cursor key: "gitea" | "github" | "fake".
    pub forge_name: String,
    pub engine: &'a dyn crate::engine::Engine,
    pub store: &'a crate::store::Store,
    pub config: &'a crate::config::Config,
    /// "main".
    pub base_branch: String,
}

impl Router<'_> {
    /// Boot-time reconcile (spec §Restart recovery): re-execute undone intents
    /// behind probes; a task in Coding/Revising with no live engine gets its
    /// stale workspace disposed and RunEngine re-queued (fresh workspace, from
    /// the immutable plan snapshot). Scoped/InReview just resume polling.
    ///
    /// Also called at the top of every [`tick`](Self::tick): a tick that
    /// failed mid-action leaves undone intents and an unadvanced cursor, and
    /// must self-heal before re-diffing.
    pub fn recover(&self) -> anyhow::Result<()> {
        for record in self.store.list_tasks()? {
            self.reconcile(record)?;
        }
        Ok(())
    }

    /// One poll tick: fetch snapshot, diff vs cursor, route events to tasks,
    /// step + execute, then advance the cursor. A failed action propagates
    /// `Err` BEFORE the cursor is saved, so the next tick re-diffs the same
    /// snapshot and converges behind the probes.
    pub fn tick(&self) -> anyhow::Result<()> {
        self.recover()?;
        let next = self.forge.fetch_snapshot()?;
        let prev = match self.store.load_cursor(&self.forge_name)? {
            Some(value) => serde_json::from_value::<RepoSnapshot>(value)?,
            // First tick ever (or lost cursor): the empty snapshot — every
            // label/review/terminal state replays once, behind the probes.
            None => RepoSnapshot {
                issues: vec![],
                prs: vec![],
                fetched_at: time::OffsetDateTime::UNIX_EPOCH,
            },
        };
        let events = crate::forge::diff(&prev, &next);
        for (idx, event) in events.iter().enumerate() {
            let Some((mut record, machine_event)) = self.route(event, &next)? else {
                continue;
            };
            let transition = machine::step(&record, &machine_event);
            // §Lifecycle: a Revising task whose PR merged/closed "mid-run" —
            // a terminal PR event later in THIS tick's batch — discards the
            // in-flight engine result; the terminal transition disposes the
            // workspace.
            let discard_engine = record.pr.is_some_and(|pr| {
                events[idx + 1..].iter().any(|later| {
                    matches!(later,
                        ForgeEvent::PrMerged { pr: p, .. } | ForgeEvent::PrClosed { pr: p }
                            if *p == pr)
                })
            });
            self.apply(&mut record, transition, discard_engine)?;
        }
        // (4) cursor advances ONLY after every event's actions completed.
        self.store
            .save_cursor(&self.forge_name, &serde_json::to_value(&next)?)?;
        Ok(())
    }

    /// Idempotent issue creation — the `conduit plan` replay path (spec
    /// §Idempotency: probe `find_issue_by_marker` before `create_issue`).
    /// On a probe hit the existing issue is adopted; either way the id is
    /// written back onto the record and saved.
    pub fn ensure_issue(&self, record: &mut TaskRecord) -> anyhow::Result<()> {
        if record.issue.is_some() {
            return Ok(());
        }
        let marker = crate::contract::task_marker(&record.id);
        let id = match self.forge.find_issue_by_marker(&marker)? {
            Some(id) => id,
            None => {
                let plan = self.store.load_plan(&record.id)?;
                self.forge.create_issue(&NewIssue {
                    title: crate::contract::pr_title(&record.adr_reference, &record.title),
                    body: format!("{}\n\n{marker}", plan.trim_end()),
                    labels: vec![crate::contract::adr_label(&record.adr_reference)],
                })?
            }
        };
        record.issue = Some(id);
        self.store.save_task(record)?;
        Ok(())
    }

    /// Per-task restart reconcile: undone intents re-execute behind probes;
    /// then a task STILL in Coding/Revising (the crash window between marking
    /// RunEngine done and persisting the EngineFinished transition) gets the
    /// engine re-queued — engines are disposable, the plan snapshot is truth.
    fn reconcile(&self, mut record: TaskRecord) -> anyhow::Result<()> {
        if record.state.is_terminal() {
            return Ok(());
        }
        if record.pending.iter().any(|i| !i.done) {
            self.run_pending(&mut record, false)?;
        }
        if matches!(record.state, TaskState::Coding | TaskState::Revising) {
            let fresh = record.state == TaskState::Coding;
            record.pending = vec![ActionIntent {
                action: Action::RunEngine {
                    fresh_workspace: fresh,
                },
                done: false,
            }];
            self.store.save_task(&record)?; // write-ahead
            self.run_pending(&mut record, false)?;
        }
        Ok(())
    }

    /// Map a ForgeEvent to (task, machine::Event). Routing keys: issue id ->
    /// record.issue; pr id -> record.pr; a PR seen for a known branch with no
    /// recorded pr id adopts it (open_pr replay reconciliation).
    fn route(
        &self,
        event: &ForgeEvent,
        snapshot: &RepoSnapshot,
    ) -> anyhow::Result<Option<(TaskRecord, Event)>> {
        let tasks = self.store.list_tasks()?;
        Ok(match event {
            ForgeEvent::IssueLabeled { issue, label } => tasks
                .into_iter()
                .find(|t| t.issue == Some(*issue))
                .map(|t| {
                    (
                        t,
                        Event::IssueLabeled {
                            label: label.clone(),
                        },
                    )
                }),
            ForgeEvent::ReviewSubmitted { pr, review } => {
                task_for_pr(tasks, *pr, snapshot).map(|t| {
                    (
                        t,
                        Event::ReviewSubmitted {
                            verdict: review.verdict,
                            body: review.body.clone(),
                        },
                    )
                })
            }
            ForgeEvent::CiChanged { pr, .. } => {
                task_for_pr(tasks, *pr, snapshot).map(|t| (t, Event::CiChanged))
            }
            ForgeEvent::PrMerged { pr, merge_sha } => task_for_pr(tasks, *pr, snapshot).map(|t| {
                (
                    t,
                    Event::PrMerged {
                        merge_sha: merge_sha.clone(),
                    },
                )
            }),
            ForgeEvent::PrClosed { pr } => {
                task_for_pr(tasks, *pr, snapshot).map(|t| (t, Event::PrClosed))
            }
        })
    }

    /// Apply one transition: mutate the record (state/feedback/attempt),
    /// replace the intents, persist (write-ahead), then execute-with-probes.
    /// `discard_engine`: drop an EngineFinished result instead of feeding it
    /// back through `step` (terminal PR event later in the same tick).
    fn apply(
        &self,
        record: &mut TaskRecord,
        t: Transition,
        discard_engine: bool,
    ) -> anyhow::Result<()> {
        record.state = t.next;
        match t.feedback {
            FeedbackOp::Keep => {}
            FeedbackOp::Append(body) => record.review_feedback.push(body),
            FeedbackOp::Clear => record.review_feedback.clear(),
        }
        if t.bump_attempt {
            record.attempt += 1;
        }
        // Replacing pending is safe: reconcile runs before any routing, so
        // every prior intent is done by the time a new transition lands.
        debug_assert!(record.pending.iter().all(|i| i.done));
        record.pending = t
            .actions
            .into_iter()
            .map(|action| ActionIntent {
                action,
                done: false,
            })
            .collect();
        // (1) write-ahead: persist new state + intents BEFORE executing.
        self.store.save_task(record)?;
        self.run_pending(record, discard_engine)
    }

    /// (2)+(3): execute each undone intent probe-first, persist the record's
    /// mutations (pr id, work_ms), then mark the intent done. An
    /// EngineFinished follow-up recurses through `step`+`apply` AFTER the
    /// RunEngine intent is marked done, so the recursive transition never
    /// clobbers an in-flight index (store caller invariant).
    fn run_pending(&self, record: &mut TaskRecord, discard_engine: bool) -> anyhow::Result<()> {
        let mut i = 0;
        while i < record.pending.len() {
            if record.pending[i].done {
                i += 1;
                continue;
            }
            let action = record.pending[i].action.clone();
            let followup = self.execute(record, &action)?;
            self.store.save_task(record)?;
            // Index is from the same record we just saved (store invariant).
            self.store.mark_intent_done(&record.id, i)?;
            record.pending[i].done = true;
            // When discarding, the engine ran for nothing — the terminal PR
            // event later in this tick disposes the workspace.
            if let Some(event) = followup
                && !discard_engine
            {
                let t = machine::step(record, &event);
                self.apply(record, t, discard_engine)?;
                // `apply` replaced `pending` with the follow-up transition's
                // intents and ran them; the loop resumes over the new
                // (all-done) vector and exits.
            }
            i += 1;
        }
        Ok(())
    }

    /// Execute one action idempotently (the probe table, spec §Idempotency):
    /// OpenPr -> find_open_pr_by_head; CommitAndPush -> ls-remote compare;
    /// comments -> marker upsert; labels -> convergent absolute set.
    /// (create_issue -> find_issue_by_marker lives in [`ensure_issue`](Self::ensure_issue),
    /// the `conduit plan` path.) RunEngine returns the EngineFinished event
    /// for the caller to feed back through `step`.
    fn execute(&self, record: &mut TaskRecord, action: &Action) -> anyhow::Result<Option<Event>> {
        match action {
            Action::RunEngine { fresh_workspace } => {
                self.run_engine(record, *fresh_workspace).map(Some)
            }
            Action::CommitAndPush => {
                self.commit_and_push(record)?;
                Ok(None)
            }
            Action::OpenPr => {
                self.open_pr(record)?;
                Ok(None)
            }
            Action::ApplyPrLabels => {
                let pr = pr_id(record)?;
                let effort = crate::contract::effort_bucket(record.work_ms, &self.config.effort);
                // Exactly one effort label, structurally: the set is absolute,
                // so the other four are absent by construction (spec §The
                // tuesday contract).
                self.forge.set_pr_labels(
                    &pr,
                    &[
                        effort.label().to_string(),
                        crate::contract::adr_label(&record.adr_reference),
                    ],
                )?;
                Ok(None)
            }
            Action::LinkComment => {
                let issue = issue_id(record)?;
                let pr = pr_id(record)?;
                let marker = crate::contract::task_marker(&record.id);
                let body = format!(
                    "Opened PR {} for {}: {}.\n\n{marker}",
                    pr.0, record.adr_reference, record.title
                );
                self.forge.upsert_issue_comment(&issue, &marker, &body)?;
                Ok(None)
            }
            Action::FailureComment { reason, log_tail } => {
                let issue = issue_id(record)?;
                let marker = crate::contract::task_marker(&record.id);
                let body = format!(
                    "Engine failed (attempt {}): {reason}\n\n```\n{log_tail}\n```\n\n{marker}",
                    record.attempt
                );
                self.forge.upsert_issue_comment(&issue, &marker, &body)?;
                Ok(None)
            }
            Action::SetIssueLabels { labels } => {
                self.forge.set_issue_labels(&issue_id(record)?, labels)?;
                Ok(None)
            }
            Action::CloseIssue { comment } => {
                let issue = issue_id(record)?;
                // The machine's comment already embeds the task marker; the
                // upsert keys on it, so replay converges to one comment.
                let marker = crate::contract::task_marker(&record.id);
                self.forge.upsert_issue_comment(&issue, &marker, comment)?;
                self.forge.close_issue(&issue)?;
                Ok(None)
            }
            Action::DisposeWorkspace => {
                let ws = self.store.workspace_dir(&record.id, record.attempt);
                if ws.exists() {
                    std::fs::remove_dir_all(&ws)?;
                }
                Ok(None)
            }
        }
    }

    /// Prepare a workspace (git.rs) and run the engine via `run_timed`,
    /// accumulating `work_ms` (always `+=`: cumulative across attempts and
    /// rounds). An existing workspace dir is disposed first — that IS the
    /// restart recovery: re-executing RunEngine always starts clean from the
    /// cache, and the plan is ALWAYS re-read from the immutable snapshot,
    /// never regenerated (spec §Plan snapshot).
    fn run_engine(&self, record: &mut TaskRecord, fresh: bool) -> anyhow::Result<Event> {
        let ws = self.store.workspace_dir(&record.id, record.attempt);
        if ws.exists() {
            std::fs::remove_dir_all(&ws)?;
        }
        let remote = self.forge.git_remote_url()?;
        let cache = self
            .store
            .root()
            .join("cache")
            .join(format!("{}.git", self.forge_name));
        crate::git::ensure_cache(&cache, &remote)?;
        crate::git::create_workspace(&cache, &ws, &self.base_branch, &record.branch, fresh)?;
        let plan = self.store.load_plan(&record.id)?;
        let spec = TaskSpec {
            adr_reference: record.adr_reference.clone(),
            title: record.title.clone(),
            // The record carries no ADR body; the verbatim plan snapshot is
            // the engine's implementation context in the spike.
            adr_body: String::new(),
            plan_markdown: plan,
            review_feedback: if record.review_feedback.is_empty() {
                None
            } else {
                Some(record.review_feedback.join("\n\n---\n\n"))
            },
            workspace: ws,
        };
        let (outcome, elapsed_ms) = crate::engine::run_timed(self.engine, &spec);
        record.work_ms += elapsed_ms; // ONE run's ms — always accumulate
        // EngineError = "could not run at all": bubble up; the intent stays
        // pending and the next tick/recover retries.
        let outcome = outcome?;
        // The conduit-enforced hard timeout for engines that do not
        // self-enforce (FakeEngine): a run that outlived the deadline is a
        // first-class Failed, never an error. ClaudeCodeEngine kills at the
        // deadline itself and already reports `reason: "timeout"`.
        let timeout_ms = self.config.engine.timeout_secs.saturating_mul(1000);
        let result = match outcome {
            EngineOutcome::Completed { .. } if elapsed_ms > timeout_ms => EngineResult::Failed {
                reason: "timeout".to_string(),
                log_tail: String::new(),
            },
            EngineOutcome::Completed { summary } => EngineResult::Completed { summary },
            EngineOutcome::Failed { reason, log_tail } => EngineResult::Failed { reason, log_tail },
        };
        Ok(Event::EngineFinished(result))
    }

    /// Commit (pathspec-staged, task doc excluded at every depth) and push.
    /// Probe: `ls_remote_sha == local HEAD` ⇒ already pushed, skip — the push
    /// replay never moves the branch twice.
    fn commit_and_push(&self, record: &TaskRecord) -> anyhow::Result<()> {
        let ws = self.store.workspace_dir(&record.id, record.attempt);
        let message = crate::contract::commit_message(&record.adr_reference, &record.title);
        // false = nothing new to commit (deterministic re-run / replay): fine,
        // the probe below decides whether HEAD still needs pushing.
        crate::git::commit_all_except_task_file(&ws, &message)?;
        let remote = self.forge.git_remote_url()?;
        let local = crate::git::head_sha(&ws)?;
        if crate::git::ls_remote_sha(&remote, &record.branch)?.as_deref() != Some(local.as_str()) {
            crate::git::push(&ws, &remote, &record.branch)?;
        }
        Ok(())
    }

    /// Open the PR with full tuesday tagging. Probe: `find_open_pr_by_head`
    /// adopts an existing PR (open_pr replay). The PrId is written back onto
    /// the record; `run_pending` persists it before marking the intent done.
    fn open_pr(&self, record: &mut TaskRecord) -> anyhow::Result<()> {
        if record.pr.is_some() {
            return Ok(());
        }
        if let Some(id) = self.forge.find_open_pr_by_head(&record.branch)? {
            record.pr = Some(id);
            return Ok(());
        }
        let plan = self.store.load_plan(&record.id)?;
        let effort = crate::contract::effort_bucket(record.work_ms, &self.config.effort);
        let draft = PrDraft {
            title: crate::contract::pr_title(&record.adr_reference, &record.title),
            // Plan-derived summary; the trailer is the final line.
            body: crate::contract::pr_body(&record.adr_reference, plan.trim_end()),
            head: record.branch.clone(),
            base: self.base_branch.clone(),
            labels: vec![
                crate::contract::adr_label(&record.adr_reference),
                effort.label().to_string(),
            ],
        };
        record.pr = Some(self.forge.open_pr(&draft)?);
        Ok(())
    }
}

/// pr id -> task; a PR seen for a known branch with no recorded pr id adopts
/// it (open_pr replay reconciliation) — the adoption is persisted by
/// `apply`'s save.
fn task_for_pr(tasks: Vec<TaskRecord>, pr: PrId, snapshot: &RepoSnapshot) -> Option<TaskRecord> {
    if let Some(t) = tasks.iter().find(|t| t.pr == Some(pr)) {
        return Some(t.clone());
    }
    let head = &snapshot.prs.iter().find(|p| p.id == pr)?.head_branch;
    let mut t = tasks
        .into_iter()
        .find(|t| t.pr.is_none() && &t.branch == head)?;
    t.pr = Some(pr);
    Some(t)
}

fn issue_id(record: &TaskRecord) -> anyhow::Result<IssueId> {
    record
        .issue
        .ok_or_else(|| anyhow::anyhow!("task {} has no issue", record.id))
}

fn pr_id(record: &TaskRecord) -> anyhow::Result<PrId> {
    record
        .pr
        .ok_or_else(|| anyhow::anyhow!("task {} has no PR", record.id))
}
