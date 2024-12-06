use clap::Parser;
use crate::Result;

mod start;
mod daemon;

#[derive(Debug, clap::Parser)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    Daemon(daemon::Daemon),
    Start(start::Start),
}

#[tokio::main]
pub async fn run() -> Result<()> {
    let args = Cli::parse();
    match args.command {
        Commands::Daemon(daemon) => daemon.run().await,
        Commands::Start(start) => start.run().await,
    }
}
