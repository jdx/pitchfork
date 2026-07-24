//! Re-adoption of orphaned daemons
//!
//! When the supervisor dies uncleanly (e.g. `kill -9`), its daemon child
//! processes survive, re-parented to init. On restart, daemons whose recorded
//! identity (PID + kernel start time) still matches a live process can be
//! re-adopted: their state is kept and supervision resumes.
//!
//! An adopted process is no longer a child of the supervisor, so `wait()`
//! based monitoring is impossible — a poll monitor watches liveness instead,
//! anchored to the verified start time so a recycled PID is never mistaken
//! for the daemon. Two consequences follow, both documented in the
//! `orphan_policy` setting:
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

/// Why the poll monitor stopped watching an adopted process.
enum PollOutcome {
    /// The adopted process exited (or its PID was recycled, which means the
    /// process exited unnoticed) while the daemon record still pointed at it.
    /// `was_stopping` snapshots whether `stop()` was in flight at the moment
    /// death was observed, tying stop intent to the observation the way the
    /// child monitor's pre-drain snapshot does.
    ProcessDied { was_stopping: bool },
    /// `stop()` finished first: the record shows no PID and status `Stopped`,
    /// and every prior observation belonged to this monitor's PID (a foreign
    /// PID would have broken out as `TakenOver` instead). State is already
    /// final, but the stop hooks still need to fire — the regular child
    /// monitor fires them, `stop()` itself never does.
    StoppedExternally,
    /// Another process took over the daemon record (e.g. a restart spawned a
    /// fresh child with its own monitor) or the record was removed. State and
    /// hooks are the successor's to manage; touch nothing.
    TakenOver,
}

/// Monotonic token distinguishing individual monitor registrations. PIDs are
/// not sufficient: the OS can recycle a dead daemon's PID for its own
/// successor, and two guards carrying the same (id, pid) pair would then be
/// indistinguishable — the stale one could unregister the live one.
static NEXT_MONITOR_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A registration in the supervisor's `monitored` map: which PID is being
/// watched and by which registration (token).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MonitorEntry {
    pub(crate) pid: u32,
    pub(crate) token: u64,
}

/// RAII registration of a daemon in the supervisor's `monitored` map.
///
/// Created *synchronously* before the monitoring task is spawned so there is
/// no window in which a supervised daemon looks unmonitored to the orphan
/// reconciler. Dropped by the monitoring task when it finishes; the entry is
/// only removed if it still carries this guard's token, so a monitor that
/// outlives a restart (e.g. during the post-exit drain) cannot unregister
/// its successor — even a successor that recycled the same numeric PID.
pub(crate) struct MonitoredGuard {
    id: DaemonId,
    token: u64,
}

impl MonitoredGuard {
    /// Unconditionally claim the daemon's registry entry, displacing any
    /// previous monitor. Used by `run_once` when spawning a child: a fresh
    /// process legitimately supersedes whatever monitor came before, and the
    /// overwrite (new token) is what lets stale monitors detect the
    /// succession.
    pub(crate) fn register(id: DaemonId, pid: u32) -> Self {
        let token = NEXT_MONITOR_TOKEN.fetch_add(1, atomic::Ordering::Relaxed);
        SUPERVISOR
            .monitored
            .lock()
            .expect("monitored lock poisoned")
            .insert(id.clone(), MonitorEntry { pid, token });
        Self { id, token }
    }

    /// Claim the daemon's registry entry only if no monitor currently holds
    /// it. Used by adoption, which must never displace a live monitor: this
    /// check-and-insert is atomic under the registry lock, making it the
    /// authoritative gate against two concurrent reconciliation passes
    /// adopting the same process twice.
    pub(crate) fn try_register(id: DaemonId, pid: u32) -> Option<Self> {
        let mut monitored = SUPERVISOR
            .monitored
            .lock()
            .expect("monitored lock poisoned");
        if monitored.contains_key(&id) {
            return None;
        }
        let token = NEXT_MONITOR_TOKEN.fetch_add(1, atomic::Ordering::Relaxed);
        monitored.insert(id.clone(), MonitorEntry { pid, token });
        Some(Self { id, token })
    }

    /// The unique token of this registration, compared by monitors to detect
    /// having been superseded.
    pub(crate) fn token(&self) -> u64 {
        self.token
    }
}

