use crate::Result;
use clap::Parser;

mod daemon;
mod run;
mod start;
mod logs;

#[derive(Debug, clap::Parser)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    Daemon(daemon::Daemon),
    Logs(logs::Logs),
    Run(run::Run),
    Start(start::Start),
}

#[tokio::main]
pub async fn run() -> Result<()> {
    let args = Cli::parse();
    match args.command {
        Commands::Daemon(daemon) => daemon.run().await,
        Commands::Logs(logs) => logs.run().await,
        Commands::Run(run) => run.run().await,
        Commands::Start(start) => start.run().await,
    }
}
