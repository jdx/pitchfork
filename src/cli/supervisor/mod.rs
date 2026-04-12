use crate::Result;
use crate::daemon_id::DaemonId;
use crate::env;
use crate::procs::PROCS;
use crate::state_file::StateFile;

mod run;
mod start;
mod status;
mod stop;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillOrStopOutcome {
    /// Process was actively killed.
    Killed,
    /// PID was in the state file but the process was already dead.
    AlreadyDead,
    /// Existing process is running and --force was not passed.
    StillRunning,
}

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

/// If `force` is true, kills the existing process.
/// Returns `KillOrStopOutcome::StillRunning` when the process is alive and `force` is false.
///
/// This is a low-level helper — callers are responsible for user-facing messages.
pub async fn kill_or_stop(existing_pid: u32, force: bool) -> Result<KillOrStopOutcome> {
    if PROCS.is_running(existing_pid) {
        if force {
            debug!("killing pid {existing_pid}");
            match PROCS.kill_async(existing_pid).await {
                Ok(true) => Ok(KillOrStopOutcome::Killed),
                Ok(false) => Ok(KillOrStopOutcome::AlreadyDead),
                Err(e) => Err(miette::miette!("{e}. Try rerun with sudo.")),
            }
        } else {
            Ok(KillOrStopOutcome::StillRunning)
        }
    } else {
        Ok(KillOrStopOutcome::AlreadyDead)
    }
}

pub fn existing_supervisor_pid() -> Result<Option<u32>> {
    let sf = StateFile::read(&*env::PITCHFORK_STATE_FILE)?;
    Ok(sf
        .daemons
        .get(&DaemonId::pitchfork())
        .and_then(|daemon| daemon.pid))
}

pub async fn resolve_existing_supervisor(force: bool) -> Result<(Option<u32>, KillOrStopOutcome)> {
    let existing_pid = existing_supervisor_pid()?;
    let outcome = if let Some(pid) = existing_pid {
        kill_or_stop(pid, force).await?
    } else {
        KillOrStopOutcome::AlreadyDead
    };
    Ok((existing_pid, outcome))
}
