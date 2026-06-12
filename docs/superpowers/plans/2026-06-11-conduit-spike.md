# Conduit Spike Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the conduit spike — a forge-neutral event router, a pure PR-lifecycle state machine, and one `Forge` trait that Gitea and GitHub implement identically (proven by a shared conformance suite and a transcript-diff demo).

**Architecture:** Single synchronous Rust crate (`bin` + `lib`, no tokio). Adapters implement `fetch_snapshot()`; one pure shared `diff(prev, next)` derives normalized `ForgeEvent`s; a pure `machine::step()` maps `(state, event)` to actions; `router.rs` owns all effects with write-ahead intents in a `.conduit/` file store (probe-before-reissue = exactly-once effect). The coding engine and adroit are subprocess seams.

**Tech Stack:** Rust edition 2024 (latest stable toolchain, matching adroit — no `rust-toolchain` file), clap, serde/serde_json, ureq 3 (rustls, behind an `HttpTransport` seam), thiserror (typed core) + anyhow (binary), time, sha2, toml; dev: assert_cmd, predicates, tempfile. Throwaway Gitea via docker compose; GitHub live reads + DryRun-decorated mutations.

---

## Normative references (read before each task)

- **Spec (normative, fully decided — do not relitigate):** `/home/brett/repos/como-tech/conduit/docs/src/dev/spike-design.md`. Tasks below cite its sections by name for rationale.
- **House style:** `/home/brett/repos/como-tech/adroit` — justfile shapes, Cargo.toml dep grouping with one-line whys, thiserror/anyhow split (typed errors in data layers, anyhow only in `main.rs`), `HttpTransport` seam (`adroit/src/forge/mod.rs:245-358`), test style (assert_cmd + tempfile + table tests).
- **Pre-verified facts (2026-06-11, do not re-derive):**
  - adroit `main` HEAD rev: `f59a5f28e5542566bc1a1318296692bcc22fffe5` (this goes in `adroit.rev`).
  - adroit has **no** `rust-toolchain` file; it uses `edition = "2024"`. conduit matches: no toolchain file, edition 2024. Local toolchain is rustc 1.96.0.
  - The installed `claude` CLI supports `-p/--print`, `--output-format json`, `--permission-mode acceptEdits`, `--disallowedTools` (verified via `claude --help`). The spec's invocation is valid as written.
  - adroit `manifest -o json` emits top-level `"tool": "adroit"` and `"manifest_schema": 1`.
  - adroit `list -o json` rows carry `reference`, `address`, `title`, `status`, `superseded_by` (nullable), plus extra fields conduit must tolerate. `show -o json` flattens the summary fields alongside `body`. `plan -o json` emits `{ "reference", "title", "plan" }` (plan = markdown).
- **Hard constraints (spec §What the spike must prove):** everything stays under `~/repos/como-tech/**`; never push to a real remote; no PR on any public forge; GitHub mutations only ever via the DryRun decorator; conduit never authors/edits/transitions an ADR.
- **Conventions for every task:** run gates via `just` recipes, never raw cargo. Commit with pathspec adds only (`git add <files>` — never `-A`). Every task ends with the repo green (`just ci` passes).

---

### Task 1: Crate scaffold — Cargo.toml, stubs, justfile, CLAUDE.md, green `just ci`

**Files:**
- Create: `/home/brett/repos/como-tech/conduit/Cargo.toml`
- Create: `/home/brett/repos/como-tech/conduit/src/lib.rs`
- Create: `/home/brett/repos/como-tech/conduit/src/main.rs`
- Create: `/home/brett/repos/como-tech/conduit/justfile`
- Create: `/home/brett/repos/como-tech/conduit/CLAUDE.md`
- Test: none (scaffold task — the gate is `just ci` on the empty crate)

- [ ] **Step 1: Write Cargo.toml**

Use the **latest published version** of each dep (`cargo search <name>` if unsure; versions below were current 2026-06-11 — bump if newer). Keep the grouping + one-line whys (adroit house rule).

```toml
[package]
name = "conduit"
version = "0.1.0"
edition = "2024"
license = "Apache-2.0"
description = "Forge-neutral agentic development harness — the Adopt-stage engine of the Como TAPS loop"

[dependencies]
# ── Core — fully synchronous, no tokio (spec §Stack: poll-tick loop, one task at a time).
# Binary-layer errors; typed thiserror enums live in the lib modules.
anyhow = "1"
clap = { version = "4", features = ["derive", "env"] }
serde = { version = "1", features = ["derive"] }
# `-o json` output, transcript JSONL, adroit/engine JSON envelopes, snapshot persistence.
serde_json = "1"
# Plan-snapshot integrity hash + FakeEngine's deterministic output.
sha2 = "0.10"
# Typed errors in the forge/store/machine/adroit layers (anyhow stays in main.rs).
thiserror = "2"
# Snapshot timestamps + review submitted_at.
time = { version = "0.3", features = ["serde-human-readable", "formatting", "parsing", "macros"] }
# conduit.toml config (not in the spec dep list, required by §Module layout src/config.rs).
toml = "1"
# Blocking HTTP (rustls) — only ever called through the HttpTransport seam.
ureq = "3"

[dev-dependencies]
assert_cmd = "2"
predicates = "3"
tempfile = "3"
```

- [ ] **Step 2: Write src/lib.rs and src/main.rs stubs**

`src/lib.rs`:

```rust
//! conduit — forge-neutral agentic development harness (Adopt-stage engine).
//!
//! Library crate: all logic lives here; `main.rs` is clap marshalling +
//! human rendering only. Spec: docs/src/dev/spike-design.md.
```

`src/main.rs`:

```rust
fn main() -> anyhow::Result<()> {
    Ok(())
}
```

- [ ] **Step 3: Write the justfile**

Shape stolen from adroit's justfile (`ci` = fmt-check + clippy + test + book). `init-adroit`, `forge-up`, `forge-down`, `demo` are added by later tasks — do NOT add them now.

```just
# Default: list available recipes
default:
    @just --list

# Install project toolchain components and tools
init:
    rustup component add clippy rustfmt
    cargo install mdbook

# Run all CI checks: format, lint, tests, book build (the house gate)
ci: fmt-check lint test book

# Format code
fmt:
    cargo fmt

# Check formatting without modifying files
fmt-check:
    cargo fmt --check

# Run clippy lints over all targets
lint:
    cargo clippy --all-targets -- -D warnings

# Run all tests (unit + integration)
test *ARGS:
    cargo test {{ARGS}}

# Type-check without building
check:
    cargo check

# Run the binary with arguments
run *ARGS:
    cargo run -- {{ARGS}}

# Build the user manual (mdbook)
book:
    mdbook build docs
    @echo "Book built -> docs/book"

# Serve the book locally with live reload
book-serve:
    mdbook serve docs --open
```

- [ ] **Step 4: Write CLAUDE.md (working agreements distilled from spec §What the spike must prove + house rules)**

