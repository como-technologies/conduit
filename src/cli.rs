//! CLI surface (spec §Module layout):
//! init | plan <address> | run [--once] | status | verify <address> | demo-transcript <address>
//! Globals: --forge <gitea|github>, -o/--output <human|json>.
//!
//! Behavior contract per spec §Demo script; `verify` is the executable spec
//! of §The tuesday contract. All wiring lives here — the binary's `main.rs`
//! is clap marshalling only, and this module never names the adroit binary
//! path (AdrSource::resolve_bin owns it).

use std::path::Path;

use clap::{Parser, Subcommand, ValueEnum};

use crate::adroit::AdrSource;
use crate::config::{Config, EngineKind, ForgeKind};
use crate::contract;
use crate::engine::Engine;
use crate::forge::{Forge, LabelSpec, PrSnapshot};
use crate::router::Router;
use crate::store::Store;
use crate::task::{TaskRecord, TaskState};

// ── Types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Parser)]
#[command(
    name = "conduit",
    version,
    about = "Forge-neutral agentic development harness"
)]
pub struct Cli {
    /// Forge adapter to use (defaults to conduit.toml [forge].default).
    #[arg(long, global = true, value_enum)]
    pub forge: Option<ForgeKind>,
    /// Output format for read verbs.
    #[arg(
        short = 'o',
        long = "output",
        global = true,
        value_enum,
        default_value = "human"
    )]
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

// ── Dispatch ───────────────────────────────────────────────────────────────

/// Entry point for all CLI subcommands. Loads config from the current directory.
pub fn dispatch(cli: Cli) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let mut config = crate::config::Config::load(&cwd)?;

    // --forge flag overrides the config default.
    if let Some(forge) = cli.forge {
        config.forge.default = forge;
    }

    match cli.command {
        Command::Status => cmd_status(&cwd, cli.output),
        Command::Init => cmd_init(&cwd, &config),
        Command::Plan { address } => cmd_plan(&cwd, &config, &address, cli.output),
        Command::Run { once } => cmd_run(&cwd, &config, once),
        Command::Verify { address } => cmd_verify(&cwd, &config, &address, cli.output),
        Command::DemoTranscript { address } => cmd_demo_transcript(&cwd, &config, &address),
    }
}

// ── Assembly helpers ───────────────────────────────────────────────────────

/// The chosen adapter + its cursor key. Gitea is the full read-write
/// lifecycle host; GitHub is ALWAYS DryRun-decorated (reads live, mutations
/// recorded — the constructor only hands out the wrapper).
fn build_forge(dir: &Path, config: &Config) -> (Box<dyn Forge>, &'static str) {
    match config.forge.default {
        ForgeKind::Gitea => {
            let token = Config::gitea_token(dir).unwrap_or_default();
            (
                Box::new(crate::forge::gitea::GiteaForge::open(
                    &config.forge.gitea,
                    token,
                )),
                "gitea",
            )
        }
        ForgeKind::Github => {
            let token = crate::forge::github::resolve_token().unwrap_or_default();
            (
                Box::new(crate::forge::github::open_github(
                    &config.forge.github,
                    token,
                )),
                "github",
            )
        }
    }
}

/// Engine per config/CONDUIT_ENGINE: deterministic fake (default demo path;
/// mode via CONDUIT_FAKE_ENGINE_MODE) or the sandboxed claude runner with the
/// config timeout.
fn build_engine(config: &Config) -> Box<dyn Engine> {
    match config.engine.kind {
        EngineKind::Fake => Box::new(crate::engine::fake::FakeEngine {
            mode: crate::engine::fake::FakeMode::from_env(),
        }),
        EngineKind::ClaudeCode => Box::new(crate::engine::claude_code::ClaudeCodeEngine {
            binary: "claude".into(),
            timeout: std::time::Duration::from_secs(config.engine.timeout_secs),
        }),
    }
}

