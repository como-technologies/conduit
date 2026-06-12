//! adroit integration contract (spec §adroit integration): handshake gate,
//! Accepted-only, superseded skip, plan-snapshot-verbatim, allowlist.
//! Hermetic by default via self-contained stub binaries written per test
//! (the AdrSource child env is CONSTRUCTED — env_clear + allowlist — so
//! FAKE_ADROIT_* fixture vars cannot pass through; canned JSON is embedded
//! in the stub instead). The PINNED binary runs the same assertions against
//! tests/fixtures/corpus behind CONDUIT_E2E_ADROIT=1 (requires
//! `just init-adroit`).

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use conduit::adroit::{AdrSource, AdroitError};
use conduit::config::AdroitConfig;
use conduit::store::Store;
use predicates::prelude::*;
use tempfile::TempDir;

/// Write a self-contained executable adroit stub answering each subcommand
/// with the given JSON (heredoc-embedded; no env plumbing needed).
fn write_stub(dir: &Path, manifest: &str, show: &str, plan: &str) -> PathBuf {
    let path = dir.join("stub-adroit");
    let script = format!(
        "#!/bin/sh\n\
         case \"$1\" in\n\
           manifest) cat <<'EOF'\n{manifest}\nEOF\n;;\n\
           show) cat <<'EOF'\n{show}\nEOF\n;;\n\
           plan) cat <<'EOF'\n{plan}\nEOF\n;;\n\
           *) echo \"unexpected subcommand: $*\" >&2; exit 2;;\n\
         esac\n"
    );
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

const GOOD_MANIFEST: &str = r#"{"tool":"adroit","manifest_schema":1,"extra":"tolerated"}"#;

fn conduit_in(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("conduit").unwrap();
    cmd.current_dir(dir.path());
    for var in [
        "CONDUIT_FORGE",
        "CONDUIT_ENGINE",
        "CONDUIT_GITEA_TOKEN",
        "CONDUIT_TIMEOUT_SECS",
        "CONDUIT_POLL_SECS",
        "CONDUIT_ADROIT_BIN",
        "CONDUIT_FAKE_ENGINE_MODE",
        "GITHUB_TOKEN",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

#[test]
fn handshake_gate_blocks_wrong_schema() {
    let d = TempDir::new().unwrap();
    let stub = write_stub(
        d.path(),
        r#"{"tool":"adroit","manifest_schema":2}"#,
        "{}",
        "{}",
    );
    let src = AdrSource::new(stub, d.path().into(), &AdroitConfig::default());
    let err = src.handshake().unwrap_err();
    assert!(
        matches!(err, AdroitError::Handshake(_)),
        "manifest_schema 2 must fail the gate: {err}"
    );
    assert!(err.to_string().contains("manifest_schema"));
}

#[test]
fn plan_is_persisted_verbatim_with_sha() {
    let d = TempDir::new().unwrap();
    // EXACT bytes the stub envelope decodes to, and their sha256 (precomputed
    // with sha256sum — the integration crate has no hashing dependency).
    let plan_md = "# Plan\n\n1. step one\n2. step two\n";
    let plan_sha = "93d39bc9dde72bcf91a2efaff012ae7508a1105e93025bf03cfa10dae12d832f";
    let stub = write_stub(
        d.path(),
        GOOD_MANIFEST,
        "{}",
        r###"{"reference":"ADR-0042","title":"Use Rust","plan":"# Plan\n\n1. step one\n2. step two\n","stored":true}"###,
    );
    let src = AdrSource::new(stub, d.path().into(), &AdroitConfig::default());
    let envelope = src.plan("42").unwrap();
    assert!(envelope.stored);
    assert_eq!(
        envelope.plan, plan_md,
        "envelope carries the exact markdown"
    );

    let store = Store::open(d.path().join(".conduit")).unwrap();
    let sha = store.save_plan("adr-0042", &envelope.plan).unwrap();
    assert_eq!(
        store.load_plan("adr-0042").unwrap(),
        plan_md,
        "snapshot is byte-verbatim"
    );
    assert_eq!(
        sha, plan_sha,
        "recorded sha is the sha256 of the exact bytes"
    );
}

/// Accepted-only (and the superseded case), through the FULL `conduit plan`
/// CLI path: the stub binary is injected via CONDUIT_ADROIT_BIN — the
/// documented test seam, resolved inside src/adroit.rs only.
#[test]
fn accepted_only_and_superseded_skip() {
    for status in ["Proposed", "Superseded"] {
        let d = TempDir::new().unwrap();
        let show = format!(
            r#"{{"reference":"ADR-0042","address":"42","title":"Use Rust","status":"{status}","body":"b"}}"#
        );
        let stub = write_stub(d.path(), GOOD_MANIFEST, &show, "{}");
        conduit_in(&d)
            .env("CONDUIT_ADROIT_BIN", &stub)
            .args(["plan", "42"])
            .assert()
            .failure()
            .stderr(predicate::str::contains(status))
            .stderr(predicate::str::contains("not Accepted"));
        // The guard fires BEFORE planning: no snapshot, no task record.
        assert!(
            !d.path().join(".conduit/plans/adr-0042.md").exists(),
            "{status}: no plan snapshot may be written"
        );
        assert!(
            !d.path().join(".conduit/tasks/adr-0042.json").exists(),
            "{status}: no task record may be written"
        );
    }
}

/// The lane boundary, CRATE-WIDE: outside src/adroit.rs (and this file, which
/// names the patterns), no file under src/ or tests/ may invoke the adroit
/// binary or carry its path fragment. Fixture corpora are excluded — the
/// allowlist governs what conduit's CODE invokes, not test data.
#[test]
fn subcommand_allowlist_holds_crate_wide() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "fixtures") {
                    continue;
                }
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(&root.join("src"), &mut files);
    walk(&root.join("tests"), &mut files);
    let mut offenders = Vec::new();
    for f in files {
        // src/adroit.rs owns the path + invocation; this file names the
        // patterns in order to scan for them.
        if f == root.join("src/adroit.rs") || f == root.join("tests/adroit_contract.rs") {
            continue;
        }
        let content = std::fs::read_to_string(&f).unwrap();
        if content.contains("bin/adroit") || content.contains("Command::new(\"adroit\"") {
            offenders.push(f);
        }
    }
    assert!(
        offenders.is_empty(),
        "adroit invoked/named outside src/adroit.rs: {offenders:?}"
    );
}

/// The PINNED binary against the committed fixture corpus (by-status profile,
/// generated by the pinned `adroit new`/`set-status`/`supersede` — never
/// hand-invented). Gated: CONDUIT_E2E_ADROIT=1 after `just init-adroit`.
#[test]
fn pinned_adroit_against_fixture_corpus() {
    if std::env::var("CONDUIT_E2E_ADROIT").as_deref() != Ok("1") {
        eprintln!("skip: set CONDUIT_E2E_ADROIT=1 (and run `just init-adroit`)");
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // resolve_bin owns the pinned path — this test never names it.
    let bin = AdrSource::resolve_bin(&root);
    assert!(bin.exists(), "run `just init-adroit` first: {bin:?}");
    let corpus = root.join("tests/fixtures/corpus");
    let src = AdrSource::new(bin, corpus, &AdroitConfig::default());

    src.handshake().expect("pinned handshake");

    let rows = src.list_accepted().expect("list accepted");
    assert_eq!(
        rows.len(),
        1,
        "exactly the non-superseded accepted ADR: {rows:?}"
    );
    assert_eq!(rows[0].reference, "ADR-0001");
    assert_eq!(rows[0].address, "1");

    let detail = src.show(&rows[0].address).expect("show accepted");
    assert_eq!(detail.status, "Accepted");
    AdrSource::require_accepted(&detail).expect("accepted guard");
    assert!(
        detail.body.contains("Use Rust"),
        "body carries the document"
    );

    // The proposed ADR is visible to `show` but rejected by conduit's guard.
    let proposed = src.show("3").expect("show proposed");
    assert!(matches!(
        AdrSource::require_accepted(&proposed),
        Err(AdroitError::NotAccepted { .. })
    ));
}