```markdown
# conduit

Forge-neutral agentic development harness — the Adopt-stage engine of the Como
TAPS loop. Spike spec (normative): `docs/src/dev/spike-design.md`.

## Working agreements (IMPORTANT — read first)

- **Never push to a real remote. Never open a PR on any public forge.** The only
  push target that ever exists is the throwaway localhost Gitea container (and
  local bare repos in tests). GitHub mutations are ALWAYS DryRun-decorated —
  the constructor only hands out `DryRun(GitHubForge)`.
- **All work stays under `~/repos/como-tech/**`.** Tokens live in gitignored
  `.secrets/`; never commit or log them.
- **Humans hold every gate.** No `Forge::merge` method exists; the `conduit:run`
  label and PR review/merge are human actions. Do not add automation that
  bypasses a gate.
- **conduit never authors, edits, or transitions an ADR** — that is adroit's
  lane. The only adroit subcommands conduit may invoke are
  `{manifest, list, show, plan}` (enforced by test).
- **All documentation lives in the mdbook** (`docs/src/**`, wired into
  `docs/src/SUMMARY.md`). No standalone Markdown docs elsewhere. Keep code and
  docs in sync; `just book` must build.
- **No client names** in docs/comments/examples — keep examples generic.
- Never write a bare `#<number>` in forge-rendered text (commits, PR/issue
  bodies) — use `task N` / plain `N`.

## Build & test

Always use `just` recipes — never raw `cargo`/`mdbook`.

```sh
just init        # toolchain components + mdbook
just init-adroit # pinned adroit -> .conduit/bin (reads adroit.rev)
just ci          # fmt-check + clippy + test + book (the gate)
just test        # all tests
just forge-up    # throwaway Gitea on localhost:3000 (demo/)
just forge-down  # destroy it
```

Env-gated test legs: `CONDUIT_E2E_GITEA=1` (live Gitea conformance),
`CONDUIT_E2E_GITHUB=1` (GitHub live reads), `CONDUIT_E2E_ADROIT=1` (pinned
adroit binary contract tests).

## Design rules

- Fully synchronous — no tokio. HTTP via ureq behind the `HttpTransport` seam;
  unit tests inject `FakeTransport`, never the network.
- Typed errors (`thiserror`) in lib modules; `anyhow` only in `main.rs`.
- Pure core, effectful shell: `contract.rs`, `machine.rs`, `forge::diff` are
  pure and exhaustively unit-tested; `router.rs` owns all effects.
- State is files under `.conduit/` you can `cat` — no database.
- Never put test-only state in a production type; use injected fakes
  (`FakeForge`, `FakeEngine`, `FakeTransport`) and documented env overrides.
```

- [ ] **Step 5: Build and run the gate**

Run: `cd /home/brett/repos/como-tech/conduit && cargo build && just ci`
Expected: build succeeds; `just ci` passes (fmt-check, clippy, an empty test run, mdbook build).

- [ ] **Step 6: Commit**

```bash
cd /home/brett/repos/como-tech/conduit
git add Cargo.toml Cargo.lock src/lib.rs src/main.rs justfile CLAUDE.md
git commit -m "chore: crate scaffold, justfile, working agreements"
```

**Verify gate:** `just ci` — all four legs pass on the empty crate.

---

### Task 2: src/contract.rs — the tuesday contract, pure and exhaustively tested

All emission lives here — the single place the contract can drift (spec §The tuesday contract).

**Files:**
- Create: `/home/brett/repos/como-tech/conduit/src/contract.rs`
- Modify: `/home/brett/repos/como-tech/conduit/src/lib.rs` (add `pub mod contract;`)
- Test: inline `#[cfg(test)] mod tests` in `src/contract.rs`

- [ ] **Step 1: Write the failing tests (full module skeleton with `todo!()` bodies so it compiles, or write tests first and stub signatures)**

Create `src/contract.rs` with the public API stubbed (`todo!()`) and these tests:

```rust
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
        assert_eq!(task_slug("Adopt Snapshot-Diff Router"), "adopt-snapshot-diff-router");
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
            assert!(b.starts_with("conduit/"), "branch {b:?} must be conduit/-rooted");
            assert!(!b.starts_with("adr/"), "branch {b:?} leaked the adr/ namespace");
        }
    }

    #[test]
    fn task_marker_is_hidden_html_comment() {
        assert_eq!(task_marker("adr-0003"), "<!-- conduit:task:adr-0003 -->");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib contract`
Expected: FAIL — panics on `todo!()` (or compile errors if signatures missing; fix signatures, keep `todo!()` bodies, re-run until failures are the `todo!()` panics).

- [ ] **Step 3: Implement the module**

```rust
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
    format!("{}\n\n{}\n", pr_title(reference, title), body_trailer(reference))
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
    if slug.is_empty() { "task".to_string() } else { slug }
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
```

Add `pub mod contract;` to `src/lib.rs`.

Note `branch_name("adr", ...)` → `task_slug("adr")` = `"adr"` → `"conduit/adr/..."` which still starts with `conduit/` — the guard is structural. Verify the test's expected values against the implementation (e.g. `task_slug("ünïcode & symbols!")` — non-ASCII chars are non-alphanumeric ASCII so they collapse to dashes: `"n-code-symbols"`); if an expectation was wrong, fix the TEST to match the documented rule (lowercase ASCII alphanumerics kept, everything else collapses), not the rule.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib contract`
Expected: PASS (all tests above).

- [ ] **Step 5: Run the gate and commit**

Run: `just ci`
Expected: PASS.

```bash
cd /home/brett/repos/como-tech/conduit
git add src/contract.rs src/lib.rs
git commit -m "feat(contract): tuesday contract emission — labels, titles, trailers, branch builder"
```

**Verify gate:** `cargo test --lib contract` all pass + `just ci` green.

---

### Task 3: src/task.rs + src/machine.rs — Task model and the pure state machine

Spec §Lifecycle state machine. `step()` is pure (zero I/O, exhaustive match); `router.rs` (Task 12) executes the actions.

**Design note (ordering, decided here):** the machine needs review verdicts and engine results before `forge/mod.rs` (Task 6) and `engine/mod.rs` (Task 11) exist. So `task.rs` owns `ReviewVerdict`, `EngineResult`, `IssueId`, `PrId`, `ReviewId`; Tasks 6/11 **reuse** these types (forge `Review.verdict: ReviewVerdict`, engine maps `EngineOutcome` → `EngineResult`). One definition, no duplicates.

**Files:**
- Create: `/home/brett/repos/como-tech/conduit/src/task.rs`
- Create: `/home/brett/repos/como-tech/conduit/src/machine.rs`
- Modify: `/home/brett/repos/como-tech/conduit/src/lib.rs` (add `pub mod task; pub mod machine;`)
- Test: `/home/brett/repos/como-tech/conduit/tests/machine.rs`

- [ ] **Step 1: Write src/task.rs (the model — needed by the tests)**

```rust
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
    pub fn new(adr_reference: &str, adr_address: &str, title: &str, plan_sha256: &str) -> TaskRecord {
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
```

- [ ] **Step 2: Write src/machine.rs signatures (stub `step` with `todo!()`)**

```rust
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
    IssueLabeled { label: String },
    ReviewSubmitted { verdict: ReviewVerdict, body: String },
    /// Consumed, never acted on in the spike (must-ignore in EVERY state).
    CiChanged,
    PrMerged { merge_sha: String },
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
    Keep,
    Append(String),
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
        Transition { next: state, actions: vec![], feedback: FeedbackOp::Keep, bump_attempt: false }
    }
}

pub fn step(record: &TaskRecord, event: &Event) -> Transition {
    todo!()
}
```

Add `pub mod task; pub mod machine;` to `src/lib.rs`.

- [ ] **Step 3: Write the failing table tests in tests/machine.rs**

The transition table being tested (spec §Lifecycle state machine, including the two reviewer-mandated rules: **PrMerged/PrClosed are must-act from ANY non-terminal state whose task has an open PR**; **CiChanged is must-ignore everywhere**):

| State \ Event | IssueLabeled `conduit:run` | ReviewSubmitted CR | ReviewSubmitted Approved/Commented | CiChanged | PrMerged (pr set) | PrClosed (pr set) | EngineFinished Completed | EngineFinished Failed |
|---|---|---|---|---|---|---|---|---|
| Scoped | → Coding `[RunEngine fresh]` | ignore | ignore | ignore | → Merged `[CloseIssue]` (can't occur in practice — pinned for table/impl agreement) | → Abandoned `[CloseIssue]` (ditto) | ignore | ignore |
| Coding | ignore | ignore (append? no — not yet InReview; ignore) | ignore | ignore | → Merged `[DisposeWorkspace, CloseIssue]` | → Abandoned `[DisposeWorkspace, CloseIssue]` | → InReview `[CommitAndPush, OpenPr, ApplyPrLabels, LinkComment]`, feedback Clear | → Failed `[FailureComment, SetIssueLabels[conduit:failed]]` |
| InReview | ignore | → Revising `[RunEngine same-branch]`, feedback Append(body) | ignore | ignore | → Merged `[CloseIssue]` | → Abandoned `[CloseIssue]` | ignore (stale) | ignore (stale) |
| Revising | ignore | stay Revising, feedback Append(body), no actions | ignore | ignore | → Merged `[DisposeWorkspace, CloseIssue]` (in-flight engine discarded) | → Abandoned `[DisposeWorkspace, CloseIssue]` | → InReview `[CommitAndPush, ApplyPrLabels]`, feedback Clear | → Failed `[FailureComment, SetIssueLabels[conduit:failed]]` |
| Failed | → Coding `[RunEngine fresh]`, bump_attempt | ignore | ignore | ignore | → Merged `[CloseIssue]` (PR existed) | → Abandoned `[CloseIssue]` | ignore | ignore |
| Merged | ignore | ignore | ignore | ignore | ignore | ignore | ignore | ignore |
| Abandoned | ignore | ignore | ignore | ignore | ignore | ignore | ignore | ignore |

Extra rules encoded in `step`:
- `IssueLabeled` with any label other than `conduit:run` is ignore in every state.
- `PrMerged`/`PrClosed` when `record.pr.is_none()` is ignore in every state (the spec's "whose task has an open PR" guard — the parenthetical names InReview/Revising/Failed-after-PR, but the bolded rule is *any* non-terminal state with a PR; a Coding retry after a Failed-with-PR also qualifies).
- `CloseIssue` comment content: for Merged, `format!("Merged as {merge_sha}. {marker}", ...)` — must contain the merge sha; for Abandoned a fixed "PR closed without merge" comment. Compose via `contract::task_marker(&record.id)`.
- `SetIssueLabels` on failure swaps the trigger: labels = `[contract::LABEL_FAILED.to_string()]` (convergent set — `conduit:run` absent removes it).

`tests/machine.rs` (complete — exhaustive by construction: an explicit must-act table, then a sweep asserting every other (state, event, pr) combination is identity):

```rust
use conduit::contract;
use conduit::machine::{step, Action, Event, FeedbackOp, Transition};
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
        Event::IssueLabeled { label: contract::LABEL_RUN.to_string() },
        Event::IssueLabeled { label: "unrelated".to_string() },
        Event::ReviewSubmitted { verdict: ReviewVerdict::ChangesRequested, body: "fix x".into() },
        Event::ReviewSubmitted { verdict: ReviewVerdict::Approved, body: "lgtm".into() },
        Event::ReviewSubmitted { verdict: ReviewVerdict::Commented, body: "note".into() },
        Event::CiChanged,
        Event::PrMerged { merge_sha: "abc123".to_string() },
        Event::PrClosed,
        Event::EngineFinished(EngineResult::Completed { summary: "done".into() }),
        Event::EngineFinished(EngineResult::Failed { reason: "boom".into(), log_tail: "tail".into() }),
    ]
}

/// Action-kind fingerprint, so the table compares shape not payload.
fn kinds(t: &Transition) -> Vec<&'static str> {
    t.actions
        .iter()
        .map(|a| match a {
            Action::RunEngine { fresh_workspace: true } => "run-fresh",
            Action::RunEngine { fresh_workspace: false } => "run-same",
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
    let run = || Event::IssueLabeled { label: contract::LABEL_RUN.to_string() };
    let cr = || Event::ReviewSubmitted { verdict: ReviewVerdict::ChangesRequested, body: "fix x".into() };
    let merged = || Event::PrMerged { merge_sha: "abc123".to_string() };
    let done = || Event::EngineFinished(EngineResult::Completed { summary: "done".into() });
    let failed = || Event::EngineFinished(EngineResult::Failed { reason: "boom".into(), log_tail: "tail".into() });

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
        t.push(Cell { state: Scoped, has_pr, event: run(), next: Coding,
               action_kinds: &["run-fresh"], feedback: FeedbackOp::Keep, bump_attempt: false });
        t.push(Cell { state: Coding, has_pr, event: done(), next: InReview,
               action_kinds: &["push", "open-pr", "pr-labels", "link"], feedback: FeedbackOp::Clear, bump_attempt: false });
        t.push(Cell { state: Coding, has_pr, event: failed(), next: Failed,
               action_kinds: &["fail-comment", "issue-labels"], feedback: FeedbackOp::Keep, bump_attempt: false });
        t.push(Cell { state: InReview, has_pr, event: cr(), next: Revising,
               action_kinds: &["run-same"], feedback: FeedbackOp::Append("fix x".into()), bump_attempt: false });
        t.push(Cell { state: Revising, has_pr, event: cr(), next: Revising,
               action_kinds: &[], feedback: FeedbackOp::Append("fix x".into()), bump_attempt: false });
        t.push(Cell { state: Revising, has_pr, event: done(), next: InReview,
               action_kinds: &["push", "pr-labels"], feedback: FeedbackOp::Clear, bump_attempt: false });
        t.push(Cell { state: Revising, has_pr, event: failed(), next: Failed,
               action_kinds: &["fail-comment", "issue-labels"], feedback: FeedbackOp::Keep, bump_attempt: false });
        t.push(Cell { state: Failed, has_pr, event: run(), next: Coding,
               action_kinds: &["run-fresh"], feedback: FeedbackOp::Keep, bump_attempt: true });
    }
    // PR-REQUIRED cells (the open-PR guard): only has_pr=true — with
    // has_pr=false these events are must-ignore (covered by the sweep).
    // ALL five non-terminal states appear: the guard in `step` is
    // `record.pr.is_some()` from any non-terminal state (Scoped-with-PR cannot
    // occur in practice, but table and implementation must agree cell-for-cell).
    // Coding/Revising additionally dispose the workspace (in-flight engine
    // result discarded — reviewer-mandated).
    for (state, dispose) in [(Scoped, false), (Coding, true), (InReview, false), (Revising, true), (Failed, false)] {
        let kinds: &'static [&'static str] = if dispose { &["dispose", "close-issue"] } else { &["close-issue"] };
        t.push(Cell { state, has_pr: true, event: merged(), next: Merged,
               action_kinds: kinds, feedback: FeedbackOp::Keep, bump_attempt: false });
        t.push(Cell { state, has_pr: true, event: Event::PrClosed, next: Abandoned,
               action_kinds: kinds, feedback: FeedbackOp::Keep, bump_attempt: false });
    }
    t
}

#[test]
fn must_act_cells() {
    for cell in must_act_table() {
        let r = rec(cell.state, cell.has_pr);
        let t = step(&r, &cell.event);
        assert_eq!(t.next, cell.next, "{:?} + {:?}", cell.state, cell.event);
        assert_eq!(kinds(&t), cell.action_kinds, "{:?} + {:?}", cell.state, cell.event);
        assert_eq!(t.feedback, cell.feedback, "{:?} + {:?}", cell.state, cell.event);
        assert_eq!(t.bump_attempt, cell.bump_attempt, "{:?} + {:?}", cell.state, cell.event);
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
                let in_table = table.iter().any(|c| {
                    c.state == state && c.has_pr == has_pr && c.event == event
                });
                if in_table {
                    continue;
                }
                let r = rec(state, has_pr);
                let t = step(&r, &event);
                assert_eq!(t, Transition::ignore(state),
                    "expected must-ignore: {state:?} (pr={has_pr}) + {event:?}");
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
                assert_eq!(step(&r, &event), Transition::ignore(state),
                    "{state:?} + {event:?}");
            }
        }
    }
}

/// PrMerged/PrClosed with NO pr on the record are ignored (the open-PR guard).
#[test]
fn terminal_pr_events_require_an_open_pr() {
    for state in [TaskState::Scoped, TaskState::Coding, TaskState::InReview,
                  TaskState::Revising, TaskState::Failed] {
        let r = rec(state, false);
        // Note: InReview/Revising "without a PR" cannot occur in practice (the
        // PR is opened entering InReview) but the guard must still hold.
        assert_eq!(step(&r, &Event::PrMerged { merge_sha: "abc".into() }),
                   Transition::ignore(state));
        assert_eq!(step(&r, &Event::PrClosed), Transition::ignore(state));
    }
}

/// The merged CloseIssue comment carries the merge sha (the completion beat).
#[test]
fn merged_close_comment_contains_sha() {
    let r = rec(TaskState::InReview, true);
    let t = step(&r, &Event::PrMerged { merge_sha: "cafe42".into() });
    let Some(Action::CloseIssue { comment }) = t.actions.iter()
        .find(|a| matches!(a, Action::CloseIssue { .. })) else {
        panic!("expected CloseIssue");
    };
    assert!(comment.contains("cafe42"));
}

/// Engine failure swaps the trigger label to conduit:failed (convergent set).
#[test]
fn failure_swaps_run_label_to_failed() {
    let r = rec(TaskState::Coding, false);
    let t = step(&r, &Event::EngineFinished(EngineResult::Failed {
        reason: "boom".into(), log_tail: "tail".into() }));
    let Some(Action::SetIssueLabels { labels }) = t.actions.iter()
        .find(|a| matches!(a, Action::SetIssueLabels { .. })) else {
        panic!("expected SetIssueLabels");
    };
    assert!(labels.contains(&contract::LABEL_FAILED.to_string()));
    assert!(!labels.contains(&contract::LABEL_RUN.to_string()));
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test --test machine`
Expected: FAIL — `todo!()` panic in `step` (compile must succeed first).

- [ ] **Step 5: Implement `step` (exhaustive match over `(record.state, event)`)**

Implement exactly the table above. Skeleton shape (fill every arm; no wildcard `_ => ...` that hides a state — match on the state first, events inside):

```rust
pub fn step(record: &TaskRecord, event: &Event) -> Transition {
    use TaskState::*;
    let ignore = || Transition::ignore(record.state);
    if record.state.is_terminal() {
        return ignore();
    }
    // Terminal PR events: must-act from ANY non-terminal state with an open PR.
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
            return Transition { next: Merged, actions, feedback: FeedbackOp::Keep, bump_attempt: false };
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
            return Transition { next: Abandoned, actions, feedback: FeedbackOp::Keep, bump_attempt: false };
        }
        _ => {}
    }
    match (record.state, event) {
        (Scoped, Event::IssueLabeled { label }) if label == crate::contract::LABEL_RUN => Transition {
            next: Coding,
            actions: vec![Action::RunEngine { fresh_workspace: true }],
            feedback: FeedbackOp::Keep,
            bump_attempt: false,
        },
        (Failed, Event::IssueLabeled { label }) if label == crate::contract::LABEL_RUN => Transition {
            next: Coding,
            actions: vec![Action::RunEngine { fresh_workspace: true }],
            feedback: FeedbackOp::Keep,
            bump_attempt: true,
        },
        (Coding, Event::EngineFinished(EngineResult::Completed { .. })) => Transition {
            next: InReview,
            actions: vec![Action::CommitAndPush, Action::OpenPr, Action::ApplyPrLabels, Action::LinkComment],
            feedback: FeedbackOp::Clear,
            bump_attempt: false,
        },
        (Revising, Event::EngineFinished(EngineResult::Completed { .. })) => Transition {
            next: InReview,
            actions: vec![Action::CommitAndPush, Action::ApplyPrLabels],
            feedback: FeedbackOp::Clear,
            bump_attempt: false,
        },
        (Coding | Revising, Event::EngineFinished(EngineResult::Failed { reason, log_tail })) => Transition {
            next: Failed,
            actions: vec![
                Action::FailureComment { reason: reason.clone(), log_tail: log_tail.clone() },
                Action::SetIssueLabels { labels: vec![crate::contract::LABEL_FAILED.to_string()] },
            ],
            feedback: FeedbackOp::Keep,
            bump_attempt: false,
        },
        (InReview, Event::ReviewSubmitted { verdict: ReviewVerdict::ChangesRequested, body }) => Transition {
            next: Revising,
            actions: vec![Action::RunEngine { fresh_workspace: false }],
            feedback: FeedbackOp::Append(body.clone()),
            bump_attempt: false,
        },
        (Revising, Event::ReviewSubmitted { verdict: ReviewVerdict::ChangesRequested, body }) => Transition {
            next: Revising,
            actions: vec![],
            feedback: FeedbackOp::Append(body.clone()),
            bump_attempt: false,
        },
        _ => ignore(),
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --test machine`
Expected: PASS (all 7 tests).

- [ ] **Step 7: Run the gate and commit**

Run: `just ci`
Expected: PASS.

```bash
cd /home/brett/repos/como-tech/conduit
git add src/task.rs src/machine.rs src/lib.rs tests/machine.rs
git commit -m "feat(machine): task model + pure 7-state lifecycle machine with exhaustive table tests"
```

**Verify gate:** `cargo test --test machine` all pass + `just ci` green.

---

### Task 4: src/store.rs — the .conduit/ file store

Spec §Crash consistency: state is files you can `cat`. Atomic tmp+rename+fsync writes; intents persisted **before** execution (the ordering is the router's job in Task 12, but the store API must make it possible and expose mark-done).

**On-disk layout (document in the module header):**

```
.conduit/
├── tasks/<task-id>.json     TaskRecord incl. pending ActionIntents
├── plans/<task-id>.md       verbatim plan snapshot (sha256 recorded on the record)
├── cursor/<forge>.json      previous RepoSnapshot per forge (the poll cursor)
├── cache/<forge>.git        local bare git cache (Task 11, git.rs)
├── workspaces/<task-id>-a<attempt>/   engine workspaces (disposable)
└── bin/                     pinned adroit (Task 10, `just init-adroit`)
```

**Files:**
- Create: `/home/brett/repos/como-tech/conduit/src/store.rs`
- Modify: `/home/brett/repos/como-tech/conduit/src/lib.rs` (add `pub mod store;`)
- Test: inline `#[cfg(test)] mod tests` in `src/store.rs` (tempfile)

- [ ] **Step 1: Write the failing tests (stub the API with `todo!()`)**

Public API:

```rust
use std::path::{Path, PathBuf};

use crate::task::TaskRecord;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store I/O at {path}: {source}")]
    Io { path: PathBuf, #[source] source: std::io::Error },
    #[error("corrupt record {path}: {source}")]
    Corrupt { path: PathBuf, #[source] source: serde_json::Error },
    #[error("no plan snapshot for task {0}")]
    MissingPlan(String),
}

pub struct Store {
    root: PathBuf, // the .conduit dir
}

impl Store {
    /// Open (and create dirs under) `<repo>/.conduit`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Store, StoreError>;
    pub fn root(&self) -> &Path;
    pub fn workspace_dir(&self, task_id: &str, attempt: u32) -> PathBuf;

    // Tasks — atomic tmp+rename+fsync (file AND parent dir).
    pub fn save_task(&self, rec: &TaskRecord) -> Result<(), StoreError>;
    pub fn load_task(&self, id: &str) -> Result<Option<TaskRecord>, StoreError>;
    pub fn list_tasks(&self) -> Result<Vec<TaskRecord>, StoreError>;
    /// Load-modify-save: set `pending[index].done = true`. Atomic.
    pub fn mark_intent_done(&self, task_id: &str, index: usize) -> Result<(), StoreError>;

    // Plan snapshots — written verbatim, fsynced; returns the sha256 hex.
    pub fn save_plan(&self, task_id: &str, markdown: &str) -> Result<String, StoreError>;
    pub fn load_plan(&self, task_id: &str) -> Result<String, StoreError>;

    // Poll cursor — the previous RepoSnapshot per forge, as opaque JSON
    // (forge types arrive in Task 6; serde_json::Value keeps the store decoupled).
    pub fn save_cursor(&self, forge: &str, snapshot: &serde_json::Value) -> Result<(), StoreError>;
    pub fn load_cursor(&self, forge: &str) -> Result<Option<serde_json::Value>, StoreError>;
}
```

Tests (complete):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Action;
    use crate::task::{ActionIntent, TaskRecord};
    use tempfile::TempDir;

    fn store() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let s = Store::open(dir.path().join(".conduit")).unwrap();
        (dir, s)
    }

    fn rec_with_pending() -> TaskRecord {
        let mut r = TaskRecord::new("ADR-0003", "3", "Adopt snapshot-diff router", "deadbeef");
        r.pending = vec![
            ActionIntent { action: Action::OpenPr, done: false },
            ActionIntent { action: Action::ApplyPrLabels, done: false },
        ];
        r
    }

    #[test]
    fn save_then_load_round_trips_including_pending_intents() {
        let (_d, s) = store();
        let r = rec_with_pending();
        s.save_task(&r).unwrap();
        let loaded = s.load_task(&r.id).unwrap().unwrap();
        assert_eq!(loaded.pending.len(), 2);
        assert!(!loaded.pending[0].done);
        assert_eq!(loaded.adr_reference, "ADR-0003");
    }

    #[test]
    fn load_missing_task_is_none() {
        let (_d, s) = store();
        assert!(s.load_task("nope").unwrap().is_none());
    }

    #[test]
    fn mark_intent_done_persists() {
        let (_d, s) = store();
        let r = rec_with_pending();
        s.save_task(&r).unwrap();
        s.mark_intent_done(&r.id, 0).unwrap();
        let loaded = s.load_task(&r.id).unwrap().unwrap();
        assert!(loaded.pending[0].done);
        assert!(!loaded.pending[1].done);
    }

    #[test]
    fn atomic_write_leaves_no_tmp_file_and_survives_stale_tmp() {
        let (_d, s) = store();
        let r = rec_with_pending();
        // A stale tmp from a "crash" mid-write must not break a later save/load.
        let tasks = s.root().join("tasks");
        std::fs::write(tasks.join(format!("{}.json.tmp", r.id)), b"{ partial").unwrap();
        s.save_task(&r).unwrap();
        assert!(!tasks.join(format!("{}.json.tmp", r.id)).exists(),
            "tmp must be renamed away");
        assert!(s.load_task(&r.id).unwrap().is_some());
        // list_tasks ignores non-.json / tmp leftovers
        std::fs::write(tasks.join("other.json.tmp"), b"junk").unwrap();
        assert_eq!(s.list_tasks().unwrap().len(), 1);
    }

    #[test]
    fn plan_snapshot_round_trips_verbatim_and_returns_sha256() {
        let (_d, s) = store();
        let md = "# Plan\n\nstep one\n"; // exact bytes, incl. trailing newline
        let sha = s.save_plan("adr-0003", md).unwrap();
        assert_eq!(s.load_plan("adr-0003").unwrap(), md);
        // sha256 of the exact bytes
        use sha2::{Digest, Sha256};
        let want = format!("{:x}", Sha256::digest(md.as_bytes()));
        assert_eq!(sha, want);
    }

    #[test]
    fn missing_plan_is_a_typed_error() {
        let (_d, s) = store();
        assert!(matches!(s.load_plan("nope"), Err(StoreError::MissingPlan(_))));
    }

    #[test]
    fn cursor_round_trips_per_forge() {
        let (_d, s) = store();
        assert!(s.load_cursor("gitea").unwrap().is_none());
        let snap = serde_json::json!({"issues": [], "prs": []});
        s.save_cursor("gitea", &snap).unwrap();
        assert_eq!(s.load_cursor("gitea").unwrap().unwrap(), snap);
        assert!(s.load_cursor("github").unwrap().is_none(), "cursors are per-forge");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib store`
Expected: FAIL — `todo!()` panics.

- [ ] **Step 3: Implement**

Core atomic write (use everywhere; never a bare `fs::write` for records/plans/cursors):

```rust
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    use std::io::Write;
    let io = |source| StoreError::Io { path: path.to_path_buf(), source };
    let tmp = path.with_extension("json.tmp"); // or "md.tmp" — derive from path: append ".tmp" to the full filename instead
    // Simpler + collision-safe: tmp = same dir, filename + ".tmp"
    let tmp = {
        let mut os = path.as_os_str().to_owned();
        os.push(".tmp");
        PathBuf::from(os)
    };
    let mut f = std::fs::File::create(&tmp).map_err(io)?;
    f.write_all(bytes).map_err(io)?;
    f.sync_all().map_err(io)?; // fsync the file
    std::fs::rename(&tmp, path).map_err(io)?;
    // fsync the parent dir so the rename itself is durable
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent).and_then(|d| d.sync_all()).map_err(io)?;
    }
    Ok(())
}
```

(Resolve the duplicate `tmp` binding above — keep the filename+`.tmp` version only.) `open` creates `tasks/ plans/ cursor/ cache/ workspaces/ bin/`. `save_task` serializes with `serde_json::to_vec_pretty` (cat-able). `list_tasks` reads `tasks/*.json` only (skip `.tmp`). `save_plan` computes `sha2::Sha256::digest` of the exact bytes, writes atomically, returns lowercase hex. Add `pub mod store;` to `src/lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib store`
Expected: PASS.

- [ ] **Step 5: Run the gate and commit**

Run: `just ci`
Expected: PASS.

```bash
cd /home/brett/repos/como-tech/conduit
git add src/store.rs src/lib.rs
git commit -m "feat(store): .conduit file store — atomic writes, intents, plan snapshots, cursors"
```

**Verify gate:** `cargo test --lib store` all pass + `just ci` green.

---

### Task 5: src/config.rs + src/cli.rs skeleton — conduit.toml, env overlay, clap surface

Spec §Module layout + §Demo script. No behavior beyond config load + `status` reading the store; the other subcommands print a clear "not implemented yet" error and exit non-zero (they are wired in Tasks 13).

**Files:**
- Create: `/home/brett/repos/como-tech/conduit/src/config.rs`
- Create: `/home/brett/repos/como-tech/conduit/src/cli.rs`
- Modify: `/home/brett/repos/como-tech/conduit/src/main.rs` (clap marshalling)
- Modify: `/home/brett/repos/como-tech/conduit/src/lib.rs` (add `pub mod config; pub mod cli;`)
- Test: `/home/brett/repos/como-tech/conduit/tests/cli.rs` + inline config tests

- [ ] **Step 1: Write src/config.rs tests first (inline), stub the API**

Config structs (complete — these are the contract for `conduit.toml`):

```rust
use serde::{Deserialize, Serialize};

use crate::contract::EffortThresholds;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ForgeKind { Gitea, Github }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EngineKind { Fake, ClaudeCode }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub forge: ForgeConfig,
    pub engine: EngineConfig,
    pub adroit: AdroitConfig,
    pub effort: EffortThresholds,
    pub poll: PollConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ForgeConfig {
    /// Which adapter `--forge` defaults to.
    pub default: ForgeKind,
    pub gitea: GiteaConfig,
    pub github: GithubConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GiteaConfig {
    pub base_url: String, // default "http://localhost:3000"
    pub owner: String,    // default "como"
    pub repo: String,     // default "conduit-dogfood"
    // token: NEVER in the file — env CONDUIT_GITEA_TOKEN, falling back to
    // .secrets/conduit-bot.token (the gitea-init.sh drop location, Task 8).
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GithubConfig {
    pub owner: String, // default ""
    pub repo: String,  // default ""
    // token: env GITHUB_TOKEN only (live READS; mutations always DryRun).
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    pub kind: EngineKind,  // default Fake (spec §Fakes: the default demo path)
    pub timeout_secs: u64, // default 1800 — the conduit-enforced hard timeout
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdroitConfig {
    pub dir: String,         // default "adr" — the in-repo dogfood corpus
    pub ai_provider: String, // default "ollama" (spec §adroit integration: Plan snapshot)
    pub ai_model: String,    // default "llama3.2"
    // ADROIT_ANTHROPIC_KEY upgrade path: passed through from conduit's env if set.
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PollConfig {
    pub interval_secs: u64, // default 15
}

impl Config {
    /// Load `conduit.toml` from `dir` (missing file = all defaults), then
    /// overlay env: CONDUIT_FORGE (gitea|github), CONDUIT_ENGINE
    /// (fake|claude-code), CONDUIT_TIMEOUT_SECS, CONDUIT_POLL_SECS.
    pub fn load(dir: &std::path::Path) -> Result<Config, ConfigError>;
    /// Gitea token: env CONDUIT_GITEA_TOKEN, else `.secrets/conduit-bot.token`
    /// under `dir` (trimmed), else None. Never logged.
    pub fn gitea_token(dir: &std::path::Path) -> Option<String>;
    /// GitHub token: env GITHUB_TOKEN, else None (reads-only adapter).
    pub fn github_token() -> Option<String>;
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Io { path: std::path::PathBuf, #[source] source: std::io::Error },
    #[error("invalid conduit.toml: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid env override {var}={value}")]
    Env { var: String, value: String },
}
```

Every struct gets a `Default` impl with the documented defaults. Inline tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_yields_defaults() {
        let d = TempDir::new().unwrap();
        let c = Config::load(d.path()).unwrap();
        assert_eq!(c.forge.default, ForgeKind::Gitea);
        assert_eq!(c.engine.kind, EngineKind::Fake);
        assert_eq!(c.engine.timeout_secs, 1800);
        assert_eq!(c.forge.gitea.base_url, "http://localhost:3000");
        assert_eq!(c.adroit.ai_provider, "ollama");
        assert_eq!(c.poll.interval_secs, 15);
    }

    #[test]
    fn file_values_override_defaults_and_partial_files_parse() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("conduit.toml"),
            "[engine]\nkind = \"claude-code\"\ntimeout_secs = 60\n").unwrap();
        let c = Config::load(d.path()).unwrap();
        assert_eq!(c.engine.kind, EngineKind::ClaudeCode);
        assert_eq!(c.engine.timeout_secs, 60);
        assert_eq!(c.poll.interval_secs, 15, "unset sections keep defaults");
    }

    #[test]
    fn effort_thresholds_load_from_toml() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("conduit.toml"),
            "[effort]\nsuper_quick_max_ms = 5\n").unwrap();
        let c = Config::load(d.path()).unwrap();
        assert_eq!(c.effort.super_quick_max_ms, 5);
        assert_eq!(c.effort.not_long_max_ms, 30 * 60 * 1000);
    }

    #[test]
    fn gitea_token_falls_back_to_secrets_file() {
        // env wins is covered by the CLI test (env in-process is racy in
        // parallel unit tests — do NOT set_var here).
        let d = TempDir::new().unwrap();
        std::fs::create_dir(d.path().join(".secrets")).unwrap();
        std::fs::write(d.path().join(".secrets/conduit-bot.token"), "tok123\n").unwrap();
        // Only assert the file fallback when the env var is absent in the test
        // runner; guard accordingly.
        if std::env::var("CONDUIT_GITEA_TOKEN").is_err() {
            assert_eq!(Config::gitea_token(d.path()).as_deref(), Some("tok123"));
        }
    }
}
```

- [ ] **Step 2: Write src/cli.rs (the full clap surface — this IS the CLI contract)**

```rust
//! CLI surface (spec §Module layout):
//! init | plan <address> | run [--once] | status | verify <address> | demo-transcript <address>
//! Globals: --forge <gitea|github>, -o/--output <human|json>.

use clap::{Parser, Subcommand, ValueEnum};

use crate::config::ForgeKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat { Human, Json }

#[derive(Debug, Parser)]
#[command(name = "conduit", version, about = "Forge-neutral agentic development harness")]
pub struct Cli {
    /// Forge adapter to use (defaults to conduit.toml [forge].default).
    #[arg(long, global = true, value_enum)]
    pub forge: Option<ForgeKind>,
    /// Output format for read verbs.
    #[arg(short = 'o', long = "output", global = true, value_enum, default_value = "human")]
    pub output: OutputFormat,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize: .conduit store + pre-create the label set on the forge.
    Init,
    /// Plan an accepted ADR into a Scoped task: adroit handshake -> show ->
    /// enforce Accepted -> adroit plan -> persist snapshot verbatim -> issue.
    Plan { address: String },
    /// Poll-tick loop: fetch -> diff -> step -> execute -> persist.
    Run {
        /// Run exactly one tick, then exit.
        #[arg(long)]
        once: bool,
    },
    /// Show every task record (the whole lifecycle, inspectable).
    Status,
    /// Machine-assert the tuesday contract on the merged PR for this ADR.
    Verify { address: String },
    /// Forge-neutrality demo: fixture events -> real machine + FakeEngine ->
    /// normalized action transcript (JSONL on stdout).
    DemoTranscript { address: String },
}
```

`src/main.rs` becomes:

```rust
use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = conduit::cli::Cli::parse();
    conduit::cli::dispatch(cli)
}
```

with `pub fn dispatch(cli: Cli) -> anyhow::Result<()>` in `cli.rs`: loads `Config` from the current dir; `Status` opens `Store::open(".conduit")` and prints records (`-o json` = `serde_json::to_string_pretty(&records)`, human = one `id  state  attempt  branch` line each); every other command returns `anyhow::bail!("not implemented yet: wired in a later task")`.

- [ ] **Step 3: Write the failing CLI tests**

`tests/cli.rs`:

```rust
use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn conduit(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("conduit").unwrap();
    cmd.current_dir(dir.path());
    // Hermetic env: drop any developer overrides.
    for var in ["CONDUIT_FORGE", "CONDUIT_ENGINE", "CONDUIT_GITEA_TOKEN",
                "CONDUIT_TIMEOUT_SECS", "CONDUIT_POLL_SECS", "GITHUB_TOKEN"] {
        cmd.env_remove(var);
    }
    cmd
}

#[test]
fn help_lists_all_subcommands() {
    let d = TempDir::new().unwrap();
    conduit(&d).arg("--help").assert().success()
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
    conduit(&d).args(["status", "-o", "json"]).assert().success()
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
    conduit(&d).env("CONDUIT_ENGINE", "claude-code")
        .args(["run", "--once"]).assert().failure()
        .stderr(predicate::str::contains("not implemented yet"));
}

#[test]
fn unimplemented_commands_fail_loudly() {
    let d = TempDir::new().unwrap();
    for args in [vec!["init"], vec!["plan", "3"], vec!["verify", "3"],
                 vec!["demo-transcript", "3"]] {
        conduit(&d).args(&args).assert().failure()
            .stderr(predicate::str::contains("not implemented yet"));
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test --test cli && cargo test --lib config`
Expected: FAIL (todo!/missing dispatch).

- [ ] **Step 5: Implement config load + env overlay + dispatch as specced; run to pass**

Run: `cargo test --test cli && cargo test --lib config`
Expected: PASS.

- [ ] **Step 6: Run the gate and commit**

Run: `just ci`
Expected: PASS.

```bash
cd /home/brett/repos/como-tech/conduit
git add src/config.rs src/cli.rs src/main.rs src/lib.rs tests/cli.rs
git commit -m "feat(cli): conduit.toml config with env overlay + clap surface skeleton"
```

**Verify gate:** `cargo test --test cli` + `cargo test --lib config` pass; `just ci` green.

---

### Task 6: src/forge/mod.rs — THE KEYSTONE: trait Forge, snapshot types, shared pure diff()

Spec §The forge adapter. The trait is specced verbatim — implement it exactly (including the two idempotency probes; **NO merge method** — humans merge in the forge UI, the gate is unrepresentable). Adapters implement `fetch_snapshot()`; the single pure `diff(prev, next)` derives events, so GitHub and Gitea behave identically by construction.

**Files:**
- Create: `/home/brett/repos/como-tech/conduit/src/forge/mod.rs`
- Modify: `/home/brett/repos/como-tech/conduit/src/lib.rs` (add `pub mod forge;`)
- Test: inline `#[cfg(test)] mod tests` in `src/forge/mod.rs` (diff tests)

- [ ] **Step 1: Write the types + trait (compiles; `diff` stubbed `todo!()`)**

```rust
//! THE KEYSTONE (spec §The forge adapter): one trait both forges implement
//! identically, proven by tests/conformance.rs. Events are NEVER produced by
//! adapters — only by the shared pure `diff`.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::task::{IssueId, PrId, ReviewId, ReviewVerdict};

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    /// Network / connectivity failure (connection refused, DNS, TLS, timeout).
    #[error("forge unreachable: {0}")]
    Offline(String),
    /// 401/403 — loud misconfiguration, never swallowed.
    #[error("forge auth failed (check the token env var): {0}")]
    Auth(String),
    /// Any other non-2xx, or unparseable response.
    #[error("forge API error {status}: {message}")]
    Api { status: u16, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CiState { Pending, Success, Failure, None }

/// Forge-native review identity + submitted_at (spec §Review identity): the
/// diff dedupes on `id`, so an EDITED review never re-fires and repeated
/// ChangesRequested rounds from the same reviewer are distinct events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub id: ReviewId,
    pub author: String,
    pub verdict: ReviewVerdict,
    pub body: String,
    #[serde(with = "time::serde::rfc3339")]
    pub submitted_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueSnapshot {
    pub id: IssueId,
    pub labels: Vec<String>,
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrSnapshot {
    pub id: PrId,
    pub head_branch: String,
    pub labels: Vec<String>,
    pub reviews: Vec<Review>,
    /// Consumed, never configured (spec §Out of scope: CI provisioning).
    pub ci: CiState,
    pub merged: bool,
    pub merge_sha: Option<String>,
    pub closed: bool,
}

/// One normalized read of the repo: conduit-labeled issues + conduit/*-branch
/// PRs ONLY (each adapter filters; asserted by the conformance suite).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoSnapshot {
    pub issues: Vec<IssueSnapshot>,
    pub prs: Vec<PrSnapshot>,
    #[serde(with = "time::serde::rfc3339")]
    pub fetched_at: OffsetDateTime,
}

/// Produced ONLY by the shared diff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ForgeEvent {
    IssueLabeled { issue: IssueId, label: String },
    ReviewSubmitted { pr: PrId, review: Review },
    CiChanged { pr: PrId, state: CiState },
    PrMerged { pr: PrId, merge_sha: String },
    PrClosed { pr: PrId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelSpec {
    pub name: String,
    pub color: String, // hex without '#', e.g. "00aabb"
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewIssue {
    pub title: String,
    /// Carries the hidden task marker (contract::task_marker) for the
    /// find_issue_by_marker probe.
    pub body: String,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrDraft {
    pub title: String,
    /// Final line is the Adr-Reference trailer (contract::pr_body).
    pub body: String,
    /// Head branch — already pushed by conduit's git.rs before open_pr runs.
    pub head: String,
    pub base: String,
    pub labels: Vec<String>,
}

pub trait Forge {
    fn describe(&self) -> String;
    /// Used ONLY by src/git.rs, never by engines (spec: sandbox is structural).
    fn git_remote_url(&self) -> Result<String, ForgeError>;
    // events in: one read, normalized
    fn fetch_snapshot(&self) -> Result<RepoSnapshot, ForgeError>;
    // idempotency probes (reads)
    fn find_open_pr_by_head(&self, branch: &str) -> Result<Option<PrId>, ForgeError>;
    fn find_issue_by_marker(&self, marker: &str) -> Result<Option<IssueId>, ForgeError>;
    // actions out — NO merge method exists: humans merge in the forge UI
    fn ensure_labels(&self, labels: &[LabelSpec]) -> Result<(), ForgeError>;
    fn create_issue(&self, new: &NewIssue) -> Result<IssueId, ForgeError>;
    fn upsert_issue_comment(&self, id: &IssueId, marker: &str, body: &str) -> Result<(), ForgeError>;
    fn set_issue_labels(&self, id: &IssueId, labels: &[String]) -> Result<(), ForgeError>;
    fn close_issue(&self, id: &IssueId) -> Result<(), ForgeError>;
    fn open_pr(&self, draft: &PrDraft) -> Result<PrId, ForgeError>;
    fn upsert_pr_comment(&self, id: &PrId, marker: &str, body: &str) -> Result<(), ForgeError>;
    fn set_pr_labels(&self, id: &PrId, labels: &[String]) -> Result<(), ForgeError>;
}

/// THE shared pure diff — event semantics defined once (spec §The forge adapter).
pub fn diff(prev: &RepoSnapshot, next: &RepoSnapshot) -> Vec<ForgeEvent> {
    todo!()
}
```

Also copy adoit's `HttpTransport` seam into this module, adapted (reference: `adroit/src/forge/mod.rs:245-358`):

```rust
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Blocking HTTP, abstracted so adapters are testable with a fake / recorded
/// fixtures and never hit the network in unit tests.
pub trait HttpTransport: Send + Sync {
    fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, ForgeError>;
}

/// Production transport over ureq (blocking, rustls). Non-2xx comes back as a
/// normal HttpResponse (adapters map 401/403 -> Auth, else Api); only a
/// connection-level failure is ForgeError::Offline. Set
/// `.http_status_as_error(false)` plus connect (20s) + global (60s) timeouts
/// on the ureq Agent — copy the adroit UreqTransport impl shape exactly.
pub struct UreqTransport;

/// Run one REST call: serialize body, send, classify status (2xx ok;
/// 401/403 -> Auth; else Api), parse JSON (Value::Null for empty 2xx body).
pub(crate) fn rest_call(
    transport: &dyn HttpTransport,
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<serde_json::Value>,
    label: &str,
) -> Result<serde_json::Value, ForgeError> {
    // same shape as adroit's rest_call (without the wire-logging hook;
    // error text extraction: try JSON "message" field, else lossy body string)
    todo!()
}
```

- [ ] **Step 2: Write the failing diff tests**

Diff semantics (the contract, also documented in the module header):
- `IssueLabeled`: fires for each label present on an issue in `next` but not on the same issue in `prev`. An issue absent from `prev` fires for ALL its labels. Label removals fire nothing.
- `ReviewSubmitted`: fires for each review whose `id` is not present on the same PR in `prev` (dedupe by forge-native id). A PR absent from `prev` fires for all its reviews.
- `CiChanged`: fires when a PR exists in both and `ci` differs. New PRs fire nothing.
- `PrMerged`: fires when `!prev.merged && next.merged`; `merge_sha` required (adapter guarantees it when merged; if absent, use empty string and let conformance catch it).
- `PrClosed`: fires when `!prev.closed && next.closed && !next.merged` — a merged PR emits ONLY `PrMerged` (forges mark merged PRs closed too).
- Within-poll flaps (submitted-then-dismissed) are invisible by design (spec §Review identity).

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{IssueId, PrId, ReviewId, ReviewVerdict};
    use time::macros::datetime;

    fn snap(issues: Vec<IssueSnapshot>, prs: Vec<PrSnapshot>) -> RepoSnapshot {
        RepoSnapshot { issues, prs, fetched_at: datetime!(2026-06-11 00:00 UTC) }
    }
    fn issue(id: u64, labels: &[&str]) -> IssueSnapshot {
        IssueSnapshot { id: IssueId(id), labels: labels.iter().map(|s| s.to_string()).collect(), closed: false }
    }
    fn pr(id: u64) -> PrSnapshot {
        PrSnapshot { id: PrId(id), head_branch: "conduit/adr-0003/x".into(), labels: vec![],
                     reviews: vec![], ci: CiState::None, merged: false, merge_sha: None, closed: false }
    }
    fn review(id: &str, verdict: ReviewVerdict, body: &str) -> Review {
        Review { id: ReviewId(id.into()), author: "reviewer".into(), verdict,
                 body: body.into(), submitted_at: datetime!(2026-06-11 00:00 UTC) }
    }

    #[test]
    fn unchanged_snapshots_produce_no_events() {
        let a = snap(vec![issue(1, &["conduit:run"])], vec![pr(7)]);
        assert!(diff(&a, &a.clone()).is_empty());
    }

    #[test]
    fn added_label_fires_once_removed_fires_nothing() {
        let prev = snap(vec![issue(1, &["adr:ADR-0003"])], vec![]);
        let next = snap(vec![issue(1, &["adr:ADR-0003", "conduit:run"])], vec![]);
        assert_eq!(diff(&prev, &next), vec![ForgeEvent::IssueLabeled {
            issue: IssueId(1), label: "conduit:run".into() }]);
        // removal: nothing
        assert!(diff(&next, &prev).is_empty());
    }

    #[test]
    fn new_issue_fires_all_its_labels() {
        let prev = snap(vec![], vec![]);
        let next = snap(vec![issue(1, &["adr:ADR-0003", "conduit:run"])], vec![]);
        let events = diff(&prev, &next);
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| matches!(e, ForgeEvent::IssueLabeled { .. })));
    }

    #[test]
    fn reviews_dedupe_by_forge_native_id() {
        let mut p_prev = pr(7);
        p_prev.reviews = vec![review("r1", ReviewVerdict::ChangesRequested, "fix x")];
        let mut p_next = p_prev.clone();
        // r1 EDITED (same id, new body) must NOT re-fire; r2 is new.
        p_next.reviews = vec![
            review("r1", ReviewVerdict::ChangesRequested, "fix x (edited)"),
            review("r2", ReviewVerdict::ChangesRequested, "fix y"),
        ];
        let events = diff(&snap(vec![], vec![p_prev]), &snap(vec![], vec![p_next]));
        assert_eq!(events.len(), 1);
        let ForgeEvent::ReviewSubmitted { pr, review } = &events[0] else { panic!() };
        assert_eq!(*pr, PrId(7));
        assert_eq!(review.id, ReviewId("r2".into()));
    }

    #[test]
    fn repeated_changes_requested_rounds_are_distinct_events() {
        // Same reviewer, new round = new forge-native id = new event.
        let mut p1 = pr(7);
        p1.reviews = vec![review("r1", ReviewVerdict::ChangesRequested, "round 1")];
        let mut p2 = p1.clone();
        p2.reviews.push(review("r9", ReviewVerdict::ChangesRequested, "round 2"));
        let events = diff(&snap(vec![], vec![p1]), &snap(vec![], vec![p2]));
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn ci_transition_fires_new_pr_ci_does_not() {
        let mut prev_pr = pr(7);
        prev_pr.ci = CiState::Pending;
        let mut next_pr = pr(7);
        next_pr.ci = CiState::Failure;
        let events = diff(&snap(vec![], vec![prev_pr]), &snap(vec![], vec![next_pr.clone()]));
        assert_eq!(events, vec![ForgeEvent::CiChanged { pr: PrId(7), state: CiState::Failure }]);
        // brand-new PR with CI state: no CiChanged
        assert!(diff(&snap(vec![], vec![]), &snap(vec![], vec![next_pr])).iter()
            .all(|e| !matches!(e, ForgeEvent::CiChanged { .. })));
    }

    #[test]
    fn merged_pr_emits_only_pr_merged_never_pr_closed() {
        let prev_pr = pr(7);
        let mut next_pr = pr(7);
        next_pr.merged = true;
        next_pr.closed = true; // forges mark merged PRs closed
        next_pr.merge_sha = Some("cafe42".into());
        let events = diff(&snap(vec![], vec![prev_pr]), &snap(vec![], vec![next_pr]));
        assert_eq!(events, vec![ForgeEvent::PrMerged { pr: PrId(7), merge_sha: "cafe42".into() }]);
    }

    #[test]
    fn closed_without_merge_emits_pr_closed() {
        let prev_pr = pr(7);
        let mut next_pr = pr(7);
        next_pr.closed = true;
        let events = diff(&snap(vec![], vec![prev_pr]), &snap(vec![], vec![next_pr]));
        assert_eq!(events, vec![ForgeEvent::PrClosed { pr: PrId(7) }]);
    }

    #[test]
    fn already_terminal_prs_do_not_refire() {
        let mut p = pr(7);
        p.merged = true;
        p.closed = true;
        p.merge_sha = Some("cafe42".into());
        assert!(diff(&snap(vec![], vec![p.clone()]), &snap(vec![], vec![p])).is_empty());
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib forge`
Expected: FAIL — `todo!()` in `diff`.

- [ ] **Step 4: Implement `diff` + `UreqTransport` + `rest_call`; run to pass**

`diff`: index `prev` issues/PRs by id, walk `next` applying the semantics above. Deterministic event order: issues in `next` order then PRs in `next` order; per PR: ReviewSubmitted (snapshot order), CiChanged, PrMerged/PrClosed.

Run: `cargo test --lib forge`
Expected: PASS.

- [ ] **Step 5: Run the gate and commit**

Run: `just ci`
Expected: PASS.

```bash
cd /home/brett/repos/como-tech/conduit
git add src/forge/mod.rs src/lib.rs
git commit -m "feat(forge): Forge trait, snapshot types, shared pure diff, HttpTransport seam"
```

**Verify gate:** `cargo test --lib forge` all pass + `just ci` green.

---

### Task 7: src/forge/fake.rs + tests/conformance.rs — FakeForge and the parameterized suite skeleton

Spec §The forge adapter: `tests/conformance.rs` is ONE parameterized suite run against all three implementations — "identically" is a CI assertion. This task builds the FakeForge leg; Tasks 8/9 plug in Gitea and GitHub.

**Files:**
- Create: `/home/brett/repos/como-tech/conduit/src/forge/fake.rs`
- Modify: `/home/brett/repos/como-tech/conduit/src/forge/mod.rs` (add `pub mod fake;` — submodule decl lives in mod.rs since forge is a dir module)
- Test: `/home/brett/repos/como-tech/conduit/tests/conformance.rs`

- [ ] **Step 1: Write FakeForge (public API + behavior contract)**

```rust
//! In-memory Forge: scripted snapshot sequences + action recording
//! (spec §Implementations). Interior mutability via Mutex (trait takes &self).

use std::collections::VecDeque;
use std::sync::Mutex;

use crate::task::{IssueId, PrId};
use super::{Forge, ForgeError, LabelSpec, NewIssue, PrDraft, RepoSnapshot};

/// Every mutation an adapter performed, for assertions.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordedAction {
    EnsureLabels(Vec<LabelSpec>),
    CreateIssue(NewIssue),
    UpsertIssueComment { id: IssueId, marker: String, body: String },
    SetIssueLabels { id: IssueId, labels: Vec<String> },
    CloseIssue(IssueId),
    OpenPr(PrDraft),
    UpsertPrComment { id: PrId, marker: String, body: String },
    SetPrLabels { id: PrId, labels: Vec<String> },
}

#[derive(Default)]
struct FakeState {
    scripted: VecDeque<RepoSnapshot>,   // fetch_snapshot pops; last one repeats
    last: Option<RepoSnapshot>,
    labels: Vec<LabelSpec>,
    issues: Vec<(IssueId, NewIssue, bool /*closed*/)>,
    issue_comments: Vec<(IssueId, String /*marker*/, String /*body*/)>,
    prs: Vec<(PrId, PrDraft, bool /*open*/)>,
    pr_comments: Vec<(PrId, String, String)>,
    actions: Vec<RecordedAction>,
    next_issue: u64,
    next_pr: u64,
}

