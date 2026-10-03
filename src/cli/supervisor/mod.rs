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
    debug!("stopping pid {existing_pid}");
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
    // Asked over IPC, the supervisor freezes new starts and then reports how
    // long its shutdown can take, from its own settings and the final list of
    // daemons it will stop. Neither is known here: settings can differ
    // between processes, and a daemon can start until the starts are frozen.
    #[cfg(unix)]
    if let Some(budget) = request_shutdown().await {
        return wait_for_exit_or_kill(record, existing_pid, expected_start_time, budget).await;
    }
    // An older supervisor, or Windows (where the stop is a forced kill of the
    // supervisor's process tree): send the stop signal, and wait as long as
    // can be worked out here.
    let stop_signal: i32 = StopSignal::default().into();
    let base = crate::settings::settings().supervisor_stop_timeout();
    // The supervisor's own list is current; the state file can lag it, missing
    // a daemon started a moment ago. If neither can be read, the supervisor's
    // own timeout is all that is waited, as it always was.
    let daemons = match live_daemons().await {
        Some(daemons) => daemons,
        None => StateFile::read(&*env::PITCHFORK_STATE_FILE)
            .map(|sf| sf.daemons.into_values().collect())
            .unwrap_or_default(),
    };
    // Shutdown stops the proxy and DNS resolver before any daemon.
    let stop_budget = crate::supervisor::daemons_stop_budget(&daemons, base)
        .saturating_add(crate::supervisor::SHUTDOWN_PRE_STOP_BUDGET);
    let killed = PROCS
        .kill_if_start_time_matches_async(
            existing_pid,
            Some(expected_start_time),
            stop_signal,
            Some(stop_budget),
        )
        .await;
    match killed {
        Ok(true) => Ok(KillOrStopOutcome::Killed),
        Ok(false) => Ok(KillOrStopOutcome::AlreadyDead),
        Err(e) => Err(miette::miette!("{e}. Try rerun with sudo.")),
    }
}

/// Ask the running supervisor to shut down over IPC, without starting one.
/// `None` if it does not answer in time or does not know the request, so the
/// caller falls back to the stop signal.
#[cfg(unix)]
async fn request_shutdown() -> Option<std::time::Duration> {
    let ask = async {
        let client = IpcClient::connect(false).await.ok()?;
        client.shutdown().await.ok().flatten()
    };
    // Freezing new starts waits for those already under way.
    tokio::time::timeout(std::time::Duration::from_secs(30), ask)
        .await
        .ok()
        .flatten()
}

/// Wait up to `budget` for the supervisor to exit after a `Shutdown` request,
/// then kill it. No stop signal is sent: the supervisor would take it as a
/// second one and force its exit.
#[cfg(unix)]
async fn wait_for_exit_or_kill(
    record: &Daemon,
    pid: u32,
    expected_start_time: u64,
    budget: std::time::Duration,
) -> Result<KillOrStopOutcome> {
    let deadline = tokio::time::Instant::now() + budget;
    while tokio::time::Instant::now() < deadline {
        if !supervisor_record_is_live(record) {
            return Ok(KillOrStopOutcome::Killed);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    warn!(
        "supervisor pid {pid} did not exit within {}ms of the stop request, sending SIGKILL",
        budget.as_millis()
    );
    PROCS
        .kill_if_start_time_matches_async(
            pid,
            Some(expected_start_time),
            libc::SIGKILL,
            Some(std::time::Duration::from_secs(1)),
        )
        .await
        .map(|_| KillOrStopOutcome::Killed)
        .map_err(|e| miette::miette!("{e}. Try rerun with sudo."))
}

/// The running supervisor's active daemons, asked for over IPC without
/// starting a supervisor. `None` if it does not answer in time.
async fn live_daemons() -> Option<Vec<Daemon>> {
    let ask = async {
        let client = IpcClient::connect(false).await.ok()?;
        client.active_daemons().await.ok()
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), ask)
        .await
        .ok()
        .flatten()
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
        if !crate::ipc::supervisor_listening().await {
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
