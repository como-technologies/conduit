//! Single source for forge action payloads (follow-up 3, done-criterion 5).
//!
//! `router.rs` (the live lifecycle) and `transcript.rs` (the forge-neutrality
//! demo) both emit forge mutations for the same machine actions. Before this
//! module each built its payloads independently — a drifted field silently
//! produced wrong bytes on one path only. Every payload both sides emit is
//! now built HERE; tests/payload_parity.rs cross-asserts the two stacks emit
//! byte-identical payloads for the same inputs.
//!
//! The ONE documented divergence between the stacks: the issue link comment
//! names the PR by `pr_display` — the router passes the raw forge number,
//! the transcript leg passes the `$PR_n` placeholder (raw numbers differ
//! between transcript legs by construction). The divergence is exactly that
//! parameter; the surrounding bytes are shared.

use crate::contract;
use crate::contract::EffortThresholds;
use crate::forge::{NewIssue, PrDraft};

/// The `conduit plan` issue: plan body + hidden task marker (the
/// find_issue_by_marker probe identity), `adr:<reference>` label.
pub fn plan_issue(reference: &str, title: &str, plan: &str, task_id: &str) -> NewIssue {
    let marker = contract::task_marker(task_id);
    NewIssue {
        title: contract::pr_title(reference, title),
        body: format!("{}\n\n{marker}", plan.trim_end()),
        labels: vec![contract::adr_label(reference)],
    }
}

/// The PR draft with full tuesday tagging: contract title, plan-derived body
/// with the trailer as final line, adr label + exactly one effort label
/// (computed from cumulative `work_ms`).
pub fn pr_draft(
    reference: &str,
    title: &str,
    plan: &str,
    head: &str,
    base: &str,
    work_ms: u64,
    effort: &EffortThresholds,
) -> PrDraft {
    PrDraft {
        title: contract::pr_title(reference, title),
        // Plan-derived summary; the trailer is the final line.
        body: contract::pr_body(reference, plan.trim_end()),
        head: head.to_string(),
        base: base.to_string(),
        labels: vec![
            contract::adr_label(reference),
            contract::effort_bucket(work_ms, effort).label().to_string(),
        ],
    }
}

/// `ApplyPrLabels`: the absolute owned label set — exactly one effort label
/// (recomputed from cumulative `work_ms`) + `adr:<reference>`
/// (spec §The tuesday contract).
pub fn pr_label_set(reference: &str, work_ms: u64, effort: &EffortThresholds) -> Vec<String> {
    vec![
        contract::effort_bucket(work_ms, effort).label().to_string(),
        contract::adr_label(reference),
    ]
}

/// `LinkComment` body: the PR link upserted onto the issue. See the module
/// doc for the `pr_display` divergence.
pub fn link_comment(reference: &str, title: &str, pr_display: &str, task_id: &str) -> String {
    format!(
        "Opened PR {pr_display} for {reference}: {title}.\n\n{}",
        contract::task_marker(task_id)
    )
}

/// `FailureComment` body: reason + fenced log tail + marker.
pub fn failure_comment(attempt: u32, reason: &str, log_tail: &str, task_id: &str) -> String {
    format!(
        "Engine failed (attempt {attempt}): {reason}\n\n```\n{log_tail}\n```\n\n{}",
        contract::task_marker(task_id)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_issue_carries_plan_marker_and_adr_label() {
        let issue = plan_issue(
            "ADR-0003",
            "Adopt snapshot-diff router",
            "# Plan\n\n1. do it\n",
            "adr-0003",
        );
        assert_eq!(issue.title, "[ADR-0003] Adopt snapshot-diff router");
        assert_eq!(
            issue.body, "# Plan\n\n1. do it\n\n<!-- conduit:task:adr-0003 -->",
            "trimmed plan + blank line + hidden marker"
        );
        assert_eq!(issue.labels, vec!["adr:ADR-0003".to_string()]);
    }

    #[test]
    fn pr_draft_carries_full_tuesday_tagging() {
        let draft = pr_draft(
            "ADR-0003",
            "Adopt snapshot-diff router",
            "# Plan\n\n1. do it\n",
            "conduit/adr-0003/adopt-snapshot-diff-router",
            "main",
            0,
            &EffortThresholds::default(),
        );
        assert_eq!(draft.title, "[ADR-0003] Adopt snapshot-diff router");
        assert_eq!(
            draft.body.lines().last().unwrap(),
            "Adr-Reference: ADR-0003",
            "trailer is the final line"
        );
        assert_eq!(draft.head, "conduit/adr-0003/adopt-snapshot-diff-router");
        assert_eq!(draft.base, "main");
        assert_eq!(
            draft.labels,
            vec![
                "adr:ADR-0003".to_string(),
                "effort:1-super-quick".to_string()
            ]
        );
    }

    #[test]
    fn pr_label_set_is_effort_then_adr() {
        let t = EffortThresholds::default();
        assert_eq!(
            pr_label_set("ADR-0003", 0, &t),
            vec![
                "effort:1-super-quick".to_string(),
                "adr:ADR-0003".to_string()
            ]
        );
        // Effort recomputes from cumulative wall-clock.
        assert_eq!(
            pr_label_set("ADR-0003", t.super_quick_max_ms, &t)[0],
            "effort:2-not-long"
        );
    }

    #[test]
    fn link_comment_names_the_pr_via_the_display_parameter() {
        assert_eq!(
            link_comment("ADR-0003", "Adopt snapshot-diff router", "7", "adr-0003"),
            "Opened PR 7 for ADR-0003: Adopt snapshot-diff router.\n\n\
             <!-- conduit:task:adr-0003 -->"
        );
        // The transcript leg's placeholder rides the same builder.
        assert!(
            link_comment("ADR-0003", "t", "$PR_1", "adr-0003-transcript")
                .starts_with("Opened PR $PR_1 for ADR-0003: t.")
        );
    }

    #[test]
    fn failure_comment_fences_the_log_tail() {
        assert_eq!(
            failure_comment(2, "timeout", "last lines", "adr-0003"),
            "Engine failed (attempt 2): timeout\n\n```\nlast lines\n```\n\n\
             <!-- conduit:task:adr-0003 -->"
        );
    }
}
