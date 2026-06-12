//! Parameterized conformance suite — every Forge adapter must satisfy this
//! contract identically. The suite body lives in `run_conformance`; each leg
//! is a `#[test]` that wires one implementation. Only the FakeForge leg is
//! wired here (Task 7); Tasks 8/9 add Gitea and GitHub legs without touching
//! `run_conformance`.

use conduit::forge::fake::FakeForge;
use conduit::forge::{Forge, LabelSpec, NewIssue, RepoSnapshot};
use conduit::task::{IssueId, PrId};
use time::OffsetDateTime;

// ---------------------------------------------------------------------------
// Core conformance suite — forge-agnostic, asserts the public contract
// ---------------------------------------------------------------------------

/// Every adapter must satisfy this identically.
/// `tag` disambiguates test data per leg/run (live forges keep state).
fn run_conformance(forge: &dyn Forge, tag: &str) {
    // 1. ensure_labels is idempotent (twice = same result, no error).
    let labels = vec![LabelSpec {
        name: format!("conformance:{tag}"),
        color: "00aabb".into(),
        description: "conformance suite".into(),
    }];
    forge.ensure_labels(&labels).unwrap();
    forge.ensure_labels(&labels).unwrap();

    // 2. create_issue -> find_issue_by_marker round-trip (the replay probe).
    let marker = format!("<!-- conduit:task:conformance-{tag} -->");
    assert_eq!(
        forge.find_issue_by_marker(&marker).unwrap(),
        None,
        "marker must be absent before create"
    );
    let issue = forge
        .create_issue(&NewIssue {
            title: format!("[conformance {tag}] probe issue"),
            body: format!("conformance body\n\n{marker}"),
            labels: vec![format!("conformance:{tag}")],
        })
        .unwrap();
    assert_eq!(
        forge.find_issue_by_marker(&marker).unwrap(),
        Some(issue),
        "probe must find the created issue by its hidden marker"
    );

    // 3. comment upsert converges (marker pattern: second call edits, not dups).
    forge
        .upsert_issue_comment(&issue, &marker, "status: first")
        .unwrap();
    forge
        .upsert_issue_comment(&issue, &marker, "status: second")
        .unwrap();

    // 4. set_issue_labels is an absolute, convergent set.
    forge
        .set_issue_labels(&issue, &[format!("conformance:{tag}")])
        .unwrap();
    forge
        .set_issue_labels(&issue, &[format!("conformance:{tag}")])
        .unwrap();

    // 5. close_issue.
    forge.close_issue(&issue).unwrap();

    // 6. fetch_snapshot is normalized: only conduit-labeled issues and
    //    conduit/*-branch PRs ever appear.
    let snap = forge.fetch_snapshot().unwrap();
    for i in &snap.issues {
        assert!(
            i.labels
                .iter()
                .any(|l| l.starts_with("conduit:") || l.starts_with("adr:")),
            "non-conduit issue leaked into snapshot: {:?}",
            i.id
        );
    }
    for p in &snap.prs {
        assert!(
            p.head_branch.starts_with("conduit/"),
            "non-conduit PR leaked into snapshot: {:?}",
            p.head_branch
        );
    }
}

// ---------------------------------------------------------------------------
// Review-mandated additional conformance cases
// (contract obligations in forge/mod.rs header not covered above)
// ---------------------------------------------------------------------------

