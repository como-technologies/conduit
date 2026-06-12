//! Router e2e over FakeForge + FakeEngine (Task 12): full lifecycle,
//! kill/restart at every state, crash-replay per mutating action kind
//! (at-least-once execution, exactly-once effect), engine fail + hang→timeout,
//! and the cursor-only-after-actions ordering (spec §Crash consistency).

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use conduit::config::Config;
use conduit::contract;
use conduit::engine::fake::{FakeEngine, FakeMode};
use conduit::forge::fake::{FakeForge, RecordedAction};
use conduit::forge::{
    CiState, Forge, IssueSnapshot, NewIssue, PrDraft, PrSnapshot, RepoSnapshot, Review,
};
use conduit::machine::Action;
use conduit::router::Router;
use conduit::store::Store;
use conduit::task::{ActionIntent, IssueId, PrId, ReviewId, ReviewVerdict, TaskRecord, TaskState};
use tempfile::TempDir;
use time::OffsetDateTime;

/// The immutable plan snapshot (spec §Plan snapshot) — saved verbatim by the
/// rig; the FakeEngine embeds its sha256 in the impl doc, which is how the
/// tests prove a re-run came from the snapshot.
const PLAN_MD: &str =
    "# Plan: Adopt snapshot-diff router\n\n1. Implement the router.\n2. Prove crash replay.\n";

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

fn intent(action: Action) -> ActionIntent {
    ActionIntent {
        action,
        done: false,
    }
}

fn done(action: Action) -> ActionIntent {
    ActionIntent { action, done: true }
}

fn kind(a: &RecordedAction) -> &'static str {
    match a {
        RecordedAction::EnsureLabels(_) => "ensure_labels",
        RecordedAction::CreateIssue(_) => "create_issue",
        RecordedAction::UpsertIssueComment { .. } => "upsert_issue_comment",
        RecordedAction::SetIssueLabels { .. } => "set_issue_labels",
        RecordedAction::CloseIssue(_) => "close_issue",
        RecordedAction::OpenPr(_) => "open_pr",
        RecordedAction::UpsertPrComment { .. } => "upsert_pr_comment",
        RecordedAction::SetPrLabels { .. } => "set_pr_labels",
    }
}

/// A crash leftover the router must dispose before re-running the engine.
fn plant_stale_workspace(ws: &Path) {
    std::fs::create_dir_all(ws).unwrap();
    std::fs::write(ws.join("stale-sentinel.txt"), "stale").unwrap();
}

/// Harness: a Scoped task in a fresh store + a FakeForge + local git remote.
/// `current` is the evolving "forge truth" the tests script snapshot-by-
/// snapshot; the FakeForge's stored state backs the probes and mutations.
struct Rig {
    _dir: TempDir,
    remote_url: String,
    store: Store,
    forge: FakeForge,
    engine: FakeEngine,
    config: Config,
    current: RefCell<RepoSnapshot>,
    task_id: String,
}

