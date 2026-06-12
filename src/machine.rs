//! Pure lifecycle state machine (spec §Lifecycle state machine).
//! `step` is a pure function: zero I/O, exhaustive match, table-tested over
//! every (state, event) pair including must-ignore cells.

use serde::{Deserialize, Serialize};

use crate::task::{EngineResult, ReviewVerdict, TaskRecord, TaskState};

/// Machine-level event: forge events (mapped from `forge::ForgeEvent` by the
/// router) + the internal engine-completion event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// A label was added to the task's issue (`conduit:run` = the human trigger).
    IssueLabeled {
        label: String,
    },
    ReviewSubmitted {
        verdict: ReviewVerdict,
        body: String,
    },
    /// Consumed, never acted on in the spike (must-ignore in EVERY state).
    CiChanged,
    PrMerged {
        merge_sha: String,
    },
    PrClosed,
    EngineFinished(EngineResult),
}

/// Effects the router executes. Serializable: persisted as write-ahead intents.
/// Runtime-resolved data (PR number/URL, workspace path) is resolved by the
/// router at execution time; event-derived data is captured here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// Prepare a workspace and run the engine. `fresh_workspace`: true for
    /// Scoped/Failed -> Coding (new clone), false for InReview -> Revising
    /// (same branch, feedback included).
    RunEngine { fresh_workspace: bool },
    /// Pathspec-stage (excluding `.conduit-task.md`), commit with the contract
    /// message, push. Probe: `git ls-remote` compare (spec §Idempotency).
    CommitAndPush,
    /// Open the PR with full tuesday tagging. Probe: `find_open_pr_by_head`.
    OpenPr,
    /// Convergent set of PR labels: exactly one effort label (recomputed from
    /// cumulative work_ms) + `adr:<reference>`. Safe to re-run.
    ApplyPrLabels,
    /// Upsert the PR link onto the issue (marker = contract::task_marker).
    LinkComment,
    /// Failure comment with log tail (marker upsert), on the issue.
    FailureComment { reason: String, log_tail: String },
    /// Convergent set of issue labels (e.g. swap conduit:run -> conduit:failed).
    SetIssueLabels { labels: Vec<String> },
    /// Close the issue with a final comment (completion w/ merge sha, or abandonment).
    CloseIssue { comment: String },
    /// Dispose the task's workspace (engine result, if in flight, is discarded).
    DisposeWorkspace,
}

/// How the transition mutates `review_feedback` (kept pure & explicit so the
/// table tests cover it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeedbackOp {
    /// Leave `TaskRecord.review_feedback` unchanged.
    Keep,
    /// Push the body onto `TaskRecord.review_feedback` (current round).
    Append(String),
    /// Reset `TaskRecord.review_feedback` to empty (a round completed).
    Clear,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub next: TaskState,
    pub actions: Vec<Action>,
    pub feedback: FeedbackOp,
    /// True only on Failed -> Coding retry.
    pub bump_attempt: bool,
}

impl Transition {
    /// Identity transition: stay, no actions, keep feedback.
    pub fn ignore(state: TaskState) -> Transition {
        Transition {
            next: state,
            actions: vec![],
            feedback: FeedbackOp::Keep,
            bump_attempt: false,
        }
    }
}

pub fn step(record: &TaskRecord, event: &Event) -> Transition {
    use TaskState::*;
    let ignore = || Transition::ignore(record.state);
    if record.state.is_terminal() {
        return ignore();
    }
    // Terminal PR events: must-act from ANY non-terminal state with an open PR.
    // INVARIANT: every arm in this block MUST carry `if record.pr.is_some()`.
    // Without that guard a new event here would act from all non-terminal
    // states (wrong); the pr=None case must fall through to the tuple match.
    match event {
        Event::PrMerged { merge_sha } if record.pr.is_some() => {
            let mut actions = Vec::new();
            if matches!(record.state, Coding | Revising) {
                actions.push(Action::DisposeWorkspace);
            }
            actions.push(Action::CloseIssue {
                comment: format!(
                    "Merged as {merge_sha}.\n\n{}",
                    crate::contract::task_marker(&record.id)
                ),
            });
            return Transition {
                next: Merged,
                actions,
                feedback: FeedbackOp::Keep,
                bump_attempt: false,
            };
        }
        Event::PrClosed if record.pr.is_some() => {
            let mut actions = Vec::new();
            if matches!(record.state, Coding | Revising) {
                actions.push(Action::DisposeWorkspace);
            }
            actions.push(Action::CloseIssue {
                comment: format!(
                    "PR closed without merge; task abandoned.\n\n{}",
                    crate::contract::task_marker(&record.id)
                ),
            });
            return Transition {
                next: Abandoned,
                actions,
                feedback: FeedbackOp::Keep,
                bump_attempt: false,
            };
        }
        _ => {}
    }
    match (record.state, event) {
        (Scoped, Event::IssueLabeled { label }) if label == crate::contract::LABEL_RUN => {
            Transition {
                next: Coding,
                actions: vec![Action::RunEngine {
                    fresh_workspace: true,
                }],
                feedback: FeedbackOp::Keep,
                bump_attempt: false,
            }
        }
        (Failed, Event::IssueLabeled { label }) if label == crate::contract::LABEL_RUN => {
            Transition {
                next: Coding,
                actions: vec![Action::RunEngine {
                    fresh_workspace: true,
                }],
                feedback: FeedbackOp::Keep,
                bump_attempt: true,
            }
        }
        (Coding, Event::EngineFinished(EngineResult::Completed { .. })) => Transition {
            next: InReview,
            actions: vec![
                Action::CommitAndPush,
                Action::OpenPr,
                Action::ApplyPrLabels,
                Action::LinkComment,
            ],
            feedback: FeedbackOp::Clear,
            bump_attempt: false,
        },
        (Revising, Event::EngineFinished(EngineResult::Completed { .. })) => Transition {
            next: InReview,
            actions: vec![Action::CommitAndPush, Action::ApplyPrLabels],
            feedback: FeedbackOp::Clear,
            bump_attempt: false,
        },
        (Coding | Revising, Event::EngineFinished(EngineResult::Failed { reason, log_tail })) => {
            Transition {
                next: Failed,
                actions: vec![
                    Action::FailureComment {
                        reason: reason.clone(),
                        log_tail: log_tail.clone(),
                    },
                    Action::SetIssueLabels {
                        labels: vec![crate::contract::LABEL_FAILED.to_string()],
                    },
                ],
                feedback: FeedbackOp::Keep,
                bump_attempt: false,
            }
        }
        (
            InReview,
            Event::ReviewSubmitted {
                verdict: ReviewVerdict::ChangesRequested,
                body,
            },
        ) => Transition {
            next: Revising,
            actions: vec![Action::RunEngine {
                fresh_workspace: false,
            }],
            feedback: FeedbackOp::Append(body.clone()),
            bump_attempt: false,
        },
        (
            Revising,
            Event::ReviewSubmitted {
                verdict: ReviewVerdict::ChangesRequested,
                body,
            },
        ) => Transition {
            next: Revising,
            actions: vec![],
            feedback: FeedbackOp::Append(body.clone()),
            bump_attempt: false,
        },
        _ => ignore(),
    }
}
