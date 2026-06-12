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
        "GITHUB_TOKEN",
    ] {
        cmd.env_remove(var);
    }
    cmd
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
fn env_overrides_config_engine() {
    // CONDUIT_ENGINE env beats conduit.toml: prove via a debug print path —
    // `status -o json` is data-only, so assert through config: write a config
    // with engine=fake, set CONDUIT_ENGINE=claude-code, and `run --once`
    // must fail with the not-implemented error (not a config parse error).
    let d = TempDir::new().unwrap();
    std::fs::write(d.path().join("conduit.toml"), "[engine]\nkind = \"fake\"\n").unwrap();
    conduit(&d)
        .env("CONDUIT_ENGINE", "claude-code")
        .args(["run", "--once"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not implemented yet"));
}

#[test]
fn unimplemented_commands_fail_loudly() {
    let d = TempDir::new().unwrap();
    for args in [
        vec!["init"],
        vec!["plan", "3"],
        vec!["verify", "3"],
        vec!["demo-transcript", "3"],
    ] {
        conduit(&d)
            .args(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("not implemented yet"));
    }
}
