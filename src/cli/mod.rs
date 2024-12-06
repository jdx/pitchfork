use clap::Parser;
use crate::Result;

mod daemon;
mod start;

#[derive(Debug, clap::Parser)]
struct Cli {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    Daemon(daemon::Daemon),
    Start(start::Start),
}

#[tokio::main]
pub async fn run() -> Result<()> {
    let args = Cli::parse();
    match args.command {
        Command::Daemon(daemon) => daemon.run().await,
        Command::Start(start) => start.run().await,
    }
}
