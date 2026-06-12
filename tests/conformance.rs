//! Parameterized conformance suite — every Forge adapter must satisfy this
//! contract identically. The suite body lives in `run_conformance`; each leg
//! is a `#[test]` that wires one implementation. Only the FakeForge leg is
//! wired here (Task 7); Tasks 8/9 add Gitea and GitHub legs without touching
//! `run_conformance`.

use conduit::contract;
use conduit::forge::fake::FakeForge;
use conduit::forge::{Forge, LabelSpec, NewIssue, PrDraft, RepoSnapshot};
use conduit::task::{IssueId, PrId};
use time::OffsetDateTime;

// ---------------------------------------------------------------------------
// Core conformance suite — forge-agnostic, asserts the public contract
// ---------------------------------------------------------------------------

/// How the leg's mutations behave — the documented honest-claim boundary
/// (spec §Risks: dry-run proves the stream, not GitHub's acceptance).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Mutations {
    /// Mutations really execute on the forge; read-backs observe them.
    Real,
    /// Mutations only reach the DryRun transcript; read-back assertions that
    /// need them visible on the forge are skipped. Every mutation is still
    /// CALLED (asserting `Ok`), and snapshot normalization always runs.
    DryRun,
}

/// Every adapter must satisfy this identically.
/// `tag` disambiguates test data per leg/run (live forges keep state).
fn run_conformance(forge: &dyn Forge, tag: &str, mutations: Mutations) {
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
    if mutations == Mutations::Real {
        // Read-back: a DryRun create never reaches the forge, so only Real
        // legs can observe it through the probe.
        assert_eq!(
            forge.find_issue_by_marker(&marker).unwrap(),
            Some(issue),
            "probe must find the created issue by its hidden marker"
        );
    }

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

    // 7. PR mutation round-trip.
    //
    // 7a. open_pr: the adapter returns a live PR id with the expected head.
    let pr_head = format!("conduit/adr-0001/conformance-{tag}");
    let pr_body_text = contract::pr_body("ADR-0001", "conformance PR body");
    let draft = PrDraft {
        head: pr_head.clone(),
        base: "main".into(),
        title: format!("[conformance {tag}] probe PR"),
        body: pr_body_text,
        labels: vec![
            "effort:1-super-quick".to_string(),
            "adr:ADR-0001".to_string(),
        ],
    };
    let pr_id = forge.open_pr(&draft).unwrap();

    // 7b. find_open_pr_by_head must locate the opened PR by its exact head —
    //     on a DryRun leg this resolves through the recorded-open-PRs overlay
    //     (the replay probe must keep working even though the PR never
    //     reached the real forge).
    assert_eq!(
        forge.find_open_pr_by_head(&pr_head).unwrap(),
        Some(pr_id),
        "find_open_pr_by_head must return the opened PR id"
    );

    // 7c. find_open_pr_by_head on a non-existent head returns None.
    assert_eq!(
        forge.find_open_pr_by_head("conduit/none/missing").unwrap(),
        None,
        "find_open_pr_by_head must return None for an unknown head"
    );

    // 7d. upsert_pr_comment is idempotent on the same marker (no error on
    //     the second call; stored state converges).
    let pr_marker = format!("<!-- conduit:pr:conformance-{tag} -->");
    forge
        .upsert_pr_comment(&pr_id, &pr_marker, "pr status: first")
        .unwrap();
    forge
        .upsert_pr_comment(&pr_id, &pr_marker, "pr status: second")
        .unwrap();

    // 7e. set_pr_labels converges: two calls with different label sets, no error.
    forge
        .set_pr_labels(
            &pr_id,
            &[
                "effort:1-super-quick".to_string(),
                "adr:ADR-0001".to_string(),
            ],
        )
        .unwrap();
    forge
        .set_pr_labels(
            &pr_id,
            &["effort:2-not-long".to_string(), "adr:ADR-0001".to_string()],
        )
        .unwrap();

    // 7f. fetch_snapshot after PR open must show the PR with the correct head
    //     (Real legs only — snapshots delegate to live reads, which cannot
    //     see a DryRun-recorded PR; the snapshot itself must still succeed).
    let snap2 = forge.fetch_snapshot().unwrap();
    if mutations == Mutations::Real {
        assert!(
            snap2.prs.iter().any(|p| p.head_branch == pr_head),
            "fetch_snapshot must include the opened PR (head: {pr_head})"
        );
    }
}

// ---------------------------------------------------------------------------
// FakeForge-typed helper legs
//
// These three functions assert adapter-contract behaviors that require
// FakeForge's script() machinery or direct state inspection — they cannot be
// expressed through the Forge trait alone. Tasks 8/9 must cover the equivalent
// behaviors (disappearance rule, 404/None on unknown ids) through their own
// fixture/live mechanisms; run_conformance above is the shared body to extend
// for any new adapter-agnostic assertions.
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
    run_conformance(&forge, "fake", Mutations::Real);

    // FakeForge-only deep assertions (stored-state convergence):
    use conduit::forge::fake::RecordedAction;
    let issue_upserts = forge.count(|a| matches!(a, RecordedAction::UpsertIssueComment { .. }));
    assert_eq!(issue_upserts, 2, "both issue upsert calls recorded");
    let pr_upserts = forge.count(|a| matches!(a, RecordedAction::UpsertPrComment { .. }));
    assert_eq!(pr_upserts, 2, "both PR upsert calls recorded");
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

