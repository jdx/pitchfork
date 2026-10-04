use crate::Result;
use crate::cli::logs;
use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::env;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use crate::procs::PROCS;
use crate::settings::settings;
use crate::state_file::StateFile;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time;

#[cfg(windows)]
use tokio::signal;
#[cfg(unix)]
use tokio::signal::unix::{self, SignalKind};

/// Wait for daemons to stop, tailing the logs along the way
///
/// Exits 0 only when every daemon stopped cleanly; otherwise the exit
/// code of the first failing daemon (in the order given) is propagated
#[derive(Debug, usage_rs::Args)]
#[usage(
    verbatim_doc_comment,
    long_about = "\
Wait for one or more daemons to stop, tailing the logs along the way

Blocks until every specified daemon stops running, while displaying its
log output in real-time. Already-finished daemons are evaluated without
waiting; their exit codes still count. With no daemon IDs and no
`--group`, shows an interactive picker of the currently running daemons.

A daemon the supervisor restarts (because a watched file changed, or
through `pitchfork restart` / `start --force`) shows as `restarting` in
between, and waiting follows it to the new process instead of returning.
With `--exit-on-restart`, waiting ends when the daemon is stopped for a
restart, and the restart counts as a clean stop (exit 0).

With `--kill`, an incoming signal (SIGINT/SIGTERM/SIGHUP/SIGQUIT, or Ctrl-C
on Windows) first stops the waited daemons via the supervisor (graceful
SIGTERM then SIGKILL, hooks fire, reverse dependency order), then the
command exits with 128 + the signal number like the shell, so Ctrl-C
yields 130.

Exit code: 0 when every waited daemon stopped cleanly. Otherwise the exit
code of the first failing daemon (in the order given) is propagated;
unknown exit codes, failed daemons, and missing statuses map to 1.

Useful in scripts that need to wait for daemons to complete.

Examples:

    pitchfork wait api              Wait for 'api' to stop, exit with its status
    pitchfork wait api worker       Wait for 'api' and 'worker' to stop
    pitchfork wait --group backend  Wait for the whole 'backend' group
    pitchfork wait --kill api       Stop 'api' gracefully when a signal arrives
    pitchfork wait --exit-on-restart api
                                    Return when 'api' is restarted
    pitchfork w api                 Alias for 'wait'
    pitchfork wait api && echo done Run command after the daemon stops"
)]
pub struct Wait {
    /// The name of the daemon(s) to wait for
    id: Vec<String>,
    /// Wait for all daemons in the named group
    #[usage(long, value_name = "GROUP")]
    group: Option<String>,
    /// Stop the waited daemons when a signal is received while waiting
    #[usage(long)]
    kill: bool,
    /// Stop waiting when a daemon is restarted instead of following it to
    /// its new process
    #[usage(long)]
    exit_on_restart: bool,
}

