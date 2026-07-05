//! A4: `playbook-demo-init.sh`'s `REPO_NAME` knob templates the generated
//! `conduit.toml` without a manual sed (run-3 wart 3 — the third sighting of
//! the hardcoded-demo-target lesson). These drive the REAL init script
//! against a minimal fake corpus and load the generated config as the oracle,
//! so a regression in the templating fails the gate, not the next live demo.

use std::path::{Path, PathBuf};
use std::process::Command;

use conduit::config::Config;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Run the real init script against a fake corpus; return the created RUN_DIR.
/// COMO_OFFLINE keeps it network-free; PLAYBOOK_DIR is explicit so no sibling
/// or clone leg is exercised. The post-sed steps are only symlinks + echo, so
/// the script completes hermetically with no forge or adroit binary.
fn run_init(tmp: &Path, repo_name: Option<&str>) -> PathBuf {
    let corpus = tmp.join("corpus");
    std::fs::create_dir_all(corpus.join("docs/src/adr")).unwrap();
    let run_dir = tmp.join("run"); // must NOT pre-exist; the script errors if it does

    let mut cmd = Command::new("bash");
    cmd.current_dir(repo_root())
        .arg("demo/playbook-demo-init.sh")
        .env("RUN_DIR", &run_dir)
        .env("PLAYBOOK_DIR", &corpus)
        .env("COMO_OFFLINE", "1");
    if let Some(name) = repo_name {
        cmd.env("REPO_NAME", name);
    }
    let out = cmd.output().expect("run playbook-demo-init.sh");
    assert!(
        out.status.success(),
        "init script failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    run_dir
}

#[test]
fn repo_name_knob_templates_the_generated_toml_without_sed() {
    let tmp = tempfile::tempdir().unwrap();
    let run_dir = run_init(tmp.path(), Some("run3-corpus-demo"));

    let toml = std::fs::read_to_string(run_dir.join("conduit.toml")).unwrap();
    assert!(
        !toml.contains("@REPO_NAME@"),
        "placeholder survived:\n{toml}"
    );

    let cfg = Config::load(&run_dir).expect("load generated conduit.toml");
    assert_eq!(cfg.forge.gitea.repo, "run3-corpus-demo");
}

#[test]
fn repo_name_default_preserves_the_playbook_demo() {
    let tmp = tempfile::tempdir().unwrap();
    let run_dir = run_init(tmp.path(), None);
    let cfg = Config::load(&run_dir).expect("load generated conduit.toml");
    assert_eq!(cfg.forge.gitea.repo, "playbook");
}

#[test]
fn generated_toml_still_resolves_the_corpus_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let run_dir = run_init(tmp.path(), Some("anything"));
    let cfg = Config::load(&run_dir).expect("load generated conduit.toml");
    assert!(
        cfg.adroit.dir.ends_with("docs/src/adr"),
        "adroit dir not resolved: {}",
        cfg.adroit.dir
    );
    assert!(!cfg.adroit.dir.contains("@ADROIT_DIR@"));
}
