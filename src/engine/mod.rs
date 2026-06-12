//! The subprocess engine contract (spec §The engine seam). Engines edit files
//! in a prepared workspace; conduit owns git, forge, and the timeout.
//!
//! Timeout is enforced INSIDE engines and surfaces as
//! `Ok(EngineOutcome::Failed { reason: "timeout", .. })` — first-class, never
//! an `EngineError`. The router maps `EngineOutcome` to `task::EngineResult`
//! one-to-one.

pub mod claude_code;
pub mod fake;

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("engine could not be spawned: {0}")]
    Spawn(String),
    #[error("engine produced unparseable output: {0}")]
    BadOutput(String),
}

#[derive(Debug, Clone)]
pub struct TaskSpec {
    pub adr_reference: String, // "ADR-0003"
    pub title: String,
    pub adr_body: String,                // AdrDetail body markdown
    pub plan_markdown: String,           // the VERBATIM persisted plan snapshot
    pub review_feedback: Option<String>, // ChangesRequested bodies of the CURRENT round only
    pub workspace: PathBuf,              // already on branch conduit/<ref-lower>/<slug>
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineOutcome {
    Completed { summary: String },
    Failed { reason: String, log_tail: String },
}

pub trait Engine {
    fn describe(&self) -> String;
    fn run(&self, spec: &TaskSpec) -> Result<EngineOutcome, EngineError>;
}

/// Run the engine, measuring wall-clock around the call. The elapsed ms feed
/// `TaskRecord::work_ms` and so the effort bucket (spec §The tuesday
/// contract).
///
/// Returns ONE run's elapsed ms. `work_ms` is cumulative across attempts and
/// revision rounds — the router must ADD (`work_ms += elapsed`), never
/// assign. Precision note: a timed-out ClaudeCodeEngine run can overshoot
/// its deadline by up to ~500ms (poll granularity) — fine for effort
/// bucketing, not a tight bound.
pub fn run_timed(
    engine: &dyn Engine,
    spec: &TaskSpec,
) -> (Result<EngineOutcome, EngineError>, u64) {
    let start = std::time::Instant::now();
    let result = engine.run(spec);
    (result, start.elapsed().as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::fake::{FakeEngine, FakeMode};
    use tempfile::TempDir;

    #[test]
    fn run_timed_measures_engine_wall_clock() {
        let ws = TempDir::new().unwrap();
        let spec = TaskSpec {
            adr_reference: "ADR-0003".into(),
            title: "Adopt snapshot-diff router".into(),
            adr_body: "body".into(),
            plan_markdown: "# Plan\n1. do it\n".into(),
            review_feedback: None,
            workspace: ws.path().to_path_buf(),
        };
        let e = FakeEngine {
            mode: FakeMode::Hang { secs: 1 },
        };
        let (result, elapsed_ms) = run_timed(&e, &spec);
        assert!(matches!(result, Ok(EngineOutcome::Completed { .. })));
        assert!(
            elapsed_ms >= 1000,
            "hang of 1s must register: {elapsed_ms}ms"
        );
    }
}
