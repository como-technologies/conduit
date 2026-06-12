use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn conduit(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("conduit").unwrap();
    cmd.current_dir(dir.path());
    // Hermetic env: drop any developer overrides.
    for var in [
        "CONDUIT_FORGE",
        "CONDUIT_ENGINE",
        "CONDUIT_GITEA_TOKEN",
        "CONDUIT_TIMEOUT_SECS",
        "CONDUIT_POLL_SECS",
        "CONDUIT_ADROIT_BIN",
        "CONDUIT_FAKE_ENGINE_MODE",
        "GITHUB_TOKEN",
        "GITLAB_TOKEN",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

/// Point the gitea adapter at a port nothing listens on: every forge call
/// fails fast with the typed Offline error, keeping these tests hermetic.
fn unreachable_gitea_config(dir: &TempDir) {
    std::fs::write(
        dir.path().join("conduit.toml"),
        "[forge.gitea]\nbase_url = \"http://127.0.0.1:1\"\n",
    )
    .unwrap();
}

/// Self-contained adroit stub: Accepted ADR-0042 + a stored plan. The `plan`
/// subcommand REFUSES a second invocation (sentinel file) — that is how the
/// replay test proves the snapshot is never regenerated.
fn write_plan_stub(dir: &Path) -> PathBuf {
    let path = dir.join("stub-adroit");
    let script = r####"#!/bin/sh
case "$1" in
  manifest) echo '{"tool":"adroit","manifest_schema":1}' ;;
  show) cat <<'EOF'
{"reference":"ADR-0042","address":"42","title":"Use Rust","status":"Accepted","body":"## Context\n\nwords\n"}
EOF
  ;;
  plan)
    if [ -e "$0.plan-called" ]; then echo "adroit plan invoked twice" >&2; exit 7; fi
    : > "$0.plan-called"
    cat <<'EOF'
{"reference":"ADR-0042","title":"Use Rust","plan":"# Plan\n\n1. step one\n2. step two\n","stored":true}
EOF
  ;;
  *) echo "unexpected subcommand: $*" >&2; exit 2 ;;
esac
"####;
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn help_lists_all_subcommands() {
    let d = TempDir::new().unwrap();
    conduit(&d)
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("plan"))
        .stdout(predicate::str::contains("run"))
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("verify"))
        .stdout(predicate::str::contains("demo-transcript"));
}