impl Wait {
    pub async fn run(&self) -> Result<()> {
        let no_target = self.id.is_empty() && self.group.is_none();

        let ids: Vec<DaemonId> = if no_target {
            // Check for a TTY before connecting, so a non-interactive
            // `pitchfork wait` without IDs fails without auto-starting the
            // supervisor.
            super::interactive::require_interactive_terminal()?;
            let ipc = Arc::new(IpcClient::connect(false).await?);
            let candidates = ipc.get_running_daemons().await?;
            super::interactive::select_daemons_interactively(&candidates, "wait")?
        } else {
            PitchforkToml::resolve_ids_and_group(&self.id, self.group.as_deref())?
        };

        // Snapshot the daemons we will actually wait on, classifying each
        // resolved target in argument order. The (possibly stale) state
        // snapshot is only used to learn the initial pids.
        let sf = StateFile::get();
        let supervisor_live = supervisor_is_live(sf);
        let mut watched_ids: Vec<DaemonId> = Vec::new();
        let mut polled: Vec<Watched> = Vec::new();
        let exit_on_restart = self.exit_on_restart;
        for id in &ids {
            match sf.daemons.get(id) {
                Some(daemon)
                    if !is_terminal_status(&daemon.status, exit_on_restart)
                        || retry_pending(sf, daemon, supervisor_live) =>
                {
                    // Not finished: evaluate the daemon after it stops for
                    // good. Its pid, when it has one, tells when the current
                    // attempt ends; the state then tells whether another
                    // attempt follows.
                    watched_ids.push(id.clone());
                    polled.push(Watched::new(id.clone(), daemon.pid, exit_on_restart));
                }
                Some(_) => {
                    // Already terminal: evaluate immediately, its exit
                    // code still counts toward the result.
                    watched_ids.push(id.clone());
                }
                None => {
                    warn!("{id} is not running");
                }
            }
        }

        if watched_ids.is_empty() {
            return Ok(());
        }

        // Only connect to the supervisor when the --kill signal handler
        // needs it, so plain `pitchfork wait <id>` keeps working without
        // IPC side effects (e.g. supervisor auto-start).
        let ipc: Option<Arc<IpcClient>> = if self.kill {
            Some(Arc::new(IpcClient::connect(false).await?))
        } else {
            None
        };

        let tail_names = watched_ids.clone();
        tokio::spawn(async move {
            logs::tail_logs(
                &tail_names,
                true,
                false,
                Vec::new(),
                Vec::new(),
                None,
                settings().logs.timestamp,
                false,
            )
            .await
            .unwrap_or_default();
        });

        // Register signal handlers only when --kill is set.
        let mut signal_rx = if self.kill {
            Some(register_signal_receiver()?)
        } else {
            None
        };

        // Daemons whose wait ended because they were restarted
        // (`--exit-on-restart`): their run ended with a clean stop.
        let mut restarted: Vec<DaemonId> = Vec::new();
        // Only live daemons are polled; when every target was already
        // finished (or gone), skip straight to the evaluation below.
        if !polled.is_empty() {
            let mut interval = time::interval(time::Duration::from_millis(100));
            let mut remaining = polled;

            loop {
                tokio::select! {
                    signo = wait_for_signal(&mut signal_rx), if signal_rx.is_some() => {
                        match signo {
                            Some(signo) => {
                                // Graceful SIGTERM -> SIGKILL stop via the supervisor
                                // (hooks fire, reverse dependency order), then exit
                                // like the shell does when killed by the signal.
                                // Stop only the daemons still being polled (live):
                                // already-finished targets are not running, so
                                // stopping them is meaningless, and daemons that
                                // were not running at snapshot time must not be
                                // killed here either.
                                let stop_ids: Vec<DaemonId> =
                                    remaining.iter().map(|w| w.id.clone()).collect();
                                let ipc = ipc.as_ref().expect("--kill connects IPC upfront");
                                if let Err(e) = ipc.stop_daemons(&stop_ids).await {
                                    warn!("failed to stop waited daemons on signal: {e}");
                                }
                                std::process::exit(128 + signo);
                            }
                            None => {
                                // Every signal listener closed without firing (e.g.
                                // ctrl_c() failed at await time on Windows). Signal
                                // handling is gone, so --kill can no longer act;
                                // disable the branch and keep polling.
                                warn!("--kill signal handling is no longer active; continuing to wait");
                                signal_rx = None;
                            }
                        }
                    }
                    _ = interval.tick() => {
                        // The state is read only once a watched process has
                        // gone, to learn whether the daemon is done.
                        if remaining.iter().any(|w| !w.process_running()) {
                            let sf = StateFile::read(&*env::PITCHFORK_STATE_FILE).ok();
                            let supervisor_live = sf.as_ref().is_some_and(supervisor_is_live);
                            remaining.retain_mut(|w| {
                                let running = w.still_running(sf.as_ref(), supervisor_live);
                                if !running && w.restarted {
                                    restarted.push(w.id.clone());
                                }
                                running
                            });
                        }
                        if remaining.is_empty() {
                            break;
                        }
                    }
                }
            }
        }

        // The supervisor updates daemon status asynchronously after the
        // process exits, so poll fresh state until every waited daemon
        // reaches a terminal status (bounded at ~2s). A restarted daemon is
        // not read again: its record now describes the new process.
        let finished_ids: Vec<DaemonId> = watched_ids
            .iter()
            .filter(|id| !restarted.contains(id))
            .cloned()
            .collect();
        let statuses = read_terminal_statuses(&finished_ids, exit_on_restart).await;
        // Exit 0 only when every watched daemon's terminal status maps to
        // 0; a status missing from the state (not persisted yet) maps to
        // 1. Otherwise propagate the exit code of the first failing daemon
        // in the order the daemons were selected (argument order).
        if let Some(exit_code) = watched_ids
            .iter()
            .map(|id| {
                if restarted.contains(id) {
                    0
                } else {
                    daemon_exit_code(id, &statuses)
                }
            })
            .find(|code| *code != 0)
        {
            std::process::exit(exit_code);
        }
        Ok(())
    }
}