/// The standing label set `conduit init` pre-creates (spec §The tuesday
/// contract: the closed effort set + the trigger/failure pair). Colors are
/// stable hex values; `adr:<reference>` labels are created on demand by the
/// adapters instead (one per ADR — not a standing set).
fn standard_labels() -> Vec<LabelSpec> {
    let spec = |name: &str, color: &str, description: &str| LabelSpec {
        name: name.to_string(),
        color: color.to_string(),
        description: description.to_string(),
    };
    vec![
        spec(contract::EFFORT_LABELS[0], "c2e0c6", "under 10 minutes"),
        spec(contract::EFFORT_LABELS[1], "bfdadc", "under 30 minutes"),
        spec(contract::EFFORT_LABELS[2], "fef2c0", "under 2 hours"),
        spec(contract::EFFORT_LABELS[3], "f9d0c4", "under 8 hours"),
        spec(contract::EFFORT_LABELS[4], "d73a4a", "8 hours or more"),
        spec(
            contract::LABEL_RUN,
            "1d76db",
            "human trigger: start this task",
        ),
        spec(
            contract::LABEL_FAILED,
            "d73a4a",
            "engine failed; needs attention",
        ),
    ]
}

// ── Commands ───────────────────────────────────────────────────────────────

fn cmd_init(dir: &Path, config: &Config) -> anyhow::Result<()> {
    let store = Store::open(dir.join(".conduit"))?;
    let (forge, _) = build_forge(dir, config);
    forge.ensure_labels(&standard_labels())?;
    println!(
        "initialized: store at {} — label set ensured on {}",
        store.root().display(),
        forge.describe()
    );
    Ok(())
}

/// `conduit plan <address>` (spec §adroit integration + §Plan snapshot):
/// handshake → show → conduit-side Accepted guard → adroit plan → persist the
/// plan VERBATIM (sha onto the record) + the ADR body sidecar → THEN the
/// probe-first issue (Router::ensure_issue). Snapshot-before-issue ordering
/// is spec behavior: a forge failure leaves the snapshot persisted, and the
/// re-run converges without regenerating the plan.
fn cmd_plan(
    dir: &Path,
    config: &Config,
    address: &str,
    output: OutputFormat,
) -> anyhow::Result<()> {
    let store = Store::open(dir.join(".conduit"))?;
    let adroit = AdrSource::new(
        AdrSource::resolve_bin(dir),
        dir.join(&config.adroit.dir),
        &config.adroit,
    )
    // The adroit subprocess inherits the engine deadline ([engine]
    // timeout_secs): a hung `plan` is group-killed, never blocks the daemon.
    .with_timeout(std::time::Duration::from_secs(config.engine.timeout_secs));
    adroit.handshake()?;
    let detail = adroit.show(address)?;
    AdrSource::require_accepted(&detail)?;

    let task_id = contract::task_slug(&detail.reference);
    let mut record = match store.load_task(&task_id)? {
        Some(existing) if existing.state.is_terminal() => anyhow::bail!(
            "task {task_id} is already {:?} — replanning = cancel + new task (out of scope in the spike)",
            existing.state
        ),
        Some(existing) => {
            // Replay (operator re-run, or crash between snapshot and issue):
            // the persisted plan snapshot is immutable — NEVER regenerated.
            eprintln!("task {task_id} already planned; converging on the existing snapshot");
            existing
        }
        None => {
            let envelope = adroit.plan(address)?;
            // Operator-facing provenance: a stored plan is a deterministic
            // read; a fresh generation is nondeterministic (and this snapshot
            // is now the only copy that matters).
            eprintln!(
                "plan for {}: {}",
                detail.reference,
                if envelope.stored {
                    "stored plan (deterministic read from the ADR document)"
                } else {
                    "freshly generated (nondeterministic; snapshot is now canonical)"
                }
            );
            let mut record = TaskRecord::new(&detail.reference, &detail.address, &detail.title, "");
            record.plan_sha256 = store.save_plan(&record.id, &envelope.plan)?;
            // Decision context for the engine seam (TaskSpec.adr_body).
            store.save_adr_body(&record.id, &detail.body)?;
            store.save_task(&record)?;
            record
        }
    };

    let (forge, forge_name) = build_forge(dir, config);
    let engine = build_engine(config);
    let router = Router::new(
        forge.as_ref(),
        forge_name,
        engine.as_ref(),
        &store,
        config,
        "main",
    );
    router.ensure_issue(&mut record)?;
    let issue = record.issue.expect("ensure_issue sets the id");

    match output {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&record)?),
        OutputFormat::Human => println!(
            "planned {} as task {} — issue {} on {}: label it {} to start",
            detail.reference,
            record.id,
            issue.0,
            forge.describe(),
            contract::LABEL_RUN
        ),
    }
    Ok(())
}

