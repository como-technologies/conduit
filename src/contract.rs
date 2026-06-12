//! ALL tuesday-contract emission (spec §The tuesday contract). Pure — no I/O.
//! tuesday (the Measure stage) reads these labels/titles/trailers at merge
//! time; this module is the single place the contract can drift.

use serde::{Deserialize, Serialize};

/// The closed effort-label set, index == `EffortBucket as usize`.
pub const EFFORT_LABELS: [&str; 5] = [
    "effort:1-super-quick",
    "effort:2-not-long",
    "effort:3-average",
    "effort:4-a-while",
    "effort:5-felt-like-forever",
];

/// The human trigger label and its failure swap (spec §Lifecycle state machine).
pub const LABEL_RUN: &str = "conduit:run";
pub const LABEL_FAILED: &str = "conduit:failed";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EffortBucket {
    SuperQuick = 0,
    NotLong = 1,
    Average = 2,
    AWhile = 3,
    FeltLikeForever = 4,
}

impl EffortBucket {
    pub fn label(self) -> &'static str {
        EFFORT_LABELS[self as usize]
    }
}

/// Effort thresholds in milliseconds — exclusive upper bounds per bucket.
/// Defaults per spec: <10m=1, <30m=2, <2h=3, <8h=4, else 5. Overridable in
/// `conduit.toml` `[effort]` (Task 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffortThresholds {
    pub super_quick_max_ms: u64,
    pub not_long_max_ms: u64,
    pub average_max_ms: u64,
    pub a_while_max_ms: u64,
}

impl Default for EffortThresholds {
    fn default() -> Self {
        EffortThresholds {
            super_quick_max_ms: 10 * 60 * 1000,
            not_long_max_ms: 30 * 60 * 1000,
            average_max_ms: 2 * 60 * 60 * 1000,
            a_while_max_ms: 8 * 60 * 60 * 1000,
        }
    }
}

/// Map cumulative engine wall-clock to the effort bucket.
pub fn effort_bucket(work_ms: u64, t: &EffortThresholds) -> EffortBucket {
    if work_ms < t.super_quick_max_ms {
        EffortBucket::SuperQuick
    } else if work_ms < t.not_long_max_ms {
        EffortBucket::NotLong
    } else if work_ms < t.average_max_ms {
        EffortBucket::Average
    } else if work_ms < t.a_while_max_ms {
        EffortBucket::AWhile
    } else {
        EffortBucket::FeltLikeForever
    }
}

/// `adr:ADR-0003`
pub fn adr_label(reference: &str) -> String {
    format!("adr:{reference}")
}

/// `[ADR-0003] <title>`
pub fn pr_title(reference: &str, title: &str) -> String {
    format!("[{reference}] {title}")
}

/// `Adr-Reference: ADR-0003`
pub fn body_trailer(reference: &str) -> String {
    format!("Adr-Reference: {reference}")
}

/// Body + blank line + trailer; the trailer is ALWAYS the final line.
pub fn pr_body(reference: &str, body: &str) -> String {
    format!("{}\n\n{}", body.trim_end(), body_trailer(reference))
}

/// `[ADR-0003] <title>\n\nAdr-Reference: ADR-0003\n`
pub fn commit_message(reference: &str, title: &str) -> String {
    format!(
        "{}\n\n{}\n",
        pr_title(reference, title),
        body_trailer(reference)
    )
}