#[test]
fn status_json_on_empty_store_is_empty_array() {
    let d = TempDir::new().unwrap();
    conduit(&d)
        .args(["status", "-o", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[]"));
}

#[test]
fn invalid_env_override_fails_loudly() {
    // The env overlay is consulted before anything runs: a bogus value is a
    // typed config error naming the variable, not a silent fallback.
    let d = TempDir::new().unwrap();
    conduit(&d)
        .env("CONDUIT_ENGINE", "bogus-engine")
        .args(["run", "--once"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("CONDUIT_ENGINE"));
}

#[test]
fn run_once_surfaces_the_typed_offline_error() {
    let d = TempDir::new().unwrap();
    unreachable_gitea_config(&d);
    conduit(&d)
        .args(["run", "--once"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("forge unreachable"));
}

#[test]
fn init_fails_on_unreachable_forge_but_opens_the_store() {
    let d = TempDir::new().unwrap();
    unreachable_gitea_config(&d);
    conduit(&d)
        .arg("init")
        .assert()
        .failure()
        .stderr(predicate::str::contains("forge unreachable"));
    // Store::open ran first: the on-disk layout exists.
    for sub in ["tasks", "plans", "cursor", "cache", "workspaces", "bin"] {
        assert!(
            d.path().join(".conduit").join(sub).is_dir(),
            ".conduit/{sub} must exist"
        );
    }
}

#[test]
fn verify_fails_cleanly_on_unknown_task() {
    let d = TempDir::new().unwrap();
    conduit(&d)
        .args(["verify", "3", "-o", "json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no task for ADR address"))
        .stderr(predicate::str::contains("conduit plan 3"));
}

#[test]
fn verify_refuses_an_unmerged_task_before_touching_the_forge() {
    let d = TempDir::new().unwrap();
    unreachable_gitea_config(&d);
    // A Scoped record straight into the store (the verify gate must fire
    // BEFORE any forge call — the unreachable forge would error differently).
    std::fs::create_dir_all(d.path().join(".conduit/tasks")).unwrap();
    std::fs::write(
        d.path().join(".conduit/tasks/adr-0042.json"),
        r#"{
          "id": "adr-0042", "adr_reference": "ADR-0042", "adr_address": "42",
          "title": "Use Rust", "state": "Scoped",
          "branch": "conduit/adr-0042/use-rust",
          "issue": null, "pr": null, "attempt": 1, "work_ms": 0,
          "plan_sha256": "x", "review_feedback": [], "pending": []
        }"#,
    )
    .unwrap();
    conduit(&d)
        .args(["verify", "42"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not Merged"))
        .stderr(predicate::str::contains("forge unreachable").not());
}

/// GAP B: cmd_plan rejects a terminal-state task immediately with the
/// "cancel + new task" message, and MUST NOT invoke `adroit plan` at all.
/// The stub sentinel guarantees the plan subcommand was never reached.
#[test]
fn plan_bails_on_terminal_task_without_invoking_adroit() {
    let d = TempDir::new().unwrap();
    // Write a Merged task record directly into the store — no forge needed.
    std::fs::create_dir_all(d.path().join(".conduit/tasks")).unwrap();
    std::fs::write(
        d.path().join(".conduit/tasks/adr-0042.json"),
        r#"{
          "id": "adr-0042", "adr_reference": "ADR-0042", "adr_address": "42",
          "title": "Use Rust", "state": "Merged",
          "branch": "conduit/adr-0042/use-rust",
          "issue": 1, "pr": 7, "attempt": 1, "work_ms": 0,
          "plan_sha256": "x", "review_feedback": [], "pending": []
        }"#,
    )
    .unwrap();
    let stub = write_plan_stub(d.path());
    conduit(&d)
        .env("CONDUIT_ADROIT_BIN", &stub)
        .args(["plan", "42"])
        .assert()
        .failure()
        // The exact bail message from cmd_plan (spec §terminal bail).
        .stderr(predicate::str::contains("cancel + new task"))
        // adroit plan must NOT have been invoked — the stub writes a sentinel
        // file on its first plan call and exits 7 on the second.  Neither
        // should have happened; the sentinel must be absent.
        .stderr(predicate::str::contains("adroit plan invoked twice").not());
    // Belt-and-suspenders: the sentinel file must not exist at all.
    assert!(
        !stub.with_extension("plan-called").exists()
            && !d.path().join("stub-adroit.plan-called").exists(),
        "adroit plan must not have been called for a terminal task"
    );
}

// GAP C (CLI-level): verify exits non-zero when the task is not yet Merged.
// This is already covered by verify_refuses_an_unmerged_task_before_touching_the_forge
// above; this comment documents why a full forge-backed verify CLI test is not
// added here: the forge call in cmd_verify requires a live or stub forge that
// can serve fetch_snapshot with the PR present, which is exercised end-to-end
// in Task 14 of the demo transcript. The violation-detection logic itself
// (tuesday_checks returning pass=false) is unit-tested exhaustively in
// src/cli.rs tests::tuesday_checks_violation_yields_pass_false.

/// The spec §Plan snapshot ordering, end-to-end: `conduit plan` persists the
/// verbatim snapshot (+ ADR-body sidecar + Scoped record) BEFORE the forge
/// call — pointing gitea at an unreachable port, the command fails with the
/// typed Offline error and the snapshot is already on disk.
#[test]
fn plan_via_stub_adroit_creates_scoped_record() {
    let d = TempDir::new().unwrap();
    unreachable_gitea_config(&d);
    let stub = write_plan_stub(d.path());

    conduit(&d)
        .env("CONDUIT_ADROIT_BIN", &stub)
        .args(["plan", "42"])
        .assert()
        .failure()
        // Operator-facing provenance: stored vs generated.
        .stderr(predicate::str::contains("stored plan"))
        // The failure is the forge, AFTER the snapshot persisted.
        .stderr(predicate::str::contains("forge unreachable"));

    // Snapshot: exact bytes, before the issue ever existed.
    let plan = std::fs::read_to_string(d.path().join(".conduit/plans/adr-0042.md")).unwrap();
    assert_eq!(plan, "# Plan\n\n1. step one\n2. step two\n");
    // ADR-body sidecar (the engine-seam context, spec §The engine seam).
    let body = std::fs::read_to_string(d.path().join(".conduit/plans/adr-0042.adr.md")).unwrap();
    assert_eq!(body, "## Context\n\nwords\n");
    // Scoped record with the snapshot sha, no issue yet.
    let record: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(d.path().join(".conduit/tasks/adr-0042.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["state"], "Scoped");
    assert_eq!(record["issue"], serde_json::Value::Null);
    assert_eq!(
        record["plan_sha256"],
        "93d39bc9dde72bcf91a2efaff012ae7508a1105e93025bf03cfa10dae12d832f"
    );

    // Replay: the second run converges on the existing snapshot and NEVER
    // calls `adroit plan` again (the stub would fail loudly if it did).
    conduit(&d)
        .env("CONDUIT_ADROIT_BIN", &stub)
        .args(["plan", "42"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already planned"))
        .stderr(predicate::str::contains("adroit plan invoked twice").not())
        .stderr(predicate::str::contains("forge unreachable"));
    let replayed = std::fs::read_to_string(d.path().join(".conduit/plans/adr-0042.md")).unwrap();
    assert_eq!(replayed, plan, "snapshot untouched by the replay");
}

/// The github transcript leg is hermetic by construction (DryRun mutations,
/// no polling, no git): two runs must be byte-identical, normalized JSONL.
#[test]
fn demo_transcript_github_leg_is_deterministic_normalized_jsonl() {
    let d = TempDir::new().unwrap();
    let run = || -> String {
        let out = conduit(&d)
            .args(["demo-transcript", "3", "--forge", "github"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let first = run();
    let second = run();
    assert_eq!(first, second, "the transcript must diff clean across runs");

    let lines: Vec<&str> = first.lines().collect();
    assert_eq!(lines.len(), 7, "the full scripted lifecycle: {first}");
    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("JSONL");
        assert!(v.get("action").is_some(), "line names its action: {line}");
    }
    assert!(first.contains("\"$ISSUE_1\""));
    assert!(first.contains("\"$PR_1\""));
    assert!(first.contains("effort:$REDACTED"));
    assert!(
        !first.contains("9000000001"),
        "synthesized DryRun ids must never leak raw"
    );
    assert!(!first.contains("_at\""), "timestamps omitted");
}

/// The gitlab transcript leg is hermetic by the same construction (DryRun
/// mutations, no polling, no git) — and the N=3 cross-leg assertion: its
/// normalized stream must be BYTE-IDENTICAL to the github leg's, because
/// both ride the one shared emitter/normalizer. The live gitea third way is
/// the demo-kit beat (it needs the throwaway forge).
#[test]
fn demo_transcript_gitlab_leg_matches_the_github_leg_byte_for_byte() {
    let d = TempDir::new().unwrap();
    let run = |forge: &str| -> String {
        let out = conduit(&d)
            .args(["demo-transcript", "3", "--forge", forge])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let gitlab_first = run("gitlab");
    let gitlab_second = run("gitlab");
    assert_eq!(
        gitlab_first, gitlab_second,
        "the gitlab transcript must diff clean across runs"
    );
    let github = run("github");
    assert_eq!(
        gitlab_first, github,
        "gitlab and github record-only legs must be byte-identical"
    );
    assert_eq!(
        gitlab_first.lines().count(),
        7,
        "the full scripted lifecycle: {gitlab_first}"
    );
}
