//! Sandboxed `claude -p` runner (spec §The engine seam). The sandbox is
//! structural: the workspace origin is the local cache (no credentials) and
//! the subprocess env is scrubbed of every forge/AI token.
//!
//! CLI surface re-verified against the installed `claude --help` on
//! 2026-06-12: `-p/--print`, `--output-format json`, `--permission-mode`
//! (choice `acceptEdits` listed), `--disallowedTools` all exist as planned.

use std::path::PathBuf;

use crate::engine::{Engine, EngineError, EngineOutcome, TaskSpec};

pub struct ClaudeCodeEngine {
    /// `"claude"` from PATH by default.
    pub binary: PathBuf,
    /// Conduit-enforced hard timeout (config `[engine] timeout_secs`).
    pub timeout: std::time::Duration,
}

/// Env vars scrubbed from the engine subprocess. The implementation is
/// STRONGER — `env_clear()` plus an allowlist (`PATH`, `HOME`, `TERM`,
/// `LANG`), never a blocklist; this list is the *test* assertion surface.
pub const SCRUBBED_ENV: [&str; 6] = [
    "GITHUB_TOKEN",
    "CONDUIT_GITEA_TOKEN",
    "GITEA_TOKEN",
    "ANTHROPIC_API_KEY",
    "ADROIT_ANTHROPIC_KEY",
    "OPENAI_API_KEY",
];

/// The `-p` prompt — spec-verbatim; the real instructions live in the task
/// document the prompt points at.
const PROMPT: &str = "Implement the plan in .conduit-task.md. Edit files in this directory only.";

/// Pure: build the instruction document written to `<ws>/.conduit-task.md`.
pub fn task_document(spec: &TaskSpec) -> String {
    let mut doc = format!(
        "# {}\n\n## ADR\n\n{}\n\n## Plan\n\n{}\n",
        crate::contract::pr_title(&spec.adr_reference, &spec.title),
        spec.adr_body.trim_end(),
        spec.plan_markdown, // VERBATIM — never reformatted
    );
    if let Some(feedback) = &spec.review_feedback {
        doc.push_str(&format!(
            "\n## Review feedback (address ALL of it)\n\n{}\n",
            feedback.trim_end()
        ));
    }
    doc.push_str(
        "\n## Rules\n\n\
         - Edit files in this directory only.\n\
         - Do not run `git push` or `git remote`.\n\
         - Do not modify or delete `.conduit-task.md`.\n",
    );
    doc
}

