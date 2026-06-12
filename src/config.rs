//! `conduit.toml` config load + env overlay (spec §Module layout).
//!
//! File is optional — missing = all defaults. Env vars overlay the file:
//! `CONDUIT_FORGE`, `CONDUIT_ENGINE`, `CONDUIT_TIMEOUT_SECS`, `CONDUIT_POLL_SECS`.

use serde::{Deserialize, Serialize};

use crate::contract::EffortThresholds;

// ── Enums ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ForgeKind {
    Gitea,
    Github,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EngineKind {
    Fake,
    ClaudeCode,
}

// ── Config structs ─────────────────────────────────────────────────────────

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
    pub base_url: String,
    pub owner: String,
    pub repo: String,
    // token: NEVER in the file — env CONDUIT_GITEA_TOKEN, falling back to
    // .secrets/conduit-bot.token (the gitea-init.sh drop location, Task 8).
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GithubConfig {
    pub owner: String,
    pub repo: String,
    // token: env GITHUB_TOKEN only (live READS; mutations always DryRun).
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    pub kind: EngineKind,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdroitConfig {
    pub dir: String,
    pub ai_provider: String,
    pub ai_model: String,
    // ADROIT_ANTHROPIC_KEY upgrade path: passed through from conduit's env if set.
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PollConfig {
    pub interval_secs: u64,
}

// ── Default impls ──────────────────────────────────────────────────────────

// Manual Default impls encode the spec's documented default values, which
// differ from field zero-values (e.g. ForgeKind::Gitea is not the first
// variant, GiteaConfig has non-empty strings). Clippy's derivable_impls
// lint fires for Config and GithubConfig but these are intentional forms.
#[allow(clippy::derivable_impls)]
impl Default for Config {
    fn default() -> Self {
        Config {
            forge: ForgeConfig::default(),
            engine: EngineConfig::default(),
            adroit: AdroitConfig::default(),
            effort: EffortThresholds::default(),
            poll: PollConfig::default(),
        }
    }
}

impl Default for ForgeConfig {
    fn default() -> Self {
        ForgeConfig {
            default: ForgeKind::Gitea,
            gitea: GiteaConfig::default(),
            github: GithubConfig::default(),
        }
    }
}

impl Default for GiteaConfig {
    fn default() -> Self {
        GiteaConfig {
            base_url: "http://localhost:3000".to_string(),
            owner: "como".to_string(),
            repo: "conduit-dogfood".to_string(),
        }
    }
}

#[allow(clippy::derivable_impls)]
impl Default for GithubConfig {
    fn default() -> Self {
        GithubConfig {
            owner: String::new(),
            repo: String::new(),
        }
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            kind: EngineKind::Fake,
            timeout_secs: 1800,
        }
    }
}

impl Default for AdroitConfig {
    fn default() -> Self {
        AdroitConfig {
            dir: "adr".to_string(),
            ai_provider: "ollama".to_string(),
            ai_model: "llama3.2".to_string(),
        }
    }
}

impl Default for PollConfig {
    fn default() -> Self {
        PollConfig { interval_secs: 15 }
    }
}

// ── Error ──────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid conduit.toml: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid env override {var}={value}")]
    Env { var: String, value: String },
    #[error("invalid configuration: {0}")]
    Validation(String),
}

// ── Config impl ────────────────────────────────────────────────────────────

impl Config {
    /// Load `conduit.toml` from `dir` (missing file = all defaults), then
    /// overlay env: CONDUIT_FORGE (gitea|github), CONDUIT_ENGINE
    /// (fake|claude-code), CONDUIT_TIMEOUT_SECS, CONDUIT_POLL_SECS.
    pub fn load(dir: &std::path::Path) -> Result<Config, ConfigError> {
        let path = dir.join("conduit.toml");
        let mut config = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str::<Config>(&text)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(source) => return Err(ConfigError::Io { path, source }),
        };

        // Env overlays
        if let Ok(val) = std::env::var("CONDUIT_FORGE") {
            config.forge.default = match val.as_str() {
                "gitea" => ForgeKind::Gitea,
                "github" => ForgeKind::Github,
                _ => {
                    return Err(ConfigError::Env {
                        var: "CONDUIT_FORGE".to_string(),
                        value: val,
                    });
                }
            };
        }

