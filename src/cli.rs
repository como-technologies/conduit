//! CLI surface (spec §Module layout):
//! init | plan <address> | run [--once] | status | verify <address> | demo-transcript <address>
//! Globals: --forge <gitea|github>, -o/--output <human|json>.

use clap::{Parser, Subcommand, ValueEnum};

use crate::config::ForgeKind;

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
        Command::Init => anyhow::bail!("not implemented yet: wired in a later task"),
        Command::Plan { .. } => anyhow::bail!("not implemented yet: wired in a later task"),
        Command::Run { .. } => anyhow::bail!("not implemented yet: wired in a later task"),
        Command::Verify { .. } => anyhow::bail!("not implemented yet: wired in a later task"),
        Command::DemoTranscript { .. } => {
            anyhow::bail!("not implemented yet: wired in a later task")
        }
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
