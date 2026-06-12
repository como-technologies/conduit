//! AdrSource: the ONLY adroit call site in the crate (spec §adroit integration).
//! Hardcoded subcommand allowlist {manifest, list, show, plan}; a test asserts
//! no other adroit invocation exists in the crate.
//!
//! The pinned binary (`adroit.rev` -> `just init-adroit` -> `.conduit/bin/adroit`)
//! speaks `-o json`; deserialization is tolerant — required fields only, deny
//! nothing — so additive drift on adroit main cannot break the pinned client
//! (spec §Enumerate). Field names verified against adroit f8547518 view types.

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
    Subprocess {
        subcommand: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error("unparseable adroit {subcommand} output: {source}")]
    BadJson {
        subcommand: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("ADR {address} is {status}, not Accepted — conduit only drives accepted ADRs")]
    NotAccepted { address: String, status: String },
}

/// Tolerant serde: require the contracted fields, deny nothing — additive
/// drift on adroit main must not break the pinned client (spec §Enumerate).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AdrSummary {
    pub reference: String, // "ADR-0003" — display
    pub address: String,   // "3" — addressing token
    pub title: String,
    pub status: String, // "Accepted" etc. — tolerant string, not enum
    #[serde(default)]
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AdrDetail {
    pub reference: String,
    pub address: String,
    pub title: String,
    pub status: String,
    pub body: String, // raw markdown (show -o json flattens summary + body)
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PlanEnvelope {
    pub reference: String,
    pub title: String,
    pub plan: String, // markdown — persisted VERBATIM
    /// `true` when adroit returned the plan persisted in the ADR document — a
    /// deterministic, provider-free read. `false` for a fresh nondeterministic
    /// generation. Additive in adroit's manifest_schema 1; default keeps the
    /// client compatible with envelopes that predate the field.
    #[serde(default)]
    pub stored: bool,
}

pub struct AdrSource {
    bin: PathBuf,                  // .conduit/bin/adroit (or injected stub in tests)
    dir: PathBuf,                  // the ADR corpus (config adroit.dir)
    ai_env: Vec<(String, String)>, // ADROIT_AI_PROVIDER/MODEL (+ key if configured)
    #[cfg(test)]
    extra_env: Vec<(String, String)>, // fixture plumbing for the stub binary
}

impl AdrSource {
    pub fn new(bin: PathBuf, dir: PathBuf, cfg: &crate::config::AdroitConfig) -> AdrSource {
        let mut ai_env = vec![
            ("ADROIT_AI_PROVIDER".to_string(), cfg.ai_provider.clone()),
            ("ADROIT_AI_MODEL".to_string(), cfg.ai_model.clone()),
        ];
        // Upgrade path: pass an Anthropic key through when conduit's own env
        // carries one; adroit ignores it for other providers.
        if let Ok(key) = std::env::var("ADROIT_ANTHROPIC_KEY")
            && !key.is_empty()
        {
            ai_env.push(("ADROIT_ANTHROPIC_KEY".to_string(), key));
        }
        AdrSource {
            bin,
            dir,
            ai_env,
            #[cfg(test)]
            extra_env: Vec::new(),
        }
    }

    /// Test-only fixture plumbing: env vars the stub binary reads (FAKE_ADROIT_*).
    /// Never process-global — set per spawned child only.
    #[cfg(test)]
    fn with_env(mut self, key: &str, value: &str) -> Self {
        self.extra_env.push((key.to_string(), value.to_string()));
        self
    }

    /// `adroit manifest -o json`; require tool=="adroit" && manifest_schema==1,
    /// else bail loudly (spec §Handshake).
    pub fn handshake(&self) -> Result<(), AdroitError> {
        #[derive(serde::Deserialize)]
        struct Manifest {
            tool: String,
            manifest_schema: u64,
        }
        let out = self.run_adroit("manifest", &[])?;
        let m: Manifest = serde_json::from_slice(&out).map_err(|source| AdroitError::BadJson {
            subcommand: "manifest".to_string(),
            source,
        })?;
        if m.tool != "adroit" || m.manifest_schema != 1 {
            return Err(AdroitError::Handshake(format!(
                "expected tool=\"adroit\" manifest_schema=1, got tool={:?} manifest_schema={}",
                m.tool, m.manifest_schema
            )));
        }
        Ok(())
    }

    /// `ADROIT_DIR=<dir> adroit list --status accepted -o json`, skipping rows
    /// with superseded_by != null (spec §Enumerate).
    pub fn list_accepted(&self) -> Result<Vec<AdrSummary>, AdroitError> {
        let out = self.run_adroit("list", &["--status", "accepted"])?;
        let rows: Vec<AdrSummary> =
            serde_json::from_slice(&out).map_err(|source| AdroitError::BadJson {
                subcommand: "list".to_string(),
                source,
            })?;
        Ok(rows
            .into_iter()
            .filter(|r| r.superseded_by.is_none())
            .collect())
    }

    /// `adroit show <address> -o json`.
    pub fn show(&self, address: &str) -> Result<AdrDetail, AdroitError> {
        let out = self.run_adroit("show", &[address])?;
        serde_json::from_slice(&out).map_err(|source| AdroitError::BadJson {
            subcommand: "show".to_string(),
            source,
        })
    }

    /// Conduit's OWN guard — adroit does not enforce this (spec §Guard).
    pub fn require_accepted(detail: &AdrDetail) -> Result<(), AdroitError> {
        if detail.status == "Accepted" {
            Ok(())
        } else {
            Err(AdroitError::NotAccepted {
                address: detail.address.clone(),
                status: detail.status.clone(),
            })
        }
    }

    /// `adroit plan <address> -o json` with ADROIT_AI_* env supplied by conduit.
    ///
    /// The envelope's `stored` flag says which kind came back: `true` = the
    /// deterministic plan persisted in the ADR document; `false` = a fresh
    /// nondeterministic generation. Conduit persists the returned markdown
    /// verbatim either way (store.save_plan — the executed-plan record).
    pub fn plan(&self, address: &str) -> Result<PlanEnvelope, AdroitError> {
        let out = self.run_adroit("plan", &[address])?;
        serde_json::from_slice(&out).map_err(|source| AdroitError::BadJson {
            subcommand: "plan".to_string(),
            source,
        })
    }

    /// Every subprocess goes through this chokepoint: rejects non-allowlisted
    /// subcommands BEFORE spawning; sets ADROIT_DIR env (the env form of --dir —
    /// conduit always uses the env form, spec §Demo script).
    ///
    /// The child env is CONSTRUCTED, not inherited: forge tokens
    /// (CONDUIT_GITEA_TOKEN, GITHUB_TOKEN, ...) must never reach the adroit
    /// subprocess — it has no business with the forge. Kept: PATH (adroit
    /// shells out to git), HOME (git config), plus the intended ADROIT_* vars.
    fn run_adroit(&self, subcommand: &str, args: &[&str]) -> Result<Vec<u8>, AdroitError> {
        if !ALLOWED_SUBCOMMANDS.contains(&subcommand) {
            return Err(AdroitError::Disallowed(subcommand.to_string()));
        }
        let mut cmd = std::process::Command::new(&self.bin);
        cmd.env_clear();
        for keep in ["PATH", "HOME"] {
            if let Ok(v) = std::env::var(keep) {
                cmd.env(keep, v);
            }
        }
        // A hung or runaway `plan` (fresh generation only — stored reads are
        // instant) currently blocks the caller; TODO(timeout): reuse the
        // engine deadline mechanism from Task 11 here.
        cmd.stdin(std::process::Stdio::null());
        cmd.env("ADROIT_DIR", &self.dir)
            .envs(self.ai_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .arg(subcommand)
            .args(args)
            .arg("-o")
            .arg("json");
        #[cfg(test)]
        cmd.envs(self.extra_env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let output = cmd.output().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AdroitError::Missing(self.bin.clone())
            } else {
                AdroitError::Subprocess {
                    subcommand: subcommand.to_string(),
                    code: None,
                    stderr: e.to_string(),
                }
            }
        })?;
        if !output.status.success() {
            return Err(AdroitError::Subprocess {
                subcommand: subcommand.to_string(),
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(output.stdout)
    }

    #[cfg(test)]
    pub(crate) fn run_adroit_for_tests(
        &self,
        subcommand: &str,
        args: &[&str],
    ) -> Result<Vec<u8>, AdroitError> {
        self.run_adroit(subcommand, args)
    }
}

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
        std::fs::write(
            &bad,
            "#!/bin/sh\necho '{\"tool\":\"other\",\"manifest_schema\":1}'\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o755)).unwrap();
        let src = AdrSource::new(bad, d.path().into(), &AdroitConfig::default());
        assert!(matches!(src.handshake(), Err(AdroitError::Handshake(_))));
    }

    #[test]
    fn handshake_bails_on_wrong_schema() {
        let d = tempfile::TempDir::new().unwrap();
        let bad = d.path().join("bad-adroit");
        std::fs::write(
            &bad,
            "#!/bin/sh\necho '{\"tool\":\"adroit\",\"manifest_schema\":2}'\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o755)).unwrap();
        let src = AdrSource::new(bad, d.path().into(), &AdroitConfig::default());
        assert!(matches!(src.handshake(), Err(AdroitError::Handshake(_))));
    }

    #[test]
    fn missing_binary_is_a_typed_error_naming_init_adroit() {
        let d = tempfile::TempDir::new().unwrap();
        let src = AdrSource::new(
            d.path().join("no-such-adroit"),
            d.path().into(),
            &AdroitConfig::default(),
        );
        let err = src.handshake().unwrap_err();
        assert!(matches!(err, AdroitError::Missing(_)));
        assert!(err.to_string().contains("just init-adroit"));
    }

    #[test]
    fn list_accepted_skips_superseded_rows_and_tolerates_extra_fields() {
        let d = tempfile::TempDir::new().unwrap();
        let list = d.path().join("list.json");
        std::fs::write(
            &list,
            r#"[
          {"reference": "ADR-0001", "address": "1", "title": "a", "status": "Accepted",
           "superseded_by": "ADR-0004", "number": 1, "created": null},
          {"reference": "ADR-0003", "address": "3", "title": "b", "status": "Accepted",
           "superseded_by": null, "unknown_future_field": {"x": 1}}
        ]"#,
        )
        .unwrap();
        let src = stub_source(d.path());
        // SAFETY-free env plumbing: pass fixture paths via the AdrSource test
        // constructor instead of process env.
        let src = src.with_env("FAKE_ADROIT_LIST", list.to_str().unwrap());
        let rows = src.list_accepted().unwrap();
        assert_eq!(rows.len(), 1, "superseded row skipped");
        assert_eq!(rows[0].address, "3");
    }

    #[test]
    fn show_parses_detail_and_tolerates_extra_fields() {
        let d = tempfile::TempDir::new().unwrap();
        let show = d.path().join("show.json");
        std::fs::write(
            &show,
            r###"{"reference": "ADR-0003", "address": "3", "title": "t",
                "status": "Accepted", "body": "## Context\n\nwords\n",
                "body_html": null, "plan": null, "related": [], "history": []}"###,
        )
        .unwrap();
        let src = stub_source(d.path()).with_env("FAKE_ADROIT_SHOW", show.to_str().unwrap());
        let detail = src.show("3").unwrap();
        assert_eq!(detail.reference, "ADR-0003");
        assert_eq!(detail.body, "## Context\n\nwords\n");
    }

    #[test]
    fn require_accepted_rejects_other_statuses() {
        let mk = |status: &str| AdrDetail {
            reference: "ADR-0003".into(),
            address: "3".into(),
            title: "t".into(),
            status: status.into(),
            body: "b".into(),
        };
        assert!(AdrSource::require_accepted(&mk("Accepted")).is_ok());
        for s in ["Proposed", "Rejected", "Superseded", "Deprecated"] {
            assert!(
                matches!(
                    AdrSource::require_accepted(&mk(s)),
                    Err(AdroitError::NotAccepted { .. })
                ),
                "{s} must be rejected"
            );
        }
    }

    #[test]
    fn plan_surfaces_stored_flag_and_defaults_to_false_when_absent() {
        let d = tempfile::TempDir::new().unwrap();
        // Stored-plan envelope (adroit >= plan-persistence): stored == true.
        let stored = d.path().join("plan-stored.json");
        std::fs::write(
            &stored,
            r###"{"reference": "ADR-0003", "title": "t", "plan": "## Steps\n\n1. do\n", "stored": true}"###,
        )
        .unwrap();
        let src = stub_source(d.path()).with_env("FAKE_ADROIT_PLAN", stored.to_str().unwrap());
        let env = src.plan("3").unwrap();
        assert!(env.stored);
        assert_eq!(env.plan, "## Steps\n\n1. do\n", "plan markdown verbatim");
        // Envelope without the additive field (older shape): tolerant default false.
        let fresh = d.path().join("plan-fresh.json");
        std::fs::write(
            &fresh,
            r#"{"reference": "ADR-0003", "title": "t", "plan": "fresh\n"}"#,
        )
        .unwrap();
        let src = stub_source(d.path()).with_env("FAKE_ADROIT_PLAN", fresh.to_str().unwrap());
        let env = src.plan("3").unwrap();
        assert!(!env.stored, "absent stored field defaults to false");
    }

    #[test]
    fn run_adroit_rejects_non_allowlisted_subcommands() {
        let d = tempfile::TempDir::new().unwrap();
        let src = stub_source(d.path());
        for bad in ["new", "set-status", "supersede", "edit", "review"] {
            assert!(
                matches!(
                    src.run_adroit_for_tests(bad, &[]),
                    Err(AdroitError::Disallowed(_))
                ),
                "{bad} must be refused before spawn"
            );
        }
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
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(&src_dir, &mut files);
        for f in files {
            if f.file_name().is_some_and(|n| n == "adroit.rs") {
                continue;
            }
            let content = std::fs::read_to_string(&f).unwrap();
            // The binary path fragment and the AdrSource-bypassing markers:
            if content.contains("bin/adroit") || content.contains("Command::new(\"adroit\"") {
                offenders.push(f);
            }
        }
        assert!(
            offenders.is_empty(),
            "adroit invoked outside src/adroit.rs: {offenders:?}"
        );
    }

    /// Live leg (CONDUIT_E2E_ADROIT=1, after `just init-adroit`): the PINNED
    /// binary against a throwaway corpus this test authors VIA THE BINARY
    /// (allowed for fixtures — the allowlist governs what conduit's code
    /// invokes). Proves: handshake; accepted-only enumeration; show; the
    /// stored-plan deterministic read (`plan --save` via ADROIT_AI_FAKE, then
    /// AdrSource::plan with no usable provider returns the exact saved bytes,
    /// stored == true); and the verbatim store.save_plan snapshot rule.
    #[test]
    fn live_pinned_adroit_end_to_end() {
        if std::env::var("CONDUIT_E2E_ADROIT").as_deref() != Ok("1") {
            return;
        }
        let bin = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".conduit/bin/adroit");
        assert!(bin.exists(), "run `just init-adroit` first: {bin:?}");
        let d = tempfile::TempDir::new().unwrap();
        let corpus = d.path().join("adr");

        // Author a tiny corpus with the binary itself (fixture setup, not conduit's lane).
        let author = |args: &[&str], extra_env: &[(&str, &str)]| {
            let mut cmd = std::process::Command::new(&bin);
            cmd.env("ADROIT_DIR", &corpus).args(args);
            for (k, v) in extra_env {
                cmd.env(k, v);
            }
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "adroit {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        author(&["new", "Adopt snapshot-diff router", "--no-edit"], &[]);
        author(&["new", "Old decision", "--no-edit"], &[]);
        author(&["new", "Pending idea", "--no-edit"], &[]);
        author(&["set-status", "1", "accepted"], &[]);
        author(&["set-status", "2", "accepted"], &[]);
        author(&["supersede", "1", "2"], &[]); // ADR 1 supersedes ADR 2
        // ADR 3 stays Proposed.

        // Persist a stored plan into ADR 1 via the always-compiled fake AI seam.
        // Trim-stable text (adroit's stored-plan extract trims the section), so
        // the deterministic read below can assert EXACT equality.
        let canned = "1. wire the chokepoint\n2. prove the seam";
        author(&["plan", "1", "--save"], &[("ADROIT_AI_FAKE", canned)]);

        // Conduit's client, configured with an unusable provider/model: every
        // read below must succeed with NO working AI — the stored-plan read is
        // deterministic and provider-free.
        let cfg = AdroitConfig {
            dir: corpus.to_string_lossy().into_owned(),
            ai_provider: "ollama".to_string(),
            ai_model: "no-such-model-e2e".to_string(),
        };
        let src = AdrSource::new(bin, corpus.clone(), &cfg);
        src.handshake().expect("pinned handshake");

        let rows = src.list_accepted().expect("list accepted");
        assert_eq!(rows.len(), 1, "superseded ADR 2 skipped: {rows:?}");
        assert_eq!(rows[0].address, "1");

        let detail = src.show(&rows[0].address).expect("show");
        AdrSource::require_accepted(&detail).expect("accepted guard");
        let proposed = src.show("3").expect("show proposed");
        assert!(matches!(
            AdrSource::require_accepted(&proposed),
            Err(AdroitError::NotAccepted { .. })
        ));

        // Deterministic stored read, twice — byte-identical, stored == true.
        let env1 = src.plan("1").expect("stored plan read");
        let env2 = src.plan("1").expect("stored plan read again");
        assert!(env1.stored, "expected the persisted plan, got a generation");
        assert_eq!(env1.plan, env2.plan, "stored read must be deterministic");
        assert_eq!(env1.plan, canned, "stored plan is the saved text, verbatim");

        // Snapshot rule: the returned markdown is persisted verbatim, sha recorded.
        let store = crate::store::Store::open(d.path().join(".conduit")).unwrap();
        let sha = store.save_plan("adr-1", &env1.plan).unwrap();
        assert_eq!(store.load_plan("adr-1").unwrap(), env1.plan);
        assert_eq!(sha.len(), 64);
    }
}
