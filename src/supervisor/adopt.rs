//! Re-adoption of orphaned daemons
//!
//! When the supervisor dies uncleanly (e.g. `kill -9`), its daemon child
//! processes survive, re-parented to init. On restart, daemons whose recorded
//! identity (PID + kernel start time) still matches a live process can be
//! re-adopted: their state is kept and supervision resumes.
//!
//! An adopted process is no longer a child of the supervisor, so `wait()`
//! based monitoring is impossible — a poll monitor watches liveness instead.
//! Two consequences follow, both documented in the `orphan_policy` setting:
//!
//! - stdout/stderr capture cannot be restored (the pipes died with the old
//!   supervisor); log capture resumes on the daemon's next restart
//! - exit codes cannot be observed; an adopted daemon that dies unexpectedly
//!   is marked `Errored(-1)` ("unknown exit code"), making it eligible for
//!   its configured retries

use super::Supervisor;
use super::hooks::{HookType, fire_hook};
use crate::daemon::Daemon;
use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::procs::PROCS;
use crate::settings::settings;
use crate::supervisor::SUPERVISOR;
use crate::supervisor::state::UpsertDaemonOpts;
use std::sync::atomic;
use std::time::Duration;
use tokio::time;

/// How often the poll monitor checks that an adopted process is still alive.
/// Chosen to roughly match the responsiveness of `child.wait()` monitoring
/// without adding measurable load.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Verify that the live process at `pid` really is the daemon we recorded,
/// not an unrelated process that received a recycled PID.
///
/// The kernel start time is a stable identity for the lifetime of a process;
/// fall back to comparing the process name for state files written by older
/// versions that didn't record `start_time`. Requires the process cache to be
/// populated for `pid` (see `PROCS.refresh_pids`).
pub(crate) fn process_identity_matches(daemon: &Daemon, pid: u32) -> bool {
    match (daemon.start_time, PROCS.start_time(pid)) {
        (Some(recorded), Some(current)) => recorded == current,
        _ => match (PROCS.title(pid), &daemon.title) {
            (Some(current), Some(expected)) => current == *expected,
            // No recorded identity at all (the process was started before
            // identity tracking was added, or the state was reset) —
            // degraded but functional: treat as a match.
            _ => true,
        },
    }
}

/// RAII registration of a daemon in the supervisor's `monitored` map.
///
/// Created *synchronously* before the monitoring task is spawned so there is
/// no window in which a supervised daemon looks unmonitored to the orphan
/// reconciler. Dropped by the monitoring task when it finishes; the entry is
/// only removed if it still refers to this guard's PID, so a monitor that
/// outlives a restart (e.g. during the post-exit drain) cannot unregister
/// its successor.
pub(crate) struct MonitoredGuard {
    id: DaemonId,
    pid: u32,
}

impl MonitoredGuard {
    pub(crate) fn register(id: DaemonId, pid: u32) -> Self {
        SUPERVISOR
            .monitored
            .lock()
            .expect("monitored lock poisoned")
            .insert(id.clone(), pid);
        Self { id, pid }
    }
}

impl Drop for MonitoredGuard {
    fn drop(&mut self) {
        let mut monitored = SUPERVISOR
            .monitored
            .lock()
            .expect("monitored lock poisoned");
        if monitored.get(&self.id) == Some(&self.pid) {
            monitored.remove(&self.id);
        }
    }
}

impl Supervisor {
    /// Whether `id` currently has a live monitoring task watching `pid`.
    pub(crate) fn is_monitored(&self, id: &DaemonId, pid: u32) -> bool {
        self.monitored
            .lock()
            .expect("monitored lock poisoned")
            .get(id)
            == Some(&pid)
    }

    /// Interval-watcher reconciliation: find state-`running` daemons that no
    /// live monitoring task is watching and bring state and reality back in
    /// line. This covers windows the startup scan cannot see (e.g. a state
    /// file restored from backup, or an orphan that appeared after startup).
    ///
    /// - dead PID → mark `Errored(-1)` so retry/cron/autostop logic behaves
    /// - live PID with matching identity → re-adopt (policy `adopt` only)
    pub(crate) async fn reconcile_unmonitored_daemons(&self) {
        if !settings().supervisor.cleanup_orphans {
            return;
        }

        let candidates: Vec<Daemon> = {
            let state = self.state_file.lock().await;
            state
                .daemons
                .values()
                .filter(|d| {
                    d.id != DaemonId::pitchfork()
                        && d.status.is_running()
                        && d.pid.is_some_and(|pid| !self.is_monitored(&d.id, pid))
                })
                .cloned()
                .collect()
        };

        if candidates.is_empty() {
            return;
        }

        let pids: Vec<u32> = candidates.iter().filter_map(|d| d.pid).collect();
        PROCS.refresh_pids(&pids);

        for daemon in candidates {
            let Some(pid) = daemon.pid else { continue };
            // Re-check under current state: a monitor may have registered
            // between the snapshot and now.
            if self.is_monitored(&daemon.id, pid) {
                continue;
            }

            if !PROCS.is_running(pid) {
                // The monitor that would have observed this exit died with a
                // previous supervisor; the exit status is unobservable. No
                // hooks fire here — mirroring the startup scan's handling of
                // daemons found dead.
                warn!(
                    "daemon {} (pid {pid}) died while unmonitored; marking errored",
                    daemon.id
                );
                let _ = self
                    .upsert_daemon(
                        UpsertDaemonOpts::builder(daemon.id.clone())
                            .set(|o| {
                                o.pid = None;
                                o.status = DaemonStatus::Errored(-1);
                                o.last_exit_success = Some(false);
                                o.active_port = None;
                            })
                            .build(),
                    )
                    .await;
                continue;
            }

            if super::orphan_policy() == "adopt" && process_identity_matches(&daemon, pid) {
                self.adopt_daemon(&daemon, pid);
            }
        }
    }