pub struct FakeForge {
    state: Mutex<FakeState>,
}

impl FakeForge {
    pub fn new() -> FakeForge;                       // next ids start at 1
    pub fn script(&self, snapshots: Vec<RepoSnapshot>); // queue fetch results
    pub fn actions(&self) -> Vec<RecordedAction>;
    /// Count of actions matching a predicate (crash-replay assertions).
    pub fn count<F: Fn(&RecordedAction) -> bool>(&self, f: F) -> usize;
}
```

Behavior:
- `fetch_snapshot`: pop front of `scripted`; when one remains, keep returning it (a stable tail). Empty script + no last → empty snapshot.
- `create_issue`: assigns `IssueId(next_issue)`, stores, records, returns id. `find_issue_by_marker` scans stored issue **bodies AND issue comments** for the marker substring.
- `open_pr`: assigns `PrId(next_pr)`, stores (open=true), records. `find_open_pr_by_head` scans stored open PRs by `head`.
- `upsert_*_comment`: replaces an existing comment with the same marker, else appends — and records the call either way (the recording is of *calls*; convergence is asserted on stored state).
- `set_*_labels`: stores the absolute set (convergent), records.
- `close_issue`: sets closed, records. Unknown ids → `ForgeError::Api { status: 404, .. }`.
- `ensure_labels`: unions by name into `labels`, records.
- `git_remote_url`: returns a configurable local path — `set_remote_url(&self, path: &str)` setter, default `"/dev/null/fake.git"`. Task 12's e2e rig points this at a seeded local bare repo so CommitAndPush works.
- `describe`: `"fake"`.

- [ ] **Step 2: Write the failing conformance suite**

`tests/conformance.rs` — the suite body is forge-agnostic; each leg is a `#[test]` that builds its forge and calls `run_conformance`. Task 7 wires only the FakeForge leg; Tasks 8/9 add legs WITHOUT touching the suite body (that is the point).

