use crate::procs::PROCS;
use crate::Result;

mod run;
mod start;
mod status;
mod stop;

/// Start, stop, and check the status of the pitchfork supervisor daemon
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "sup", verbatim_doc_comment)]
pub struct Supervisor {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    Run(run::Run),
    Start(start::Start),
    Status(status::Status),
    Stop(stop::Stop),
}

impl Supervisor {
    pub async fn run(self) -> Result<()> {
        match self.command {
            Commands::Run(run) => run.run().await,
            Commands::Start(start) => start.run().await,
            Commands::Status(status) => status.run().await,
            Commands::Stop(stop) => stop.run().await,
        }
    }
}

/// if --force is passed, will kill existing process
/// Returns false if existing pid is running and --force was not passed (so we should cancel starting the daemon)
pub async fn kill_or_stop(existing_pid: u32, force: bool) -> Result<bool> {
    if PROCS.is_running(existing_pid) {
        if force {
            debug!("killing pid {existing_pid}");
            PROCS.kill_async(existing_pid).await?;
            Ok(true)
        } else {
            warn!("pitchfork supervisor is already running with pid {existing_pid}. Kill it with `--force`");
            Ok(false)
        }
    } else {
        Ok(true)
    }
}