/// Slug: ASCII-lowercase alphanumerics, runs of anything else collapse to one
/// `-`, trimmed of leading/trailing `-`, capped at 40 chars, never empty
/// (falls back to `"task"`).
pub fn task_slug(title: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = true; // suppress leading dash
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

/// `conduit/<reference-lower>/<task-slug>` — structurally always rooted at
/// `conduit/`, so it can never emit adroit's `adr/` namespace.
pub fn branch_name(reference: &str, title: &str) -> String {
    format!("conduit/{}/{}", task_slug(reference), task_slug(title))
}

/// Hidden HTML marker carried in issue bodies / comments for idempotency
/// probes (spec §Idempotency: probe before reissue; adroit's marker pattern).
pub fn task_marker(task_id: &str) -> String {
    format!("<!-- conduit:task:{task_id} -->")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_labels_are_the_closed_five() {
        assert_eq!(
            EFFORT_LABELS,
            [
                "effort:1-super-quick",
                "effort:2-not-long",
                "effort:3-average",
                "effort:4-a-while",
                "effort:5-felt-like-forever",
            ]
        );
    }

    #[test]
    fn effort_bucket_default_thresholds_table() {
        // Spec: <10m=1, <30m=2, <2h=3, <8h=4, else 5. Boundaries are exclusive
        // upper bounds: exactly 10m falls in bucket 2.
        let t = EffortThresholds::default();
        let cases: [(u64, EffortBucket); 9] = [
            (0, EffortBucket::SuperQuick),
            (599_999, EffortBucket::SuperQuick),
            (600_000, EffortBucket::NotLong),
            (1_799_999, EffortBucket::NotLong),
            (1_800_000, EffortBucket::Average),
            (7_199_999, EffortBucket::Average),
            (7_200_000, EffortBucket::AWhile),
            (28_799_999, EffortBucket::AWhile),
            (28_800_000, EffortBucket::FeltLikeForever),
        ];
        for (ms, want) in cases {
            assert_eq!(effort_bucket(ms, &t), want, "work_ms={ms}");
        }
    }

    #[test]
    fn effort_bucket_respects_config_thresholds() {
        let t = EffortThresholds {
            super_quick_max_ms: 10,
            not_long_max_ms: 20,
            average_max_ms: 30,
            a_while_max_ms: 40,
        };
        assert_eq!(effort_bucket(9, &t), EffortBucket::SuperQuick);
        assert_eq!(effort_bucket(10, &t), EffortBucket::NotLong);
        assert_eq!(effort_bucket(39, &t), EffortBucket::AWhile);
        assert_eq!(effort_bucket(40, &t), EffortBucket::FeltLikeForever);
    }

    #[test]
    fn effort_bucket_label_maps_one_to_one() {
        let t = EffortThresholds::default();
        assert_eq!(effort_bucket(0, &t).label(), "effort:1-super-quick");
        assert_eq!(
            effort_bucket(u64::MAX, &t).label(),
            "effort:5-felt-like-forever"
        );
    }

    #[test]
    fn adr_label_prefixes_reference() {
        assert_eq!(adr_label("ADR-0003"), "adr:ADR-0003");
    }

    #[test]
    fn pr_title_carries_bracketed_reference_prefix() {
        assert_eq!(
            pr_title("ADR-0003", "Adopt snapshot-diff router"),
            "[ADR-0003] Adopt snapshot-diff router"
        );
    }

    #[test]
    fn body_trailer_is_adr_reference() {
        assert_eq!(body_trailer("ADR-0003"), "Adr-Reference: ADR-0003");
    }

    #[test]
    fn pr_body_trailer_is_the_final_line() {
        let body = pr_body("ADR-0003", "Implements the accepted decision.");
        let last = body.lines().last().unwrap();
        assert_eq!(last, "Adr-Reference: ADR-0003");
        assert!(body.starts_with("Implements the accepted decision."));
        // blank line separates body from trailer
        assert!(body.contains("\n\nAdr-Reference: ADR-0003"));
    }

    #[test]
    fn commit_message_has_prefix_and_trailer() {
        let msg = commit_message("ADR-0003", "Adopt snapshot-diff router");
        assert_eq!(
            msg,
            "[ADR-0003] Adopt snapshot-diff router\n\nAdr-Reference: ADR-0003\n"
        );
    }

    #[test]
    fn task_slug_normalizes() {
        // lowercase, non-alphanumerics -> single dash, trimmed, capped at 40 chars
        assert_eq!(
            task_slug("Adopt Snapshot-Diff Router"),
            "adopt-snapshot-diff-router"
        );
        assert_eq!(task_slug("  weird  ++  spacing  "), "weird-spacing");
        assert_eq!(task_slug("ünïcode & symbols!"), "n-code-symbols");
        let long = task_slug(&"x".repeat(100));
        assert!(long.len() <= 40);
        // never empty: fall back to "task"
        assert_eq!(task_slug("!!!"), "task");
    }

    #[test]
    fn branch_name_shape() {
        assert_eq!(
            branch_name("ADR-0003", "Adopt Snapshot-Diff Router"),
            "conduit/adr-0003/adopt-snapshot-diff-router"
        );
    }

    #[test]
    fn branch_name_can_never_emit_adroits_adr_namespace() {
        // Spec §The tuesday contract: a unit test proves the builder can never
        // emit the `adr/` prefix (adroit's branch namespace).
        let adversarial = [
            ("adr", "anything"),
            ("ADR-0001", "adr/sneaky"),
            ("", ""),
            ("adr/", "adr/"),
            ("ADR", "x"),
        ];
        for (reference, title) in adversarial {
            let b = branch_name(reference, title);
            assert!(
                b.starts_with("conduit/"),
                "branch {b:?} must be conduit/-rooted"
            );
            assert!(
                !b.starts_with("adr/"),
                "branch {b:?} leaked the adr/ namespace"
            );
        }
    }

    #[test]
    fn task_marker_is_hidden_html_comment() {
        assert_eq!(task_marker("adr-0003"), "<!-- conduit:task:adr-0003 -->");
    }
}