    /// Resume supervision of a live orphaned daemon process.
    ///
    /// The daemon's state (status, ports, proxy routing) is kept as-is; a
    /// poll monitor takes over for the `child.wait()` monitor that died with
    /// the previous supervisor.
    pub(crate) fn adopt_daemon(&self, daemon: &Daemon, pid: u32) {
        info!("re-adopting orphaned daemon {} (pid {pid})", daemon.id);
        let guard = MonitoredGuard::register(daemon.id.clone(), pid);
        let id = daemon.id.clone();
        let daemon_dir = daemon
            .dir
            .clone()
            .unwrap_or_else(|| crate::env::CWD.clone());
        let hook_env = daemon.env.clone();
        let hook_retry = daemon.retry;
        let hook_retry_count = daemon.retry_count;

        tokio::spawn(async move {
            let _guard = guard;

            loop {
                time::sleep(POLL_INTERVAL).await;

                // Stand down if another process took over this daemon (e.g.
                // `pitchfork restart` spawned a fresh child with its own
                // monitor). State is the successor's to manage.
                let current = SUPERVISOR.get_daemon(&id).await;
                let owns_daemon = current
                    .as_ref()
                    .is_some_and(|d| d.pid == Some(pid) || d.status.is_stopping());
                if !owns_daemon {
                    debug!("adopted daemon {id} was taken over or removed; poll monitor exiting");
                    return;
                }

                if !PROCS.is_running(pid) {
                    break;
                }
            }

            // The adopted process died. Mirror the child monitor's exit path,
            // minus everything that requires the (long-gone) stdio pipes.
            SUPERVISOR
                .active_monitors
                .fetch_add(1, atomic::Ordering::Release);
            struct MonitorGuard;
            impl Drop for MonitorGuard {
                fn drop(&mut self) {
                    SUPERVISOR
                        .active_monitors
                        .fetch_sub(1, atomic::Ordering::Release);
                    SUPERVISOR.monitor_done.notify_waiters();
                }
            }
            let _monitor_guard = MonitorGuard;

            {
                let mut state_file = SUPERVISOR.state_file.lock().await;
                state_file.clear_active_port(&id);
            }

            let current = SUPERVISOR.get_daemon(&id).await;
            let already_stopped = current.as_ref().is_some_and(|d| d.status.is_stopped());
            let is_stopping =
                already_stopped || current.as_ref().is_some_and(|d| d.status.is_stopping());

            // Exit codes of non-child processes cannot be observed.
            let (exit_code, exit_reason) = if is_stopping {
                (-1, "stop")
            } else {
                (-1, "fail")
            };
            info!("adopted daemon {id} (pid {pid}) exited ({exit_reason}, exit status unknown)");

            if !already_stopped {
                let new_status = match exit_reason {
                    "stop" => DaemonStatus::Stopped,
                    _ => DaemonStatus::Errored(exit_code),
                };
                let last_exit_success = exit_reason == "stop";
                if let Err(e) = SUPERVISOR
                    .upsert_daemon(
                        UpsertDaemonOpts::builder(id.clone())
                            .set(|o| {
                                o.pid = None;
                                o.status = new_status;
                                o.last_exit_success = Some(last_exit_success);
                            })
                            .build(),
                    )
                    .await
                {
                    error!("failed to update state for adopted daemon {id}: {e}");
                }
            }

            let hook_extra_env = vec![
                ("PITCHFORK_EXIT_CODE".to_string(), exit_code.to_string()),
                ("PITCHFORK_EXIT_REASON".to_string(), exit_reason.to_string()),
            ];
            let hooks_to_fire: Vec<HookType> = match exit_reason {
                "stop" => vec![HookType::OnStop, HookType::OnExit],
                // "fail": fire on_fail + on_exit only when retries are exhausted
                _ if hook_retry_count >= hook_retry.count() => {
                    vec![HookType::OnFail, HookType::OnExit]
                }
                _ => vec![],
            };
            for hook_type in hooks_to_fire {
                fire_hook(
                    hook_type,
                    id.clone(),
                    daemon_dir.clone(),
                    hook_retry_count,
                    hook_env.clone(),
                    hook_extra_env.clone(),
                )
                .await;
            }
        });
    }
}