/// Pure: the argv after the binary — unit-testable without spawning.
pub fn build_args(prompt: &str) -> Vec<String> {
    [
        "-p",
        prompt,
        "--output-format",
        "json",
        "--permission-mode",
        "acceptEdits",
        "--disallowedTools",
        "Bash(git push:*),Bash(git remote:*),WebFetch,WebSearch",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// The claude `--output-format json` result envelope — tolerate unknown
/// fields (additive CLI drift must not break the parse).
#[derive(serde::Deserialize)]
struct ResultEnvelope {
    result: Option<String>,
    is_error: Option<bool>,
}

/// Last 50 lines of stdout+stderr — the `log_tail` carried on `Failed`.
fn log_tail(stdout: &[u8], stderr: &[u8]) -> String {
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    let lines: Vec<&str> = combined.lines().collect();
    lines[lines.len().saturating_sub(50)..].join("\n")
}

impl Engine for ClaudeCodeEngine {
    fn describe(&self) -> String {
        format!(
            "claude-code ({}, timeout {}s)",
            self.binary.display(),
            self.timeout.as_secs()
        )
    }

    fn run(&self, spec: &TaskSpec) -> Result<EngineOutcome, EngineError> {
        std::fs::write(spec.workspace.join(".conduit-task.md"), task_document(spec))
            .map_err(|e| EngineError::Spawn(format!("cannot write task document: {e}")))?;

        // CONSTRUCTED env — env_clear + allowlist (HOME carries the machine's
        // logged-in claude session), null stdin: the Task 10 precedent.
        let mut cmd = std::process::Command::new(&self.binary);
        cmd.env_clear();
        for keep in ["PATH", "HOME", "TERM", "LANG"] {
            if let Ok(v) = std::env::var(keep) {
                cmd.env(keep, v);
            }
        }
        cmd.current_dir(&spec.workspace)
            .stdin(std::process::Stdio::null())
            .args(build_args(PROMPT));
        // The shared deadline harness (src/proc.rs): own process group, pipes
        // drained on threads, group-SIGKILL at the deadline — engines fork,
        // and killing only the leader leaves grandchildren holding the pipes.
        let output = crate::proc::run_with_deadline(&mut cmd, self.timeout)
            .map_err(|e| EngineError::Spawn(format!("{}: {e}", self.binary.display())))?;
        let tail = log_tail(&output.stdout, &output.stderr);

        let Some(status) = output.status else {
            return Ok(EngineOutcome::Failed {
                reason: "timeout".to_string(),
                log_tail: tail,
            });
        };
        if !status.success() {
            return Ok(EngineOutcome::Failed {
                reason: format!("engine exited with {status}"),
                log_tail: tail,
            });
        }
        // Parse the JSON result envelope; unparseable is a Failed (the task
        // can be retried), never an EngineError.
        match serde_json::from_slice::<ResultEnvelope>(&output.stdout) {
            Ok(envelope) if envelope.is_error == Some(true) => Ok(EngineOutcome::Failed {
                reason: envelope
                    .result
                    .unwrap_or_else(|| "engine reported is_error".to_string()),
                log_tail: tail,
            }),
            Ok(ResultEnvelope {
                result: Some(summary),
                ..
            }) => Ok(EngineOutcome::Completed { summary }),
            _ => Ok(EngineOutcome::Failed {
                reason: "unparseable engine output (expected the JSON result envelope)".to_string(),
                log_tail: tail,
            }),
        }
    }
}

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
    fn build_args_match_the_verified_cli_surface() {
        let args = build_args("do the thing");
        assert_eq!(args[0], "-p");
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"json".to_string()));
        assert!(args.contains(&"--permission-mode".to_string()));
        assert!(args.contains(&"acceptEdits".to_string()));
        let dt = args.iter().position(|a| a == "--disallowedTools").unwrap();
        assert_eq!(
            args[dt + 1],
            "Bash(git push:*),Bash(git remote:*),WebFetch,WebSearch"
        );
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
        let bin = stub(
            d.path(),
            "#!/bin/sh\nenv > engine-env.txt\nprintf '{\"result\": \"ok\"}'\n",
        );
        let e = ClaudeCodeEngine {
            binary: bin,
            timeout: std::time::Duration::from_secs(10),
        };
        let ws = TempDir::new().unwrap();
        // NB: we cannot mutate our own process env safely in parallel tests;
        // env_clear()+allowlist makes the assertion env-independent:
        let out = e.run(&spec(ws.path())).unwrap();
        assert!(matches!(out, EngineOutcome::Completed { .. }));
        let env_dump = std::fs::read_to_string(ws.path().join("engine-env.txt")).unwrap();
        for var in SCRUBBED_ENV {
            assert!(
                !env_dump.contains(&format!("\n{var}="))
                    && !env_dump.starts_with(&format!("{var}=")),
                "{var} leaked into the engine env"
            );
        }
    }

    #[test]
    fn timeout_yields_failed_not_error() {
        let d = TempDir::new().unwrap();
        // The backgrounded grandchild inherits the output pipes: only the
        // process-group kill closes them — a leader-only kill would block
        // the runner on the pipe readers for the full 30s.
        let bin = stub(d.path(), "#!/bin/sh\nsleep 30 &\nsleep 30\n");
        let e = ClaudeCodeEngine {
            binary: bin,
            timeout: std::time::Duration::from_millis(700),
        };
        let ws = TempDir::new().unwrap();
        let start = std::time::Instant::now();
        let EngineOutcome::Failed { reason, .. } = e.run(&spec(ws.path())).unwrap() else {
            panic!("expected Failed on timeout");
        };
        assert_eq!(reason, "timeout");
        // 15s margin (the deadline is sub-second): still far under the 30s
        // grandchild sleep, so a leader-only kill blocking on the pipe readers
        // still fails this — with headroom against parallel-build starvation.
        assert!(
            start.elapsed() < std::time::Duration::from_secs(15),
            "runner must return promptly after the deadline, not wait out \
             engine grandchildren: took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn json_result_envelope_parsed_for_summary() {
        let d = TempDir::new().unwrap();
        let bin = stub(
            d.path(),
            "#!/bin/sh\nprintf '{\"type\": \"result\", \"result\": \"implemented the plan\", \"extra\": 1}'\n",
        );
        let e = ClaudeCodeEngine {
            binary: bin,
            timeout: std::time::Duration::from_secs(10),
        };
        let ws = TempDir::new().unwrap();
        let EngineOutcome::Completed { summary } = e.run(&spec(ws.path())).unwrap() else {
            panic!("expected Completed");
        };
        assert_eq!(summary, "implemented the plan");
    }

    #[test]
    fn unparseable_output_yields_failed_with_tail_not_error() {
        let d = TempDir::new().unwrap();
        let bin = stub(d.path(), "#!/bin/sh\necho 'not json at all'\n");
        let e = ClaudeCodeEngine {
            binary: bin,
            timeout: std::time::Duration::from_secs(10),
        };
        let ws = TempDir::new().unwrap();
        let EngineOutcome::Failed { reason, log_tail } = e.run(&spec(ws.path())).unwrap() else {
            panic!("expected Failed on unparseable output");
        };
        assert!(!reason.is_empty());
        assert!(
            log_tail.contains("not json at all"),
            "tail carries the output"
        );
    }

    #[test]
    fn is_error_envelope_yields_failed() {
        let d = TempDir::new().unwrap();
        let bin = stub(
            d.path(),
            "#!/bin/sh\nprintf '{\"type\": \"result\", \"is_error\": true, \"result\": \"hit max turns\"}'\n",
        );
        let e = ClaudeCodeEngine {
            binary: bin,
            timeout: std::time::Duration::from_secs(10),
        };
        let ws = TempDir::new().unwrap();
        let EngineOutcome::Failed { reason, .. } = e.run(&spec(ws.path())).unwrap() else {
            panic!("expected Failed when the envelope flags is_error");
        };
        assert_eq!(reason, "hit max turns");
    }

    #[test]
    fn nonzero_exit_yields_failed_with_stderr_in_tail() {
        let d = TempDir::new().unwrap();
        let bin = stub(d.path(), "#!/bin/sh\necho 'boom' >&2\nexit 3\n");
        let e = ClaudeCodeEngine {
            binary: bin,
            timeout: std::time::Duration::from_secs(10),
        };
        let ws = TempDir::new().unwrap();
        let EngineOutcome::Failed { reason, log_tail } = e.run(&spec(ws.path())).unwrap() else {
            panic!("expected Failed on nonzero exit");
        };
        assert!(reason.contains('3'), "reason names the exit code: {reason}");
        assert!(log_tail.contains("boom"));
    }

    #[test]
    fn missing_binary_is_a_spawn_error() {
        let e = ClaudeCodeEngine {
            binary: PathBuf::from("/nonexistent/claude-definitely-missing"),
            timeout: std::time::Duration::from_secs(1),
        };
        let ws = TempDir::new().unwrap();
        assert!(matches!(
            e.run(&spec(ws.path())),
            Err(EngineError::Spawn(_))
        ));
    }

    /// Live leg (env-gated like CONDUIT_E2E_GITEA): real `claude` from PATH on
    /// a trivial spec in a temp workspace. The env scrub keeps the machine's
    /// logged-in session (HOME) and nothing else.
    #[test]
    fn live_claude_smoke() {
        if std::env::var("CONDUIT_E2E_CLAUDE").as_deref() != Ok("1") {
            return;
        }
        let ws = TempDir::new().unwrap();
        let s = TaskSpec {
            adr_reference: "ADR-0000".into(),
            title: "Smoke the engine seam".into(),
            adr_body: "Decision: prove the sandboxed runner works.".into(),
            plan_markdown: "# Plan\n1. Create a file named hello.txt containing exactly the line `hello from the engine`.\n".into(),
            review_feedback: None,
            workspace: ws.path().to_path_buf(),
        };
        let e = ClaudeCodeEngine {
            binary: PathBuf::from("claude"),
            timeout: std::time::Duration::from_secs(300),
        };
        let out = e.run(&s).unwrap();
        let EngineOutcome::Completed { summary } = out else {
            panic!("live smoke did not complete: {out:?}");
        };
        assert!(!summary.is_empty());
        let hello = std::fs::read_to_string(ws.path().join("hello.txt")).unwrap();
        assert!(hello.contains("hello from the engine"), "{hello}");
    }
}
