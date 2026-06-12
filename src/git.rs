//! Local bare cache + workspace lifecycle (spec §Sandbox — structural).
//! The ONLY module that ever sees an authenticated remote URL. Push is only
//! ever used against localhost (Gitea) or local paths (tests) — enforced here.

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git {args:?} failed (exit {code:?}): {stderr}")]
    Command {
        args: Vec<String>,
        code: Option<i32>,
        stderr: String,
    },
    #[error("refusing to push to non-local remote {0} (spike hard constraint)")]
    NonLocalPush(String),
    #[error("git I/O: {0}")]
    Io(#[from] std::io::Error),
}

/// Commit identity — never the user's; set via env at commit time so no
/// workspace config is required.
const IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "conduit-bot"),
    ("GIT_AUTHOR_EMAIL", "conduit-bot@localhost"),
    ("GIT_COMMITTER_NAME", "conduit-bot"),
    ("GIT_COMMITTER_EMAIL", "conduit-bot@localhost"),
];

/// Every git subprocess goes through here: CONSTRUCTED env (env_clear +
/// PATH/HOME — the Task 10 precedent: tokens never reach a child), null
/// stdin, explicit args. Returns the raw Output; callers decide which exit
/// codes are meaningful (`git diff --quiet` uses 1 as data).
fn run_git<S: AsRef<std::ffi::OsStr>>(
    dir: Option<&Path>,
    envs: &[(&str, &str)],
    args: &[S],
) -> Result<std::process::Output, GitError> {
    let mut cmd = std::process::Command::new("git");
    cmd.env_clear();
    for keep in ["PATH", "HOME"] {
        if let Ok(v) = std::env::var(keep) {
            cmd.env(keep, v);
        }
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.stdin(std::process::Stdio::null());
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    cmd.args(args);
    Ok(cmd.output()?)
}

/// `run_git` + non-zero exit is an error.
fn run_git_ok<S: AsRef<std::ffi::OsStr>>(
    dir: Option<&Path>,
    envs: &[(&str, &str)],
    args: &[S],
) -> Result<std::process::Output, GitError> {
    let out = run_git(dir, envs, args)?;
    if out.status.success() {
        Ok(out)
    } else {
        Err(GitError::Command {
            args: args
                .iter()
                .map(|a| a.as_ref().to_string_lossy().into_owned())
                .collect(),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Clone-or-fetch the bare cache at `.conduit/cache/<forge>.git`.
/// Mirror semantics: fetch prunes so the cache tracks remote deletions.
pub fn ensure_cache(cache: &Path, remote_url: &str) -> Result<(), GitError> {
    if cache.exists() {
        run_git_ok(
            Some(cache),
            &[],
            &["fetch", "--prune", remote_url, "+refs/heads/*:refs/heads/*"],
        )?;
    } else {
        if let Some(parent) = cache.parent() {
            std::fs::create_dir_all(parent)?;
        }
        run_git_ok(
            None,
            &[],
            &[
                "clone".as_ref(),
                "--bare".as_ref(),
                std::ffi::OsStr::new(remote_url),
                cache.as_os_str(),
            ],
        )?;
    }
    Ok(())
}

/// Clone the cache into `ws` (origin = the cache path: credential-free),
/// create-or-reset `branch` from `base` (fresh workspace) or check out the
/// existing remote branch (revising).
pub fn create_workspace(
    cache: &Path,
    ws: &Path,
    base: &str,
    branch: &str,
    fresh: bool,
) -> Result<(), GitError> {
    if let Some(parent) = ws.parent() {
        std::fs::create_dir_all(parent)?;
    }
    run_git_ok(
        None,
        &[],
        &["clone".as_ref(), cache.as_os_str(), ws.as_os_str()],
    )?;
    let start = if fresh {
        format!("origin/{base}")
    } else {
        format!("origin/{branch}")
    };
    run_git_ok(Some(ws), &[], &["checkout", "-B", branch, &start])?;
    Ok(())
}

/// Stage everything EXCEPT conduit's artifacts (recursive glob pathspec
/// `:(exclude,glob)**/.conduit-task.md`), delete the root task file first,
/// commit with `message`. Returns false when there is nothing to commit.
///
/// The exclude is RECURSIVE: the engine is untrusted, and a nested copy of
/// the task doc it writes in any subdirectory must never land in a PR
/// (spec §Committing — "never lands" is absolute).
pub fn commit_all_except_task_file(ws: &Path, message: &str) -> Result<bool, GitError> {
    commit_all_except_task_file_with_env(ws, message, &[])
}

/// [`commit_all_except_task_file`] with extra env on the `git commit` child —
/// the transcript demo pins GIT_AUTHOR_DATE/GIT_COMMITTER_DATE so a rerun
/// reproduces the identical sha (deterministic FakeEngine bytes + pinned
/// dates) and its re-push probe becomes a no-op. Production (router) never
/// pins dates: tuesday measures real PRs with real timestamps.
pub fn commit_all_except_task_file_with_env(
    ws: &Path,
    message: &str,
    extra_env: &[(&str, &str)],
) -> Result<bool, GitError> {
    match std::fs::remove_file(ws.join(".conduit-task.md")) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    // Exclude-only pathspec = "everything except" — also guards against a
    // task file that somehow became tracked historically. `**/` with glob
    // magic matches zero or more leading directories (root + any depth).
    run_git_ok(
        Some(ws),
        &[],
        &["add", "-A", "--", ":(exclude,glob)**/.conduit-task.md"],
    )?;
    // `diff --cached --quiet`: exit 0 = nothing staged, 1 = changes staged.
    let diff = run_git(Some(ws), &[], &["diff", "--cached", "--quiet"])?;
    match diff.status.code() {
        Some(0) => return Ok(false),
        Some(1) => {}
        code => {
            return Err(GitError::Command {
                args: vec!["diff".into(), "--cached".into(), "--quiet".into()],
                code,
                stderr: String::from_utf8_lossy(&diff.stderr).into_owned(),
            });
        }
    }
    let mut envs: Vec<(&str, &str)> = IDENTITY.to_vec();
    envs.extend_from_slice(extra_env);
    run_git_ok(Some(ws), &envs, &["commit", "-m", message])?;
    Ok(true)
}

/// Push `branch` from `ws` to the authenticated URL. REFUSES any URL that is
/// not localhost/127.0.0.1/a filesystem path — the structural never-push guard.
pub fn push(ws: &Path, remote_url: &str, branch: &str) -> Result<(), GitError> {
    if !is_local_remote(remote_url) {
        return Err(GitError::NonLocalPush(remote_url.to_string()));
    }
    run_git_ok(
        Some(ws),
        &[],
        &["push", remote_url, &format!("HEAD:refs/heads/{branch}")],
    )?;
    Ok(())
}

/// `git rev-parse HEAD` in `ws` — the local side of the push replay probe
/// (router compares it against [`ls_remote_sha`]; equal ⇒ already pushed).
pub fn head_sha(ws: &Path) -> Result<String, GitError> {
    let out = run_git_ok(Some(ws), &[], &["rev-parse", "HEAD"])?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `git ls-remote <url> refs/heads/<branch>` -> Some(sha) — the push replay probe.
pub fn ls_remote_sha(remote_url: &str, branch: &str) -> Result<Option<String>, GitError> {
    let out = run_git_ok(
        None,
        &[],
        &["ls-remote", remote_url, &format!("refs/heads/{branch}")],
    )?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout.split_whitespace().next().map(str::to_string))
}

/// The local-push guard, pure and unit-testable: true for filesystem paths
/// (no scheme and not scp-like `[user@]host:path`), `file://`, and http(s)
/// URLs whose host (after stripping `user:pass@` and `:port`) is `localhost`
/// or `127.0.0.1`.
pub fn is_local_remote(url: &str) -> bool {
    if url.starts_with("file://") {
        return true;
    }
    if let Some((scheme, rest)) = url.split_once("://") {
        if scheme != "http" && scheme != "https" {
            return false;
        }
        let authority = rest.split('/').next().unwrap_or("");
        let host_port = authority
            .rsplit_once('@')
            .map_or(authority, |(_userinfo, h)| h);
        let host = host_port.rsplit_once(':').map_or(host_port, |(h, _port)| h);
        return host == "localhost" || host == "127.0.0.1";
    }
    // No scheme: a filesystem path unless scp-like (`[user@]host:path` — the
    // part before the first `:` contains no `/`).
    match url.split_once(':') {
        None => true,
        Some((before, _)) => before.contains('/'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sh(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
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
        sh(
            d.path(),
            &[
                "clone",
                "--bare",
                work.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        let url = bare.to_str().unwrap().to_string();
        (d, url)
    }

    #[test]
    fn is_local_remote_guard() {
        assert!(is_local_remote("/tmp/x.git"));
        assert!(is_local_remote("file:///tmp/x.git"));
        assert!(is_local_remote("http://localhost:3000/como/x.git"));
        assert!(is_local_remote(
            "http://conduit-bot:tok@localhost:3000/como/x.git"
        ));
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
            .args(["remote", "get-url", "origin"])
            .current_dir(&ws)
            .output()
            .unwrap();
        let origin = String::from_utf8_lossy(&origin.stdout);
        assert!(
            origin.trim().ends_with("cache.git"),
            "origin must be the local cache: {origin}"
        );
        // engine writes files incl. the task doc; commit excludes the task
        // doc at EVERY depth (untrusted engine may write nested copies)
        std::fs::write(ws.join(".conduit-task.md"), "instructions").unwrap();
        std::fs::create_dir_all(ws.join("docs/impl")).unwrap();
        std::fs::write(ws.join("docs/impl/.conduit-task.md"), "nested copy").unwrap();
        std::fs::create_dir_all(ws.join("docs/impl")).unwrap();
        std::fs::write(ws.join("docs/impl/adr-0003.md"), "impl").unwrap();
        let committed =
            commit_all_except_task_file(&ws, &crate::contract::commit_message("ADR-0003", "x"))
                .unwrap();
        assert!(committed);
        let show = std::process::Command::new("git")
            .args(["show", "--stat", "--format=%s", "HEAD"])
            .current_dir(&ws)
            .output()
            .unwrap();
        let show = String::from_utf8_lossy(&show.stdout);
        assert!(show.contains("docs/impl/adr-0003.md"));
        assert!(
            !show.contains(".conduit-task.md"),
            "task file must never land in a commit"
        );
        assert!(show.contains("[ADR-0003] x"));
        // push to the local bare remote; ls-remote sees the branch (the probe)
        assert!(ls_remote_sha(&url, "conduit/adr-0003/x").unwrap().is_none());
        push(&ws, &url, "conduit/adr-0003/x").unwrap();
        // replay probe semantics: remote sha == local HEAD -> push skippable
        assert_eq!(
            ls_remote_sha(&url, "conduit/adr-0003/x").unwrap().unwrap(),
            head_sha(&ws).unwrap()
        );
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

    #[test]
    fn revising_workspace_checks_out_the_existing_remote_branch() {
        // Round 1: fresh workspace, commit, push. ensure_cache refresh pulls
        // the branch into the cache; round 2 (fresh=false) resumes from it.
        let (_d, url) = seeded_remote();
        let root = TempDir::new().unwrap();
        let cache = root.path().join("cache.git");
        ensure_cache(&cache, &url).unwrap();
        let ws1 = root.path().join("ws1");
        create_workspace(&cache, &ws1, "main", "conduit/adr-0003/x", true).unwrap();
        std::fs::write(ws1.join("round1.txt"), "r1\n").unwrap();
        assert!(commit_all_except_task_file(&ws1, "round 1").unwrap());
        push(&ws1, &url, "conduit/adr-0003/x").unwrap();
        ensure_cache(&cache, &url).unwrap();

        let ws2 = root.path().join("ws2");
        create_workspace(&cache, &ws2, "main", "conduit/adr-0003/x", false).unwrap();
        assert!(
            ws2.join("round1.txt").exists(),
            "revising workspace must carry round-1 work"
        );
        let head = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&ws2)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&head.stdout).trim(),
            "conduit/adr-0003/x"
        );
    }
}