/// Recorded-fixture GitHub leg — ALWAYS ON, no network. Reads come from
/// tests/fixtures/github/ (recorded from the public fixture repo), mutations
/// go to the DryRun transcript; the suite's read-side assertions run;
/// mutation assertions check the transcript shape.
#[test]
fn github_recorded_fixtures_conform() {
    let forge = conduit::forge::github::fixture_forge("tests/fixtures/github");
    run_conformance(&forge, "gh-fixture", Mutations::DryRun);

    // The DryRun transcript is the mutation-side evidence: every mutation
    // the suite issued, normalized — ids -> first-seen placeholders, effort
    // label values redacted, repo slug -> $REPO, no timestamps, one JSON
    // object per line with the action key.
    let t = forge.transcript();
    assert_eq!(
        t.len(),
        13,
        "all 13 suite mutations recorded (2 ensure_labels, create_issue, \
         2 issue upserts, 2 set_issue_labels, close_issue, open_pr, \
         2 PR upserts, 2 set_pr_labels): {t:#?}"
    );
    for line in &t {
        let v: serde_json::Value = serde_json::from_str(line).expect("transcript line is JSON");
        assert!(v.get("action").is_some(), "line names its action: {line}");
        assert!(!line.contains("_at\""), "timestamps omitted: {line}");
    }
    assert!(t.iter().any(|l| l.contains("\"$ISSUE_1\"")));
    assert!(t.iter().any(|l| l.contains("\"$PR_1\"")));
    assert!(t.iter().any(|l| l.contains("effort:$REDACTED")));
    assert!(
        t.iter()
            .all(|l| !l.contains("super-quick") && !l.contains("not-long")),
        "effort label VALUES must never appear in a transcript"
    );
    assert!(
        t.iter().all(|l| !l.contains(&format!(
            "{}/{}",
            conduit::forge::github::FIXTURE_OWNER,
            conduit::forge::github::FIXTURE_REPO
        ))),
        "repo slug must be redacted to $REPO"
    );
}

/// Live GitHub READS leg (CONDUIT_E2E_GITHUB=1): fetch_snapshot + probes
/// against the real API; mutations still hit only the DryRun transcript.
/// Read-side normalization assertions only — no lifecycle on a real repo.
#[test]
fn github_live_reads_conform() {
    if std::env::var("CONDUIT_E2E_GITHUB").as_deref() != Ok("1") {
        eprintln!("skip: set CONDUIT_E2E_GITHUB=1 (with GITHUB_TOKEN or `gh auth login`)");
        return;
    }
    let token = conduit::forge::github::resolve_token().expect("GITHUB_TOKEN or gh login");
    let cfg = conduit::config::GithubConfig {
        owner: conduit::forge::github::FIXTURE_OWNER.into(),
        repo: conduit::forge::github::FIXTURE_REPO.into(),
    };
    let forge = conduit::forge::github::open_github(&cfg, token);
    let snap = forge.fetch_snapshot().unwrap();
    for i in &snap.issues {
        assert!(
            i.labels
                .iter()
                .any(|l| l.starts_with("conduit:") || l.starts_with("adr:")),
            "non-conduit issue leaked into live snapshot: {:?}",
            i.id
        );
    }
    for p in &snap.prs {
        assert!(
            p.head_branch.starts_with("conduit/"),
            "non-conduit PR leaked into live snapshot: {:?}",
            p.head_branch
        );
    }
    assert_eq!(
        forge.find_open_pr_by_head("conduit/never-exists").unwrap(),
        None
    );
    assert_eq!(
        forge
            .find_issue_by_marker("<!-- conduit:task:never -->")
            .unwrap(),
        None
    );
}

/// Live Gitea leg — needs `just forge-up` first. The tag is time-randomized
/// so re-runs don't collide on the persistent container state.
#[test]
fn gitea_live_conforms() {
    if std::env::var("CONDUIT_E2E_GITEA").as_deref() != Ok("1") {
        eprintln!("skip: set CONDUIT_E2E_GITEA=1 (and run `just forge-up`)");
        return;
    }
    let token = std::fs::read_to_string(".secrets/conduit-bot.token")
        .expect("run `just forge-up` first")
        .trim()
        .to_string();
    let cfg = conduit::config::GiteaConfig::default();
    let forge = conduit::forge::gitea::GiteaForge::open(&cfg, token.clone());
    let tag = format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );
    // run_conformance opens a PR from head `conduit/adr-0001/conformance-{tag}`.
    // On a real forge that branch must exist BEFORE open_pr (in production
    // conduit's git layer pushes it first — see PrDraft::head). Seed it with
    // one committed file via the contents API.
    seed_branch(
        &cfg,
        &token,
        &format!("conduit/adr-0001/conformance-{tag}"),
        &tag,
    );
    run_conformance(&forge, &tag, Mutations::Real);
}

/// Create `branch` off main with one new file (Gitea contents API:
/// `new_branch` creates the branch as part of the commit).
fn seed_branch(cfg: &conduit::config::GiteaConfig, token: &str, branch: &str, tag: &str) {
    use conduit::forge::{HttpTransport, UreqTransport};
    let url = format!(
        "{}/api/v1/repos/{}/{}/contents/conformance-{tag}.txt",
        cfg.base_url, cfg.owner, cfg.repo
    );
    let body = serde_json::json!({
        "content": base64(b"conformance probe\n"),
        "message": format!("test: conformance probe {tag}"),
        "branch": "main",
        "new_branch": branch,
    });
    let auth = format!("token {token}");
    let resp = UreqTransport
        .request(
            "POST",
            &url,
            &[
                ("Authorization", &auth),
                ("Content-Type", "application/json"),
            ],
            Some(&serde_json::to_vec(&body).unwrap()),
        )
        .expect("seed branch request");
    assert!(
        (200..300).contains(&resp.status),
        "seed branch failed: HTTP {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );
}

/// Minimal RFC 4648 base64 (the contents API wants base64 content; not worth
/// a dependency for one test helper).
fn base64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
