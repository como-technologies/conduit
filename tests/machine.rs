use conduit::contract;
use conduit::machine::{Action, Event, FeedbackOp, Transition, step};
use conduit::task::{EngineResult, IssueId, PrId, ReviewVerdict, TaskRecord, TaskState};

const ALL_STATES: [TaskState; 7] = [
    TaskState::Scoped,
    TaskState::Coding,
    TaskState::InReview,
    TaskState::Revising,
    TaskState::Failed,
    TaskState::Merged,
    TaskState::Abandoned,
];

fn rec(state: TaskState, has_pr: bool) -> TaskRecord {
    let mut r = TaskRecord::new("ADR-0003", "3", "Adopt snapshot-diff router", "deadbeef");
    r.state = state;
    r.issue = Some(IssueId(1));
    r.pr = if has_pr { Some(PrId(7)) } else { None };
    r
}

fn all_events() -> Vec<Event> {
    vec![
        Event::IssueLabeled {
            label: contract::LABEL_RUN.to_string(),
        },
        Event::IssueLabeled {
            label: "unrelated".to_string(),
        },
        Event::ReviewSubmitted {
            verdict: ReviewVerdict::ChangesRequested,
            body: "fix x".into(),
        },
        Event::ReviewSubmitted {
            verdict: ReviewVerdict::Approved,
            body: "lgtm".into(),
        },
        Event::ReviewSubmitted {
            verdict: ReviewVerdict::Commented,
            body: "note".into(),
        },
        Event::CiChanged,
        Event::PrMerged {
            merge_sha: "abc123".to_string(),
        },
        Event::PrClosed,
        Event::EngineFinished(EngineResult::Completed {
            summary: "done".into(),
        }),
        Event::EngineFinished(EngineResult::Failed {
            reason: "boom".into(),
            log_tail: "tail".into(),
        }),
    ]
}

/// Action-kind fingerprint, so the table compares shape not payload.
fn kinds(t: &Transition) -> Vec<&'static str> {
    t.actions
        .iter()
        .map(|a| match a {
            Action::RunEngine {
                fresh_workspace: true,
            } => "run-fresh",
            Action::RunEngine {
                fresh_workspace: false,
            } => "run-same",
            Action::CommitAndPush => "push",
            Action::OpenPr => "open-pr",
            Action::ApplyPrLabels => "pr-labels",
            Action::LinkComment => "link",
            Action::FailureComment { .. } => "fail-comment",
            Action::SetIssueLabels { .. } => "issue-labels",
            Action::CloseIssue { .. } => "close-issue",
            Action::DisposeWorkspace => "dispose",
        })
        .collect()
}

struct Cell {
    state: TaskState,
    has_pr: bool,
    event: Event,
    next: TaskState,
    action_kinds: &'static [&'static str],
    feedback: FeedbackOp,
    bump_attempt: bool,
}