impl Drop for MonitoredGuard {
    fn drop(&mut self) {
        let mut monitored = SUPERVISOR
            .monitored
            .lock()
            .expect("monitored lock poisoned");
        if monitored.get(&self.id).map(|e| e.token) == Some(self.token) {
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
            .is_some_and(|e| e.pid == pid)
    }

    /// Whether the registry entry for `id` still belongs to the registration
    /// identified by `token`. Unlike `is_monitored` this cannot be confused
    /// by a successor that recycled the same numeric PID.
    fn monitor_token_valid(&self, id: &DaemonId, token: u64) -> bool {
        self.monitored
            .lock()
            .expect("monitored lock poisoned")
            .get(id)
            .is_some_and(|e| e.token == token)
    }

    /// Interval-watcher reconciliation: find state-`running` daemons that no
    /// live monitoring task is watching and bring state and reality back in
    /// line. This covers windows the startup scan cannot see (e.g. a state
    /// file restored from backup, or an orphan that appeared after startup).
    ///
    /// Mirrors the startup scan's fail-closed identity handling, then applies
    /// `supervisor.orphan_policy` exactly like startup does:
    ///
    /// - dead PID → mark `Errored(-1)` so retry/cron/autostop logic behaves
    /// - recycled PID → the daemon died unnoticed; mark `Errored(-1)`
    /// - unverifiable identity → retain running state, touch nothing
    /// - live and verified → adopt, or terminate under the `kill` policy
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

        let policy = super::orphan_policy();

        for daemon in candidates {
            let Some(pid) = daemon.pid else { continue };
            // Re-check under current state: a monitor may have registered
            // between the snapshot and now.
            if self.is_monitored(&daemon.id, pid) {
                continue;
            }

            // Refresh each candidate immediately before checking it, matching
            // the startup scan: processing an earlier candidate can await a
            // stop timeout, during which this PID may exit and be recycled.
            PROCS.refresh_pids(&[pid]);

            if !PROCS.is_running(pid) {
                // The monitor that would have observed this exit died with a
                // previous supervisor; the exit status is unobservable.
                warn!(
                    "daemon {} (pid {pid}) died while unmonitored; marking errored",
                    daemon.id
                );
                self.finalize_if_pid(&daemon.id, pid, DaemonStatus::Errored(-1), Some(false))
                    .await;
                continue;
            }

            let current_start_time = PROCS.start_time(pid);
            let current_title = PROCS.title(pid);
            let matches = super::process_identity_matches(
                daemon.start_time,
                daemon.title.as_deref(),
                current_start_time,
                current_title.as_deref(),
            );

            if !matches {
                if daemon.start_time.is_some() && current_start_time.is_none() {
                    warn!(
                        "could not verify start time for live pid {pid} recorded for daemon {}; retaining running state",
                        daemon.id,
                    );
                    continue;
                }
                // The PID belongs to a different process now, which means the
                // daemon itself died unnoticed. Never touch the new process.
                warn!(
                    "pid {pid} recorded for daemon {} belongs to a different process now (PID recycled); marking errored",
                    daemon.id,
                );
                self.finalize_if_pid(&daemon.id, pid, DaemonStatus::Errored(-1), Some(false))
                    .await;
                continue;
            }

            let Some(expected_start_time) = current_start_time else {
                warn!(
                    "could not read start time for live pid {pid} recorded for daemon {}; retaining running state",
                    daemon.id,
                );
                continue;
            };

            if policy == "adopt" {
                self.adopt_daemon(&daemon, pid, expected_start_time).await;
                continue;
            }

            // `kill` policy applies on the interval path too — otherwise an
            // unmonitored live daemon found at runtime would stay running
            // unsupervised even though startup would have terminated it.
            info!(
                "terminating unmonitored orphaned daemon {} (pid {pid})",
                daemon.id
            );
            let stop_cfg = daemon.stop_signal.unwrap_or_default();
            match PROCS
                .kill_process_group_if_start_time_matches_async(
                    pid,
                    expected_start_time,
                    stop_cfg.signal.into(),
                    stop_cfg.timeout,
                )
                .await
            {
                Ok(true) => {
                    self.finalize_if_pid(&daemon.id, pid, DaemonStatus::Stopped, None)
                        .await;
                }
                Ok(false) => {
                    warn!(
                        "could not securely terminate unmonitored orphaned daemon {} (pid {pid}); retaining running state",
                        daemon.id
                    );
                }
                Err(err) => {
                    warn!(
                        "failed to terminate unmonitored orphaned daemon {} (pid {pid}): {err}; retaining running state",
                        daemon.id
                    );
                }
            }
        }
    }

