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
    // The state file may be unreadable; the supervisor's own timeout is then
    // all that is waited, as it always was.
    let base = crate::settings::settings().supervisor_stop_timeout();
    let stop_budget = match StateFile::read(&*env::PITCHFORK_STATE_FILE) {
        Ok(sf) => supervisor_stop_budget(sf.daemons.values(), base),
        Err(_) => base,
    };
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
            Some(stop_budget),
        )
        .await;
    match killed {
        Ok(true) => Ok(KillOrStopOutcome::Killed),
        Ok(false) => Ok(KillOrStopOutcome::AlreadyDead),
        Err(e) => Err(miette::miette!("{e}. Try rerun with sudo.")),
    }
}

/// How long to wait for the supervisor to exit after the stop signal before
/// killing it.
///
/// On the stop signal the supervisor stops its daemons, dependents first and
/// one level after another, and each daemon can take its whole stop timeout
/// plus the wait after SIGKILL. Killed before it is done, it leaves the
/// daemons of the later levels running with no supervisor. So `base` (the
/// `supervisor.stop_timeout` setting) is extended by every running daemon's
/// stop in turn: an upper bound, since the daemons of one level stop at the
/// same time. A supervisor that exits sooner ends the wait sooner.
fn supervisor_stop_budget<'a>(
    daemons: impl IntoIterator<Item = &'a Daemon>,
    base: std::time::Duration,
) -> std::time::Duration {
    let pitchfork_id = DaemonId::pitchfork();
    daemons
        .into_iter()
        .filter(|d| d.pid.is_some() && d.id != pitchfork_id)
        .map(|d| {
            // A daemon without its own timeout is given the supervisor's.
            let timeout = d.stop_signal.and_then(|s| s.timeout).unwrap_or(base);
            timeout + crate::procs::PROCESS_GROUP_SIGKILL_WAIT
        })
        .fold(base, |total, stop| total.saturating_add(stop))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_types::StopConfig;
    use std::time::Duration;

    fn daemon(name: &str, pid: Option<u32>, timeout: Option<Duration>) -> Daemon {
        Daemon {
            id: DaemonId::new("proj", name),
            pid,
            stop_signal: timeout.map(|timeout| StopConfig {
                timeout: Some(timeout),
                ..StopConfig::default()
            }),
            ..Daemon::default()
        }
    }

    #[test]
    fn stop_budget_covers_each_running_daemon_in_turn() {
        let base = Duration::from_secs(5);
        let sigkill = crate::procs::PROCESS_GROUP_SIGKILL_WAIT;
        let daemons = [
            daemon("db", Some(1), Some(Duration::from_secs(4))),
            daemon("app", Some(2), Some(Duration::from_secs(4))),
            // No timeout of its own: the supervisor's applies.
            daemon("worker", Some(3), None),
            // Not running: nothing to stop.
            daemon("idle", None, Some(Duration::from_secs(60))),
            Daemon {
                id: DaemonId::pitchfork(),
                pid: Some(4),
                ..Daemon::default()
            },
        ];
        assert_eq!(
            supervisor_stop_budget(&daemons, base),
            base + (Duration::from_secs(4) + sigkill) * 2 + (base + sigkill)
        );
    }

    #[test]
    fn stop_budget_is_the_base_with_no_running_daemons() {
        let base = Duration::from_secs(5);
        assert_eq!(supervisor_stop_budget(&[], base), base);
    }
}