/// Register one-shot handlers for the signals that should stop waited
/// daemons under `--kill`, returning a receiver that yields the signal
/// number once one of them arrives.
///
/// Errors if not a single handler could be registered: `--kill` would then
/// silently do nothing.
#[cfg(unix)]
fn register_signal_receiver() -> Result<mpsc::Receiver<i32>> {
    let (tx, rx) = mpsc::channel(4);
    let mut registered = 0;
    for (kind, signo) in [
        (SignalKind::interrupt(), libc::SIGINT),
        (SignalKind::terminate(), libc::SIGTERM),
        (SignalKind::hangup(), libc::SIGHUP),
        (SignalKind::quit(), libc::SIGQUIT),
    ] {
        let stream = match unix::signal(kind) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to register signal handler for {kind:?}: {e}");
                continue;
            }
        };
        registered += 1;
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut stream = stream;
            if stream.recv().await.is_some() {
                let _ = tx.send(signo).await;
            }
        });
    }
    if registered == 0 {
        return Err(miette::miette!(
            "failed to register any signal handler for --kill"
        ));
    }
    Ok(rx)
}

/// Windows has no POSIX signals; Ctrl-C is the only stop signal. The
/// registration itself cannot fail here; a ctrl_c() error at await time
/// closes the receiver and is handled by the main loop.
#[cfg(windows)]
fn register_signal_receiver() -> Result<mpsc::Receiver<i32>> {
    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(async move {
        if signal::ctrl_c().await.is_ok() {
            // Ctrl-C is SIGINT: exit code 130
            let _ = tx.send(2).await;
        }
    });
    Ok(rx)
}

/// Resolves when a registered signal arrives, yielding its signal number.
/// Returns None if the stream closed without ever receiving a signal.
async fn wait_for_signal(signal_rx: &mut Option<mpsc::Receiver<i32>>) -> Option<i32> {
    signal_rx.as_mut()?.recv().await
}

/// Exit code a waited daemon's terminal status represents: the mapped
/// status, or 1 when no terminal status was recorded yet (the supervisor
/// has not persisted it, so the exit code is unknown).
fn daemon_exit_code(id: &DaemonId, statuses: &[(DaemonId, DaemonStatus)]) -> i32 {
    statuses
        .iter()
        .find(|(status_id, _)| status_id == id)
        .map_or(1, |(_, status)| status_exit_code(status))
}

/// Map a daemon's terminal status to the exit code it represents.
fn status_exit_code(status: &DaemonStatus) -> i32 {
    match status {
        DaemonStatus::Stopped => 0,
        DaemonStatus::Completed => 0,
        // Only final with --exit-on-restart, where a restart ends the wait
        // the way a stop used to.
        DaemonStatus::Restarting => 0,
        DaemonStatus::Errored(code) if *code != -1 => *code,
        // -1 means the exit code is unknown.
        DaemonStatus::Errored(_) => 1,
        DaemonStatus::Failed(_) => 1,
        // Transient states only occur when the status read gave up before
        // the supervisor persisted a terminal status; treat as failure.
        _ => 1,
    }
}

/// How long a daemon whose process has gone may keep a non-final status
/// before it is evaluated anyway. The supervisor records the exit, or starts
/// the next attempt, only after reading the output left in the process's pipe
/// (which a child holding it open can drag out to the drain timeout), and the
/// state file shows it at the next flush. Evaluated sooner, a failed attempt
/// whose retry has yet to start would be reported as the result.
const SETTLE_TIMEOUT: time::Duration = crate::supervisor::EXIT_OUTPUT_DRAIN_TIMEOUT
    .saturating_add(crate::supervisor::STATE_FLUSH_INTERVAL)
    .saturating_add(time::Duration::from_secs(2));

/// A daemon being waited on, and the process of its current attempt.
struct Watched {
    id: DaemonId,
    pid: Option<u32>,
    /// When its process was first seen gone while the record was not final.
    gone_since: Option<time::Instant>,
    /// Treat a restart as the end of the daemon's run (`--exit-on-restart`).
    exit_on_restart: bool,
    /// Whether the wait ended because the daemon was restarted.
    restarted: bool,
}

impl Watched {
    fn new(id: DaemonId, pid: Option<u32>, exit_on_restart: bool) -> Self {
        Self {
            id,
            pid,
            gone_since: None,
            exit_on_restart,
            restarted: false,
        }
    }

    fn process_running(&self) -> bool {
        self.pid.is_some_and(|pid| PROCS.is_running(pid))
    }

