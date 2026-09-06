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
        let mut watched_ids: Vec<DaemonId> = Vec::new();
        let mut polled: Vec<(DaemonId, u32)> = Vec::new();
        for id in &ids {
            match sf.daemons.get(id) {
                Some(daemon) if !is_terminal_status(&daemon.status) => {
                    // Non-terminal: evaluate the daemon after it stops and
                    // poll its pid to learn when; a missing pid means the
                    // process is already gone, which the bounded re-read
                    // after the poll loop resolves (missing maps to 1).
                    watched_ids.push(id.clone());
                    if let Some(pid) = daemon.pid {
                        polled.push((id.clone(), pid));
                    }
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
                                    remaining.iter().map(|(id, _)| id.clone()).collect();
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
                        let mut i = 0;
                        while i < remaining.len() {
                            let (_, pid) = &remaining[i];
                            if !PROCS.is_running(*pid) {
                                remaining.remove(i);
                            } else {
                                i += 1;
                            }
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
        // reaches a terminal status (bounded at ~2s).
        let statuses = read_terminal_statuses(&watched_ids).await;
        // Exit 0 only when every watched daemon's terminal status maps to
        // 0; a status missing from the state (not persisted yet) maps to
        // 1. Otherwise propagate the exit code of the first failing daemon
        // in the order the daemons were selected (argument order).
        if let Some(exit_code) = watched_ids
            .iter()
            .map(|id| daemon_exit_code(id, &statuses))
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
        DaemonStatus::Errored(code) if *code != -1 => *code,
        // -1 means the exit code is unknown.
        DaemonStatus::Errored(_) => 1,
        DaemonStatus::Failed(_) => 1,
        // Transient states only occur when the status read gave up before
        // the supervisor persisted a terminal status; treat as failure.
        _ => 1,
    }
}

/// Whether the supervisor has recorded a final status for the daemon, as
/// opposed to the transient Running/Waiting/Stopping states.
fn is_terminal_status(status: &DaemonStatus) -> bool {
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
async fn read_terminal_statuses(ids: &[DaemonId]) -> Vec<(DaemonId, DaemonStatus)> {
    for _ in 0..40 {
        let statuses = fresh_statuses(ids);
        if statuses.len() == ids.len()
            && statuses
                .iter()
                .all(|(_, status)| is_terminal_status(status))
        {
            return statuses;
        }
        time::sleep(time::Duration::from_millis(50)).await;
    }
    fresh_statuses(ids)
}