    /// Finalize a daemon's terminal state only if its record still names
    /// `pid`. The ownership re-check and the mutation share one state-lock
    /// critical section, so a successor installed after the caller's
    /// snapshot is never overwritten — its upsert serializes after ours and
    /// the in-lock check stands down. No hooks fire from these transitions —
    /// the monitor that would have observed the exit died with a previous
    /// supervisor, mirroring the startup scan's handling.
    ///
    /// Returns whether the write happened.
    async fn finalize_if_pid(
        &self,
        id: &DaemonId,
        pid: u32,
        status: DaemonStatus,
        last_exit_success: Option<bool>,
    ) -> bool {
        let mut state_file = self.state_file.lock().await;
        let Some(d) = state_file.daemons.get(id) else {
            return false;
        };
        if d.pid != Some(pid) {
            debug!("daemon {id} was claimed by a successor; skipping finalization");
            return false;
        }
        let mut d = d.clone();
        d.pid = None;
        d.title = None;
        d.start_time = None;
        d.status = status;
        if let Some(les) = last_exit_success {
            d.last_exit_success = Some(les);
        }
        d.active_port = None;
        state_file.clear_active_port(id);
        state_file.insert_daemon(id, d);
        true
    }

    /// Resume supervision of a live orphaned daemon process whose identity
    /// has been verified against `expected_start_time`.
    ///
    /// The daemon's state (status, ports, proxy routing) is kept as-is; a
    /// poll monitor takes over for the `child.wait()` monitor that died with
    /// the previous supervisor.
    pub(crate) async fn adopt_daemon(&self, daemon: &Daemon, pid: u32, expected_start_time: u64) {
        // Claim the registry entry BEFORE any await so two concurrent
        // reconciliation passes (or startup scan + interval tick) cannot
        // both adopt the same process and spawn duplicate poll monitors.
        // The earlier is_monitored checks are advisory; this is the gate.
        let Some(guard) = MonitoredGuard::try_register(daemon.id.clone(), pid) else {
            debug!(
                "daemon {} (pid {pid}) is already monitored; skipping duplicate adoption",
                daemon.id
            );
            return;
        };
        info!("re-adopting orphaned daemon {} (pid {pid})", daemon.id);

        // Legacy records matched by title have no persisted start_time.
        // Re-upsert while the process cache is fresh so the identity fields
        // are recorded for future scans and crashes. active_port must be
        // carried over explicitly: upsert_daemon intentionally never inherits
        // it (a restarted process hasn't bound its port yet), but this
        // process never stopped — wiping it would break proxy routing.
        if daemon.start_time.is_none() {
            let active_port = daemon.active_port;
            let _ = self
                .upsert_daemon(
                    UpsertDaemonOpts::builder(daemon.id.clone())
                        .set(|o| {
                            o.pid = Some(pid);
                            o.status = DaemonStatus::Running;
                            o.active_port = active_port;
                        })
                        .build(),
                )
                .await;
        }
        let id = daemon.id.clone();
        let daemon_dir = daemon
            .dir
            .clone()
            .unwrap_or_else(|| crate::env::CWD.clone());
        let hook_env = daemon.env.clone();
        let hook_retry = daemon.retry;
        let hook_retry_count = daemon.retry_count;

        tokio::spawn(async move {
            let token = guard.token();
            let _guard = guard;

            let outcome = loop {
                time::sleep(POLL_INTERVAL).await;

                // The monitored registry doubles as a generation marker:
                // run_once overwrites this daemon's entry synchronously
                // before any successor spawns, and each monitor's guard only
                // removes its own registration. If the entry no longer
                // carries our token, a successor existed at some point —
                // even one that already ran its full lifecycle between our
                // polls, or one that recycled our numeric PID — and its
                // monitor owns the record's transitions and hooks.
                if !SUPERVISOR.monitor_token_valid(&id, token) {
                    break PollOutcome::TakenOver;
                }

                let Some(current) = SUPERVISOR.get_daemon(&id).await else {
                    break PollOutcome::TakenOver;
                };

                if current.pid == Some(pid) {
                    // Still ours — check liveness and identity. A start time
                    // that no longer matches means our process died and the
                    // OS recycled its PID: treat as death, and never touch
                    // the unrelated new process.
                    let was_stopping = current.status.is_stopping();
                    PROCS.refresh_pids(&[pid]);
                    if !PROCS.is_running(pid) {
                        break PollOutcome::ProcessDied { was_stopping };
                    }
                    match PROCS.start_time(pid) {
                        Some(current_start_time) if current_start_time != expected_start_time => {
                            break PollOutcome::ProcessDied { was_stopping };
                        }
                        None => {
                            // Transient identity read failure: keep watching
                            // on liveness alone. Treating this as death could
                            // start a duplicate next to a live process.
                            debug!(
                                "could not read start time for adopted daemon {id} (pid {pid}); continuing on liveness only"
                            );
                        }
                        _ => {}
                    }
                } else if current.pid.is_none() && current.status.is_stopping() {
                    // Transitional: a stop of our record is mid-flight — wait
                    // for it to settle so the exit path can fire stop hooks.
                    // (A Stopping record with a *different* PID is a successor
                    // and falls through to TakenOver below.)
                } else if current.pid.is_none() && current.status.is_stopped() {
                    break PollOutcome::StoppedExternally;
                } else {
                    break PollOutcome::TakenOver;
                }
            };

            if matches!(outcome, PollOutcome::TakenOver) {
                debug!("adopted daemon {id} was taken over or removed; poll monitor exiting");
                return;
            }

            // Mirror the child monitor's exit path, minus everything that
            // requires the (long-gone) stdio pipes.
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

            // Re-read state and verify this monitor still owns the record
            // before mutating anything. A restart can spawn a successor
            // between observing the death and reaching this point; its
            // state (PID, status, active port) and hooks are not ours to
            // touch — even if the successor is itself Stopping or Stopped,
            // its own monitor handles that lifecycle. Ownership here means
            // the record still names our PID, or was finalized with no PID
            // at all (stop() observed our dead process and completed the
            // transition itself).
            // Re-check the generation marker: a PID-less stopped record only
            // belongs to this monitor if no successor ever registered over
            // our entry. Without this, a successor's complete start+stop
            // between two polls would be misattributed to our process and
            // its stop hooks fired twice.
            if !SUPERVISOR.monitor_token_valid(&id, token) {
                debug!("adopted daemon {id} was superseded; skipping exit handling");
                return;
            }

            let current = SUPERVISOR.get_daemon(&id).await;
            let owns_pid = current.as_ref().is_some_and(|d| d.pid == Some(pid));
            let finalized_ours = current.as_ref().is_some_and(|d| {
                d.pid.is_none() && (d.status.is_stopped() || d.status.is_stopping())
            });
            if !owns_pid && !finalized_ours {
                debug!("adopted daemon {id} has a successor; skipping exit handling");
                return;
            }

            // Exit codes of non-child processes cannot be observed. Stop
            // intent comes from the snapshot taken when death was observed,
            // or from the record having been finalized by stop() since.
            let was_stopping = matches!(outcome, PollOutcome::ProcessDied { was_stopping: true });
            let intentional = was_stopping
                || finalized_ours
                || matches!(outcome, PollOutcome::StoppedExternally)
                || current
                    .as_ref()
                    .is_some_and(|d| d.status.is_stopping() || d.status.is_stopped());
            let (exit_code, exit_reason) = if intentional {
                (-1, "stop")
            } else {
                (-1, "fail")
            };
            info!("adopted daemon {id} (pid {pid}) exited ({exit_reason}, exit status unknown)");

            // Update state unless stop() already finalized it. Ownership is
            // re-validated inside the same state-lock critical section that
            // performs the write, so a successor starting between the checks
            // above and the mutation cannot have its record overwritten —
            // its own upsert would be serialized after ours and we would see
            // its PID here and stand down.
            if owns_pid {
                let new_status = match exit_reason {
                    "stop" => DaemonStatus::Stopped,
                    _ => DaemonStatus::Errored(exit_code),
                };
                let last_exit_success = exit_reason == "stop";
                let mut state_file = SUPERVISOR.state_file.lock().await;
                let still_ours = SUPERVISOR.monitor_token_valid(&id, token)
                    && state_file
                        .daemons
                        .get(&id)
                        .is_some_and(|d| d.pid == Some(pid));
                if !still_ours {
                    debug!(
                        "adopted daemon {id} was claimed by a successor; skipping exit handling"
                    );
                    return;
                }
                state_file.clear_active_port(&id);
                if let Some(d) = state_file.daemons.get(&id) {
                    let mut d = d.clone();
                    d.pid = None;
                    d.title = None;
                    d.start_time = None;
                    d.status = new_status;
                    d.last_exit_success = Some(last_exit_success);
                    d.active_port = None;
                    state_file.insert_daemon(&id, d);
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
