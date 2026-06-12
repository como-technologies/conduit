//! Cross-assertion (done-criterion 5, follow-up 3): the live router stack and
//! the demo-transcript stack must emit BYTE-IDENTICAL forge action payloads
//! for the same inputs. Both now build every payload via src/payload.rs; this
//! test proves it END-TO-END by driving the transcript's scripted scenario
//! through the REAL Router (scripted snapshots + ticks) and through
//! transcript::run, on two FakeForges that record every mutation verbatim
//! (raw payloads — no transcript normalization in the comparison).
//!
//! The ONE documented divergence (payload.rs module doc): the link comment
//! names the PR by raw number on the router and by `$PR_n` placeholder on the
//! transcript leg — pinned here by substituting exactly that token.

use std::path::Path;

use conduit::config::Config;
use conduit::contract;
use conduit::engine::fake::{FakeEngine, FakeMode};
use conduit::forge::fake::{FakeForge, RecordedAction};
use conduit::forge::{CiState, IssueSnapshot, PrSnapshot, RepoSnapshot, Review};
use conduit::router::Router;
use conduit::store::Store;
use conduit::task::{ReviewId, ReviewVerdict, TaskRecord};
use conduit::transcript::{self, FIXTURE_MERGE_SHA, FIXTURE_TITLE, GitContext};
use tempfile::TempDir;
use time::OffsetDateTime;

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

fn snap(issues: Vec<IssueSnapshot>, prs: Vec<PrSnapshot>) -> RepoSnapshot {
    RepoSnapshot {
        issues,
        prs,
        fetched_at: OffsetDateTime::now_utc(),
    }
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

/// The REAL router driven through the transcript's scripted scenario with the
/// transcript's exact task identity (same id, title, plan — the "same
/// inputs" of the cross-assertion).
fn router_leg(dir: &TempDir) -> Vec<RecordedAction> {
    let remote = seed_remote(dir.path());
    let store = Store::open(dir.path().join(".conduit")).unwrap();
    let plan = transcript::fixture_plan("ADR-0003");
    let mut record = TaskRecord::new("ADR-0003", "3", FIXTURE_TITLE, "");
    record.id = format!("{}-transcript", record.id); // the transcript identity
    record.plan_sha256 = store.save_plan(&record.id, &plan).unwrap();
    store.save_task(&record).unwrap();

    let forge = FakeForge::new();
    forge.set_remote_url(&remote);
    let engine = FakeEngine {
        mode: FakeMode::Complete,
    };
    let config = Config::default();
    let router = Router::new(&forge, "fake", &engine, &store, &config, "main");

    // The `conduit plan` beat.
    router.ensure_issue(&mut record).unwrap();
    let issue = record.issue.expect("ensure_issue sets the id");
    let issue_snap = IssueSnapshot {
        id: issue,
        labels: vec![
            contract::adr_label("ADR-0003"),
            contract::LABEL_RUN.to_string(),
        ],
        closed: false,
    };

    // Beat 1: conduit:run → Coding → engine completes → InReview
    // (CommitAndPush, OpenPr, ApplyPrLabels, LinkComment).
    forge.script(vec![snap(vec![issue_snap.clone()], vec![])]);
    router.tick().unwrap();

    // Beat 2: ChangesRequested (the fixture review) → Revising → engine
    // completes → InReview (CommitAndPush, ApplyPrLabels).
    let record = store.load_task(&record.id).unwrap().unwrap();
    let pr = record.pr.expect("the tick opened the PR");
    let pr_snap = PrSnapshot {
        id: pr,
        title: contract::pr_title("ADR-0003", FIXTURE_TITLE),
        body: contract::pr_body("ADR-0003", plan.trim_end()),
        head_branch: record.branch.clone(),
        labels: vec![],
        reviews: vec![Review {
            id: ReviewId("r1".into()),
            author: "reviewer".into(),
            verdict: ReviewVerdict::ChangesRequested,
            body: "Please tighten the docs.".into(),
            submitted_at: OffsetDateTime::now_utc(),
        }],
        ci: CiState::None,
        merged: false,
        merge_sha: None,
        closed: false,
    };
    forge.script(vec![snap(vec![issue_snap.clone()], vec![pr_snap.clone()])]);
    router.tick().unwrap();

    // Beat 3: PrMerged with the fixture sha → Merged (close comment + close).
    let mut merged = pr_snap;
    merged.merged = true;
    merged.closed = true;
    merged.merge_sha = Some(FIXTURE_MERGE_SHA.to_string());
    forge.script(vec![snap(vec![issue_snap], vec![merged])]);
    router.tick().unwrap();

    forge.actions()
}

/// The transcript stack on its own forge + remote, same fixture inputs.
fn transcript_leg(dir: &TempDir) -> Vec<RecordedAction> {
    let remote = seed_remote(dir.path());
    let forge = FakeForge::new();
    forge.set_remote_url(&remote);
    let git = GitContext {
        remote_url: remote,
        auth: None,
        cache_dir: dir.path().join("cache.git"),
        workspace_root: dir.path().join("workspaces"),
        base_branch: "main".into(),
    };
    let config = Config::default();
    transcript::run(&forge, None, "ADR-0003", "3", &config, Some(&git)).unwrap();
    forge.actions()
}

/// Substitute the documented divergence: the transcript link comment names
/// the PR `$PR_1` where the router wrote the raw number `1` (FakeForge ids
/// are deterministic). Everything else must match without help.
fn substitute_pr_display(action: &RecordedAction) -> RecordedAction {
    match action {
        RecordedAction::UpsertIssueComment { id, marker, body } => {
            RecordedAction::UpsertIssueComment {
                id: *id,
                marker: marker.clone(),
                body: body.replace("$PR_1", "1"),
            }
        }
        other => other.clone(),
    }
}

#[test]
fn router_and_transcript_emit_byte_identical_payloads() {
    let da = TempDir::new().unwrap();
    let db = TempDir::new().unwrap();
    let a = router_leg(&da);
    let b = transcript_leg(&db);

    // Same action sequence — the full lifecycle's forge mutations, in order.
    let kinds: Vec<&'static str> = a.iter().map(kind).collect();
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
        ],
        "router leg action sequence"
    );
    assert_eq!(
        kinds,
        b.iter().map(kind).collect::<Vec<_>>(),
        "transcript leg action sequence"
    );

    // Byte-identical payloads, pairwise.
    for (i, (ra, rb)) in a.iter().zip(b.iter()).enumerate() {
        assert_eq!(
            *ra,
            substitute_pr_display(rb),
            "payload {i} ({}) diverged between router and transcript",
            kind(ra)
        );
    }
}