impl Rig {
    fn new() -> Rig {
        let dir = TempDir::new().unwrap();
        // Seeded local bare repo — the stand-in forge git host.
        let work = dir.path().join("seed");
        std::fs::create_dir(&work).unwrap();
        sh(&work, &["init", "-b", "main"]);
        std::fs::write(work.join("README.md"), "seed\n").unwrap();
        sh(&work, &["add", "README.md"]);
        sh(&work, &["commit", "-m", "seed"]);
        let bare = dir.path().join("remote.git");
        sh(
            dir.path(),
            &[
                "clone",
                "--bare",
                work.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        let remote_url = bare.to_str().unwrap().to_string();

        let store = Store::open(dir.path().join(".conduit")).unwrap();
        let sha = store.save_plan("adr-0003", PLAN_MD).unwrap();
        let record = TaskRecord::new("ADR-0003", "3", "Adopt snapshot-diff router", &sha);
        let task_id = record.id.clone();
        store.save_task(&record).unwrap();

        let forge = FakeForge::new();
        forge.set_remote_url(&remote_url);

        let rig = Rig {
            _dir: dir,
            remote_url,
            store,
            forge,
            engine: FakeEngine {
                mode: FakeMode::Complete,
            },
            config: Config::default(),
            current: RefCell::new(RepoSnapshot {
                issues: vec![],
                prs: vec![],
                fetched_at: OffsetDateTime::now_utc(),
            }),
            task_id,
        };

        // The `conduit plan` step: probe-first issue creation.
        let mut record = rig.record();
        rig.router().ensure_issue(&mut record).unwrap();
        let issue = record.issue.expect("ensure_issue sets the id");
        rig.current.borrow_mut().issues.push(IssueSnapshot {
            id: issue,
            labels: vec![contract::adr_label("ADR-0003")],
            closed: false,
        });
        rig
    }

    fn router(&self) -> Router<'_> {
        Router {
            forge: &self.forge,
            forge_name: "fake".into(),
            engine: &self.engine,
            store: &self.store,
            config: &self.config,
            base_branch: "main".into(),
        }
    }

    fn record(&self) -> TaskRecord {
        self.store.load_task(&self.task_id).unwrap().unwrap()
    }

    fn save(&self, record: &TaskRecord) {
        self.store.save_task(record).unwrap();
    }

    /// Script the current forge truth as the next snapshot, then tick once.
    fn tick(&self) -> anyhow::Result<()> {
        self.forge.script(vec![self.current.borrow().clone()]);
        self.router().tick()
    }

    fn tick_ok(&self) {
        self.tick().unwrap();
    }

    /// Script one snapshot that adds `label` to the task's issue, then tick.
    fn label_and_tick(&self, label: &str) {
        {
            let mut cur = self.current.borrow_mut();
            let issue = &mut cur.issues[0];
            if !issue.labels.iter().any(|l| l == label) {
                issue.labels.push(label.to_string());
            }
        }
        self.tick_ok();
    }

    /// Replace the issue's labels in the scripted truth (mirrors the absolute
    /// `set_issue_labels` the router performed on the real forge).
    fn set_issue_labels(&self, labels: &[&str]) {
        self.current.borrow_mut().issues[0].labels = labels.iter().map(|s| s.to_string()).collect();
    }

    /// Surface the router-opened PR in the scripted truth for later ticks.
    fn pr_into_current(&self) {
        let record = self.record();
        let pr = record.pr.expect("record has a PR");
        let mut cur = self.current.borrow_mut();
        if cur.prs.iter().any(|p| p.id == pr) {
            return;
        }
        cur.prs.push(PrSnapshot {
            id: pr,
            title: contract::pr_title(&record.adr_reference, &record.title),
            body: contract::pr_body(&record.adr_reference, PLAN_MD.trim_end()),
            head_branch: record.branch.clone(),
            labels: vec![],
            reviews: vec![],
            ci: CiState::None,
            merged: false,
            merge_sha: None,
            closed: false,
        });
    }

    fn add_review(&self, id: &str, verdict: ReviewVerdict, body: &str) {
        self.current.borrow_mut().prs[0].reviews.push(Review {
            id: ReviewId(id.into()),
            author: "reviewer".into(),
            verdict,
            body: body.into(),
            submitted_at: OffsetDateTime::now_utc(),
        });
    }

    fn merge_pr(&self, sha: &str) {
        let mut cur = self.current.borrow_mut();
        let pr = &mut cur.prs[0];
        pr.merged = true;
        pr.closed = true;
        pr.merge_sha = Some(sha.to_string());
    }

    fn close_pr(&self) {
        self.current.borrow_mut().prs[0].closed = true;
    }

    fn workspace(&self, attempt: u32) -> PathBuf {
        self.store.workspace_dir(&self.task_id, attempt)
    }

    fn remote_sha(&self) -> Option<String> {
        conduit::git::ls_remote_sha(&self.remote_url, &self.record().branch, None).unwrap()
    }

    /// Process restart: same store + same git remote, brand-new FakeForge
    /// seeded to mirror what a real forge still holds across a conduit
    /// restart (issue + open PR; in-memory FakeForge state does not survive).
    /// Ids realign because the fake's counters are deterministic (issue 1,
    /// PR 1).
    fn restart(&mut self) {
        let record = self.record();
        let forge = FakeForge::new();
        forge.set_remote_url(&self.remote_url);
        let marker = contract::task_marker(&record.id);
        if record.issue.is_some() {
            let labels = self.current.borrow().issues[0].labels.clone();
            forge
                .create_issue(&NewIssue {
                    title: contract::pr_title(&record.adr_reference, &record.title),
                    body: format!("{PLAN_MD}\n\n{marker}"),
                    labels,
                })
                .unwrap();
        }
        if record.pr.is_some() {
            forge
                .open_pr(&PrDraft {
                    title: contract::pr_title(&record.adr_reference, &record.title),
                    body: contract::pr_body(&record.adr_reference, PLAN_MD.trim_end()),
                    head: record.branch.clone(),
                    base: "main".into(),
                    labels: vec![],
                })
                .unwrap();
        }
        self.forge = forge;
    }
}

#[test]
fn full_lifecycle_scoped_to_merged() {
    let rig = Rig::new();
    let r = rig.record();
    assert_eq!(r.state, TaskState::Scoped);
    assert_eq!(r.issue, Some(IssueId(1)));

    // Scoped --label conduit:run--> Coding -> engine completes -> InReview.
    rig.label_and_tick("conduit:run");
    let r = rig.record();
    assert_eq!(r.state, TaskState::InReview);
    assert_eq!(r.pr, Some(PrId(1)));
    assert_eq!(r.attempt, 1);
    assert!(r.review_feedback.is_empty());
    assert!(r.pending.iter().all(|i| i.done));
    // (work_ms accumulation is asserted in timeout_is_failed — a sub-ms
    // FakeEngine run legitimately rounds to 0 here.)
    rig.remote_sha().expect("branch pushed to the remote");

    // PR draft carries the full tuesday tagging.
    let draft = rig
        .forge
        .actions()
        .into_iter()
        .find_map(|a| match a {
            RecordedAction::OpenPr(d) => Some(d),
            _ => None,
        })
        .expect("OpenPr recorded");
    assert_eq!(draft.title, "[ADR-0003] Adopt snapshot-diff router");
    assert_eq!(draft.head, r.branch);
    assert_eq!(draft.base, "main");
    assert_eq!(
        draft.body.lines().last().unwrap(),
        "Adr-Reference: ADR-0003",
        "trailer is the final line"
    );
    assert!(draft.labels.contains(&"adr:ADR-0003".to_string()));
    assert_eq!(
        draft
            .labels
            .iter()
            .filter(|l| l.starts_with("effort:"))
            .count(),
        1,
        "exactly one effort label at open"
    );

    // Link comment upserted onto the issue (no bare number refs).
    let stored = rig.forge.issue_comments(&IssueId(1));
    assert_eq!(stored.len(), 1);
    assert!(stored[0].1.contains("PR 1"), "comment: {}", stored[0].1);

    // --ChangesRequested--> Revising -> engine completes -> InReview
    // (effort label recomputed).
    rig.pr_into_current();
    rig.add_review(
        "r1",
        ReviewVerdict::ChangesRequested,
        "please tweak the docs",
    );
    rig.tick_ok();
    let r = rig.record();
    assert_eq!(r.state, TaskState::InReview);
    assert!(
        r.review_feedback.is_empty(),
        "completed round clears feedback"
    );
    let label_sets: Vec<Vec<String>> = rig
        .forge
        .actions()
        .into_iter()
        .filter_map(|a| match a {
            RecordedAction::SetPrLabels { labels, .. } => Some(labels),
            _ => None,
        })
        .collect();
    assert_eq!(label_sets.len(), 2, "effort recomputed after the round");
    for set in &label_sets {
        assert_eq!(
            set.iter().filter(|l| l.starts_with("effort:")).count(),
            1,
            "exactly one effort label, structurally: {set:?}"
        );
        assert!(set.contains(&"adr:ADR-0003".to_string()));
    }

    // --PrMerged--> Merged (issue closed with the sha comment).
    rig.merge_pr("cafe42");
    rig.tick_ok();
    let r = rig.record();
    assert_eq!(r.state, TaskState::Merged);
    let stored = rig.forge.issue_comments(&IssueId(1));
    assert_eq!(stored.len(), 1, "one marker-keyed status comment");
    assert!(stored[0].1.contains("cafe42"));

    // The recorded forge actions, in order.
    let kinds: Vec<&'static str> = rig.forge.actions().iter().map(kind).collect();
    assert_eq!(
        kinds,
        vec![
            "create_issue",         // rig setup (the `conduit plan` step)
            "open_pr",              // Coding -> InReview
            "set_pr_labels",        //   ApplyPrLabels
            "upsert_issue_comment", //   LinkComment
            "set_pr_labels",        // Revising -> InReview: effort relabel
            "upsert_issue_comment", // Merged: closing comment ...
            "close_issue",          //   ... then close
        ]
    );
}

#[test]
fn engine_failure_goes_to_failed_and_relabel_retries() {
    let mut rig = Rig::new();
    rig.engine.mode = FakeMode::Fail;
    rig.label_and_tick("conduit:run");
    let r = rig.record();
    assert_eq!(r.state, TaskState::Failed);
    assert_eq!(r.attempt, 1);
    let stored = rig.forge.issue_comments(&IssueId(1));
    assert_eq!(stored.len(), 1);
    assert!(stored[0].1.contains("scripted failure"));
    assert!(stored[0].1.contains("fake engine scripted log tail"));
    assert!(rig.forge.actions().iter().any(|a| matches!(a,
        RecordedAction::SetIssueLabels { labels, .. }
            if labels.len() == 1 && labels[0] == "conduit:failed")));

    // The poll loop observes the post-failure label state ...
    rig.set_issue_labels(&["conduit:failed"]);
    rig.tick_ok();
    // ... then the human re-labels conduit:run: attempt 2, fresh workspace.
    rig.engine.mode = FakeMode::Complete;
    rig.label_and_tick("conduit:run");
    let r = rig.record();
    assert_eq!(r.state, TaskState::InReview);
    assert_eq!(r.attempt, 2);
    assert!(
        rig.workspace(2).join("docs/impl/adr-0003.md").exists(),
        "retry ran in a fresh attempt-2 workspace"
    );
}

#[test]
fn timeout_is_failed() {
    // FakeEngine Hang{secs: 5} + engine timeout 1s (config) -> Failed, timeout.
    let mut rig = Rig::new();
    rig.engine.mode = FakeMode::Hang { secs: 5 };
    rig.config.engine.timeout_secs = 1;
    rig.label_and_tick("conduit:run");
    let r = rig.record();
    assert_eq!(r.state, TaskState::Failed);
    assert!(r.work_ms >= 5_000, "wall clock accumulated: {}", r.work_ms);
    let stored = rig.forge.issue_comments(&IssueId(1));
    assert!(
        stored[0].1.contains("timeout"),
        "failure reason is timeout: {}",
        stored[0].1
    );
}

#[test]
fn pr_closed_without_merge_abandons() {
    let rig = Rig::new();
    rig.label_and_tick("conduit:run");
    rig.pr_into_current();
    rig.close_pr();
    rig.tick_ok();
    let r = rig.record();
    assert_eq!(r.state, TaskState::Abandoned);
    assert_eq!(
        rig.forge
            .count(|a| matches!(a, RecordedAction::CloseIssue(_))),
        1
    );
    let stored = rig.forge.issue_comments(&IssueId(1));
    assert!(stored[0].1.contains("abandoned"), "{}", stored[0].1);
}

#[test]
fn revising_pr_merged_mid_run_discards_engine_result() {
    let rig = Rig::new();
    rig.label_and_tick("conduit:run");
    rig.pr_into_current();
    let sha_before = rig.remote_sha().unwrap();

    // ChangesRequested AND PrMerged land in the SAME tick's diff: the task
    // goes Revising (engine runs), but the merge supersedes the result.
    rig.add_review("r1", ReviewVerdict::ChangesRequested, "tweak");
    rig.merge_pr("cafe42");
    rig.tick_ok();

    let r = rig.record();
    assert_eq!(r.state, TaskState::Merged);
    assert!(!rig.workspace(1).exists(), "workspace disposed");
    assert_eq!(
        rig.remote_sha().unwrap(),
        sha_before,
        "no CommitAndPush from the stale engine run"
    );
    assert_eq!(
        rig.forge
            .count(|a| matches!(a, RecordedAction::SetPrLabels { .. })),
        1,
        "no ApplyPrLabels from the discarded engine run"
    );
    assert!(
        matches!(
            r.pending.as_slice(),
            [
                ActionIntent {
                    action: Action::DisposeWorkspace,
                    done: true
                },
                ActionIntent {
                    action: Action::CloseIssue { .. },
                    done: true
                }
            ]
        ),
        "terminal transition's intents only: {:?}",
        r.pending
    );
}

/// Kill/restart at EVERY state: persist the store as a crash would leave it,
/// build a brand-new Router (same store, fresh FakeForge mirroring the forge
/// truth), recover(), continue, and assert the lifecycle completes.
/// Coding/Revising only ever persist as crash states (the engine runs
/// synchronously inside a tick), so those two iterations write the exact
/// record the write-ahead persists before executing.
#[test]
fn restart_at_every_state_converges() {
    for stop_at in [
        TaskState::Scoped,
        TaskState::Coding,
        TaskState::InReview,
        TaskState::Revising,
        TaskState::Failed,
    ] {
        let mut rig = Rig::new();

        match stop_at {
            TaskState::Scoped => {}
            TaskState::Coding => {
                let mut r = rig.record();
                r.state = TaskState::Coding;
                r.pending = vec![intent(Action::RunEngine {
                    fresh_workspace: true,
                })];
                rig.save(&r);
                plant_stale_workspace(&rig.workspace(1));
            }
            TaskState::InReview => {
                rig.label_and_tick("conduit:run");
                rig.pr_into_current();
            }
            TaskState::Revising => {
                rig.label_and_tick("conduit:run");
                rig.pr_into_current();
                let mut r = rig.record();
                r.state = TaskState::Revising;
                r.review_feedback = vec!["crash-round feedback".into()];
                r.pending = vec![intent(Action::RunEngine {
                    fresh_workspace: false,
                })];
                rig.save(&r);
                plant_stale_workspace(&rig.workspace(1));
            }
            TaskState::Failed => {
                rig.engine.mode = FakeMode::Fail;
                rig.label_and_tick("conduit:run");
                rig.engine.mode = FakeMode::Complete;
            }
            _ => unreachable!(),
        }
        assert_eq!(rig.record().state, stop_at, "drive to {stop_at:?}");

        // Kill -9: drop the router, restart with a fresh forge + same store.
        rig.restart();
        rig.router().recover().unwrap();

        if matches!(stop_at, TaskState::Coding | TaskState::Revising) {
            let r = rig.record();
            assert_eq!(r.state, TaskState::InReview, "{stop_at:?}: engine re-ran");
            let ws = rig.workspace(r.attempt);
            assert!(
                !ws.join("stale-sentinel.txt").exists(),
                "{stop_at:?}: stale workspace disposed"
            );
            let doc = std::fs::read_to_string(ws.join("docs/impl/adr-0003.md")).unwrap();
            assert!(
                doc.contains(&r.plan_sha256),
                "{stop_at:?}: engine re-ran from the immutable plan snapshot"
            );
            rig.pr_into_current();
        }

        // Continue to a terminal state.
        match stop_at {
            TaskState::Scoped => {
                rig.label_and_tick("conduit:run");
                rig.pr_into_current();
            }
            TaskState::Failed => {
                rig.set_issue_labels(&["conduit:failed"]);
                rig.tick_ok();
                rig.label_and_tick("conduit:run");
                assert_eq!(
                    rig.record().attempt,
                    2,
                    "relabel after Failed bumps attempt"
                );
                rig.pr_into_current();
            }
            _ => {}
        }
        rig.merge_pr("cafe42");
        rig.tick_ok();
        assert_eq!(
            rig.record().state,
            TaskState::Merged,
            "{stop_at:?} converged to Merged"
        );
        // Exactly-once on the restarted forge: probes prevented duplicates.
        assert_eq!(
            rig.forge
                .count(|a| matches!(a, RecordedAction::CreateIssue(_))),
            1,
            "{stop_at:?}: one issue"
        );
        assert_eq!(
            rig.forge.count(|a| matches!(a, RecordedAction::OpenPr(_))),
            1,
            "{stop_at:?}: one PR"
        );
    }
}

#[test]
fn crash_replay_create_issue_is_exactly_once() {
    let rig = Rig::new();
    assert_eq!(
        rig.forge
            .count(|a| matches!(a, RecordedAction::CreateIssue(_))),
        1
    );
    // Crash AFTER create_issue executed, BEFORE the id write-back persisted.
    let mut r = rig.record();
    r.issue = None;
    rig.save(&r);
    // Replay (`conduit plan` re-run) probes find_issue_by_marker first.
    rig.router().ensure_issue(&mut r).unwrap();
    assert_eq!(
        rig.forge
            .count(|a| matches!(a, RecordedAction::CreateIssue(_))),
        1,
        "probe hit: no second create_issue"
    );
    assert_eq!(r.issue, Some(IssueId(1)));
    assert_eq!(rig.record().issue, Some(IssueId(1)), "adopted id persisted");
}

#[test]
fn crash_replay_open_pr_is_exactly_once() {
    let rig = Rig::new();
    rig.label_and_tick("conduit:run");
    assert_eq!(
        rig.forge.count(|a| matches!(a, RecordedAction::OpenPr(_))),
        1
    );
    // Crash BEFORE mark-done and BEFORE the pr-id write-back.
    let mut r = rig.record();
    r.pr = None;
    r.pending = vec![
        done(Action::CommitAndPush),
        intent(Action::OpenPr),
        intent(Action::ApplyPrLabels),
        intent(Action::LinkComment),
    ];
    rig.save(&r);
    rig.router().recover().unwrap();
    assert_eq!(
        rig.forge.count(|a| matches!(a, RecordedAction::OpenPr(_))),
        1,
        "find_open_pr_by_head adopted the existing PR"
    );
    let r = rig.record();
    assert_eq!(r.pr, Some(PrId(1)));
    assert!(r.pending.iter().all(|i| i.done));
}

#[test]
fn crash_replay_push_is_exactly_once() {
    let rig = Rig::new();
    rig.label_and_tick("conduit:run");
    let sha = rig.remote_sha().expect("pushed");
    // Crash BEFORE mark-done of CommitAndPush (push already happened).
    let mut r = rig.record();
    r.pending = vec![
        intent(Action::CommitAndPush),
        done(Action::OpenPr),
        done(Action::ApplyPrLabels),
        done(Action::LinkComment),
    ];
    rig.save(&r);
    rig.router().recover().unwrap();
    assert_eq!(
        rig.remote_sha().unwrap(),
        sha,
        "ls-remote probe: replay left the remote branch sha unchanged"
    );
    assert!(rig.record().pending.iter().all(|i| i.done));
}

#[test]
fn crash_replay_comment_converges() {
    let rig = Rig::new();
    rig.label_and_tick("conduit:run");
    let mut r = rig.record();
    r.pending = vec![
        done(Action::CommitAndPush),
        done(Action::OpenPr),
        done(Action::ApplyPrLabels),
        intent(Action::LinkComment),
    ];
    rig.save(&r);
    rig.router().recover().unwrap();
    // At-least-once execution ...
    assert_eq!(
        rig.forge
            .count(|a| matches!(a, RecordedAction::UpsertIssueComment { .. })),
        2
    );
    // ... exactly-once effect: the marker upsert replaced, never duplicated.
    assert_eq!(rig.forge.issue_comments(&IssueId(1)).len(), 1);
}

#[test]
fn crash_replay_labels_converge() {
    let rig = Rig::new();
    rig.label_and_tick("conduit:run");
    let mut r = rig.record();
    r.pending = vec![
        done(Action::CommitAndPush),
        done(Action::OpenPr),
        intent(Action::ApplyPrLabels),
        done(Action::LinkComment),
    ];
    rig.save(&r);
    rig.router().recover().unwrap();
    let sets: Vec<Vec<String>> = rig
        .forge
        .actions()
        .into_iter()
        .filter_map(|a| match a {
            RecordedAction::SetPrLabels { labels, .. } => Some(labels),
            _ => None,
        })
        .collect();
    assert_eq!(sets.len(), 2, "at-least-once execution");
    assert_eq!(sets[0], sets[1], "absolute set: final labels identical");
    assert_eq!(
        sets[1].iter().filter(|l| l.starts_with("effort:")).count(),
        1
    );
    assert!(sets[1].contains(&"adr:ADR-0003".to_string()));
}

#[test]
fn cursor_advances_only_after_actions_complete() {
    let rig = Rig::new();
    rig.forge.fail_next("open_pr");
    {
        let mut cur = rig.current.borrow_mut();
        cur.issues[0].labels.push("conduit:run".to_string());
    }
    let result = rig.tick();
    assert!(result.is_err(), "injected open_pr failure fails the tick");
    assert!(
        rig.store.load_cursor("fake").unwrap().is_none(),
        "cursor NOT advanced after a failed action"
    );
    let r = rig.record();
    // InReview here is the RECURSIVE apply's write: the engine ran
    // synchronously inside the tick and wrote InReview (with its full
    // pending intent list) before open_pr was attempted and failed.
    // It is NOT the Coding write-ahead — Coding is only ever a crash state
    // and is never the final persisted value after a synchronous engine run.
    assert_eq!(r.state, TaskState::InReview);
    assert_eq!(r.pr, None);
    assert!(
        r.pending.iter().any(|i| !i.done),
        "failed action stays pending"
    );
    assert_eq!(
        rig.forge.count(|a| matches!(a, RecordedAction::OpenPr(_))),
        0
    );

    // Next tick re-diffs the same snapshot and completes.
    rig.tick_ok();
    assert!(
        rig.store.load_cursor("fake").unwrap().is_some(),
        "cursor advanced once all actions completed"
    );
    let r = rig.record();
    assert_eq!(r.state, TaskState::InReview);
    assert_eq!(r.pr, Some(PrId(1)));
    assert!(r.pending.iter().all(|i| i.done));
    // Probes: no duplicate issue or PR.
    assert_eq!(
        rig.forge
            .count(|a| matches!(a, RecordedAction::CreateIssue(_))),
        1
    );
    assert_eq!(
        rig.forge.count(|a| matches!(a, RecordedAction::OpenPr(_))),
        1
    );
}