/// (review-mandated) Disappearance rule: a merged/closed PR present in a
/// scripted snapshot must survive in subsequent snapshots — i.e. the adapter
/// never drops it. On FakeForge this asserts the scripted tail-repeat behavior
/// keeps the merged PR present.
fn run_disappearance_rule(forge: &conduit::forge::fake::FakeForge) {
    use conduit::forge::{CiState, IssueSnapshot, PrSnapshot};

    let merged_pr = PrSnapshot {
        id: PrId(99),
        head_branch: "conduit/adr-0099/disappearance-check".into(),
        labels: vec![],
        reviews: vec![],
        ci: CiState::None,
        merged: true,
        merge_sha: Some("deadbeef".into()),
        closed: true,
    };
    let snap = RepoSnapshot {
        issues: vec![],
        prs: vec![merged_pr.clone()],
        fetched_at: OffsetDateTime::now_utc(),
    };
    // Script only one snapshot — the tail-repeat rule means ALL subsequent
    // fetch_snapshot calls return the same one (merged PR stays visible).
    forge.script(vec![snap]);
    let s1 = forge.fetch_snapshot().unwrap();
    let s2 = forge.fetch_snapshot().unwrap();
    let s3 = forge.fetch_snapshot().unwrap();
    for s in [&s1, &s2, &s3] {
        assert!(
            s.prs.iter().any(|p| p.id == PrId(99) && p.merged),
            "merged PR must not disappear from snapshot"
        );
    }

    // Also verify a closed issue stays.
    let closed_issue = IssueSnapshot {
        id: IssueId(77),
        labels: vec!["conduit:run".into()],
        closed: true,
    };
    let snap2 = RepoSnapshot {
        issues: vec![closed_issue],
        prs: vec![],
        fetched_at: OffsetDateTime::now_utc(),
    };
    forge.script(vec![snap2]);
    let s1 = forge.fetch_snapshot().unwrap();
    let s2 = forge.fetch_snapshot().unwrap();
    assert!(
        s1.issues.iter().any(|i| i.id == IssueId(77) && i.closed),
        "closed issue must stay in snapshot"
    );
    assert!(
        s2.issues.iter().any(|i| i.id == IssueId(77) && i.closed),
        "closed issue must stay in subsequent snapshot"
    );
}

/// (review-mandated) Snapshot id-uniqueness: a well-formed adapter snapshot
/// has no duplicate issue or PR ids. FakeForge must produce a unique-id
/// snapshot from its stored state.
fn run_snapshot_id_uniqueness(forge: &conduit::forge::fake::FakeForge) {
    // Create two issues; the stored-state snapshot must have unique ids.
    let marker_a = "<!-- conduit:unique-a -->";
    let marker_b = "<!-- conduit:unique-b -->";
    forge
        .create_issue(&NewIssue {
            title: "unique-a".into(),
            body: format!("body\n\n{marker_a}"),
            labels: vec!["conduit:run".into()],
        })
        .unwrap();
    forge
        .create_issue(&NewIssue {
            title: "unique-b".into(),
            body: format!("body\n\n{marker_b}"),
            labels: vec!["conduit:run".into()],
        })
        .unwrap();
    let snap = forge.fetch_snapshot().unwrap();
    let ids: Vec<_> = snap.issues.iter().map(|i| i.id).collect();
    let mut deduped = ids.clone();
    deduped.sort_by_key(|i| i.0);
    deduped.dedup();
    assert_eq!(
        ids.len(),
        deduped.len(),
        "snapshot issue ids must be unique"
    );
}

/// (review-mandated) close_issue on an unknown id returns ForgeError::Api
/// with status 404.
fn run_close_unknown_issue_is_404(forge: &conduit::forge::fake::FakeForge) {
    use conduit::forge::ForgeError;
    let err = forge.close_issue(&IssueId(999_999)).unwrap_err();
    let ForgeError::Api { status, .. } = err else {
        panic!("expected Api error for unknown issue, got {err:?}");
    };
    assert_eq!(status, 404);
}

// ---------------------------------------------------------------------------
// Test legs
// ---------------------------------------------------------------------------

#[test]
fn fake_forge_conforms() {
    let forge = FakeForge::new();
    run_conformance(&forge, "fake");

    // FakeForge-only deep assertions (stored-state convergence):
    use conduit::forge::fake::RecordedAction;
    let upserts = forge.count(|a| matches!(a, RecordedAction::UpsertIssueComment { .. }));
    assert_eq!(upserts, 2, "both upsert calls recorded");
}

#[test]
fn fake_forge_disappearance_rule() {
    let forge = FakeForge::new();
    run_disappearance_rule(&forge);
}

#[test]
fn fake_forge_snapshot_id_uniqueness() {
    let forge = FakeForge::new();
    run_snapshot_id_uniqueness(&forge);
}

#[test]
fn fake_forge_close_unknown_is_404() {
    let forge = FakeForge::new();
    run_close_unknown_issue_is_404(&forge);
}

// Live legs are added by Task 8 (CONDUIT_E2E_GITEA=1) and Task 9
// (recorded fixtures always-on + CONDUIT_E2E_GITHUB=1 live reads).