fn must_act_table() -> Vec<Cell> {
    use TaskState::*;
    let run = || Event::IssueLabeled {
        label: contract::LABEL_RUN.to_string(),
    };
    let cr = || Event::ReviewSubmitted {
        verdict: ReviewVerdict::ChangesRequested,
        body: "fix x".into(),
    };
    let merged = || Event::PrMerged {
        merge_sha: "abc123".to_string(),
    };
    let done = || {
        Event::EngineFinished(EngineResult::Completed {
            summary: "done".into(),
        })
    };
    let failed = || {
        Event::EngineFinished(EngineResult::Failed {
            reason: "boom".into(),
            log_tail: "tail".into(),
        })
    };

    let mut t = Vec::new();
    // PR-INSENSITIVE must-act cells: `step` does not consult `record.pr` for
    // these, so the expectation is identical for has_pr in {false, true} and
    // BOTH variants go in the table (the exhaustive sweep relies on this).
    // Some pr=false/pr=true combinations cannot occur in practice (e.g.
    // InReview without a PR; Scoped with a PR) — the table still pins their
    // behavior so table and implementation agree cell-for-cell.
    // Coding-with-PR is REAL: Failed-with-PR --relabel--> Coding retry; its
    // EngineFinished cells must act exactly like Coding-without-PR (OpenPr's
    // probe makes the replay idempotent at execution time).
    for has_pr in [false, true] {
        t.push(Cell {
            state: Scoped,
            has_pr,
            event: run(),
            next: Coding,
            action_kinds: &["run-fresh"],
            feedback: FeedbackOp::Keep,
            bump_attempt: false,
        });
        t.push(Cell {
            state: Coding,
            has_pr,
            event: done(),
            next: InReview,
            action_kinds: &["push", "open-pr", "pr-labels", "link"],
            feedback: FeedbackOp::Clear,
            bump_attempt: false,
        });
        t.push(Cell {
            state: Coding,
            has_pr,
            event: failed(),
            next: Failed,
            action_kinds: &["fail-comment", "issue-labels"],
            feedback: FeedbackOp::Keep,
            bump_attempt: false,
        });
        t.push(Cell {
            state: InReview,
            has_pr,
            event: cr(),
            next: Revising,
            action_kinds: &["run-same"],
            feedback: FeedbackOp::Append("fix x".into()),
            bump_attempt: false,
        });
        t.push(Cell {
            state: Revising,
            has_pr,
            event: cr(),
            next: Revising,
            action_kinds: &[],
            feedback: FeedbackOp::Append("fix x".into()),
            bump_attempt: false,
        });
        t.push(Cell {
            state: Revising,
            has_pr,
            event: done(),
            next: InReview,
            action_kinds: &["push", "pr-labels"],
            feedback: FeedbackOp::Clear,
            bump_attempt: false,
        });
        t.push(Cell {
            state: Revising,
            has_pr,
            event: failed(),
            next: Failed,
            action_kinds: &["fail-comment", "issue-labels"],
            feedback: FeedbackOp::Keep,
            bump_attempt: false,
        });
        t.push(Cell {
            state: Failed,
            has_pr,
            event: run(),
            next: Coding,
            action_kinds: &["run-fresh"],
            feedback: FeedbackOp::Keep,
            bump_attempt: true,
        });
    }
    // PR-REQUIRED cells (the open-PR guard): only has_pr=true — with
    // has_pr=false these events are must-ignore (covered by the sweep).
    // ALL five non-terminal states appear: the guard in `step` is
    // `record.pr.is_some()` from any non-terminal state (Scoped-with-PR cannot
    // occur in practice, but table and implementation must agree cell-for-cell).
    // Coding/Revising additionally dispose the workspace (in-flight engine
    // result discarded — reviewer-mandated).
    for (state, dispose) in [
        (Scoped, false),
        (Coding, true),
        (InReview, false),
        (Revising, true),
        (Failed, false),
    ] {
        let kinds: &'static [&'static str] = if dispose {
            &["dispose", "close-issue"]
        } else {
            &["close-issue"]
        };
        t.push(Cell {
            state,
            has_pr: true,
            event: merged(),
            next: Merged,
            action_kinds: kinds,
            feedback: FeedbackOp::Keep,
            bump_attempt: false,
        });
        t.push(Cell {
            state,
            has_pr: true,
            event: Event::PrClosed,
            next: Abandoned,
            action_kinds: kinds,
            feedback: FeedbackOp::Keep,
            bump_attempt: false,
        });
    }
    t
}

#[test]
fn must_act_cells() {
    for cell in must_act_table() {
        let r = rec(cell.state, cell.has_pr);
        let t = step(&r, &cell.event);
        assert_eq!(t.next, cell.next, "{:?} + {:?}", cell.state, cell.event);
        assert_eq!(
            kinds(&t),
            cell.action_kinds,
            "{:?} + {:?}",
            cell.state,
            cell.event
        );
        assert_eq!(
            t.feedback, cell.feedback,
            "{:?} + {:?}",
            cell.state, cell.event
        );
        assert_eq!(
            t.bump_attempt, cell.bump_attempt,
            "{:?} + {:?}",
            cell.state, cell.event
        );
    }
}

