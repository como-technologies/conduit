use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = conduit::cli::Cli::parse();
    conduit::cli::dispatch(cli)
}
