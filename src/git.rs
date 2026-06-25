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

/// Credentials for an authenticated remote (follow-up 1): supplied to git via
/// a one-shot inline credential helper — the token rides the child ENV
/// (`GIT_PASSWORD`), never argv, which is world-readable (`ps`,
/// `/proc/<pid>/cmdline`, process-auditing daemons).
#[derive(Debug, Clone)]
pub struct GitAuth {
    pub username: String,
    pub token: String,
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
    // conduit commits only to throwaway repos with the `conduit-bot` identity
    // (never the user's), so it must NOT honor a contributor's global
    // `commit.gpgsign = true`: no signing key for the bot exists, and signing a
    // disposable commit is meaningless — it would only hard-fail the Adopt
    // commit and the demo. Inject the override hermetically, same spirit as the
    // env_clear above (HOME is kept so git still reads ~/.gitconfig). Honored by
    // git >= 2.31; an older git ignores it and was never the signing case.
    cmd.env("GIT_CONFIG_COUNT", "1");
    cmd.env("GIT_CONFIG_KEY_0", "commit.gpgsign");
    cmd.env("GIT_CONFIG_VALUE_0", "false");
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
                .map(|a| redact_userinfo(&a.as_ref().to_string_lossy()))
                .collect(),
            code: out.status.code(),
            stderr: redact_userinfo(&String::from_utf8_lossy(&out.stderr)),
        })
    }
}

/// The `-c` config args + env pairs that make git authenticate WITHOUT a
/// secret in argv (follow-up 1): the helper text contains the LITERAL string
/// `$GIT_PASSWORD`; git's shell expands it from the constructed child env.
/// The empty `credential.helper=` clears inherited helpers first so the
/// supplied credential always wins; `GIT_TERMINAL_PROMPT=0` turns a bad
/// credential into a loud failure instead of a prompt-hang (stdin is null).
fn auth_plumbing(auth: Option<&GitAuth>) -> (Vec<String>, Vec<(String, String)>) {
    let Some(auth) = auth else {
        return (vec![], vec![]);
    };
    (
        vec![
            "-c".into(),
            "credential.helper=".into(),
            "-c".into(),
            "credential.helper=!f() { echo \"username=$GIT_USERNAME\"; \
             echo \"password=$GIT_PASSWORD\"; }; f"
                .into(),
        ],
        vec![
            ("GIT_USERNAME".into(), auth.username.clone()),
            ("GIT_PASSWORD".into(), auth.token.clone()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
        ],
    )
}

/// Chokepoint for every git subprocess that touches a (possibly
/// authenticated) remote: auth plumbing prepended, plus the STRUCTURAL
/// argv-leak guarantee — an assert that no argv carries the token, so a
/// regression can never even spawn (follow-up 1; the userinfo redaction in
/// GitError stays as defense-in-depth).
fn run_git_remote_ok<S: AsRef<std::ffi::OsStr>>(
    dir: Option<&Path>,
    auth: Option<&GitAuth>,
    args: &[S],
) -> Result<std::process::Output, GitError> {
    let (auth_args, envs) = auth_plumbing(auth);
    let mut full: Vec<std::ffi::OsString> = auth_args.into_iter().map(Into::into).collect();
    full.extend(args.iter().map(|a| a.as_ref().to_os_string()));
    if let Some(a) = auth {
        assert!(
            full.iter()
                .all(|arg| !arg.to_string_lossy().contains(&a.token)),
            "git argv must never carry the forge token (follow-up 1)"
        );
    }
    let env_refs: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    run_git_ok(dir, &env_refs, &full)
}

/// Clone-or-fetch the bare cache at `.conduit/cache/<forge>.git`.
/// Mirror semantics: fetch prunes so the cache tracks remote deletions.
pub fn ensure_cache(
    cache: &Path,
    remote_url: &str,
    auth: Option<&GitAuth>,
) -> Result<(), GitError> {
    if cache.exists() {
        run_git_remote_ok(
            Some(cache),
            auth,
            &["fetch", "--prune", remote_url, "+refs/heads/*:refs/heads/*"],
        )?;
    } else {
        if let Some(parent) = cache.parent() {
            std::fs::create_dir_all(parent)?;
        }
        run_git_remote_ok(
            None,
            auth,
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
                stderr: redact_userinfo(&String::from_utf8_lossy(&diff.stderr)),
            });
        }
    }
    let mut envs: Vec<(&str, &str)> = IDENTITY.to_vec();
    envs.extend_from_slice(extra_env);
    run_git_ok(Some(ws), &envs, &["commit", "-m", message])?;
    Ok(true)
}

