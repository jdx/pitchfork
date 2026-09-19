use crate::Result;
use crate::daemon::Daemon;
use crate::daemon_id::DaemonId;
use crate::env;
use crate::ipc::client::IpcClient;
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
    /// A supervisor is listening on the IPC socket, but the state file does
    /// not identify its process (and it did not restore its record when
    /// connected to, as supervisors older than this check do not), so it can
    /// be neither signalled nor safely started beside.
    Unidentified,
}

/// The error for acting on a supervisor that is running but that the state
/// file does not identify (see [`KillOrStopOutcome::Unidentified`]).
pub fn unidentified_supervisor_error() -> miette::Report {
    miette::miette!(
        "a pitchfork supervisor is listening on {}, but the state file does not record its pid, \
         so it cannot be stopped or replaced automatically. Stop its `pitchfork supervisor run` \
         process manually.",
        crate::ipc::socket_display()
    )
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
    // Bind the kill to a process generation so a PID recycled between the
    // check above and the signal is still refused. A legacy record without a
    // start time was just verified to be a live pitchfork process, so bind to
    // the generation observed now.
    let Some(expected_start_time) = record.start_time.or_else(|| PROCS.start_time(existing_pid))
    else {
        // The process is alive (the check above said so) but its identity
        // cannot be read, so the kill cannot be bound. That is a failure to
        // report, not proof of death: mapping it to `AlreadyDead` would have
        // `stop` drop the record of a supervisor that keeps running, and a
        // forced `start`/`run` launch a second one beside it.
        return Err(miette::miette!(
            "cannot verify the identity of supervisor pid {existing_pid}: its start time is unreadable; not signalling it. Try rerun with sudo."
        ));
    };
    let killed = PROCS
        .kill_if_start_time_matches_async(
            existing_pid,
            Some(expected_start_time),
            stop_signal,
            None,
        )
        .await;
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

/// Find the running supervisor and, if `force` is true, kill it.
///
/// The state-file record is the only way to identify the supervisor's
/// process, but it can be lost (e.g. state.toml replaced) while the
/// supervisor keeps running. So when the record is missing or stale, the IPC
/// socket decides: if a supervisor answers there, connecting to it makes it
/// restore its record, which is then read again.
pub async fn resolve_existing_supervisor(force: bool) -> Result<(Option<u32>, KillOrStopOutcome)> {
    let mut record = existing_supervisor()?;
    if !record.as_ref().is_some_and(supervisor_record_is_live) {
        if !crate::ipc::supervisor_listening() {
            let existing_pid = record.and_then(|d| d.pid);
            return Ok((existing_pid, KillOrStopOutcome::AlreadyDead));
        }
        debug!("supervisor is listening on the IPC socket but not recorded; asking it to restore");
        if let Err(err) = IpcClient::connect(false).await {
            debug!("failed to connect to the unrecorded supervisor: {err:?}");
        }
        record = existing_supervisor()?;
        if !record.as_ref().is_some_and(supervisor_record_is_live) {
            return Ok((None, KillOrStopOutcome::Unidentified));
        }
    }
    let record = record.expect("a live record was found above");
    let outcome = kill_or_stop(&record, force).await?;
    Ok((record.pid, outcome))
}
