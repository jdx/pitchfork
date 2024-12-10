use crate::Result;
use clap::Parser;

mod activate;
mod add;
mod cd;
mod clean;
mod completion;
mod disable;
mod enable;
mod list;
mod logs;
mod remove;
mod run;
mod start;
mod status;
mod stop;
mod supervisor;
mod usage;
mod wait;

#[derive(Debug, clap::Parser)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    Activate(activate::Activate),
    Add(add::Add),
    Cd(cd::Cd),
    Clean(clean::Clean),
    Completion(completion::Completion),
    Disable(disable::Disable),
    Enable(enable::Enable),
    List(list::List),
    Logs(logs::Logs),
    Remove(remove::Remove),
    Run(run::Run),
    Start(start::Start),
    Status(status::Status),
    Stop(stop::Stop),
    Supervisor(supervisor::Supervisor),
    Usage(usage::Usage),
    Wait(wait::Wait),
}

#[tokio::main]
pub async fn run() -> Result<()> {
    let args = Cli::parse();
    match args.command {
        Commands::Activate(activate) => activate.run().await,
        Commands::Add(add) => add.run().await,
        Commands::Cd(cd) => cd.run().await,
        Commands::Clean(clean) => clean.run().await,
        Commands::Completion(completion) => completion.run().await,
        Commands::Disable(disable) => disable.run().await,
        Commands::Enable(enable) => enable.run().await,
        Commands::List(list) => list.run().await,
        Commands::Logs(logs) => logs.run().await,
        Commands::Remove(remove) => remove.run().await,
        Commands::Run(run) => run.run().await,
        Commands::Start(start) => start.run().await,
        Commands::Status(status) => status.run().await,
        Commands::Stop(stop) => stop.run().await,
        Commands::Supervisor(supervisor) => supervisor.run().await,
        Commands::Usage(usage) => usage.run().await,
        Commands::Wait(wait) => wait.run().await,
    }
}