```rust
use conduit::forge::fake::FakeForge;
use conduit::forge::{Forge, LabelSpec, NewIssue, PrDraft};

/// The shared behavioral contract every adapter must satisfy identically.
/// `tag` disambiguates test data per leg/run (live forges keep state).
fn run_conformance(forge: &dyn Forge, tag: &str) {
    // 1. ensure_labels is idempotent (twice = same result, no error).
    let labels = vec![LabelSpec {
        name: format!("conformance:{tag}"),
        color: "00aabb".into(),
        description: "conformance suite".into(),
    }];
    forge.ensure_labels(&labels).unwrap();
    forge.ensure_labels(&labels).unwrap();

    // 2. create_issue -> find_issue_by_marker round-trip (the replay probe).
    let marker = format!("<!-- conduit:task:conformance-{tag} -->");
    assert_eq!(forge.find_issue_by_marker(&marker).unwrap(), None,
        "marker must be absent before create");
    let issue = forge.create_issue(&NewIssue {
        title: format!("[conformance {tag}] probe issue"),
        body: format!("conformance body\n\n{marker}"),
        labels: vec![format!("conformance:{tag}")],
    }).unwrap();
    assert_eq!(forge.find_issue_by_marker(&marker).unwrap(), Some(issue),
        "probe must find the created issue by its hidden marker");

    // 3. comment upsert converges (marker pattern: second call edits, not dups).
    forge.upsert_issue_comment(&issue, &marker, "status: first").unwrap();
    forge.upsert_issue_comment(&issue, &marker, "status: second").unwrap();

    // 4. set_issue_labels is an absolute, convergent set.
    forge.set_issue_labels(&issue, &[format!("conformance:{tag}")]).unwrap();
    forge.set_issue_labels(&issue, &[format!("conformance:{tag}")]).unwrap();

    // 5. close_issue.
    forge.close_issue(&issue).unwrap();

    // 6. fetch_snapshot is normalized: only conduit-labeled issues and
    //    conduit/*-branch PRs ever appear.
    let snap = forge.fetch_snapshot().unwrap();
    for i in &snap.issues {
        assert!(i.labels.iter().any(|l| l.starts_with("conduit:") || l.starts_with("adr:")),
            "non-conduit issue leaked into snapshot: {:?}", i.id);
    }
    for p in &snap.prs {
        assert!(p.head_branch.starts_with("conduit/"),
            "non-conduit PR leaked into snapshot: {:?}", p.head_branch);
    }
}

#[test]
fn fake_forge_conforms() {
    let forge = FakeForge::new();
    run_conformance(&forge, "fake");
    // FakeForge-only deep assertions (stored-state convergence):
    use conduit::forge::fake::RecordedAction;
    let upserts = forge.count(|a| matches!(a, RecordedAction::UpsertIssueComment { .. }));
    assert_eq!(upserts, 2, "both upsert calls recorded");
}

// Live legs are added by Task 8 (CONDUIT_E2E_GITEA=1) and Task 9
// (recorded fixtures always-on + CONDUIT_E2E_GITHUB=1 live reads).
```

Note: step 2 of the suite cannot pass for FakeForge until create/find are implemented; step 6 needs `fetch_snapshot` on FakeForge to derive a snapshot from its stored issues/PRs when no script is queued — implement exactly that (stored state → snapshot, applying the same normalization filter: only issues with a `conduit:`/`adr:`/`effort:`-prefixed label... **decision:** the snapshot filter is "any label starting with `conduit:` or `adr:`" for issues, `head_branch.starts_with("conduit/")` for PRs; conformance-created issues carry `conformance:<tag>` labels so they may legitimately be filtered OUT — adjust assertion 6 to only check that nothing non-conforming leaks, which the code above already does).

- [ ] **Step 3: Run to verify failure, implement FakeForge, run to pass**

Run: `cargo test --test conformance`
Expected first: FAIL (todo!s) → implement → PASS.

- [ ] **Step 4: Run the gate and commit**

Run: `just ci`
Expected: PASS.

```bash
cd /home/brett/repos/como-tech/conduit
git add src/forge/fake.rs src/forge/mod.rs tests/conformance.rs
git commit -m "feat(forge): FakeForge with scripted snapshots + parameterized conformance suite"
```

**Verify gate:** `cargo test --test conformance` passes + `just ci` green.

---

### Task 8: src/forge/gitea.rs + demo Gitea — REST v1 adapter, fixtures, live conformance leg

Spec §Implementations (Gitea = the real lifecycle host, full read-write) + §Self-dogfood (two-user bootstrap). The adapter sits on the `HttpTransport` seam: unit tests use a fixture transport; the live leg runs behind `CONDUIT_E2E_GITEA=1`.

**Files:**
- Create: `/home/brett/repos/como-tech/conduit/src/forge/gitea.rs`
- Create: `/home/brett/repos/como-tech/conduit/demo/docker-compose.yml`
- Create: `/home/brett/repos/como-tech/conduit/demo/gitea-init.sh`
- Modify: `/home/brett/repos/como-tech/conduit/src/forge/mod.rs` (add `pub mod gitea;`)
- Modify: `/home/brett/repos/como-tech/conduit/justfile` (add `forge-up`, `forge-down`)
- Modify: `/home/brett/repos/como-tech/conduit/tests/conformance.rs` (add the gitea legs)
- Test: fixture-based unit tests inline in `gitea.rs`; live leg in `tests/conformance.rs`

- [ ] **Step 1: Define the adapter struct + endpoint map**

```rust
pub struct GiteaForge {
    transport: Box<dyn super::HttpTransport>,
    base_url: String, // e.g. "http://localhost:3000"
    owner: String,
    repo: String,
    token: String,
}

impl GiteaForge {
    pub fn new(transport: Box<dyn super::HttpTransport>, base_url: &str,
               owner: &str, repo: &str, token: &str) -> GiteaForge;
    /// Production constructor: UreqTransport + config + token resolution.
    pub fn open(cfg: &crate::config::GiteaConfig, token: String) -> GiteaForge;
}
```

**Gitea REST v1 endpoint map (auth header on every call: `Authorization: token <token>`; base `{base_url}/api/v1`):**

| Operation | Method + path | Request JSON | Response fields used | Errors |
|---|---|---|---|---|
| list labels (for name→id) | `GET /repos/{owner}/{repo}/labels?page=1&limit=50` | — | `[].id`, `[].name` | 401/403→Auth, else Api |
| `ensure_labels` | `POST /repos/{owner}/{repo}/labels` per missing name | `{"name","color","description"}` | `id` | 409 already-exists → treat as ok (re-list) |
| `fetch_snapshot` issues | `GET /repos/{owner}/{repo}/issues?type=issues&state=all&page=N&limit=50` | — | `number`, `labels[].name`, `state` (`"closed"`), `body` | paginate until short page |
| `fetch_snapshot` PRs | `GET /repos/{owner}/{repo}/pulls?state=all&page=N&limit=50` | — | `number`, `head.ref`, `head.sha`, `labels[].name`, `merged`, `merge_commit_sha`*, `state` | * field name to be confirmed live (see note) |
| PR reviews | `GET /repos/{owner}/{repo}/pulls/{number}/reviews` | — | `[].id`, `[].user.login`, `[].state` (`APPROVED`/`REQUEST_CHANGES`/`COMMENT`), `[].body`, `[].submitted_at` | dismissed/`PENDING` rows skipped |
| PR CI state | `GET /repos/{owner}/{repo}/commits/{head_sha}/status` | — | `state`: `pending`/`success`/`failure`/`error`(→Failure), `""`(→None) | 404 → CiState::None |
| `create_issue` | `POST /repos/{owner}/{repo}/issues` | `{"title","body","labels":[<label IDs, i64>]}` — **Gitea takes label IDs, not names**: resolve via list-labels first | `number` | |
| `set_issue_labels` | `PUT /repos/{owner}/{repo}/issues/{number}/labels` | `{"labels":[<ids>]}` (replaces — convergent) | — | |
| `close_issue` | `PATCH /repos/{owner}/{repo}/issues/{number}` | `{"state":"closed"}` | — | |
| comments (list/create/edit) | `GET`/`POST /repos/{owner}/{repo}/issues/{number}/comments`; `PATCH /repos/{owner}/{repo}/issues/comments/{id}` | `{"body"}` | `[].id`, `[].body` | PR comments use the SAME issue-comment endpoints (PR number works) |
| `open_pr` | `POST /repos/{owner}/{repo}/pulls` | `{"title","body","head","base"}` | `number` | then `PUT .../issues/{number}/labels` for the draft's labels |
| `find_open_pr_by_head` | `GET /repos/{owner}/{repo}/pulls?state=open&page=N&limit=50` | — | filter client-side: `head.ref == branch` | |
| `find_issue_by_marker` | reuse the issues listing | — | scan `body` for the marker substring; also scan each candidate's comments | |
| `git_remote_url` | (no API call) | — | `http://conduit-bot:{token}@{host}/{owner}/{repo}.git` (strip scheme from base_url for host) | |

Snapshot normalization (adapter-side filter, per spec): issues kept only if any label starts with `conduit:` or `adr:`; PRs kept only if `head.ref` starts with `conduit/`. Review state mapping: `APPROVED`→Approved, `REQUEST_CHANGES`→ChangesRequested, `COMMENT`→Commented, anything else skipped. Comment upsert: list comments, find one whose body contains the marker → PATCH it; none → POST with the marker embedded (`{marker}\n\n{body}`).

> **Live-verification note:** the merged-PR sha field name on Gitea's PR object (`merge_commit_sha` vs `merged_commit_id`) and the exact review-state strings must be confirmed against the running container (`curl -s -H "Authorization: token $TOK" http://localhost:3000/api/v1/repos/como/conduit-dogfood/pulls?state=all | python3 -m json.tool`) during this task. The fixture files must mirror what the live forge actually returns — write the fixtures FROM live responses, then the unit tests and the live leg cannot diverge. This is the spec's named keystone risk (§Risks: snapshot fidelity).

- [ ] **Step 2: Write demo/docker-compose.yml + demo/gitea-init.sh + just recipes**

`demo/docker-compose.yml`:

```yaml
# Throwaway Gitea for the conduit spike (spec §Self-dogfood). Disposable:
# `just forge-down` removes the container AND the volume. Nothing here ever
# leaves localhost.
services:
  gitea:
    image: gitea/gitea:1.24
    container_name: conduit-gitea
    environment:
      - GITEA__security__INSTALL_LOCK=true
      - GITEA__server__ROOT_URL=http://localhost:3000/
      - GITEA__server__HTTP_PORT=3000
      - GITEA__service__DISABLE_REGISTRATION=false
      - GITEA__webhook__ALLOWED_HOST_LIST=*
    ports:
      - "3000:3000"
    volumes:
      - gitea-data:/data
volumes:
  gitea-data:
```

`demo/gitea-init.sh` (bash, `set -euo pipefail`; idempotent — safe to re-run):
1. Wait for `http://localhost:3000/api/healthz` (curl retry loop, 60s budget).
2. Create the two users via the container CLI (Gitea restricts self-approval, hence two users — spec §Self-dogfood): `docker compose -f demo/docker-compose.yml exec -u git gitea gitea admin user create --username conduit-bot --password <random> --email conduit-bot@localhost --admin --must-change-password=false` and the same for `reviewer` (not admin). Ignore "already exists" errors.
3. Mint tokens: `docker compose ... exec -u git gitea gitea admin user generate-access-token --username conduit-bot --token-name conduit --scopes all --raw` → write to `.secrets/conduit-bot.token` (mkdir -p .secrets; chmod 600). Same for `reviewer` → `.secrets/reviewer.token`. If token-name exists, delete + re-mint or keep existing file.
4. As conduit-bot (API, `Authorization: token ...`): `POST /api/v1/orgs {"username":"como"}` (ignore 422 exists); `POST /api/v1/orgs/como/repos {"name":"conduit-dogfood","private":false,"default_branch":"main"}` (ignore exists).
5. Seed the repo by pushing THIS repo to localhost (allowed — throwaway container): `git push http://conduit-bot:$(cat .secrets/conduit-bot.token)@localhost:3000/como/conduit-dogfood.git main:main` (force is acceptable here: `-f`; this is the one sanctioned push target).
6. Pre-create labels via API `POST /api/v1/repos/como/conduit-dogfood/labels`: the five `effort:*` labels (`EFFORT_LABELS`), `conduit:run` (color `1d76db`), `conduit:failed` (color `d73a4a`). Ignore "already exists".
7. Add `reviewer` as a collaborator: `PUT /api/v1/repos/como/conduit-dogfood/collaborators/reviewer {"permission":"write"}`.

justfile additions:

```just
# Throwaway Gitea on localhost:3000 — two users, labels, seeded repo
forge-up:
    docker compose -f demo/docker-compose.yml up -d
    bash demo/gitea-init.sh

# Destroy the throwaway forge (container + volume — nothing survives)
forge-down:
    docker compose -f demo/docker-compose.yml down -v
```

- [ ] **Step 3: Write the failing fixture-based unit tests**