/// `conduit run [--once]`: the poll-tick daemon. `--once` = recover + one
/// tick (errors propagate). The daemon loop logs a failed tick and keeps
/// polling — transient forge outages must not kill it; the unadvanced cursor
/// makes the next tick re-diff the same snapshot and converge.
fn cmd_run(dir: &Path, config: &Config, once: bool) -> anyhow::Result<()> {
    let store = Store::open(dir.join(".conduit"))?;
    let (forge, forge_name) = build_forge(dir, config);
    let engine = build_engine(config);
    let router = Router::new(
        forge.as_ref(),
        forge_name,
        engine.as_ref(),
        &store,
        config,
        "main",
    );
    eprintln!(
        "conduit run: {} via {} (engine: {})",
        if once { "single tick" } else { "poll loop" },
        forge.describe(),
        engine.describe()
    );
    router.recover()?;
    if once {
        return router.tick();
    }
    loop {
        if let Err(e) = router.tick() {
            eprintln!("conduit run: tick failed (retrying next poll): {e:#}");
        }
        std::thread::sleep(std::time::Duration::from_secs(config.poll.interval_secs));
    }
}

fn cmd_status(dir: &std::path::Path, output: OutputFormat) -> anyhow::Result<()> {
    let store_dir = dir.join(".conduit");
    let store = crate::store::Store::open(&store_dir)?;
    let records = store.list_tasks()?;

    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&records)?);
        }
        OutputFormat::Human => {
            if records.is_empty() {
                println!("no tasks");
            } else {
                println!("{:<36}  {:<16}  {:<8}  branch", "id", "state", "attempt");
                for r in &records {
                    println!(
                        "{:<36}  {:<16}  {:<8}  {}",
                        r.id,
                        format!("{:?}", r.state),
                        r.attempt,
                        r.branch,
                    );
                }
            }
        }
    }
    Ok(())
}

// ── verify — the executable tuesday contract ──────────────────────────────

/// One machine-asserted contract element. The six names are FIXED (asserted
/// in tests): tuesday-side consumers key on them.
#[derive(Debug, serde::Serialize)]
pub struct Check {
    pub name: &'static str,
    pub pass: bool,
    pub detail: String,
}

