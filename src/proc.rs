//! Shared subprocess deadline harness: spawn the child in its OWN process
//! group, drain its pipes on threads, poll against the deadline, and SIGKILL
//! the whole group on expiry. One implementation for every deadline-bounded
//! child — the engine runner (src/engine/claude_code.rs) and the adroit
//! client (src/adroit.rs) — so a hung child can never block its caller past
//! the configured timeout.
//!
//! Why a GROUP kill: children fork (claude spawns tools; adroit shells out).
//! Killing only the leader leaves grandchildren holding the output pipes,
//! which would block the pipe-reader join arbitrarily far past the deadline.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// What a deadline-bounded run produced. `status == None` means the deadline
/// expired and the process group was killed; stdout/stderr carry whatever
/// the child wrote before it died.
#[derive(Debug)]
pub struct DeadlineOutput {
    pub status: Option<std::process::ExitStatus>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// SIGKILL the child's process group (it was spawned with `process_group(0)`,
/// so `child.id()` == the pgid), then reap the leader.
/// `kill -9 -- -<pgid>` keeps the crate libc-free; constructed env (the
/// Task 10 precedent) and a fallback leader-kill if `kill` is unavailable.
pub fn kill_group(child: &mut Child) {
    let mut kill = Command::new("kill");
    kill.env_clear();
    if let Ok(path) = std::env::var("PATH") {
        kill.env("PATH", path);
    }
    let _ = kill
        .args(["-9", "--", &format!("-{}", child.id())])
        .stdin(Stdio::null())
        .output();
    let _ = child.kill();
    let _ = child.wait();
}

/// Run `cmd` to completion under `timeout`. Stdout/stderr are piped and
/// drained on threads (a full pipe would deadlock the poll loop); the caller
/// keeps ownership of stdin/env/cwd decisions on `cmd`. A spawn or wait
/// failure is the only `Err`; a deadline expiry is `status: None` with the
/// group already killed.
pub fn run_with_deadline(cmd: &mut Command, timeout: Duration) -> std::io::Result<DeadlineOutput> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    // Own process group: on timeout the WHOLE group is killed (see module doc).
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;

    let mut child_stdout = child.stdout.take().expect("stdout was piped");
    let mut child_stderr = child.stderr.take().expect("stderr was piped");
    let stdout_reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = child_stdout.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = child_stderr.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                let now = Instant::now();
                if now >= deadline {
                    kill_group(&mut child);
                    break None; // timeout
                }
                std::thread::sleep((deadline - now).min(Duration::from_millis(500)));
            }
            Err(e) => {
                kill_group(&mut child);
                return Err(e);
            }
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(DeadlineOutput {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn stub(dir: &std::path::Path, script: &str) -> std::path::PathBuf {
        let p = dir.join("stub");
        std::fs::write(&p, script).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn completed_child_yields_status_and_streams() {
        let d = tempfile::TempDir::new().unwrap();
        let bin = stub(d.path(), "#!/bin/sh\necho out\necho err >&2\nexit 3\n");
        let out = run_with_deadline(&mut Command::new(bin), Duration::from_secs(10)).unwrap();
        let status = out.status.expect("child finished");
        assert_eq!(status.code(), Some(3));
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, b"err\n");
    }

    #[test]
    fn deadline_expiry_kills_the_whole_group_promptly() {
        let d = tempfile::TempDir::new().unwrap();
        // The backgrounded grandchild inherits the pipes: only the group kill
        // closes them — a leader-only kill would block on the readers.
        let bin = stub(d.path(), "#!/bin/sh\nsleep 30 &\nsleep 30\n");
        let start = Instant::now();
        let out = run_with_deadline(&mut Command::new(bin), Duration::from_millis(300)).unwrap();
        assert!(out.status.is_none(), "expiry reports status None");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must return promptly after the deadline: took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn missing_binary_is_a_spawn_error() {
        let err = run_with_deadline(
            &mut Command::new("/nonexistent/definitely-missing"),
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