/// Push `branch` from `ws` to the credential-free URL (auth rides the env —
/// see [`auth_plumbing`]). REFUSES any URL that is not
/// localhost/127.0.0.1/a filesystem path — the structural never-push guard.
pub fn push(
    ws: &Path,
    remote_url: &str,
    branch: &str,
    auth: Option<&GitAuth>,
) -> Result<(), GitError> {
    if !is_local_remote(remote_url) {
        return Err(GitError::NonLocalPush(remote_url.to_string()));
    }
    run_git_remote_ok(
        Some(ws),
        auth,
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
pub fn ls_remote_sha(
    remote_url: &str,
    branch: &str,
    auth: Option<&GitAuth>,
) -> Result<Option<String>, GitError> {
    let out = run_git_remote_ok(
        None,
        auth,
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

/// Redact `user:password@` userinfo from any `scheme://user:secret@host`
/// occurrences in `s`, replacing with `scheme://$REDACTED@host`. Operates on
/// the whole string so it catches URLs embedded in args or git stderr. A URL
/// with no userinfo is returned unchanged.
pub(crate) fn redact_userinfo(s: &str) -> String {
    // Find every `://` and check whether what follows looks like `user:pass@`.
    // We scan left-to-right and build the output incrementally.
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(sep) = rest.find("://") {
        // Emit everything up to and including `://`.
        out.push_str(&rest[..sep + 3]);
        rest = &rest[sep + 3..];
        // Authority ends at the first `/`, `?`, `#`, or end-of-string.
        let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..auth_end];
        if let Some(at_pos) = authority.rfind('@') {
            let userinfo = &authority[..at_pos];
            // Only redact if there is a `:` in userinfo (i.e. user:password).
            if userinfo.contains(':') {
                out.push_str("$REDACTED@");
                out.push_str(&authority[at_pos + 1..]);
            } else {
                out.push_str(authority);
            }
        } else {
            out.push_str(authority);
        }
        rest = &rest[auth_end..];
    }
    out.push_str(rest);
    out
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
            // Disposable test repo: never sign — a contributor's global
            // commit.gpgsign=true would hard-fail (no key for this identity).
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
            .env("GIT_CONFIG_VALUE_0", "false")
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

    // ── redact_userinfo tests ──────────────────────────────────────────────

    #[test]
    fn redact_userinfo_url_in_arg() {
        let s = "http://conduit-bot:secret123@localhost:3000/como/x.git";
        let out = redact_userinfo(s);
        assert_eq!(out, "http://$REDACTED@localhost:3000/como/x.git");
        assert!(!out.contains("secret123"), "token must not appear");
    }

    #[test]
    fn redact_userinfo_url_in_stderr() {
        let s = "fatal: unable to access 'http://conduit-bot:tok42@localhost:3000/x.git/': \
                 Could not resolve host";
        let out = redact_userinfo(s);
        assert!(
            out.contains("$REDACTED@localhost"),
            "userinfo must be redacted: {out}"
        );
        assert!(!out.contains("tok42"), "token must not appear: {out}");
    }

    #[test]
    fn redact_userinfo_no_userinfo_passthrough() {
        let s = "http://localhost:3000/como/x.git";
        assert_eq!(
            redact_userinfo(s),
            s,
            "URL without userinfo must pass through unchanged"
        );
    }

    #[test]
    fn redact_userinfo_multiple_occurrences() {
        let s = "push http://bot:t1@localhost/a.git and fetch http://bot:t2@localhost/b.git";
        let out = redact_userinfo(s);
        assert!(
            !out.contains("t1") && !out.contains("t2"),
            "both tokens must vanish: {out}"
        );
        assert_eq!(
            out.matches("$REDACTED@").count(),
            2,
            "two redactions expected: {out}"
        );
    }

    #[test]
    fn redact_userinfo_user_only_no_password_passthrough() {
        // `user@host` with no `:password` — not a credential, leave as-is.
        let s = "http://conduit-bot@localhost:3000/x.git";
        assert_eq!(
            redact_userinfo(s),
            s,
            "user-only (no colon) authority must not be redacted"
        );
    }

    // ── auth plumbing (follow-up 1) ───────────────────────────────────────

    #[test]
    fn auth_plumbing_keeps_the_token_out_of_argv() {
        let auth = GitAuth {
            username: "conduit-bot".into(),
            token: "sekret-token-123".into(),
        };
        let (args, envs) = auth_plumbing(Some(&auth));
        assert!(
            args.iter().all(|a| !a.contains("sekret-token-123")),
            "token leaked into argv: {args:?}"
        );
        assert!(
            args.iter().any(|a| a.contains("credential.helper=!")),
            "inline helper configured: {args:?}"
        );
        // git expands $GIT_PASSWORD itself — from the constructed child env.
        assert_eq!(
            envs.iter()
                .find(|(k, _)| k == "GIT_PASSWORD")
                .map(|(_, v)| v.as_str()),
            Some("sekret-token-123")
        );
        assert_eq!(
            envs.iter()
                .find(|(k, _)| k == "GIT_USERNAME")
                .map(|(_, v)| v.as_str()),
            Some("conduit-bot")
        );
        let (args, envs) = auth_plumbing(None);
        assert!(args.is_empty() && envs.is_empty(), "no auth, no plumbing");
    }

    /// The argv-leak regression (done-criterion 3): run_git_remote_ok ASSERTS
    /// no constructed argv carries the token before spawning, so the whole
    /// remote lifecycle below would panic on any leak. Local path remotes
    /// ignore the credential helper — the plumbing must be inert there.
    #[test]
    fn remote_ops_with_auth_never_put_the_token_in_argv() {
        let (_d, url) = seeded_remote();
        let root = TempDir::new().unwrap();
        let auth = GitAuth {
            username: "conduit-bot".into(),
            token: "sekret-token-123".into(),
        };
        let cache = root.path().join("cache.git");
        ensure_cache(&cache, &url, Some(&auth)).unwrap();
        ensure_cache(&cache, &url, Some(&auth)).unwrap(); // fetch leg too
        let ws = root.path().join("ws");
        create_workspace(&cache, &ws, "main", "conduit/adr-0003/x", true).unwrap();
        std::fs::write(ws.join("f.txt"), "x").unwrap();
        commit_all_except_task_file(&ws, "msg").unwrap();
        assert!(
            ls_remote_sha(&url, "conduit/adr-0003/x", Some(&auth))
                .unwrap()
                .is_none()
        );
        push(&ws, &url, "conduit/adr-0003/x", Some(&auth)).unwrap();
        assert_eq!(
            ls_remote_sha(&url, "conduit/adr-0003/x", Some(&auth))
                .unwrap()
                .unwrap(),
            head_sha(&ws).unwrap()
        );
    }

    // ── is_local_remote / push tests ──────────────────────────────────────

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
        let err = push(
            d.path(),
            "https://github.com/owner/repo.git",
            "conduit/x/y",
            None,
        );
        assert!(matches!(err, Err(GitError::NonLocalPush(_))));
    }

    #[test]
    fn cache_workspace_commit_push_roundtrip() {
        let (_d, url) = seeded_remote();
        let root = TempDir::new().unwrap();
        let cache = root.path().join("cache.git");
        ensure_cache(&cache, &url, None).unwrap();
        ensure_cache(&cache, &url, None).unwrap(); // idempotent: second call fetches
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
        assert!(
            ls_remote_sha(&url, "conduit/adr-0003/x", None)
                .unwrap()
                .is_none()
        );
        push(&ws, &url, "conduit/adr-0003/x", None).unwrap();
        // replay probe semantics: remote sha == local HEAD -> push skippable
        assert_eq!(
            ls_remote_sha(&url, "conduit/adr-0003/x", None)
                .unwrap()
                .unwrap(),
            head_sha(&ws).unwrap()
        );
    }

    #[test]
    fn nothing_to_commit_returns_false() {
        let (_d, url) = seeded_remote();
        let root = TempDir::new().unwrap();
        let cache = root.path().join("cache.git");
        ensure_cache(&cache, &url, None).unwrap();
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
        ensure_cache(&cache, &url, None).unwrap();
        let ws1 = root.path().join("ws1");
        create_workspace(&cache, &ws1, "main", "conduit/adr-0003/x", true).unwrap();
        std::fs::write(ws1.join("round1.txt"), "r1\n").unwrap();
        assert!(commit_all_except_task_file(&ws1, "round 1").unwrap());
        push(&ws1, &url, "conduit/adr-0003/x", None).unwrap();
        ensure_cache(&cache, &url, None).unwrap();

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
