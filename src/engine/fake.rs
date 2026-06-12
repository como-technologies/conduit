//! Deterministic engine (spec §Fakes) — the default demo path. Same spec in,
//! same bytes out; scripted `fail`/`hang` modes drive the `Failed` and
//! timeout transitions as first-class tested paths.

use crate::engine::{Engine, EngineError, EngineOutcome, TaskSpec};

/// Deterministic engine (spec §Fakes) — the default demo path.
#[derive(Debug)]
pub enum FakeMode {
    Complete,
    Fail,
    Hang { secs: u64 },
}

#[derive(Debug)]
pub struct FakeEngine {
    pub mode: FakeMode,
}

impl FakeMode {
    /// CLI-path mode selection: `CONDUIT_FAKE_ENGINE_MODE=complete|fail|hang`.
    /// `hang:<secs>` overrides the hang duration (default 3600s — outlasts any
    /// demo timeout). Unset or unrecognized falls back to `Complete`, the
    /// default demo path.
    pub fn from_env() -> FakeMode {
        match std::env::var("CONDUIT_FAKE_ENGINE_MODE") {
            Ok(val) => FakeMode::parse(&val).unwrap_or(FakeMode::Complete),
            Err(_) => FakeMode::Complete,
        }
    }

    /// Pure parser behind `from_env` (env mutation is racy in parallel tests).
    pub fn parse(s: &str) -> Option<FakeMode> {
        match s {
            "complete" => Some(FakeMode::Complete),
            "fail" => Some(FakeMode::Fail),
            "hang" => Some(FakeMode::Hang { secs: 3600 }),
            _ => {
                let secs = s.strip_prefix("hang:")?.parse().ok()?;
                Some(FakeMode::Hang { secs })
            }
        }
    }
}

impl Engine for FakeEngine {
    fn describe(&self) -> String {
        match self.mode {
            FakeMode::Complete => "fake (complete)".to_string(),
            FakeMode::Fail => "fake (fail)".to_string(),
            FakeMode::Hang { secs } => format!("fake (hang {secs}s)"),
        }
    }

    fn run(&self, spec: &TaskSpec) -> Result<EngineOutcome, EngineError> {
        match self.mode {
            FakeMode::Fail => Ok(EngineOutcome::Failed {
                reason: "scripted failure".to_string(),
                log_tail: "fake engine scripted log tail".to_string(),
            }),
            FakeMode::Hang { secs } => {
                std::thread::sleep(std::time::Duration::from_secs(secs));
                complete(spec)
            }
            FakeMode::Complete => complete(spec),
        }
    }
}

/// Write `docs/impl/<ref-lower>.md` — title + SHA-256 (hex) of the verbatim
/// plan snapshot. Pure function of the spec: same spec, same bytes. An I/O
/// failure is an engine-side failure (first-class `Failed`), not an
/// `EngineError` — the seam reserves errors for "could not run at all".
fn complete(spec: &TaskSpec) -> Result<EngineOutcome, EngineError> {
    use sha2::Digest;
    let plan_sha: String = sha2::Sha256::digest(spec.plan_markdown.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let rel = format!(
        "docs/impl/{}.md",
        crate::contract::task_slug(&spec.adr_reference)
    );
    let doc = format!(
        "# {}\n\nDeterministic implementation artifact for {}.\n\nplan-sha256: {plan_sha}\n",
        spec.title, spec.adr_reference,
    );
    let path = spec.workspace.join(&rel);
    let io = std::fs::create_dir_all(path.parent().expect("rel path has a parent"))
        .and_then(|()| std::fs::write(&path, doc));
    Ok(match io {
        Ok(()) => EngineOutcome::Completed {
            summary: format!("fake engine wrote {rel}"),
        },
        Err(e) => EngineOutcome::Failed {
            reason: format!("fake engine could not write {rel}: {e}"),
            log_tail: String::new(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Engine, EngineOutcome, TaskSpec};
    use tempfile::TempDir;

    fn spec(ws: &std::path::Path) -> TaskSpec {
        TaskSpec {
            adr_reference: "ADR-0003".into(),
            title: "Adopt snapshot-diff router".into(),
            adr_body: "body".into(),
            plan_markdown: "# Plan\n1. do it\n".into(),
            review_feedback: None,
            workspace: ws.to_path_buf(),
        }
    }

    #[test]
    fn complete_mode_writes_deterministic_impl_doc() {
        let ws = TempDir::new().unwrap();
        let e = FakeEngine {
            mode: FakeMode::Complete,
        };
        let out = e.run(&spec(ws.path())).unwrap();
        assert!(matches!(out, EngineOutcome::Completed { .. }));
        let doc = std::fs::read_to_string(ws.path().join("docs/impl/adr-0003.md")).unwrap();
        assert!(doc.contains("Adopt snapshot-diff router"));
        // sha2 0.11's digest output no longer implements LowerHex (store.rs
        // precedent) — the plan's `{:x}` adapted to the byte-map form.
        use sha2::{Digest, Sha256};
        let plan_sha: String = Sha256::digest("# Plan\n1. do it\n".as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(doc.contains(&plan_sha), "doc embeds the plan snapshot hash");
        // determinism: run again in a fresh ws, same bytes
        let ws2 = TempDir::new().unwrap();
        e.run(&spec(ws2.path())).unwrap();
        let doc2 = std::fs::read_to_string(ws2.path().join("docs/impl/adr-0003.md")).unwrap();
        assert_eq!(doc, doc2);
    }

    #[test]
    fn fail_mode_reports_failed_with_log_tail() {
        let ws = TempDir::new().unwrap();
        let e = FakeEngine {
            mode: FakeMode::Fail,
        };
        let EngineOutcome::Failed { reason, log_tail } = e.run(&spec(ws.path())).unwrap() else {
            panic!("expected Failed");
        };
        assert!(!reason.is_empty() && !log_tail.is_empty());
        assert!(
            std::fs::read_dir(ws.path()).unwrap().next().is_none(),
            "writes nothing"
        );
    }

    #[test]
    fn hang_mode_sleeps_then_completes() {
        let ws = TempDir::new().unwrap();
        let e = FakeEngine {
            mode: FakeMode::Hang { secs: 1 },
        };
        let start = std::time::Instant::now();
        let out = e.run(&spec(ws.path())).unwrap();
        assert!(start.elapsed() >= std::time::Duration::from_secs(1));
        assert!(matches!(out, EngineOutcome::Completed { .. }));
        assert!(
            ws.path().join("docs/impl/adr-0003.md").exists(),
            "hang completes like Complete after the sleep"
        );
    }

    #[test]
    fn mode_parses_the_documented_env_forms() {
        assert!(matches!(
            FakeMode::parse("complete"),
            Some(FakeMode::Complete)
        ));
        assert!(matches!(FakeMode::parse("fail"), Some(FakeMode::Fail)));
        assert!(matches!(
            FakeMode::parse("hang"),
            Some(FakeMode::Hang { secs: 3600 })
        ));
        assert!(matches!(
            FakeMode::parse("hang:2"),
            Some(FakeMode::Hang { secs: 2 })
        ));
        assert!(FakeMode::parse("bogus").is_none());
        assert!(FakeMode::parse("hang:notanumber").is_none());
    }
}