    /// Whether to keep waiting: the current attempt is still running, the
    /// next one has started or will start, or the supervisor has not yet
    /// recorded how the last one ended.
    fn still_running(&mut self, sf: Option<&StateFile>, supervisor_live: bool) -> bool {
        if self.process_running() {
            return true;
        }
        let Some(sf) = sf else { return false };
        let Some(daemon) = sf.daemons.get(&self.id) else {
            return false;
        };
        if let Some(pid) = daemon.pid
            && Some(pid) != self.pid
            && PROCS.is_running(pid)
        {
            // A process that replaced a running one with no failed attempt in
            // between (a retry always counts one) was started by a restart,
            // which may have come and gone between two reads of the state.
            if self.exit_on_restart && self.pid.is_some() && daemon.retry_count == 0 {
                self.restarted = true;
                return false;
            }
            // The next attempt has started.
            self.pid = Some(pid);
            self.gone_since = None;
            return true;
        }
        if is_terminal_status(&daemon.status, self.exit_on_restart) {
            if retry_pending(sf, daemon, supervisor_live) {
                self.pid = None;
                self.gone_since = None;
                return true;
            }
            // Only a restart is final here with --exit-on-restart.
            self.restarted = daemon.status.is_restarting();
            return false;
        }
        if daemon.status.is_restarting() && supervisor_live {
            // The supervisor is between stopping the old process and starting
            // the new one, which the pid check above picks up once it runs.
            // How long that takes (the restart delay, readiness of a forced
            // start) is not bounded by the settle timeout below.
            self.gone_since = None;
            return true;
        }
        let gone_since = *self.gone_since.get_or_insert_with(time::Instant::now);
        gone_since.elapsed() < SETTLE_TIMEOUT
    }
}

/// Whether the supervisor will start this daemon again: it failed with no
/// process left and has retries to spare, which the supervisor's retry
/// checker (or a start sleeping out a backoff) still runs. A disabled
/// daemon, or one whose supervisor is gone, is not retried.
fn retry_pending(sf: &StateFile, daemon: &crate::daemon::Daemon, supervisor_live: bool) -> bool {
    supervisor_live
        && daemon.status.is_errored()
        && daemon.pid.is_none()
        && daemon.retry.count() > 0
        && daemon.retry_count < daemon.retry.count()
        && !sf.disabled.contains(&daemon.id)
}

fn supervisor_is_live(sf: &StateFile) -> bool {
    sf.daemons
        .get(&DaemonId::pitchfork())
        .is_some_and(crate::supervisor::supervisor_record_is_live)
}

/// Whether the supervisor has recorded a final status for the daemon, as
/// opposed to the transient Running/Waiting/Stopping/Restarting states.
/// With `exit_on_restart`, a restart counts as final: the run being waited
/// on has been stopped.
fn is_terminal_status(status: &DaemonStatus, exit_on_restart: bool) -> bool {
    if status.is_restarting() {
        return exit_on_restart;
    }
    !status.is_running() && !status.is_waiting() && !status.is_stopping()
}

/// Fresh statuses for `ids` read from the state file (missing daemons omitted).
fn fresh_statuses(ids: &[DaemonId]) -> Vec<(DaemonId, DaemonStatus)> {
    StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .map(|sf| {
            ids.iter()
                .filter_map(|id| sf.daemons.get(id).map(|d| (id.clone(), d.status.clone())))
                .collect()
        })
        .unwrap_or_default()
}

