use crate::Result;
use clap::Parser;

mod logs;
mod run;
mod start;
mod supervisor;
mod usage;

#[derive(Debug, clap::Parser)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    Supervisor(supervisor::Supervisor),
    Logs(logs::Logs),
    Run(run::Run),
    Start(start::Start),
    Usage(usage::Usage),
}

#[tokio::main]
pub async fn run() -> Result<()> {
    let args = Cli::parse();
    match args.command {
        Commands::Supervisor(supervisor) => supervisor.run().await,
        Commands::Logs(logs) => logs.run().await,
        Commands::Run(run) => run.run().await,
        Commands::Start(start) => start.run().await,
        Commands::Usage(usage) => usage.run().await,
    }
}