/// Exhaustive sweep: every (state, event, has_pr) combination NOT in the
/// must-act table is the identity transition — the must-ignore cells.
#[test]
fn every_other_cell_is_must_ignore() {
    let table = must_act_table();
    for state in ALL_STATES {
        for has_pr in [false, true] {
            for event in all_events() {
                let in_table = table
                    .iter()
                    .any(|c| c.state == state && c.has_pr == has_pr && c.event == event);
                if in_table {
                    continue;
                }
                let r = rec(state, has_pr);
                let t = step(&r, &event);
                assert_eq!(
                    t,
                    Transition::ignore(state),
                    "expected must-ignore: {state:?} (pr={has_pr}) + {event:?}"
                );
            }
        }
    }
}

/// CiChanged is must-ignore in EVERY state — called out as its own test
/// because it is a reviewer-mandated contract (spec §Lifecycle state machine).
#[test]
fn ci_changed_is_must_ignore_everywhere() {
    for state in ALL_STATES {
        for has_pr in [false, true] {
            let r = rec(state, has_pr);
            assert_eq!(step(&r, &Event::CiChanged), Transition::ignore(state));
        }
    }
}

/// Terminal states ignore everything.
#[test]
fn terminal_states_ignore_all_events() {
    for state in [TaskState::Merged, TaskState::Abandoned] {
        for has_pr in [false, true] {
            for event in all_events() {
                let r = rec(state, has_pr);
                assert_eq!(
                    step(&r, &event),
                    Transition::ignore(state),
                    "{state:?} + {event:?}"
                );
            }
        }
    }
}

/// PrMerged/PrClosed with NO pr on the record are ignored (the open-PR guard).
#[test]
fn terminal_pr_events_require_an_open_pr() {
    for state in [
        TaskState::Scoped,
        TaskState::Coding,
        TaskState::InReview,
        TaskState::Revising,
        TaskState::Failed,
    ] {
        let r = rec(state, false);
        // Note: InReview/Revising "without a PR" cannot occur in practice (the
        // PR is opened entering InReview) but the guard must still hold.
        assert_eq!(
            step(
                &r,
                &Event::PrMerged {
                    merge_sha: "abc".into()
                }
            ),
            Transition::ignore(state)
        );
        assert_eq!(step(&r, &Event::PrClosed), Transition::ignore(state));
    }
}

/// The merged CloseIssue comment carries the merge sha (the completion beat).
#[test]
fn merged_close_comment_contains_sha() {
    let r = rec(TaskState::InReview, true);
    let t = step(
        &r,
        &Event::PrMerged {
            merge_sha: "cafe42".into(),
        },
    );
    let Some(Action::CloseIssue { comment }) = t
        .actions
        .iter()
        .find(|a| matches!(a, Action::CloseIssue { .. }))
    else {
        panic!("expected CloseIssue");
    };
    assert!(comment.contains("cafe42"));
}

/// Engine failure swaps the trigger label to conduit:failed (convergent set).
#[test]
fn failure_swaps_run_label_to_failed() {
    let r = rec(TaskState::Coding, false);
    let t = step(
        &r,
        &Event::EngineFinished(EngineResult::Failed {
            reason: "boom".into(),
            log_tail: "tail".into(),
        }),
    );
    let Some(Action::SetIssueLabels { labels }) = t
        .actions
        .iter()
        .find(|a| matches!(a, Action::SetIssueLabels { .. }))
    else {
        panic!("expected SetIssueLabels");
    };
    assert!(labels.contains(&contract::LABEL_FAILED.to_string()));
    assert!(!labels.contains(&contract::LABEL_RUN.to_string()));
}