/// Read fresh state until every waited daemon reports a terminal status,
/// bounded at ~2s (the supervisor persists status asynchronously after the
/// process exits).
async fn read_terminal_statuses(
    ids: &[DaemonId],
    exit_on_restart: bool,
) -> Vec<(DaemonId, DaemonStatus)> {
    for _ in 0..40 {
        let statuses = fresh_statuses(ids);
        if statuses.len() == ids.len()
            && statuses
                .iter()
                .all(|(_, status)| is_terminal_status(status, exit_on_restart))
        {
            return statuses;
        }
        time::sleep(time::Duration::from_millis(50)).await;
    }
    fresh_statuses(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::Daemon;

    fn failed(retry: u32, retry_count: u32) -> Daemon {
        Daemon {
            id: DaemonId::new("proj", "api"),
            status: DaemonStatus::Errored(3),
            retry: crate::config_types::Retry(retry),
            retry_count,
            ..Default::default()
        }
    }

    fn state_with(daemon: &Daemon) -> StateFile {
        let mut sf = StateFile::new(std::path::PathBuf::from("state.toml"));
        sf.daemons.insert(daemon.id.clone(), daemon.clone());
        sf
    }

    #[test]
    fn a_failed_attempt_with_retries_left_is_not_the_end() {
        let daemon = failed(2, 0);
        let sf = state_with(&daemon);
        assert!(retry_pending(&sf, &daemon, true));

        let mut watched = Watched::new(daemon.id.clone(), None, false);
        assert!(watched.still_running(Some(&sf), true));
    }

    #[test]
    fn the_last_attempt_failing_is_the_end() {
        let daemon = failed(2, 2);
        let sf = state_with(&daemon);
        assert!(!retry_pending(&sf, &daemon, true));

        let mut watched = Watched::new(daemon.id.clone(), None, false);
        assert!(!watched.still_running(Some(&sf), true));
    }

    #[test]
    fn no_retry_follows_without_a_supervisor_or_when_disabled() {
        let daemon = failed(2, 0);
        let mut sf = state_with(&daemon);
        assert!(!retry_pending(&sf, &daemon, false));

        sf.disabled.insert(daemon.id.clone());
        assert!(!retry_pending(&sf, &daemon, true));
    }

    #[test]
    fn an_exit_not_yet_recorded_is_waited_out() {
        // The process is gone but the record still says running.
        let daemon = Daemon {
            id: DaemonId::new("proj", "api"),
            status: DaemonStatus::Running,
            ..Default::default()
        };
        let sf = state_with(&daemon);
        let mut watched = Watched::new(daemon.id.clone(), None, false);
        assert!(watched.still_running(Some(&sf), true));

        watched.gone_since = Some(time::Instant::now() - SETTLE_TIMEOUT);
        assert!(!watched.still_running(Some(&sf), true));
    }

    fn restarting(pid: Option<u32>) -> Daemon {
        Daemon {
            id: DaemonId::new("proj", "api"),
            status: DaemonStatus::Restarting,
            pid,
            ..Default::default()
        }
    }

    #[test]
    fn a_restart_is_followed_by_default() {
        // The old process is gone and the new one has not started yet.
        let daemon = restarting(None);
        let sf = state_with(&daemon);
        let mut watched = Watched::new(daemon.id.clone(), Some(i32::MAX as u32), false);
        assert!(watched.still_running(Some(&sf), true));

        // However long the restart takes, it is not settled by the timeout.
        watched.gone_since = Some(time::Instant::now() - SETTLE_TIMEOUT);
        assert!(watched.still_running(Some(&sf), true));
        assert!(!is_terminal_status(&DaemonStatus::Restarting, false));
    }

    #[test]
    fn the_new_process_of_a_restart_is_picked_up() {
        // The record names a live process other than the one being watched.
        let me = std::process::id();
        let daemon = Daemon {
            id: DaemonId::new("proj", "api"),
            status: DaemonStatus::Running,
            pid: Some(me),
            ..Default::default()
        };
        let sf = state_with(&daemon);
        let mut watched = Watched::new(daemon.id.clone(), None, false);
        assert!(watched.still_running(Some(&sf), true));
        assert_eq!(watched.pid, Some(me));
    }

    #[test]
    fn a_restart_left_behind_by_a_dead_supervisor_is_settled() {
        let daemon = restarting(None);
        let sf = state_with(&daemon);
        let mut watched = Watched::new(daemon.id.clone(), None, false);
        assert!(watched.still_running(Some(&sf), false));
        watched.gone_since = Some(time::Instant::now() - SETTLE_TIMEOUT);
        assert!(!watched.still_running(Some(&sf), false));
    }

    #[test]
    fn exit_on_restart_ends_the_wait_cleanly() {
        let daemon = restarting(None);
        let sf = state_with(&daemon);
        let mut watched = Watched::new(daemon.id.clone(), None, true);
        assert!(!watched.still_running(Some(&sf), true));
        assert!(watched.restarted);
        assert!(is_terminal_status(&DaemonStatus::Restarting, true));
        assert_eq!(status_exit_code(&DaemonStatus::Restarting), 0);
    }

    #[test]
    fn exit_on_restart_ends_the_wait_when_the_restart_already_finished() {
        // The state was not read while the daemon was restarting: it already
        // names the new process.
        let mut daemon = Daemon {
            id: DaemonId::new("proj", "api"),
            status: DaemonStatus::Running,
            pid: Some(std::process::id()),
            ..Default::default()
        };
        let sf = state_with(&daemon);
        let mut watched = Watched::new(daemon.id.clone(), Some(i32::MAX as u32), true);
        assert!(!watched.still_running(Some(&sf), true));
        assert!(watched.restarted);

        // A retry of a failed attempt is followed, not taken for a restart.
        daemon.retry = crate::config_types::Retry(2);
        daemon.retry_count = 1;
        let sf = state_with(&daemon);
        let mut watched = Watched::new(daemon.id.clone(), Some(i32::MAX as u32), true);
        assert!(watched.still_running(Some(&sf), true));
        assert!(!watched.restarted);
    }
}