/// `conduit verify <address> -o json` (spec §The tuesday contract): load the
/// Merged task, re-read the merged PR LIVE from the forge (one
/// fetch_snapshot), machine-assert every contract element, emit the report
/// `{"task", "pr", "checks": [..], "pass"}`. Exit non-zero on any violation.
fn cmd_verify(
    dir: &Path,
    config: &Config,
    address: &str,
    output: OutputFormat,
) -> anyhow::Result<()> {
    let store = Store::open(dir.join(".conduit"))?;
    let records = store.list_tasks()?;
    // Addressed by ADR address while one ADR = one task holds (spec §Demo
    // script); the task id / display reference are accepted as conveniences.
    let record = records
        .iter()
        .find(|r| r.adr_address == address || r.id == address || r.adr_reference == address)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no task for ADR address {address:?} — run `conduit plan {address}` first \
                 (`conduit status` lists known tasks)"
            )
        })?;
    if record.state != TaskState::Merged {
        anyhow::bail!(
            "task {} is {:?}, not Merged — verify asserts the contract on the MERGED PR",
            record.id,
            record.state
        );
    }
    let pr_id = record
        .pr
        .ok_or_else(|| anyhow::anyhow!("task {} is Merged but has no PR id recorded", record.id))?;

    let (forge, _) = build_forge(dir, config);
    let snapshot = forge.fetch_snapshot()?;
    let pr = snapshot.prs.iter().find(|p| p.id == pr_id).ok_or_else(|| {
        anyhow::anyhow!(
            "PR {} not present in the {} snapshot — the adapter must keep merged PRs visible",
            pr_id.0,
            forge.describe()
        )
    })?;

    let checks = tuesday_checks(record, pr);
    let pass = checks.iter().all(|c| c.pass);
    match output {
        OutputFormat::Json => {
            let report = serde_json::json!({
                "task": record.id,
                "pr": pr_id.0,
                "checks": checks,
                "pass": pass,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        OutputFormat::Human => {
            println!("verify {} (PR {}):", record.id, pr_id.0);
            for c in &checks {
                println!(
                    "  {} {:<24} {}",
                    if c.pass { "PASS" } else { "FAIL" },
                    c.name,
                    c.detail
                );
            }
        }
    }
    if !pass {
        let failed = checks.iter().filter(|c| !c.pass).count();
        anyhow::bail!(
            "tuesday contract violated: {failed} of {} checks failed",
            checks.len()
        );
    }
    Ok(())
}

/// Pure: the six contract assertions against the live PR snapshot
/// (spec §The tuesday contract table). Exhaustively unit-tested below.
pub fn tuesday_checks(record: &TaskRecord, pr: &PrSnapshot) -> Vec<Check> {
    let mut checks = Vec::new();

    // ^\[ADR-\d{4}\] ⁠ on the PR title.
    checks.push(Check {
        name: contract::CHECK_TITLE_PREFIX,
        pass: title_has_adr_prefix(&pr.title),
        detail: format!("title {:?} (want ^\\[ADR-dddd\\] )", pr.title),
    });

    // The trailer is the FINAL line of the PR body.
    let trailer = contract::body_trailer(&record.adr_reference);
    let last_line = pr.body.trim_end().lines().last().unwrap_or_default();
    checks.push(Check {
        name: contract::CHECK_TRAILER_FINAL_LINE,
        pass: last_line == trailer,
        detail: format!("final body line {last_line:?} (want {trailer:?})"),
    });

    // Exactly ONE effort:* label, and from the closed set.
    let efforts: Vec<&String> = pr
        .labels
        .iter()
        .filter(|l| l.starts_with("effort:"))
        .collect();
    checks.push(Check {
        name: contract::CHECK_EXACTLY_ONE_EFFORT,
        pass: efforts.len() == 1 && contract::EFFORT_LABELS.contains(&efforts[0].as_str()),
        detail: format!("effort labels {efforts:?} (want exactly one from the closed set)"),
    });

    // adr:<reference> label present.
    let adr_label = contract::adr_label(&record.adr_reference);
    checks.push(Check {
        name: contract::CHECK_ADR_LABEL_PRESENT,
        pass: pr.labels.contains(&adr_label),
        detail: format!("labels {:?} (want {adr_label:?})", pr.labels),
    });

    // ^conduit/adr-\d{4}/[a-z0-9-]+$ on the head branch.
    checks.push(Check {
        name: contract::CHECK_BRANCH_SHAPE,
        pass: branch_is_conduit_shaped(&pr.head_branch),
        detail: format!(
            "head branch {:?} (want conduit/adr-dddd/<slug>)",
            pr.head_branch
        ),
    });

    // Never adroit's namespace.
    checks.push(Check {
        name: contract::CHECK_NEVER_ADR_NAMESPACE,
        pass: !pr.head_branch.starts_with("adr/"),
        detail: format!("head branch {:?} (must never start adr/)", pr.head_branch),
    });

    checks
}

/// `^\[ADR-\d{4}\] ` — hand-rolled (the crate carries no regex dependency).
fn title_has_adr_prefix(title: &str) -> bool {
    let Some(rest) = title.strip_prefix("[ADR-") else {
        return false;
    };
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    digits == 4 && rest[digits..].starts_with("] ")
}

/// `^conduit/adr-\d{4}/[a-z0-9-]+$` — hand-rolled, same reason.
fn branch_is_conduit_shaped(branch: &str) -> bool {
    let Some(rest) = branch.strip_prefix("conduit/adr-") else {
        return false;
    };
    let Some((digits, slug)) = rest.split_once('/') else {
        return false;
    };
    digits.len() == 4
        && digits.chars().all(|c| c.is_ascii_digit())
        && !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

// ── demo-transcript ────────────────────────────────────────────────────────

/// `conduit demo-transcript <address>` (spec §Transcript-diff semantics): no
/// polling — the fixture scenario through the real machine, actions emitted
/// through the chosen adapter, normalized JSONL on stdout. The gitea leg
/// EXECUTES (live adapter + real git against the throwaway forge); the
/// github leg is DryRun + no git, transcript-only by construction.
fn cmd_demo_transcript(dir: &Path, config: &Config, address: &str) -> anyhow::Result<()> {
    let reference = reference_from_address(address)?;
    let lines = match config.forge.default {
        ForgeKind::Gitea => {
            let token = Config::gitea_token(dir).unwrap_or_default();
            let forge = crate::forge::gitea::GiteaForge::open(&config.forge.gitea, token);
            let store = Store::open(dir.join(".conduit"))?;
            let git = crate::transcript::GitContext {
                remote_url: forge.git_remote_url()?,
                cache_dir: store.root().join("cache").join("gitea.git"),
                workspace_root: store.root().join("workspaces"),
                base_branch: "main".to_string(),
            };
            let slug = repo_slug(&config.forge.gitea.owner, &config.forge.gitea.repo);
            crate::transcript::run(&forge, slug, &reference, address, config, Some(&git))?
        }
        ForgeKind::Github => {
            let token = crate::forge::github::resolve_token().unwrap_or_default();
            let forge = crate::forge::github::open_github(&config.forge.github, token);
            let slug = repo_slug(&config.forge.github.owner, &config.forge.github.repo);
            crate::transcript::run(&forge, slug, &reference, address, config, None)?
        }
    };
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

/// `{owner}/{repo}` for transcript redaction — None when either half is
/// unconfigured (a "/" slug would literal-replace every slash in bodies).
fn repo_slug(owner: &str, repo: &str) -> Option<String> {
    if owner.is_empty() || repo.is_empty() {
        None
    } else {
        Some(format!("{owner}/{repo}"))
    }
}

/// Display reference for the fixture scenario: a bare number zero-pads to
/// adroit's reference shape (`3` → `ADR-0003`); an explicit reference passes
/// through. No adroit call — the transcript is a fixture, not a real plan.
fn reference_from_address(address: &str) -> anyhow::Result<String> {
    if let Ok(n) = address.parse::<u64>() {
        return Ok(format!("ADR-{n:04}"));
    }
    let upper = address.to_ascii_uppercase();
    if upper.strip_prefix("ADR-").is_some_and(|d| !d.is_empty()) {
        return Ok(upper);
    }
    anyhow::bail!(
        "demo-transcript address must be an ADR number (e.g. 3) or reference (e.g. ADR-0003), got {address:?}"
    )
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::CiState;
    use crate::task::PrId;

    /// A PR snapshot that satisfies every contract element.
    fn conforming_pr() -> PrSnapshot {
        PrSnapshot {
            id: PrId(7),
            title: "[ADR-0003] Adopt snapshot-diff router".into(),
            body: contract::pr_body("ADR-0003", "Implements the accepted decision."),
            head_branch: "conduit/adr-0003/adopt-snapshot-diff-router".into(),
            labels: vec!["effort:1-super-quick".into(), "adr:ADR-0003".into()],
            reviews: vec![],
            ci: CiState::None,
            merged: true,
            merge_sha: Some("cafe42".into()),
            closed: true,
        }
    }

    fn record() -> TaskRecord {
        TaskRecord::new("ADR-0003", "3", "Adopt snapshot-diff router", "sha")
    }

    #[test]
    fn tuesday_checks_names_are_fixed_and_a_conforming_pr_passes_all() {
        let checks = tuesday_checks(&record(), &conforming_pr());
        let names: Vec<&str> = checks.iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            vec![
                contract::CHECK_TITLE_PREFIX,
                contract::CHECK_TRAILER_FINAL_LINE,
                contract::CHECK_EXACTLY_ONE_EFFORT,
                contract::CHECK_ADR_LABEL_PRESENT,
                contract::CHECK_BRANCH_SHAPE,
                contract::CHECK_NEVER_ADR_NAMESPACE,
            ]
        );
        assert!(
            checks.iter().all(|c| c.pass),
            "conforming PR must pass: {checks:?}"
        );
    }

    #[test]
    fn each_contract_violation_fails_its_named_check() {
        let fail_of = |pr: PrSnapshot| -> Vec<&'static str> {
            tuesday_checks(&record(), &pr)
                .into_iter()
                .filter(|c| !c.pass)
                .map(|c| c.name)
                .collect()
        };

        let mut pr = conforming_pr();
        pr.title = "ADR-0003: no bracket".into();
        assert_eq!(fail_of(pr), vec![contract::CHECK_TITLE_PREFIX]);

        let mut pr = conforming_pr();
        pr.title = "[ADR-003] three digits".into();
        assert_eq!(fail_of(pr), vec![contract::CHECK_TITLE_PREFIX]);

        let mut pr = conforming_pr();
        pr.body = format!("{}\n\ntrailing prose", pr.body);
        assert_eq!(fail_of(pr), vec![contract::CHECK_TRAILER_FINAL_LINE]);

        let mut pr = conforming_pr();
        pr.labels = vec!["adr:ADR-0003".into()]; // zero effort labels
        assert_eq!(fail_of(pr), vec![contract::CHECK_EXACTLY_ONE_EFFORT]);

        let mut pr = conforming_pr();
        pr.labels = vec![
            "effort:1-super-quick".into(),
            "effort:3-average".into(),
            "adr:ADR-0003".into(),
        ];
        assert_eq!(fail_of(pr), vec![contract::CHECK_EXACTLY_ONE_EFFORT]);

        let mut pr = conforming_pr();
        pr.labels = vec!["effort:9-bogus".into(), "adr:ADR-0003".into()];
        assert_eq!(
            fail_of(pr),
            vec![contract::CHECK_EXACTLY_ONE_EFFORT],
            "an effort label outside the closed set must fail"
        );

        let mut pr = conforming_pr();
        pr.labels = vec!["effort:1-super-quick".into()];
        assert_eq!(fail_of(pr), vec![contract::CHECK_ADR_LABEL_PRESENT]);

        let mut pr = conforming_pr();
        pr.head_branch = "conduit/adr-3/short".into();
        assert_eq!(fail_of(pr), vec![contract::CHECK_BRANCH_SHAPE]);

        let mut pr = conforming_pr();
        pr.head_branch = "adr/0003-sneaky".into();
        assert_eq!(
            fail_of(pr),
            vec![
                contract::CHECK_BRANCH_SHAPE,
                contract::CHECK_NEVER_ADR_NAMESPACE
            ]
        );
    }

    /// GAP C: verify-report assembly — the pure path.
    ///
    /// `tuesday_checks` is the pure core of `cmd_verify`; this test asserts
    /// that ANY single violation makes the overall pass=false, confirming the
    /// `checks.iter().all(|c| c.pass)` aggregation that drives the non-zero
    /// exit. Full CLI-level forge-backed verify (fetch_snapshot + PR lookup) is
    /// exercised in Task 14 of the demo transcript; the CLI-level non-zero exit
    /// for store-missing / unmerged tasks is covered in tests/cli.rs.
    #[test]
    fn tuesday_checks_violation_yields_pass_false() {
        let mut pr = conforming_pr();
        pr.title = "no bracket prefix".into(); // violates title_prefix
        let checks = tuesday_checks(&record(), &pr);
        assert!(
            !checks.iter().all(|c| c.pass),
            "at least one check must fail for a violating PR"
        );
        let failed: Vec<&str> = checks.iter().filter(|c| !c.pass).map(|c| c.name).collect();
        assert!(
            failed.contains(&contract::CHECK_TITLE_PREFIX),
            "title_prefix violation must fail: {failed:?}"
        );
    }

    #[test]
    fn trailer_check_tolerates_trailing_newlines_only() {
        let mut pr = conforming_pr();
        pr.body.push_str("\n\n");
        let checks = tuesday_checks(&record(), &pr);
        assert!(
            checks
                .iter()
                .find(|c| c.name == contract::CHECK_TRAILER_FINAL_LINE)
                .unwrap()
                .pass,
            "forge-appended trailing whitespace must not fail the trailer"
        );
    }

    #[test]
    fn branch_shape_accepts_the_builder_output_only() {
        assert!(branch_is_conduit_shaped(&contract::branch_name(
            "ADR-0042",
            "Some Decision Title"
        )));
        for bad in [
            "conduit/adr-0003/",      // empty slug
            "conduit/adr-0003/UPPER", // case
            "conduit/adr-0003/a/b",   // extra segment
            "conduit/adr-12345/x",    // five digits
            "feature/adr-0003/x",     // wrong root
            "adr/0003/x",             // adroit namespace
        ] {
            assert!(!branch_is_conduit_shaped(bad), "{bad} must fail");
        }
    }

    #[test]
    fn reference_from_address_pads_numbers_and_passes_references() {
        assert_eq!(reference_from_address("3").unwrap(), "ADR-0003");
        assert_eq!(reference_from_address("42").unwrap(), "ADR-0042");
        assert_eq!(reference_from_address("ADR-0003").unwrap(), "ADR-0003");
        assert_eq!(reference_from_address("adr-0003").unwrap(), "ADR-0003");
        assert!(reference_from_address("bogus").is_err());
        assert!(reference_from_address("ADR-").is_err());
    }
}
