use crate::Result;
use clap::Parser;

mod activate;
mod boot;
mod cd;
mod clean;
mod completion;
mod config;
mod disable;
mod enable;
mod list;
pub mod logs;
mod run;
mod start;
mod status;
mod stop;
mod supervisor;
mod usage;
mod wait;

#[derive(Debug, clap::Parser)]
#[clap(name = "pitchfork", version = env!("CARGO_PKG_VERSION"), about = env!("CARGO_PKG_DESCRIPTION"))]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    Activate(activate::Activate),
    Boot(boot::Boot),
    Cd(cd::Cd),
    Clean(clean::Clean),
    Config(config::Config),
    Completion(completion::Completion),
    Disable(disable::Disable),
    Enable(enable::Enable),
    List(list::List),
    Logs(logs::Logs),
    Run(run::Run),
    Start(start::Start),
    Status(status::Status),
    Stop(stop::Stop),
    Supervisor(supervisor::Supervisor),
    Usage(usage::Usage),
    Wait(wait::Wait),
}

pub async fn run() -> Result<()> {
    let args = Cli::parse();
    match args.command {
        Commands::Activate(activate) => activate.run().await,
        Commands::Boot(boot) => boot.run().await,
        Commands::Cd(cd) => cd.run().await,
        Commands::Clean(clean) => clean.run().await,
        Commands::Config(config) => config.run().await,
        Commands::Completion(completion) => completion.run().await,
        Commands::Disable(disable) => disable.run().await,
        Commands::Enable(enable) => enable.run().await,
        Commands::List(list) => list.run().await,
        Commands::Logs(logs) => logs.run().await,
        Commands::Run(run) => run.run().await,
        Commands::Start(start) => start.run().await,
        Commands::Status(status) => status.run().await,
        Commands::Stop(stop) => stop.run().await,
        Commands::Supervisor(supervisor) => supervisor.run().await,
        Commands::Usage(usage) => usage.run().await,
        Commands::Wait(wait) => wait.run().await,
    }
}