        if let Ok(val) = std::env::var("CONDUIT_ENGINE") {
            config.engine.kind = match val.as_str() {
                "fake" => EngineKind::Fake,
                "claude-code" => EngineKind::ClaudeCode,
                _ => {
                    return Err(ConfigError::Env {
                        var: "CONDUIT_ENGINE".to_string(),
                        value: val,
                    });
                }
            };
        }

        if let Ok(val) = std::env::var("CONDUIT_TIMEOUT_SECS") {
            config.engine.timeout_secs = val.parse::<u64>().map_err(|_| ConfigError::Env {
                var: "CONDUIT_TIMEOUT_SECS".to_string(),
                value: val.clone(),
            })?;
        }

        if let Ok(val) = std::env::var("CONDUIT_POLL_SECS") {
            config.poll.interval_secs = val.parse::<u64>().map_err(|_| ConfigError::Env {
                var: "CONDUIT_POLL_SECS".to_string(),
                value: val.clone(),
            })?;
        }

        config.validate()?;
        Ok(config)
    }

    /// Validate that effort thresholds are strictly increasing
    /// (super_quick < not_long < average < a_while) and the poll interval
    /// is non-zero.
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        let e = &self.effort;
        let increasing = e.super_quick_max_ms < e.not_long_max_ms
            && e.not_long_max_ms < e.average_max_ms
            && e.average_max_ms < e.a_while_max_ms;
        if !increasing {
            return Err(ConfigError::Validation(format!(
                "[effort] thresholds must be strictly increasing: {}<{}<{}<{}",
                e.super_quick_max_ms, e.not_long_max_ms, e.average_max_ms, e.a_while_max_ms
            )));
        }
        if self.poll.interval_secs == 0 {
            return Err(ConfigError::Validation(
                "[poll] interval_secs must be >= 1 (0 would spin the daemon hot)".into(),
            ));
        }
        Ok(())
    }

    /// Gitea token: env CONDUIT_GITEA_TOKEN, else `.secrets/conduit-bot.token`
    /// under `dir` (trimmed), else None. Never logged.
    pub fn gitea_token(dir: &std::path::Path) -> Option<String> {
        if let Ok(tok) = std::env::var("CONDUIT_GITEA_TOKEN")
            && !tok.is_empty()
        {
            return Some(tok);
        }
        let file = dir.join(".secrets/conduit-bot.token");
        match std::fs::read_to_string(&file) {
            Ok(s) => {
                let trimmed = s.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            }
            // NotFound is the normal "no secrets provisioned" case. Anything
            // else (EISDIR, EPERM, ...) is swallowed too — the symptom is a
            // 401 from the forge; revisit if a logger lands post-spike.
            Err(_) => None,
        }
    }

    /// GitHub token: env GITHUB_TOKEN, else None (reads-only adapter).
    pub fn github_token() -> Option<String> {
        std::env::var("GITHUB_TOKEN").ok().filter(|s| !s.is_empty())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

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
        std::fs::write(
            d.path().join("conduit.toml"),
            "[engine]\nkind = \"claude-code\"\ntimeout_secs = 60\n",
        )
        .unwrap();
        let c = Config::load(d.path()).unwrap();
        assert_eq!(c.engine.kind, EngineKind::ClaudeCode);
        assert_eq!(c.engine.timeout_secs, 60);
        assert_eq!(c.poll.interval_secs, 15, "unset sections keep defaults");
    }

    #[test]
    fn effort_thresholds_load_from_toml() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("conduit.toml"),
            "[effort]\nsuper_quick_max_ms = 5\n",
        )
        .unwrap();
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

    #[test]
    fn validate_rejects_non_strictly_increasing_thresholds() {
        let mut c = Config::default();
        // Make super_quick >= not_long
        c.effort.super_quick_max_ms = c.effort.not_long_max_ms;
        assert!(
            c.validate().is_err(),
            "equal thresholds should fail validation"
        );
        c.effort.super_quick_max_ms = c.effort.not_long_max_ms + 1;
        assert!(
            c.validate().is_err(),
            "inverted thresholds should fail validation"
        );
    }

    #[test]
    fn validate_accepts_default_thresholds() {
        let c = Config::default();
        assert!(c.validate().is_ok(), "default thresholds must be valid");
    }
}