Inline `#[cfg(test)]` in `gitea.rs` with a `FixtureTransport` (a `Vec<((method, url_suffix), HttpResponse)>` matcher that panics on an unexpected request — every test names its exact wire traffic):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::{Forge, HttpResponse, HttpTransport, ForgeError};
    use std::sync::Mutex;

    struct FixtureTransport {
        // (method, url substring) -> response; consumed in order per match
        routes: Mutex<Vec<(String, String, u16, &'static str)>>,
    }
    impl HttpTransport for FixtureTransport {
        fn request(&self, method: &str, url: &str, _h: &[(&str, &str)], _b: Option<&[u8]>)
            -> Result<HttpResponse, ForgeError> {
            let mut routes = self.routes.lock().unwrap();
            let pos = routes.iter().position(|(m, frag, _, _)| m == method && url.contains(frag.as_str()))
                .unwrap_or_else(|| panic!("unexpected request: {method} {url}"));
            let (_, _, status, body) = routes.remove(pos);
            Ok(HttpResponse { status, body: body.as_bytes().to_vec() })
        }
    }

    fn forge_with(routes: Vec<(&str, &str, u16, &'static str)>) -> GiteaForge {
        GiteaForge::new(
            Box::new(FixtureTransport { routes: Mutex::new(
                routes.into_iter().map(|(m, u, s, b)| (m.into(), u.into(), s, b)).collect()) }),
            "http://localhost:3000", "como", "conduit-dogfood", "tok",
        )
    }

    #[test]
    fn create_issue_resolves_label_names_to_ids() {
        let f = forge_with(vec![
            ("GET", "/labels", 200, r#"[{"id": 11, "name": "adr:ADR-0003"}]"#),
            ("POST", "/issues", 201, r#"{"number": 5}"#),
        ]);
        let id = f.create_issue(&crate::forge::NewIssue {
            title: "t".into(), body: "b".into(), labels: vec!["adr:ADR-0003".into()],
        }).unwrap();
        assert_eq!(id, crate::task::IssueId(5));
    }

    #[test]
    fn snapshot_filters_to_conduit_issues_and_branches() {
        let f = forge_with(vec![
            ("GET", "/issues", 200, r#"[
                {"number": 1, "state": "open", "body": "x",
                 "labels": [{"name": "adr:ADR-0003"}]},
                {"number": 2, "state": "open", "body": "y",
                 "labels": [{"name": "bug"}]}
            ]"#),
            ("GET", "/pulls?state=all", 200, r#"[
                {"number": 7, "state": "open", "merged": false,
                 "head": {"ref": "conduit/adr-0003/x", "sha": "abc"},
                 "labels": []},
                {"number": 8, "state": "open", "merged": false,
                 "head": {"ref": "feature/other", "sha": "def"},
                 "labels": []}
            ]"#),
            ("GET", "/pulls/7/reviews", 200, r#"[
                {"id": 31, "user": {"login": "reviewer"}, "state": "REQUEST_CHANGES",
                 "body": "fix x", "submitted_at": "2026-06-11T10:00:00Z"}
            ]"#),
            ("GET", "/commits/abc/status", 200, r#"{"state": "success"}"#),
        ]);
        let snap = f.fetch_snapshot().unwrap();
        assert_eq!(snap.issues.len(), 1, "non-conduit issue filtered");
        assert_eq!(snap.prs.len(), 1, "non-conduit/* PR filtered");
        assert_eq!(snap.prs[0].reviews.len(), 1);
        assert_eq!(snap.prs[0].reviews[0].verdict, crate::task::ReviewVerdict::ChangesRequested);
        assert_eq!(snap.prs[0].ci, crate::forge::CiState::Success);
    }

    #[test]
    fn comment_upsert_edits_existing_marker_comment() {
        let marker = "<!-- conduit:task:adr-0003 -->";
        let f = forge_with(vec![
            ("GET", "/issues/5/comments", 200,
             r#"[{"id": 42, "body": "<!-- conduit:task:adr-0003 -->\n\nold"}]"#),
            ("PATCH", "/issues/comments/42", 200, r#"{}"#),
        ]);
        f.upsert_issue_comment(&crate::task::IssueId(5), marker, "new").unwrap();
        // FixtureTransport panics on a POST — reaching here proves PATCH path.
    }

    #[test]
    fn auth_errors_map_to_forge_auth() {
        let f = forge_with(vec![("GET", "/labels", 401, r#"{"message": "bad token"}"#)]);
        let err = f.ensure_labels(&[crate::forge::LabelSpec {
            name: "conduit:run".into(), color: "1d76db".into(), description: "trigger".into(),
        }]).unwrap_err();
        assert!(matches!(err, ForgeError::Auth(_)), "401 must map to Auth, got {err:?}");
    }
}
```

(Clean up the last test as noted: non-empty input, `assert!(matches!(err, ForgeError::Auth(_)))`.)

- [ ] **Step 4: Run to verify failure, implement the adapter, run to pass**

Run: `cargo test --lib gitea`
Expected: FAIL → implement via `rest_call` against the endpoint map → PASS.

- [ ] **Step 5: Add the live conformance leg**

Append to `tests/conformance.rs`:

```rust
/// Live Gitea leg — needs `just forge-up` first. Tag is time-randomized so
/// re-runs don't collide on the persistent container state.
#[test]
fn gitea_live_conforms() {
    if std::env::var("CONDUIT_E2E_GITEA").as_deref() != Ok("1") {
        eprintln!("skip: set CONDUIT_E2E_GITEA=1 (and run `just forge-up`)");
        return;
    }
    let token = std::fs::read_to_string(".secrets/conduit-bot.token")
        .expect("run `just forge-up` first").trim().to_string();
    let forge = conduit::forge::gitea::GiteaForge::open(
        &conduit::config::GiteaConfig::default(), token);
    let tag = format!("{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
    run_conformance(&forge, &tag);
}
```

- [ ] **Step 6: Run the live leg and reconcile fixtures**

Run: `just forge-up && CONDUIT_E2E_GITEA=1 cargo test --test conformance gitea_live -- --nocapture`
Expected: PASS. If any wire shape differs from the fixtures (the merged-sha field, review states), fix the ADAPTER + FIXTURES to match the live forge, re-run both legs.

- [ ] **Step 7: Run the gate and commit**

Run: `just ci` (fixture tests only — no network) — Expected: PASS.

```bash
cd /home/brett/repos/como-tech/conduit
git add src/forge/gitea.rs src/forge/mod.rs demo/docker-compose.yml demo/gitea-init.sh justfile tests/conformance.rs
git commit -m "feat(forge): Gitea REST v1 adapter + throwaway two-user demo forge"
```

**Verify gate:** `cargo test --lib gitea` + `cargo test --test conformance` green; `CONDUIT_E2E_GITEA=1 cargo test --test conformance gitea_live` shown passing against the container; `just ci` green.

---

### Task 9: src/forge/github.rs + src/forge/dry_run.rs — GitHub reads live, mutations always DryRun

Spec §Implementations + §Transcript-diff semantics + hard constraint: **no mutation of github.com, ever**. The constructor only returns a DryRun-wrapped instance — the unwrapped `GitHubForge` is not reachable from outside the module.

**Files:**
- Create: `/home/brett/repos/como-tech/conduit/src/forge/github.rs`
- Create: `/home/brett/repos/como-tech/conduit/src/forge/dry_run.rs`
- Create: `/home/brett/repos/como-tech/conduit/tests/fixtures/github/` (recorded JSON responses)
- Modify: `/home/brett/repos/como-tech/conduit/src/forge/mod.rs` (add `pub mod github; pub mod dry_run;`)
- Modify: `/home/brett/repos/como-tech/conduit/tests/conformance.rs` (add the two GitHub legs)
- Test: inline unit tests in `dry_run.rs` (normalization) + the conformance legs

- [ ] **Step 1: Write src/forge/dry_run.rs tests first (normalization + redaction are the demo's money shot)**

Public API:

```rust
//! DryRunForge: reads delegate to the inner forge; mutations are serialized to
//! a transcript (JSONL) in normalized form and NEVER executed
//! (spec §Transcript-diff semantics). Synthesized ids keep callers working.

use std::sync::Mutex;

use crate::task::{IssueId, PrId};
use super::{Forge, ForgeError, LabelSpec, NewIssue, PrDraft, RepoSnapshot};

pub struct DryRunForge<F: Forge> {
    inner: F,
    state: Mutex<DryRunState>, // transcript lines + id placeholder maps + counters
}

impl<F: Forge> DryRunForge<F> {
    pub fn new(inner: F) -> DryRunForge<F>;
    /// The normalized transcript so far, one JSON object per line.
    pub fn transcript(&self) -> Vec<String>;
}
```

**Normalization rules (spec §Transcript-diff semantics, exact):**
- Forge-assigned ids → `$ISSUE_1`/`$PR_1`… placeholders in first-seen order (synthesized ids from mutations AND ids passed back in by the caller map through the same table; an id never seen before through a mutation gets the next placeholder).
- Timestamps and durations: omitted entirely.
- Effort label **values** redacted: any label matching `effort:*` → `effort:$REDACTED` (transcript-only; real PRs always carry the real label).
- Repo slug → `$REPO` (replace `{owner}/{repo}` occurrences in bodies/urls).
- Line shape: `{"action": "<kind>", ...normalized fields...}` with stable key order (serde_json with sorted maps — use `BTreeMap` or build `serde_json::Value` objects with keys inserted alphabetically; `serde_json::Value::Object` preserves insertion order, so insert alphabetically).

Mutation → transcript line mapping (reads `describe`/`git_remote_url`/`fetch_snapshot`/probes delegate to `inner` untouched):

| Call | Line |
|---|---|
| `ensure_labels` | `{"action":"ensure_labels","labels":[{"name":..., "color":..., "description":...} (effort names redacted)]}` |
| `create_issue` | `{"action":"create_issue","title":...,"body":...,"labels":[...]}` → returns synthesized `IssueId`, registered as `$ISSUE_n` |
| `upsert_issue_comment` | `{"action":"upsert_issue_comment","issue":"$ISSUE_1","marker":...,"body":...}` |
| `set_issue_labels` | `{"action":"set_issue_labels","issue":"$ISSUE_1","labels":[...]}` |
| `close_issue` | `{"action":"close_issue","issue":"$ISSUE_1"}` |
| `open_pr` | `{"action":"open_pr","title":...,"body":...,"head":...,"base":...,"labels":[...]}` → synthesized `PrId` = `$PR_n` |
| `upsert_pr_comment` | `{"action":"upsert_pr_comment","pr":"$PR_1","marker":...,"body":...}` |
| `set_pr_labels` | `{"action":"set_pr_labels","pr":"$PR_1","labels":["effort:$REDACTED","adr:ADR-0003"]}` |

Tests (complete):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::fake::FakeForge;
    use crate::forge::{Forge, NewIssue, PrDraft};

    fn dry() -> DryRunForge<FakeForge> {
        DryRunForge::new(FakeForge::new())
    }

    #[test]
    fn mutations_never_reach_the_inner_forge() {
        let d = dry();
        d.create_issue(&NewIssue { title: "t".into(), body: "b".into(), labels: vec![] }).unwrap();
        d.close_issue(&crate::task::IssueId(1)).unwrap();
        assert!(d.inner_ref_for_tests().actions().is_empty(),
            "DryRun must record, not execute");
        // expose a #[cfg(test)] pub(crate) fn inner_ref_for_tests(&self) -> &F
    }

    #[test]
    fn ids_become_placeholders_in_first_seen_order() {
        let d = dry();
        let i1 = d.create_issue(&NewIssue { title: "a".into(), body: "".into(), labels: vec![] }).unwrap();
        let p1 = d.open_pr(&PrDraft { title: "p".into(), body: "".into(),
            head: "conduit/adr-0003/x".into(), base: "main".into(), labels: vec![] }).unwrap();
        d.close_issue(&i1).unwrap();
        d.set_pr_labels(&p1, &["adr:ADR-0003".into()]).unwrap();
        let t = d.transcript();
        assert!(t[2].contains("\"$ISSUE_1\""));
        assert!(t[3].contains("\"$PR_1\""));
    }

    #[test]
    fn effort_label_value_is_redacted() {
        let d = dry();
        let p = d.open_pr(&PrDraft { title: "p".into(), body: "".into(),
            head: "conduit/adr-0003/x".into(), base: "main".into(), labels: vec![] }).unwrap();
        d.set_pr_labels(&p, &["effort:3-average".into(), "adr:ADR-0003".into()]).unwrap();
        let line = d.transcript().pop().unwrap();
        assert!(line.contains("effort:$REDACTED"));
        assert!(!line.contains("3-average"));
        assert!(line.contains("adr:ADR-0003"), "non-effort labels stay verbatim");
    }

    #[test]
    fn transcript_lines_are_valid_json_with_action_key() {
        let d = dry();
        d.create_issue(&NewIssue { title: "t".into(), body: "x".into(), labels: vec![] }).unwrap();
        for line in d.transcript() {
            let v: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert!(v.get("action").is_some());
        }
    }

    #[test]
    fn no_timestamps_in_transcript() {
        let d = dry();
        d.create_issue(&NewIssue { title: "t".into(), body: "x".into(), labels: vec![] }).unwrap();
        for line in d.transcript() {
            assert!(!line.contains("_at\""), "timestamps must be omitted: {line}");
        }
    }
}
```

- [ ] **Step 2: Run (FAIL), implement DryRunForge, run (PASS)**

Run: `cargo test --lib dry_run`

- [ ] **Step 3: Write src/forge/github.rs (reads live-capable; constructor returns DryRun only)**

```rust
pub struct GitHubForge {
    transport: Box<dyn super::HttpTransport>,
    owner: String,
    repo: String,
    token: String, // env GITHUB_TOKEN / `gh auth token` — reads only
}

/// The ONLY public way to construct a GitHub forge in the spike:
/// always DryRun-wrapped (spec hard constraint).
pub fn open_github(cfg: &crate::config::GithubConfig, token: String)
    -> super::dry_run::DryRunForge<GitHubForge>;

/// Token resolution helper: env GITHUB_TOKEN, else `gh auth token` subprocess
/// output (trimmed), else None.
pub fn resolve_token() -> Option<String>;

#[cfg(test)]
pub(crate) fn raw_for_tests(transport: Box<dyn super::HttpTransport>, owner: &str, repo: &str)
    -> GitHubForge; // fixtures need the unwrapped reads
```

`GitHubForge` itself is `pub` (the wrapper type names it) but its fields and inherent constructor are private; `impl Forge for GitHubForge` implements mutations as real REST calls (they exist so DryRun *could* delegate one day and so the payload builders are unit-testable) — but in the spike nothing can reach them: `open_github` wraps, and the conformance/e2e code only ever uses `open_github`.

**GitHub REST v3 endpoint map (headers on every call: `Authorization: Bearer <token>`, `Accept: application/vnd.github+json`, `User-Agent: conduit-spike`; base `https://api.github.com`):**

| Operation | Method + path | Request JSON | Response fields used | Errors |
|---|---|---|---|---|
| `fetch_snapshot` issues | `GET /repos/{owner}/{repo}/issues?state=all&per_page=100&page=N` | — | `number`, `labels[].name`, `state` (`"closed"`), `body`; **skip rows with a `pull_request` key** (GitHub lists PRs as issues) | 401/403→Auth, else Api |
| `fetch_snapshot` PRs | `GET /repos/{owner}/{repo}/pulls?state=all&per_page=100&page=N` | — | `number`, `head.ref`, `head.sha`, `labels[].name`, `merged_at` (null/=merged), `merge_commit_sha`, `state` | |
| PR reviews | `GET /repos/{owner}/{repo}/pulls/{number}/reviews` | — | `[].id` (int → ReviewId string), `[].user.login`, `[].state` (`APPROVED`/`CHANGES_REQUESTED`/`COMMENTED`; skip `PENDING`/`DISMISSED`), `[].body`, `[].submitted_at` | |
| PR CI state | `GET /repos/{owner}/{repo}/commits/{head_sha}/status` | — | combined `state`: `pending`/`success`/`failure`; zero `total_count` → CiState::None | |
| `find_open_pr_by_head` | `GET /repos/{owner}/{repo}/pulls?state=open&head={owner}:{branch}` | — | `[0].number` | |
| `find_issue_by_marker` | reuse issues listing, scan `body` client-side (identical semantics to Gitea — deliberately NOT the search API) | — | `number` | |
| `ensure_labels` | `GET /repos/{owner}/{repo}/labels?per_page=100` + `POST /repos/{owner}/{repo}/labels` `{"name","color","description"}` | | | 422 exists → ok |
| `create_issue` | `POST /repos/{owner}/{repo}/issues` | `{"title","body","labels":[<names>]}` (names, not ids — GitHub difference) | `number` | |
| `set_issue_labels` | `PUT /repos/{owner}/{repo}/issues/{number}/labels` | `{"labels":[<names>]}` (replaces) | — | |
| `close_issue` | `PATCH /repos/{owner}/{repo}/issues/{number}` | `{"state":"closed"}` | — | |
| comments | `GET`/`POST /repos/{owner}/{repo}/issues/{number}/comments`; `PATCH /repos/{owner}/{repo}/issues/comments/{id}` | `{"body"}` | `[].id`, `[].body` | PR comments via the same issue endpoints |
| `open_pr` | `POST /repos/{owner}/{repo}/pulls` | `{"title","body","head","base"}` | `number` | then PUT labels |
| `git_remote_url` | (no API call) | — | `https://github.com/{owner}/{repo}.git` — **but src/git.rs must refuse to push to any non-localhost URL (Task 11 guard)** | |

- [ ] **Step 4: Record fixtures + add the two conformance legs**

Record once from a real public repo read (or hand-write from the documented shapes — but recorded is the spec's word): with `GITHUB_TOKEN` set, run a small recorder test (`#[ignore]`, run manually: `cargo test --lib github::record_fixtures -- --ignored`) that performs the snapshot reads against a small public repo you can see (e.g. an existing como-tech public repo; READS ONLY) and writes raw response bodies to `tests/fixtures/github/{issues,pulls,reviews_<n>,status_<sha>}.json`. Redact nothing (public data), but verify no token appears in the files.

Conformance legs appended to `tests/conformance.rs`:

```rust
/// Recorded-fixture GitHub leg — ALWAYS ON, no network. Reads come from
/// tests/fixtures/github/, mutations go to the DryRun transcript; the suite's
/// read-side assertions run; mutation assertions check the transcript shape.
#[test]
fn github_recorded_fixtures_conform() {
    let forge = conduit::forge::github::fixture_forge("tests/fixtures/github");
    // fixture_forge: pub fn building DryRunForge<GitHubForge> over a
    // FixtureTransport that serves the recorded files by URL pattern.
    run_conformance(&forge, "gh-fixture");
}

/// Live GitHub READS leg (CONDUIT_E2E_GITHUB=1): fetch_snapshot + probes
/// against the real API; mutations still hit only the DryRun transcript.
#[test]
fn github_live_reads_conform() {
    if std::env::var("CONDUIT_E2E_GITHUB").as_deref() != Ok("1") { 
        eprintln!("skip: set CONDUIT_E2E_GITHUB=1 with GITHUB_TOKEN");
        return;
    }
    let token = conduit::forge::github::resolve_token().expect("GITHUB_TOKEN or gh login");
    let cfg = conduit::config::GithubConfig { /* the recorded repo's owner/repo */ };
    let forge = conduit::forge::github::open_github(&cfg, token);
    let snap = forge.fetch_snapshot().unwrap();
    // Read-side normalization assertions only (no lifecycle on a real repo):
    for p in &snap.prs { assert!(p.head_branch.starts_with("conduit/")); }
    let _ = forge.find_open_pr_by_head("conduit/never-exists").unwrap();
    let _ = forge.find_issue_by_marker("<!-- conduit:task:never -->").unwrap();
}
```

**Adjustment to `run_conformance` for DryRun legs:** assertions 2 and 3 (probe sees the created issue; upsert converges) cannot observe DryRun mutations through live reads. Parameterize the suite: `run_conformance(forge, tag, Mutations::Real | Mutations::DryRun)` — with `DryRun`, steps 1-5 still CALL every mutation (asserting `Ok`) but skip the read-back assertions, and step 6 (snapshot normalization) always runs. FakeForge/Gitea legs use `Mutations::Real`. Keep ONE suite body; the enum is the documented honest-claim boundary (spec §Risks: dry-run proves the stream, not GitHub's acceptance).

- [ ] **Step 5: Run everything, gate, commit**

Run: `cargo test --lib github && cargo test --lib dry_run && cargo test --test conformance`
Expected: PASS (fixture leg green with no network).
Run (live reads, requires token): `CONDUIT_E2E_GITHUB=1 cargo test --test conformance github_live -- --nocapture`
Expected: PASS.
Run: `just ci` — Expected: PASS.

```bash
cd /home/brett/repos/como-tech/conduit
git add src/forge/github.rs src/forge/dry_run.rs src/forge/mod.rs tests/fixtures/github tests/conformance.rs
git commit -m "feat(forge): GitHub REST v3 reads + DryRun transcript decorator with recorded-fixture conformance"
```

**Verify gate:** `cargo test --test conformance` (fake + gitea-fixture + github-fixture legs, no network) + `just ci` green; live-reads leg shown passing once.

---

### Task 10: src/adroit.rs — pinned adroit, handshake, allowlist, plan snapshots

Spec §adroit integration (read-only, allowlisted). The lane boundary is enforced in code, not convention.

**Files:**
- Create: `/home/brett/repos/como-tech/conduit/adroit.rev`
- Create: `/home/brett/repos/como-tech/conduit/src/adroit.rs`
- Create: `/home/brett/repos/como-tech/conduit/tests/fixtures/fake-adroit` (executable stub script)
- Modify: `/home/brett/repos/como-tech/conduit/justfile` (add `init-adroit`)
- Modify: `/home/brett/repos/como-tech/conduit/src/lib.rs` (add `pub mod adroit;`)
- Test: inline unit tests in `src/adroit.rs` (full contract tests come in Task 13's `tests/adroit_contract.rs`)

- [ ] **Step 1: Write adroit.rev and the justfile recipe**

`adroit.rev` (one line + newline — the SINGLE pin location):

```
f59a5f28e5542566bc1a1318296692bcc22fffe5
```

justfile addition:

```just
# Build the pinned adroit into .conduit/bin (no network: file:// + --locked)
init-adroit:
    cargo install --git file:///home/brett/repos/como-tech/adroit --rev $(cat adroit.rev) --locked --root .conduit adroit
    .conduit/bin/adroit manifest -o json > /dev/null && echo "adroit handshake OK"
```

- [ ] **Step 2: Write the API + failing unit tests**

```rust
//! AdrSource: the ONLY adroit call site in the crate (spec §adroit integration).
//! Hardcoded subcommand allowlist {manifest, list, show, plan}; a test asserts
//! no other adroit invocation exists in the crate.

use std::path::PathBuf;

pub const ALLOWED_SUBCOMMANDS: [&str; 4] = ["manifest", "list", "show", "plan"];

#[derive(Debug, thiserror::Error)]
pub enum AdroitError {
    #[error("adroit not found at {0} — run `just init-adroit`")]
    Missing(PathBuf),
    #[error("adroit handshake failed: {0}")]
    Handshake(String),
    #[error("adroit subcommand {0:?} is not allowlisted (conduit lane violation)")]
    Disallowed(String),
    #[error("adroit {subcommand} failed (exit {code:?}): {stderr}")]
    Subprocess { subcommand: String, code: Option<i32>, stderr: String },
    #[error("unparseable adroit {subcommand} output: {source}")]
    BadJson { subcommand: String, #[source] source: serde_json::Error },
    #[error("ADR {address} is {status}, not Accepted — conduit only drives accepted ADRs")]
    NotAccepted { address: String, status: String },
}

/// Tolerant serde: require the contracted fields, deny nothing — additive
/// drift on adroit main must not break the pinned client (spec §Enumerate).
/// Field names verified against adroit f59a5f28 view types.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AdrSummary {
    pub reference: String,        // "ADR-0003" — display
    pub address: String,          // "3" — addressing token
    pub title: String,
    pub status: String,           // "Accepted" etc. — tolerant string, not enum
    #[serde(default)]
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AdrDetail {
    pub reference: String,
    pub address: String,
    pub title: String,
    pub status: String,
    pub body: String,             // raw markdown (show -o json flattens summary + body)
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PlanEnvelope {
    pub reference: String,
    pub title: String,
    pub plan: String,             // markdown — persisted VERBATIM
}

pub struct AdrSource {
    bin: PathBuf,               // .conduit/bin/adroit (or injected stub in tests)
    dir: PathBuf,               // the ADR corpus (config adroit.dir)
    ai_env: Vec<(String, String)>, // ADROIT_AI_PROVIDER/MODEL (+ key if configured)
}

impl AdrSource {
    pub fn new(bin: PathBuf, dir: PathBuf, cfg: &crate::config::AdroitConfig) -> AdrSource;

    /// `adroit manifest -o json`; require tool=="adroit" && manifest_schema==1,
    /// else bail loudly (spec §Handshake).
    pub fn handshake(&self) -> Result<(), AdroitError>;

    /// `ADROIT_DIR=<dir> adroit list --status accepted -o json`, skipping rows
    /// with superseded_by != null (spec §Enumerate).
    pub fn list_accepted(&self) -> Result<Vec<AdrSummary>, AdroitError>;

    /// `adroit show <address> -o json`.
    pub fn show(&self, address: &str) -> Result<AdrDetail, AdroitError>;

    /// Conduit's OWN guard — adroit does not enforce this (spec §Guard).
    pub fn require_accepted(detail: &AdrDetail) -> Result<(), AdroitError>;

    /// `adroit plan <address> -o json` with ADROIT_AI_* env supplied by conduit.
    pub fn plan(&self, address: &str) -> Result<PlanEnvelope, AdroitError>;
}

/// Every subprocess goes through this chokepoint: rejects non-allowlisted
/// subcommands BEFORE spawning; sets ADROIT_DIR env (the env form of --dir —
/// conduit always uses the env form, spec §Demo script).
fn run_adroit(&self, subcommand: &str, args: &[&str]) -> Result<Vec<u8>, AdroitError>;
```

Stub binary for hermetic tests — `tests/fixtures/fake-adroit` (committed executable shell script):

```sh
#!/bin/sh
# Hermetic adroit stand-in: emits canned JSON per subcommand. The real pinned
# binary is exercised behind CONDUIT_E2E_ADROIT=1 (tests/adroit_contract.rs).
case "$1" in
  manifest) echo '{"tool": "adroit", "manifest_schema": 1, "extra": "tolerated"}' ;;
  list) cat "${FAKE_ADROIT_LIST:-/dev/null}" ;;
  show) cat "${FAKE_ADROIT_SHOW:-/dev/null}" ;;
  plan) cat "${FAKE_ADROIT_PLAN:-/dev/null}" ;;
  *) echo "unknown subcommand $1" >&2; exit 2 ;;
esac
```

(`chmod +x tests/fixtures/fake-adroit` before committing; `git add` preserves the mode bit.)

Unit tests (inline; complete):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AdroitConfig;

    fn stub_source(dir: &std::path::Path) -> AdrSource {
        AdrSource::new(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-adroit"),
            dir.to_path_buf(),
            &AdroitConfig::default(),
        )
    }

    #[test]
    fn handshake_accepts_manifest_schema_1_with_extra_fields() {
        let d = tempfile::TempDir::new().unwrap();
        stub_source(d.path()).handshake().unwrap();
    }

    #[test]
    fn handshake_bails_on_wrong_tool() {
        // Point at a stub that answers wrongly: a one-off script in the tempdir.
        let d = tempfile::TempDir::new().unwrap();
        let bad = d.path().join("bad-adroit");
        std::fs::write(&bad, "#!/bin/sh\necho '{\"tool\":\"other\",\"manifest_schema\":1}'\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o755)).unwrap();
        let src = AdrSource::new(bad, d.path().into(), &AdroitConfig::default());
        assert!(matches!(src.handshake(), Err(AdroitError::Handshake(_))));
    }

    #[test]
    fn list_accepted_skips_superseded_rows_and_tolerates_extra_fields() {
        let d = tempfile::TempDir::new().unwrap();
        let list = d.path().join("list.json");
        std::fs::write(&list, r#"[
          {"reference": "ADR-0001", "address": "1", "title": "a", "status": "Accepted",
           "superseded_by": "ADR-0004", "number": 1, "created": null},
          {"reference": "ADR-0003", "address": "3", "title": "b", "status": "Accepted",
           "superseded_by": null, "unknown_future_field": {"x": 1}}
        ]"#).unwrap();
        let src = stub_source(d.path());
        // SAFETY-free env plumbing: pass fixture paths via the AdrSource test
        // constructor instead of process env — add #[cfg(test)] fn with_env(self, k, v).
        let src = src.with_env("FAKE_ADROIT_LIST", list.to_str().unwrap());
        let rows = src.list_accepted().unwrap();
        assert_eq!(rows.len(), 1, "superseded row skipped");
        assert_eq!(rows[0].address, "3");
    }

    #[test]
    fn require_accepted_rejects_other_statuses() {
        let mk = |status: &str| AdrDetail {
            reference: "ADR-0003".into(), address: "3".into(), title: "t".into(),
            status: status.into(), body: "b".into(),
        };
        assert!(AdrSource::require_accepted(&mk("Accepted")).is_ok());
        for s in ["Proposed", "Rejected", "Superseded", "Deprecated"] {
            assert!(matches!(AdrSource::require_accepted(&mk(s)),
                Err(AdroitError::NotAccepted { .. })), "{s} must be rejected");
        }
    }

    #[test]
    fn run_adroit_rejects_non_allowlisted_subcommands() {
        let d = tempfile::TempDir::new().unwrap();
        let src = stub_source(d.path());
        for bad in ["new", "set-status", "supersede", "edit", "review"] {
            assert!(matches!(src.run_adroit_for_tests(bad, &[]),
                Err(AdroitError::Disallowed(_))), "{bad} must be refused before spawn");
        }
        // expose run_adroit via #[cfg(test)] pub(crate) wrapper run_adroit_for_tests
    }

    /// The lane boundary is enforced in code: outside src/adroit.rs, no file in
    /// the crate may invoke the adroit binary or mention a non-allowlisted
    /// adroit subcommand in an adroit invocation context.
    #[test]
    fn adroit_binary_is_only_invoked_from_this_module() {
        let src_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() { walk(&p, out); }
                else if p.extension().is_some_and(|x| x == "rs") { out.push(p); }
            }
        }
        let mut files = Vec::new();
        walk(&src_dir, &mut files);
        for f in files {
            if f.file_name().is_some_and(|n| n == "adroit.rs") { continue; }
            let content = std::fs::read_to_string(&f).unwrap();
            // The binary path fragment and the AdrSource-bypassing markers:
            if content.contains("bin/adroit") || content.contains("Command::new(\"adroit\"") {
                offenders.push(f);
            }
        }
        assert!(offenders.is_empty(), "adroit invoked outside src/adroit.rs: {offenders:?}");
    }
}
```

- [ ] **Step 3: Run (FAIL), implement, run (PASS)**

Run: `cargo test --lib adroit`
Implementation notes: `run_adroit` = `std::process::Command::new(&self.bin)` + `env("ADROIT_DIR", &self.dir)` + `envs(ai_env)` + `args([subcommand]).args(args).arg("-o").arg("json")`; capture stdout/stderr; non-zero exit → `Subprocess`. `plan()` sets `ADROIT_AI_PROVIDER`/`ADROIT_AI_MODEL` from config (+ `ADROIT_ANTHROPIC_KEY` passthrough if present in conduit's env — optional upgrade path).

- [ ] **Step 4: Run `just init-adroit` once and verify the real handshake**

Run: `just init-adroit`
Expected: cargo builds adroit at the pinned rev into `.conduit/bin/adroit`; the recipe's handshake echo prints `adroit handshake OK`.

- [ ] **Step 5: Gate and commit**

Run: `just ci` — Expected: PASS.

```bash
cd /home/brett/repos/como-tech/conduit
git add adroit.rev src/adroit.rs src/lib.rs justfile tests/fixtures/fake-adroit
git commit -m "feat(adroit): pinned read-only AdrSource — handshake, allowlist, Accepted guard"
```

**Verify gate:** `cargo test --lib adroit` all pass; `just init-adroit` succeeds; `just ci` green.

---

### Task 11: src/engine/ + src/git.rs — the engine seam, fakes, sandboxed Claude Code, git plumbing

Spec §The engine seam. The contract is deliberately dumb: given a prepared workspace and an instruction document, edit files; report success. Conduit owns ALL git and ALL forge interaction; the sandbox is structural (credential-free origin + scrubbed env).

**Files:**
- Create: `/home/brett/repos/como-tech/conduit/src/engine/mod.rs`
- Create: `/home/brett/repos/como-tech/conduit/src/engine/fake.rs`
- Create: `/home/brett/repos/como-tech/conduit/src/engine/claude_code.rs`
- Create: `/home/brett/repos/como-tech/conduit/src/git.rs`
- Modify: `/home/brett/repos/como-tech/conduit/src/lib.rs` (add `pub mod engine; pub mod git;`)
- Test: inline tests in each module (tempfile + local bare repos; a stub `claude` script for timeout)

- [ ] **Step 1: Write src/engine/mod.rs (the trait — spec-verbatim)**

```rust
//! The subprocess engine contract (spec §The engine seam). Engines edit files
//! in a prepared workspace; conduit owns git, forge, and the timeout.

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
    pub adr_reference: String,           // "ADR-0003"
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
```

(Timeout ⇒ the runner returns `Ok(EngineOutcome::Failed { reason: "timeout", .. })` — first-class, not an `EngineError`. The router maps `EngineOutcome` to `task::EngineResult` 1:1.)

- [ ] **Step 2: Write src/engine/fake.rs tests, then the impl**

```rust
/// Deterministic engine (spec §Fakes) — the default demo path.
pub enum FakeMode { Complete, Fail, Hang { secs: u64 } }

pub struct FakeEngine { pub mode: FakeMode }
```

Behavior: `Complete` writes `docs/impl/<ref-lower>.md` into the workspace containing the title + the SHA-256 (hex) of `spec.plan_markdown`, returns `Completed { summary }`; deterministic — same spec, same bytes. `Fail` returns `Failed { reason: "scripted failure", log_tail: "fake engine scripted log tail" }` writing nothing. `Hang` sleeps `secs` then completes (drives the conduit-side timeout test). Mode from env for the CLI path: `CONDUIT_FAKE_ENGINE_MODE=complete|fail|hang`.

Tests (complete):

```rust
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
        let e = FakeEngine { mode: FakeMode::Complete };
        let out = e.run(&spec(ws.path())).unwrap();
        assert!(matches!(out, EngineOutcome::Completed { .. }));
        let doc = std::fs::read_to_string(ws.path().join("docs/impl/adr-0003.md")).unwrap();
        assert!(doc.contains("Adopt snapshot-diff router"));
        use sha2::{Digest, Sha256};
        let plan_sha = format!("{:x}", Sha256::digest("# Plan\n1. do it\n".as_bytes()));
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
        let e = FakeEngine { mode: FakeMode::Fail };
        let EngineOutcome::Failed { reason, log_tail } = e.run(&spec(ws.path())).unwrap() else {
            panic!("expected Failed");
        };
        assert!(!reason.is_empty() && !log_tail.is_empty());
        assert!(std::fs::read_dir(ws.path()).unwrap().next().is_none(), "writes nothing");
    }
}
```

- [ ] **Step 3: Write src/git.rs tests, then the impl (local bare "remote" — never the network)**

```rust
//! Local bare cache + workspace lifecycle (spec §Sandbox — structural).
//! The ONLY module that ever sees an authenticated remote URL. Push is only
//! ever used against localhost (Gitea) or local paths (tests) — enforced here.

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git {args:?} failed (exit {code:?}): {stderr}")]
    Command { args: Vec<String>, code: Option<i32>, stderr: String },
    #[error("refusing to push to non-local remote {0} (spike hard constraint)")]
    NonLocalPush(String),
    #[error("git I/O: {0}")]
    Io(#[from] std::io::Error),
}

/// Clone-or-fetch the bare cache at `.conduit/cache/<forge>.git`.
pub fn ensure_cache(cache: &std::path::Path, remote_url: &str) -> Result<(), GitError>;

/// Clone the cache into `ws` (origin = the cache path: credential-free),
/// create-or-reset `branch` from `base` (fresh workspace) or check out the
/// existing remote branch (revising).
pub fn create_workspace(cache: &std::path::Path, ws: &std::path::Path,
                        base: &str, branch: &str, fresh: bool) -> Result<(), GitError>;

/// Stage everything EXCEPT conduit's artifacts (pathspec
/// `:(exclude).conduit-task.md`), delete the task file first, commit with
/// `message`. Returns false when there is nothing to commit.
pub fn commit_all_except_task_file(ws: &std::path::Path, message: &str) -> Result<bool, GitError>;

/// Push `branch` from `ws` to the authenticated URL. REFUSES any URL that is
/// not localhost/127.0.0.1/a filesystem path — the structural never-push guard.
pub fn push(ws: &std::path::Path, remote_url: &str, branch: &str) -> Result<(), GitError>;

/// `git ls-remote <url> refs/heads/<branch>` -> Some(sha) — the push replay probe.
pub fn ls_remote_sha(remote_url: &str, branch: &str) -> Result<Option<String>, GitError>;

/// The local-push guard, pure and unit-testable.
pub fn is_local_remote(url: &str) -> bool;
```

Tests (complete; helpers shell out to real `git` in tempdirs):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sh(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git").args(args).current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
            .output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// A local bare repo with one commit on main — the stand-in "forge remote".
    fn seeded_remote() -> (TempDir, String) {
        let d = TempDir::new().unwrap();
        let work = d.path().join("seed");
        std::fs::create_dir(&work).unwrap();
        sh(&work, &["init", "-b", "main"]);
        std::fs::write(work.join("README.md"), "seed\n").unwrap();
        sh(&work, &["add", "README.md"]);
        sh(&work, &["commit", "-m", "seed"]);
        let bare = d.path().join("remote.git");
        sh(d.path(), &["clone", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()]);
        let url = bare.to_str().unwrap().to_string();
        (d, url)
    }

    #[test]
    fn is_local_remote_guard() {
        assert!(is_local_remote("/tmp/x.git"));
        assert!(is_local_remote("file:///tmp/x.git"));
        assert!(is_local_remote("http://localhost:3000/como/x.git"));
        assert!(is_local_remote("http://conduit-bot:tok@localhost:3000/como/x.git"));
        assert!(is_local_remote("http://127.0.0.1:3000/x.git"));
        assert!(!is_local_remote("https://github.com/owner/repo.git"));
        assert!(!is_local_remote("git@github.com:owner/repo.git"));
    }

    #[test]
    fn push_refuses_non_local_remotes() {
        let d = TempDir::new().unwrap();
        let err = push(d.path(), "https://github.com/owner/repo.git", "conduit/x/y");
        assert!(matches!(err, Err(GitError::NonLocalPush(_))));
    }

    #[test]
    fn cache_workspace_commit_push_roundtrip() {
        let (_d, url) = seeded_remote();
        let root = TempDir::new().unwrap();
        let cache = root.path().join("cache.git");
        ensure_cache(&cache, &url).unwrap();
        ensure_cache(&cache, &url).unwrap(); // idempotent: second call fetches
        let ws = root.path().join("ws");
        create_workspace(&cache, &ws, "main", "conduit/adr-0003/x", true).unwrap();
        // workspace origin is the CACHE path — no credentials, no real remote
        let origin = std::process::Command::new("git")
            .args(["remote", "get-url", "origin"]).current_dir(&ws).output().unwrap();
        let origin = String::from_utf8_lossy(&origin.stdout);
        assert!(origin.trim().ends_with("cache.git"), "origin must be the local cache: {origin}");
        // engine writes files incl. the task doc; commit excludes the task doc
        std::fs::write(ws.join(".conduit-task.md"), "instructions").unwrap();
        std::fs::create_dir_all(ws.join("docs/impl")).unwrap();
        std::fs::write(ws.join("docs/impl/adr-0003.md"), "impl").unwrap();
        let committed = commit_all_except_task_file(
            &ws, &crate::contract::commit_message("ADR-0003", "x")).unwrap();
        assert!(committed);
        let show = std::process::Command::new("git")
            .args(["show", "--stat", "--format=%s", "HEAD"]).current_dir(&ws).output().unwrap();
        let show = String::from_utf8_lossy(&show.stdout);
        assert!(show.contains("docs/impl/adr-0003.md"));
        assert!(!show.contains(".conduit-task.md"), "task file must never land in a commit");
        assert!(show.contains("[ADR-0003] x"));
        // push to the local bare remote; ls-remote sees the branch (the probe)
        assert!(ls_remote_sha(&url, "conduit/adr-0003/x").unwrap().is_none());
        push(&ws, &url, "conduit/adr-0003/x").unwrap();
        assert!(ls_remote_sha(&url, "conduit/adr-0003/x").unwrap().is_some());
        // replay probe semantics: same sha -> push skippable by caller
    }

    #[test]
    fn nothing_to_commit_returns_false() {
        let (_d, url) = seeded_remote();
        let root = TempDir::new().unwrap();
        let cache = root.path().join("cache.git");
        ensure_cache(&cache, &url).unwrap();
        let ws = root.path().join("ws");
        create_workspace(&cache, &ws, "main", "conduit/adr-0003/x", true).unwrap();
        assert!(!commit_all_except_task_file(&ws, "msg").unwrap());
    }
}
```

Implementation notes: every operation shells out to `git` with explicit args (`Command::new("git")`), author/committer set to `conduit-bot <conduit-bot@localhost>` via env at commit time. `commit_all_except_task_file`: `rm -f .conduit-task.md` (fs), then `git add -A -- ':(exclude).conduit-task.md'` (the pathspec exclusion ALSO guards against historical task files), `git diff --cached --quiet` to detect nothing-staged, then `git commit -m <message>`. `push`: guard `is_local_remote`, then `git push <url> HEAD:refs/heads/<branch>`. `is_local_remote`: true for paths (no `://` and not scp-like `host:`), `file://`, and http(s) URLs whose host (after stripping `user:pass@`) is `localhost` or `127.0.0.1`.

- [ ] **Step 4: Write src/engine/claude_code.rs**

Flags were pre-verified against the installed CLI (`claude --help`, 2026-06-11): `-p`, `--output-format json`, `--permission-mode acceptEdits`, `--disallowedTools` all exist. Re-run `claude --help` once during this task; if anything changed, adjust and note it in the module docs.

```rust
//! Sandboxed `claude -p` runner (spec §The engine seam). The sandbox is
//! structural: the workspace origin is the local cache (no credentials) and
//! the subprocess env is scrubbed of every forge/AI token.

pub struct ClaudeCodeEngine {
    pub binary: PathBuf,        // "claude" from PATH by default
    pub timeout: std::time::Duration, // conduit-enforced hard timeout
}

/// Env vars scrubbed from the engine subprocess (prefix match for *_TOKEN/*_KEY
/// plus the explicit list).
pub const SCRUBBED_ENV: [&str; 6] = [
    "GITHUB_TOKEN", "CONDUIT_GITEA_TOKEN", "GITEA_TOKEN",
    "ANTHROPIC_API_KEY", "ADROIT_ANTHROPIC_KEY", "OPENAI_API_KEY",
];

/// Pure: build the instruction document written to `<ws>/.conduit-task.md`.
pub fn task_document(spec: &TaskSpec) -> String;
// Contents: H1 with pr_title(reference, title); "## ADR" + adr_body;
// "## Plan" + plan_markdown (verbatim); if review_feedback:
// "## Review feedback (address ALL of it)" + the feedback;
// closing rules: edit files in this directory only; do not run git push or
// git remote; do not touch .conduit-task.md.

/// Pure: the argv after the binary — unit-testable without spawning.
pub fn build_args(prompt: &str) -> Vec<String>;
// ["-p", prompt, "--output-format", "json", "--permission-mode", "acceptEdits",
//  "--disallowedTools", "Bash(git push:*),Bash(git remote:*),WebFetch,WebSearch"]
```

`run()`: write `task_document` to `.conduit-task.md`; spawn with `cwd = spec.workspace`, `env_clear()` then re-add a minimal safe set (`PATH`, `HOME`, `TERM`, `LANG`) — strictly allowlist, do NOT blocklist (stronger than `SCRUBBED_ENV`, which becomes the *test* assertion list); poll `child.try_wait()` every 500ms against the deadline; on timeout `child.kill()` → `Ok(Failed { reason: "timeout", log_tail: <last 50 lines of captured stdout/stderr> })`; on exit parse stdout as the claude JSON result envelope (`{"result": "...", ...}` — take `result` as the summary; tolerate unknown fields; unparseable → `Failed` with the tail, not an error). Wall-clock measured around the spawn (`Instant::now()`); the runner RETURNS the elapsed ms to the router via a wrapper: `pub fn run_timed(engine: &dyn Engine, spec: &TaskSpec) -> (Result<EngineOutcome, EngineError>, u64)` in `engine/mod.rs` (feeds `work_ms` → the effort bucket).

Tests (stub binary, no real claude in CI):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Engine, EngineOutcome, TaskSpec};
    use tempfile::TempDir;

    fn stub(dir: &std::path::Path, script: &str) -> std::path::PathBuf {
        let p = dir.join("claude-stub");
        std::fs::write(&p, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn spec(ws: &std::path::Path) -> TaskSpec { /* same helper as fake.rs tests */ }

    #[test]
    fn build_args_match_the_verified_cli_surface() {
        let args = build_args("do the thing");
        assert_eq!(args[0], "-p");
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"json".to_string()));
        assert!(args.contains(&"--permission-mode".to_string()));
        assert!(args.contains(&"acceptEdits".to_string()));
        let dt = args.iter().position(|a| a == "--disallowedTools").unwrap();
        assert_eq!(args[dt + 1], "Bash(git push:*),Bash(git remote:*),WebFetch,WebSearch");
    }

    #[test]
    fn task_document_includes_plan_verbatim_and_feedback_section() {
        let ws = TempDir::new().unwrap();
        let mut s = spec(ws.path());
        s.review_feedback = Some("please rename x".into());
        let doc = task_document(&s);
        assert!(doc.contains("# Plan") || doc.contains(&s.plan_markdown));
        assert!(doc.contains("please rename x"));
        assert!(doc.contains("[ADR-0003]"));
    }

    #[test]
    fn forge_tokens_are_scrubbed_from_the_engine_env() {
        let d = TempDir::new().unwrap();
        // The stub dumps its env to a file in cwd (the workspace) so we can
        // assert on what the engine subprocess actually saw.
        let bin = stub(d.path(), "#!/bin/sh\nenv > engine-env.txt\nprintf '{\"result\": \"ok\"}'\n");
        let e = ClaudeCodeEngine { binary: bin, timeout: std::time::Duration::from_secs(10) };
        let ws = TempDir::new().unwrap();
        // NB: we cannot mutate our own process env safely in parallel tests;
        // env_clear()+allowlist makes the assertion env-independent:
        let out = e.run(&spec(ws.path())).unwrap();
        assert!(matches!(out, EngineOutcome::Completed { .. }));
        let env_dump = std::fs::read_to_string(ws.path().join("engine-env.txt")).unwrap();
        for var in SCRUBBED_ENV {
            assert!(!env_dump.contains(&format!("\n{var}=")) && !env_dump.starts_with(&format!("{var}=")),
                "{var} leaked into the engine env");
        }
    }

    #[test]
    fn timeout_yields_failed_not_error() {
        let d = TempDir::new().unwrap();
        let bin = stub(d.path(), "#!/bin/sh\nsleep 30\n");
        let e = ClaudeCodeEngine { binary: bin, timeout: std::time::Duration::from_millis(700) };
        let ws = TempDir::new().unwrap();
        let EngineOutcome::Failed { reason, .. } = e.run(&spec(ws.path())).unwrap() else {
            panic!("expected Failed on timeout");
        };
        assert_eq!(reason, "timeout");
    }

    #[test]
    fn json_result_envelope_parsed_for_summary() {
        let d = TempDir::new().unwrap();
        let bin = stub(d.path(),
            "#!/bin/sh\nprintf '{\"type\": \"result\", \"result\": \"implemented the plan\", \"extra\": 1}'\n");
        let e = ClaudeCodeEngine { binary: bin, timeout: std::time::Duration::from_secs(10) };
        let ws = TempDir::new().unwrap();
        let EngineOutcome::Completed { summary } = e.run(&spec(ws.path())).unwrap() else {
            panic!("expected Completed");
        };
        assert_eq!(summary, "implemented the plan");
    }
}
```

- [ ] **Step 5: Run all (FAIL → implement → PASS), gate, commit**

Run: `cargo test --lib engine && cargo test --lib git`
Expected: PASS after implementation.
Run: `just ci` — Expected: PASS.

```bash
cd /home/brett/repos/como-tech/conduit
git add src/engine/mod.rs src/engine/fake.rs src/engine/claude_code.rs src/git.rs src/lib.rs
git commit -m "feat(engine): Engine seam, deterministic FakeEngine, sandboxed claude runner, git plumbing"
```

**Verify gate:** `cargo test --lib engine` + `cargo test --lib git` all pass + `just ci` green.

---

### Task 12: src/router.rs — the tick loop + full-lifecycle and crash-replay e2e

Spec §Crash consistency (the defined ordering) + §Restart recovery. The router owns ALL effects; everything upstream is pure.

**Files:**
- Create: `/home/brett/repos/como-tech/conduit/src/router.rs`
- Modify: `/home/brett/repos/como-tech/conduit/src/lib.rs` (add `pub mod router;`)
- Test: `/home/brett/repos/como-tech/conduit/tests/e2e_fake.rs`

- [ ] **Step 1: Write the router API (stubs)**

```rust
//! The tick loop (spec §Module layout): fetch -> diff -> step -> execute -> persist.
//! Per-transition ordering (spec §Crash consistency):
//!   (1) persist new state + pending intents (tmp+rename+fsync) BEFORE executing
//!   (2) execute each action, probe-first
//!   (3) mark it done in the record
//!   (4) advance the forge cursor only after the tick's actions complete.
//! Crash anywhere -> restart converges: pending intents re-execute behind their
//! probes (at-least-once execution, exactly-once effect).

pub struct Router<'a> {
    pub forge: &'a dyn crate::forge::Forge,
    pub forge_name: String,                 // cursor key: "gitea" | "github" | "fake"
    pub engine: &'a dyn crate::engine::Engine,
    pub store: &'a crate::store::Store,
    pub config: &'a crate::config::Config,
    pub base_branch: String,                // "main"
}

impl Router<'_> {
    /// Boot-time reconcile (spec §Restart recovery): re-execute undone intents
    /// behind probes; a task in Coding/Revising with no live engine gets its
    /// stale workspace disposed and RunEngine re-queued (fresh workspace, from
    /// the immutable plan snapshot). Scoped/InReview just resume polling.
    pub fn recover(&self) -> anyhow::Result<()>;

    /// One poll tick: fetch snapshot, diff vs cursor, route events to tasks,
    /// step + execute, then advance the cursor.
    pub fn tick(&self) -> anyhow::Result<()>;

    /// Map a ForgeEvent to (task, machine::Event). Routing keys: issue id ->
    /// record.issue; pr id -> record.pr; a PR seen for a known branch with no
    /// recorded pr id adopts it (open_pr replay reconciliation).
    fn route(&self, event: &crate::forge::ForgeEvent)
        -> anyhow::Result<Option<(crate::task::TaskRecord, crate::machine::Event)>>;

    /// Apply one transition: mutate the record (state/feedback/attempt), append
    /// intents, save, execute-with-probes, mark done, save.
    fn apply(&self, record: crate::task::TaskRecord, t: crate::machine::Transition)
        -> anyhow::Result<()>;

    /// Execute one action idempotently (the probe table, spec §Idempotency):
    /// CreateIssue->find_issue_by_marker; OpenPr->find_open_pr_by_head;
    /// CommitAndPush->ls_remote compare; comments->marker upsert;
    /// labels->convergent set. RunEngine: prepare workspace (git.rs), run
    /// engine via run_timed, accumulate work_ms, then feed
    /// Event::EngineFinished back through step()+apply() recursively.
    fn execute(&self, record: &mut crate::task::TaskRecord, action: &crate::machine::Action)
        -> anyhow::Result<()>;
}
```

Execution details the implementer must honor:
- `OpenPr` builds the `PrDraft` from the contract module: `pr_title`, `pr_body(reference, plan-derived summary)` (trailer final line), `head = record.branch`, `base = self.base_branch`, labels = `[adr_label, effort label from effort_bucket(work_ms)]`. After create (or probe hit), write the `PrId` back onto the record and save.
- `ApplyPrLabels` recomputes the effort bucket from cumulative `work_ms` and calls `set_pr_labels` with exactly `[effort.label(), adr_label]` — the other four effort labels absent by construction (the "exactly one" structural guarantee, spec §The tuesday contract).
- `RunEngine`: workspace = `store.workspace_dir(&record.id, record.attempt)`; fresh ⇒ delete dir if present, `git::create_workspace(fresh=true)`; revising ⇒ fresh clone of the existing branch (`fresh=false`). TaskSpec from the record + `store.load_plan` + `record.review_feedback.join("\n\n---\n\n")`. ALWAYS re-read the plan from the snapshot — never regenerate (spec §Plan snapshot).
- `CommitAndPush` probe: `ls_remote_sha(url, branch)` equal to local `HEAD` sha ⇒ skip (already pushed).
- Cursor: `tick` saves the fetched snapshot as the new cursor ONLY after every event's actions completed; a failed action leaves the cursor unadvanced so the tick re-runs (idempotent behind probes).
- First tick ever (no cursor): treat `prev` as the empty snapshot.

- [ ] **Step 2: Write the failing e2e tests**

`tests/e2e_fake.rs` (FakeForge + FakeEngine; complete test list, bodies follow the shown patterns):

```rust
use conduit::config::Config;
use conduit::engine::fake::{FakeEngine, FakeMode};
use conduit::forge::fake::{FakeForge, RecordedAction};
use conduit::forge::{CiState, IssueSnapshot, PrSnapshot, RepoSnapshot, Review};
use conduit::router::Router;
use conduit::store::Store;
use conduit::task::{IssueId, PrId, ReviewId, ReviewVerdict, TaskState};
use tempfile::TempDir;

/// Harness: a Scoped task in a fresh store + a FakeForge + local git remote.
/// Returns everything a test needs to drive ticks and assert state.
struct Rig { /* dir: TempDir, store: Store, forge: FakeForge, config: Config, ... */ }

impl Rig {
    fn new() -> Rig { /* store with a Scoped TaskRecord for ADR-0003 (plan saved),
                         FakeForge whose git_remote_url is a seeded local bare repo,
                         FakeEngine Complete */ }
    fn router(&self) -> Router<'_> { /* borrow everything */ }
    /// Script one snapshot that adds `label` to the task's issue, then tick.
    fn label_and_tick(&self, label: &str) { /* ... */ }
}

#[test]
fn full_lifecycle_scoped_to_merged() {
    // Scoped --label conduit:run--> Coding -> engine completes -> InReview
    // (branch pushed, PR opened w/ title/labels/trailer, link comment)
    // --ChangesRequested--> Revising -> engine completes -> InReview (effort
    // label recomputed) --PrMerged--> Merged (issue closed w/ sha comment).
    // Assert state after each tick + the recorded forge actions in order.
}

#[test]
fn engine_failure_goes_to_failed_and_relabel_retries() {
    // FakeEngine Fail: Coding -> Failed (failure comment + conduit:failed label).
    // Re-label conduit:run -> Coding with attempt == 2, fresh workspace.
}

#[test]
fn timeout_is_failed() {
    // FakeEngine Hang{secs: 5} + engine timeout 1s (config) -> Failed, reason timeout.
}

#[test]
fn pr_closed_without_merge_abandons() { /* InReview --PrClosed--> Abandoned, issue closed */ }

#[test]
fn revising_pr_merged_mid_run_discards_engine_result() {
    // Revising + PrMerged in the same tick's diff: task -> Merged, workspace
    // disposed; no CommitAndPush from the stale engine run afterward.
}

/// Kill/restart at EVERY state: serialize the store after reaching each state,
/// build a brand-new Router (same store, fresh FakeForge scripted with the
/// NEXT snapshot), recover(), continue, and assert the lifecycle completes.
#[test]
fn restart_at_every_state_converges() {
    for stop_at in [TaskState::Scoped, TaskState::Coding, TaskState::InReview,
                    TaskState::Revising, TaskState::Failed] {
        // drive to stop_at, drop the router, recover() with a new one,
        // drive to a terminal state. Coding/Revising: assert the stale
        // workspace was disposed and the engine re-ran from the plan snapshot.
    }
}

/// Crash-replay per mutating action kind (spec §Idempotency table):
/// persist the intent, execute it, CRASH BEFORE mark-done (simulated: run
/// recover() on a fresh router with the same store), assert exactly-once
/// effect on the forge.
#[test]
fn crash_replay_create_issue_is_exactly_once() {
    // intent executed once, recover() re-executes behind find_issue_by_marker:
    // forge.count(CreateIssue) stays 1 OR the probe hit means count(CreateIssue)
    // after recover == 1.
}

#[test]
fn crash_replay_open_pr_is_exactly_once() { /* probe: find_open_pr_by_head */ }

#[test]
fn crash_replay_push_is_exactly_once() { /* probe: ls-remote sha compare — assert
    the remote branch sha unchanged after replay */ }

#[test]
fn crash_replay_comment_converges() { /* marker upsert: stored comment count == 1 */ }

#[test]
fn crash_replay_labels_converge() { /* absolute set: final labels identical */ }

#[test]
fn cursor_advances_only_after_actions_complete() {
    // Make the forge fail open_pr once (FakeForge fail-injection: add
    // `fail_next: Mutex<Option<&'static str>>` to FakeForge in this task);
    // tick() returns Err, cursor not saved; next tick re-diffs the same
    // snapshot and completes; no duplicate issue/PR (probes).
}
```

Implementation prerequisites inside this task (both live in the fake, production-free): (a) extend `FakeForge` with `fail_next(method_name)` single-shot fault injection (one new `Mutex<Option<String>>` + a check in each mutator) — needed by the cursor test; (b) the Rig calls `forge.set_remote_url(<seeded local bare repo path>)` (the Task 7 setter) so `CommitAndPush` has a pushable target.

- [ ] **Step 3: Run (FAIL), implement the router, run (PASS)**

Run: `cargo test --test e2e_fake`
Expected: PASS after implementation. These tests are the spike's heart — do not weaken an assertion to get green; fix the router ordering instead.

- [ ] **Step 4: Gate and commit**

Run: `just ci` — Expected: PASS.

```bash
cd /home/brett/repos/como-tech/conduit
git add src/router.rs src/lib.rs src/forge/fake.rs tests/e2e_fake.rs
git commit -m "feat(router): tick loop with write-ahead intents, probe-first replay, cursor ordering"
```

**Verify gate:** `cargo test --test e2e_fake` all pass (lifecycle + restart-at-every-state + 5 crash-replay kinds) + `just ci` green.

---

### Task 13: CLI completion — plan/run/status/verify/demo-transcript wired end-to-end

Spec §Demo script (the exact command sequence the CLI must support) + §The tuesday contract (`conduit verify` = the executable spec tuesday is built against).

**Files:**
- Modify: `/home/brett/repos/como-tech/conduit/src/cli.rs` (replace the not-implemented stubs)
- Create: `/home/brett/repos/como-tech/conduit/src/transcript.rs` (demo-transcript fixture sequence + normalization shared with dry_run)
- Create: `/home/brett/repos/como-tech/conduit/tests/adroit_contract.rs`
- Create: `/home/brett/repos/como-tech/conduit/tests/fixtures/corpus/` (a small adroit by-status corpus: 2 accepted ADRs — one superseded — + 1 proposed, hand-written markdown files in adroit's MADR format)
- Modify: `/home/brett/repos/como-tech/conduit/justfile` (add `demo`, `demo-trigger`, `conformance`)
- Modify: `/home/brett/repos/como-tech/conduit/src/forge/{mod,gitea,github,fake,dry_run}.rs` + `/home/brett/repos/como-tech/conduit/tests/e2e_fake.rs` (the `PrSnapshot` title/body ripple — see verify below)
- Test: `/home/brett/repos/como-tech/conduit/tests/adroit_contract.rs` + extended `tests/cli.rs`

- [ ] **Step 1: Wire the subcommands (behavior contract per spec §Demo script)**

- `conduit init`: `Store::open(".conduit")` + `forge.ensure_labels` of the closed set — the five `EFFORT_LABELS` (colors: pick five stable hex values, e.g. `c2e0c6`/`bfdadc`/`fef2c0`/`f9d0c4`/`d73a4a`) + `conduit:run` (`1d76db`) + `conduit:failed` (`d73a4a`).
- `conduit plan <address>`: `AdrSource::handshake()` → `show(address)` → `require_accepted` → `plan(address)` → `store.save_plan` (verbatim; sha onto the record) → `forge.create_issue` (probe `find_issue_by_marker` first — replay-safe; body = plan markdown + hidden task marker; labels = `[adr_label]`) → save `TaskRecord` (Scoped). Human output: the issue id + "label it conduit:run to start"; `-o json`: the record.
- `conduit run [--once]`: `Router::recover()` then `tick()` once, or loop `tick()` + `sleep(poll.interval_secs)` forever. Engine from config/`CONDUIT_ENGINE` (fake default; `claude-code` = ClaudeCodeEngine with config timeout). Forge from `--forge`/config default (gitea → GiteaForge::open with token; github → `open_github` DryRun — run against github is a transcript-only demo by construction).
- `conduit verify <address> -o json`: load the task record by address → must be Merged with a PR id → **re-read the merged PR from the live forge API** (one `fetch_snapshot`, find the PR) → machine-assert every contract element; emit a JSON report `{"task", "pr", "checks": [{"name", "pass", "detail"}], "pass"}`; exit non-zero if any check fails. Checks (names fixed, asserted in tests): `title_prefix` (regex `^\[ADR-\d{4}\] `), `trailer_final_line` (PR body's last line == `Adr-Reference: <ref>` — requires `title` + `body` fields on `PrSnapshot`: ADD `pub title: String, pub body: String` to `PrSnapshot` in this task, populated by both adapters — Gitea/GitHub `title`/`body` response fields; FakeForge from the stored draft), `exactly_one_effort_label`, `adr_label_present`, `branch_shape` (regex `^conduit/adr-\d{4}/[a-z0-9-]+$`), `never_adr_namespace` (`!head_branch.starts_with("adr/")`). **Ripple warning:** adding fields to `PrSnapshot` breaks every existing struct literal — update the Task 6 inline diff-test `pr()` helper (src/forge/mod.rs), the Gitea snapshot parser + fixtures (src/forge/gitea.rs), the GitHub parser (src/forge/github.rs), FakeForge's snapshot derivation (src/forge/fake.rs), and the e2e snapshot builders (tests/e2e_fake.rs) in the same change, BEFORE running the gate.
- `conduit demo-transcript <address> --forge <f>`: does NOT poll (spec §Transcript-diff semantics). `src/transcript.rs` provides `pub fn fixture_events(reference: &str) -> Vec<machine::Event>` — the scripted scenario: `IssueLabeled(conduit:run)` → `EngineFinished(Completed)` → `ReviewSubmitted(ChangesRequested)` → `EngineFinished(Completed)` → `PrMerged`. Feed through the REAL `machine::step` with FakeEngine and an in-memory record, emitting every resulting action through the chosen adapter wrapped in a transcript emitter: for gitea the LIVE adapter wrapped in `DryRunForge` too? **No** — per spec the gitea leg emits through the live Gitea adapter and the github leg through `DryRun(GitHubForge)`; both legs serialize each emitted action with the SAME normalization. Factor normalization out of `dry_run.rs` into `transcript.rs` (`pub fn normalize_action(...) -> serde_json::Value` + the id-placeholder table struct `Redactor`); `dry_run.rs` and the gitea transcript path both call it. JSONL to stdout. The two outputs must be byte-identical (`diff` in the demo).
- justfile additions:

```just
# Scripted demo trigger: label the demo issue conduit:run as the reviewer
demo-trigger:
    bash demo/demo-trigger.sh

# The full demo walkthrough is docs/src/usage/demo.md (Task 14)
demo:
    @echo "Follow docs/src/usage/demo.md — `just forge-up` first."

# Conformance suite, all legs that need no secrets
conformance:
    cargo test --test conformance
```

(`demo/demo-trigger.sh`: curl PUT the `conduit:run` label onto the newest open issue as `reviewer` — using `.secrets/reviewer.token`; ~10 lines, written here.)

- [ ] **Step 2: Write tests/adroit_contract.rs (against the fixture corpus + stub; live behind CONDUIT_E2E_ADROIT=1)**

```rust
//! adroit integration contract (spec §adroit integration): handshake gate,
//! Accepted-only, superseded skip, plan-snapshot-verbatim, allowlist.
//! Hermetic by default via tests/fixtures/fake-adroit; the PINNED binary runs
//! the same assertions against tests/fixtures/corpus behind CONDUIT_E2E_ADROIT=1
//! (requires `just init-adroit`).

#[test]
fn handshake_gate_blocks_wrong_schema() { /* stub answering manifest_schema 2 -> Handshake err */ }

#[test]
fn plan_is_persisted_verbatim_with_sha() {
    // AdrSource::plan via stub returning a fixed markdown -> store.save_plan ->
    // load_plan == EXACT bytes; record.plan_sha256 == recomputed sha.
}

#[test]
fn accepted_only_and_superseded_skip() { /* as unit-tested in Task 10, but through
    the full `conduit plan` CLI path with assert_cmd + the stub binary
    (CONDUIT_ADROIT_BIN env override on AdrSource — add it in this task,
    documented as a test seam: env override > .conduit/bin/adroit).
    NB: BOTH the env-override resolution AND the ".conduit/bin/adroit" default
    path string must live inside src/adroit.rs (cli.rs constructs AdrSource
    without naming the binary path) — otherwise Task 10's bin/adroit
    source-walker test fires. */ }

#[test]
fn subcommand_allowlist_holds_crate_wide() { /* same walker as Task 10's test but
    also over tests/ (excluding this file + fixtures) */ }

#[test]
fn pinned_adroit_against_fixture_corpus() {
    if std::env::var("CONDUIT_E2E_ADROIT").as_deref() != Ok("1") { return; }
    // .conduit/bin/adroit + ADROIT_DIR=tests/fixtures/corpus:
    // handshake OK; list_accepted returns exactly the non-superseded accepted
    // ADR; show(addr).status == "Accepted".
}
```

Fixture corpus: write 3 files under `tests/fixtures/corpus/` in adroit's default markdown/by-status profile — `accepted/0001-use-rust.md`, `accepted/0002-old-decision.md` (with `Superseded by [ADR-0003](...)` in its `## Status` — NOTE: superseded ADRs live in `superseded/` in by-status; put it there: `superseded/0002-old-decision.md`), `proposed/0003-pending.md`. Crib the exact file shape from `/home/brett/repos/como-tech/adroit/docs/src/` examples or by running the pinned adroit `new` into a temp dir once and copying the output (do NOT hand-invent the format).

- [ ] **Step 3: Extend tests/cli.rs**

```rust
#[test]
fn verify_fails_cleanly_on_unknown_task() { /* exit != 0, helpful stderr */ }

#[test]
fn plan_via_stub_adroit_creates_scoped_record() {
    // CONDUIT_ADROIT_BIN=tests/fixtures/fake-adroit + FAKE_ADROIT_* fixtures +
    // CONDUIT_FORGE=... -> needs a forge: add a `fake` ForgeKind variant?
    // NO — keep the CLI surface specced (gitea|github). Instead run `plan`
    // against the live Gitea ONLY in the demo; the CLI-level plan test asserts
    // the adroit+store half by pointing gitea config at an unreachable port
    // and asserting the typed Offline error AFTER the plan snapshot was
    // persisted (ordering: snapshot before issue — assert the plan file exists
    // even though the forge call failed).
}
```

(That ordering assertion is real spec behavior: the plan snapshot is persisted *before* the task leaves Scoped-creation — spec §Plan snapshot.)

- [ ] **Step 4: Run everything, gate, commit**

Run: `cargo test` then `just ci`
Expected: PASS (no network, no real adroit needed).
Run once with the pinned binary: `just init-adroit && CONDUIT_E2E_ADROIT=1 cargo test --test adroit_contract -- --nocapture`
Expected: PASS.

```bash
cd /home/brett/repos/como-tech/conduit
git add src/cli.rs src/adroit.rs src/transcript.rs src/forge/mod.rs src/forge/gitea.rs src/forge/github.rs src/forge/fake.rs src/forge/dry_run.rs tests/adroit_contract.rs tests/fixtures/corpus tests/cli.rs tests/e2e_fake.rs justfile demo/demo-trigger.sh
git commit -m "feat(cli): plan/run/verify/demo-transcript wired end-to-end + adroit contract tests"
```

**Verify gate:** `cargo test` green; `CONDUIT_E2E_ADROIT=1 cargo test --test adroit_contract` shown passing; `just ci` green.

---

### Task 14: Dogfood + docs — the in-repo ADR corpus, mdbook pages, validated demo

Spec §Self-dogfood + the CLAUDE.md docs rule. The corpus is authored WITH the pinned adroit binary (conduit never writes ADRs; a human/agent driving `adroit` does).

**Files:**
- Create: `/home/brett/repos/como-tech/conduit/adr/` (authored via `.conduit/bin/adroit`, NOT by hand)
- Create: `/home/brett/repos/como-tech/conduit/docs/src/dev/architecture.md`
- Create: `/home/brett/repos/como-tech/conduit/docs/src/dev/forge-contract.md`
- Create: `/home/brett/repos/como-tech/conduit/docs/src/dev/state-machine.md`
- Create: `/home/brett/repos/como-tech/conduit/docs/src/dev/testing.md`
- Create: `/home/brett/repos/como-tech/conduit/docs/src/usage/demo.md`
- Modify: `/home/brett/repos/como-tech/conduit/docs/src/SUMMARY.md`
- Modify: `/home/brett/repos/como-tech/conduit/justfile` (add `adr-check`, wire into `ci`)

- [ ] **Step 1: Author the founding ADR corpus with the pinned adroit**

Prereq: `just init-adroit`. All commands run with `--dir adr` (explicit, so a developer's `ADROIT_DIR` can't redirect them). The corpus keeps adroit's forge integration DISABLED — no forge config, so adroit never opens `adr/`-branch PRs on the demo forge (spec §Self-dogfood).

The founding decisions (spec §Self-dogfood's list — one `adroit new` + body edit + `set-status accepted` each; write real Context/Decision/Consequences prose from the spec sections, generic, no client names):

1. `Rust single crate, fully synchronous` (spec §Stack)
2. `Snapshot-diff event router, polling not webhooks` (spec §The forge adapter)
3. `Filesystem store with write-ahead intents` (spec §Crash consistency)
4. `Structural engine sandbox` (spec §The engine seam)
5. `Effort labels from cumulative wall-clock` (spec §The tuesday contract)
6. `MCP exposure of adroit deferred` (spec §adroit integration: MCP)

Per ADR: `.conduit/bin/adroit new "<title>" --dir adr` (EDITOR=true to skip the editor), edit the body file directly is FORBIDDEN for status — set prose via the editor or `adroit` verbs; then `.conduit/bin/adroit set-status <n> accepted --dir adr`. Validate: `.conduit/bin/adroit check --dir adr` clean and `.conduit/bin/adroit list --status accepted --dir adr -o json` shows all six.

justfile:

```just
# Validate conduit's own ADR corpus with the pinned adroit
adr-check:
    .conduit/bin/adroit check --dir adr
```

Add `adr-check` to the `ci` recipe ONLY if `.conduit/bin/adroit` existing is acceptable in CI — it is not (CI = fresh checkout). Instead: `ci` keeps its four legs; `adr-check` is a standalone recipe documented in CLAUDE.md and run in the demo. (adroit's own repo gates `adr-check` in CI because it builds the binary in-repo; conduit's pin lives in `.conduit`, which is gitignored.)

- [ ] **Step 2: Write the mdbook pages (synced to what got BUILT, not aspirations)**

- `dev/architecture.md`: module map (mirror the spec's tree, updated to reality), the pure-core/effectful-shell split, the seams (Forge/Engine/HttpTransport/AdrSource), crate layering rules.
- `dev/forge-contract.md`: the `Forge` trait, snapshot normalization rules, the diff event semantics table (copy the documented contract from `forge/mod.rs`), the idempotency probe table, endpoint maps for both adapters, the DryRun normalization rules.
- `dev/state-machine.md`: the 7-state diagram + the FULL transition table from Task 3 (including must-ignore cells and the open-PR guard), crash-consistency ordering, restart recovery.
- `dev/testing.md`: the test inventory (machine / conformance legs + env flags / e2e_fake incl. crash-replay-per-action-kind / adroit_contract incl. CONDUIT_E2E_ADROIT), how to run each, the fixture-recording procedure.
- `usage/demo.md`: the spec §Demo script sequence verbatim-but-verified (run every command; paste REAL output snippets), including the restart beat (`kill -9` mid-Coding) and the forge-neutrality transcript diff.

`docs/src/SUMMARY.md` becomes:

```markdown
# Summary

[Introduction](./introduction.md)

# Usage

- [Demo walkthrough](./usage/demo.md)

# Development

- [Spike design](./dev/spike-design.md)
- [Architecture](./dev/architecture.md)
- [Forge contract](./dev/forge-contract.md)
- [State machine](./dev/state-machine.md)
- [Testing](./dev/testing.md)
```

(`book.toml` has `create-missing = false` — create every file before building.)

- [ ] **Step 3: Validate the demo end-to-end (the spike's acceptance run)**

Run the FULL spec §Demo script sequence against the throwaway forge:

```sh
cd /home/brett/repos/como-tech/conduit
just init && just init-adroit
just forge-up
ADROIT_DIR=adr .conduit/bin/adroit list --status accepted -o json   # 6 ADRs
cargo run -- plan 2 --forge gitea          # ADR-0002 snapshot-diff router (pick any accepted)
just demo-trigger                          # reviewer labels conduit:run
cargo run -- run --forge gitea --once      # -> Coding -> InReview (FakeEngine)
# in the Gitea UI (or via curl as reviewer): Request changes; then:
cargo run -- run --forge gitea --once      # -> Revising -> InReview
# approve + merge as reviewer; then:
cargo run -- run --forge gitea --once      # -> Merged
cat .conduit/tasks/*.json
cargo run -- verify 2 --forge gitea -o json   # every check passes
cargo run -- demo-transcript 2 --forge gitea  > /tmp/t-gitea.jsonl
cargo run -- demo-transcript 2 --forge github > /tmp/t-github.jsonl
diff /tmp/t-gitea.jsonl /tmp/t-github.jsonl && echo "FORGE-NEUTRAL: identical"
# encore: the real engine on a second accepted ADR (spec §Demo script)
CONDUIT_ENGINE=claude-code cargo run -- run --forge gitea --once
just forge-down
```

Also demo the restart beat once (kill mid-Coding with the claude engine or a Hang-mode fake, rerun, assert no duplicate issue/PR). Paste real outputs into `usage/demo.md`. If any step fails, fix the CODE (or fixtures), not the demo doc.

- [ ] **Step 4: Final gate and commit**

Run: `just ci && just adr-check`
Expected: PASS — including the mdbook build with all new pages.

```bash
cd /home/brett/repos/como-tech/conduit
git add adr docs/src/SUMMARY.md docs/src/dev/architecture.md docs/src/dev/forge-contract.md docs/src/dev/state-machine.md docs/src/dev/testing.md docs/src/usage/demo.md justfile CLAUDE.md
git commit -m "docs: founding ADR corpus (dogfooded via adroit), architecture/contract/testing book pages, validated demo"
```

**Verify gate:** `just ci` green; `just adr-check` clean; the demo sequence shown working end-to-end (gitea lifecycle + verify + identical transcripts).

---

## Execution notes for the orchestrator

- **Sequence is strict** — each task builds on the previous; each leaves `just ci` green.
- **Live-forge tasks** (8, 13-step-4, 14) need docker; everything else is hermetic. The env-gated legs (`CONDUIT_E2E_GITEA/GITHUB/ADROIT`) must each be shown passing at least once in their task, but never block `just ci`.
- **Do not relitigate the spec.** Where this plan had to pin something the spec left open, the decision is marked inline (snapshot label filter, diff event ordering, the `Mutations::DryRun` conformance parameter, the `CONDUIT_ADROIT_BIN` test seam, `PrSnapshot.body` for verify). Follow them as written.
- **Never push** to any remote other than the localhost Gitea / local bare test repos. Never `git add -A`.









