use crate::Result;
use crate::daemon::Daemon;
use crate::daemon_id::DaemonId;
use crate::env;
use crate::pitchfork_toml::StopSignal;
use crate::procs::PROCS;
use crate::state_file::StateFile;
use crate::supervisor::supervisor_record_is_live;

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
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
pub struct Supervisor {
    #[usage(subcommand)]
    command: Commands,
}

#[derive(Debug, usage_rs::Subcommands)]
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

/// If `force` is true, kills the existing supervisor process.
/// Returns `KillOrStopOutcome::StillRunning` when the supervisor is alive and `force` is false.
///
/// `record` is the supervisor's own entry from the state file. Its PID is only
/// acted on when the live process still matches the identity the supervisor
/// recorded about itself (see [`supervisor_record_is_live`]): the entry
/// outlives crashes and reboots, and a PID recycled to an unrelated process
/// must be reported as `AlreadyDead` so callers clear the stale record
/// instead of signalling a stranger (jdx/pitchfork discussion #877).
///
/// This is a low-level helper — callers are responsible for user-facing messages.
pub async fn kill_or_stop(record: &Daemon, force: bool) -> Result<KillOrStopOutcome> {
    let Some(existing_pid) = record.pid else {
        return Ok(KillOrStopOutcome::AlreadyDead);
    };
    if !supervisor_record_is_live(record) {
        return Ok(KillOrStopOutcome::AlreadyDead);
    }
    if !force {
        return Ok(KillOrStopOutcome::StillRunning);
    }
    debug!("killing pid {existing_pid}");
    let stop_signal: i32 = StopSignal::default().into();
    let killed = match record.start_time {
        // Bind the kill to the recorded process generation so a PID recycled
        // between the check above and the signal is still refused.
        Some(start_time) => {
            PROCS
                .kill_if_start_time_matches_async(existing_pid, Some(start_time), stop_signal, None)
                .await
        }
        // A record written by a supervisor that predates start-time tracking
        // has nothing to bind to; it was already accepted as live above (same
        // boot, if known), so stop it the way it always was.
        None => PROCS.kill_async(existing_pid, stop_signal, None).await,
    };
    match killed {
        Ok(true) => Ok(KillOrStopOutcome::Killed),
        Ok(false) => Ok(KillOrStopOutcome::AlreadyDead),
        Err(e) => Err(miette::miette!("{e}. Try rerun with sudo.")),
    }
}

/// The supervisor's own entry in the state file, if any. The entry may be
/// stale: use [`supervisor_record_is_live`] before trusting its PID.
pub fn existing_supervisor() -> Result<Option<Daemon>> {
    let sf = StateFile::read(&*env::PITCHFORK_STATE_FILE)?;
    Ok(sf.daemons.get(&DaemonId::pitchfork()).cloned())
}

pub async fn resolve_existing_supervisor(force: bool) -> Result<(Option<u32>, KillOrStopOutcome)> {
    let record = existing_supervisor()?;
    let existing_pid = record.as_ref().and_then(|d| d.pid);
    let outcome = if let Some(record) = &record {
        kill_or_stop(record, force).await?
    } else {
        KillOrStopOutcome::AlreadyDead
    };
    Ok((existing_pid, outcome))
}
