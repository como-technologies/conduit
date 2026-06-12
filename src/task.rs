//! Task model: one ADR = one task = one PR (spec §Out of scope: no decomposition).

use serde::{Deserialize, Serialize};

/// Forge-native issue number (Gitea index / GitHub number).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IssueId(pub u64);

/// Forge-native PR number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrId(pub u64);

/// Forge-native review id — string because forges differ (spec §Review identity).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReviewId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Scoped,
    Coding,
    InReview,
    Revising,
    Failed,
    Merged,    // terminal
    Abandoned, // terminal
}

impl TaskState {
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskState::Merged | TaskState::Abandoned)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewVerdict {
    Approved,
    ChangesRequested,
    Commented,
}

/// What the engine reported (timeout is mapped to `Failed` by the engine
/// runner before it reaches the machine — spec §The engine seam).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineResult {
    Completed { summary: String },
    Failed { reason: String, log_tail: String },
}

/// A persisted action intent: written BEFORE execution, marked done after
/// (spec §Crash consistency). `Action` is defined in `machine.rs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionIntent {
    pub action: crate::machine::Action,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    /// Stable task id: the lowercased reference, e.g. `adr-0003`.
    pub id: String,
    /// Display reference, e.g. `ADR-0003`.
    pub adr_reference: String,
    /// adroit addressing token, e.g. `3` (spec §adroit integration: Enumerate).
    pub adr_address: String,
    pub title: String,
    pub state: TaskState,
    /// `conduit/<ref-lower>/<slug>` (contract::branch_name).
    pub branch: String,
    pub issue: Option<IssueId>,
    pub pr: Option<PrId>,
    /// 1-based; bumped on Failed -> Coding retry (fresh workspace).
    pub attempt: u32,
    /// Cumulative engine wall-clock across all runs — feeds the effort bucket.
    pub work_ms: u64,
    /// sha256 (hex) of the verbatim plan snapshot in `.conduit/plans/<id>.md`.
    pub plan_sha256: String,
    /// ChangesRequested bodies of the CURRENT round only: reviews received
    /// since the task last entered InReview (spec §The engine seam, TaskSpec).
    pub review_feedback: Vec<String>,
    /// Write-ahead action intents (spec §Crash consistency).
    pub pending: Vec<ActionIntent>,
}

impl TaskRecord {
    pub fn new(
        adr_reference: &str,
        adr_address: &str,
        title: &str,
        plan_sha256: &str,
    ) -> TaskRecord {
        TaskRecord {
            id: crate::contract::task_slug(adr_reference),
            adr_reference: adr_reference.to_string(),
            adr_address: adr_address.to_string(),
            title: title.to_string(),
            state: TaskState::Scoped,
            branch: crate::contract::branch_name(adr_reference, title),
            issue: None,
            pr: None,
            attempt: 1,
            work_ms: 0,
            plan_sha256: plan_sha256.to_string(),
            review_feedback: Vec::new(),
            pending: Vec::new(),
        }
    }
}
