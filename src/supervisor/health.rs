//! Per-daemon health checks.
//!
//! Daemons with `health_cmd`, `health_http` and/or `health_port` configured
//! get a background task, spawned lazily by the interval watcher, that probes
//! on the configured interval and kills the daemon after `retries` consecutive
//! failures. The kill goes through [`Supervisor::kill_daemon_as_crash`], so the
//! monitor observes a non-zero exit and records `Errored` — making the death
//! look exactly like a crash and letting the existing retry logic restart the
//! daemon.

use super::{ExitObservation, SUPERVISOR, Supervisor, signalling_pid_is_authorized};
use crate::config_types::{HealthCmd, HealthHttp, HealthPort};
use crate::daemon::Daemon;
use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::env;
use crate::procs::PROCS;
use crate::settings::settings;
use crate::supervisor::lifecycle::spawn_cmd_probe;
use std::collections::HashMap;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time;

impl Supervisor {
    /// Prune finished health-check tasks and spawn new ones for daemons that
    /// are running with a health check configured and no live task.
    ///
    /// Called from the interval watcher on every tick.
    pub(crate) async fn manage_health_tasks(&self, tasks: &mut HashMap<DaemonId, JoinHandle<()>>) {
        tasks.retain(|id, handle| {
            if handle.is_finished() {
                debug!("health check task for daemon {id} finished");
                false
            } else {
                true
            }
        });

        let pitchfork_id = DaemonId::pitchfork();
        let to_spawn: Vec<DaemonId> = {
            let state = self.state_file.lock().await;
            state
                .daemons
                .values()
                .filter(|d| {
                    d.id != pitchfork_id
                        && d.status.is_running()
                        && d.pid.is_some()
                        && (d.health_cmd.is_some()
                            || d.health_http.is_some()
                            || d.health_port.is_some())
                        && !tasks.contains_key(&d.id)
                })
                .map(|d| d.id.clone())
                .collect()
        };
        for id in to_spawn {
            info!("starting health checks for daemon {id}");
            let task_id = id.clone();
            let handle = tokio::spawn(async move {
                SUPERVISOR.run_health_checks(task_id).await;
            });
            tasks.insert(id, handle);
        }
    }

    /// Body of a per-daemon health-check task.
    ///
    /// Loops: sleep one interval, re-read the daemon (exiting when it is gone,
    /// no longer running, or has lost its health configuration), run all
    /// configured probes, and kill the daemon as a crash once the consecutive
    /// failure count reaches the retry threshold. A restart resets the failure
    /// budget, so the `retries * interval` window doubles as the grace period
    /// for a daemon that is still becoming ready.
    async fn run_health_checks(&self, id: DaemonId) {
        let mut last_pid: Option<u32> = None;
        let mut last_start_time: Option<u64> = None;
        let mut consecutive_failures: u32 = 0;
        let mut http_client: Option<reqwest::Client> = None;
        loop {
            let Some(daemon) = self.current_health_target(&id).await else {
                debug!("health checks for daemon {id}: not running or not configured, stopping");
                return;
            };
            time::sleep(effective_interval(
                &daemon.health_cmd,
                &daemon.health_http,
                &daemon.health_port,
            ))
            .await;

            // Re-read after the sleep: the daemon may have stopped or restarted
            // while we waited.
            let Some(daemon) = self.current_health_target(&id).await else {
                return;
            };
            let Some(pid) = daemon.pid else {
                return;
            };
            // A restarted daemon (crashed and retried, or restarted by the
            // user) gets a fresh failure budget. PID alone is not enough to
            // detect a restart: the OS may reuse the same PID for a new
            // process, so the start time disambiguates process generations.
            if process_identity_changed(last_pid, last_start_time, daemon.pid, daemon.start_time) {
                consecutive_failures = 0;
                last_pid = daemon.pid;
                last_start_time = daemon.start_time;
            }
            let retries =
                effective_retries(&daemon.health_cmd, &daemon.health_http, &daemon.health_port);

            // A daemon is unhealthy if ANY configured probe fails; all
            // configured probes always run.
            let mut failed_kinds: Vec<&str> = Vec::new();
            if let Some(cmd) = &daemon.health_cmd
                && !health_cmd_probe(&id, &daemon, cmd).await
            {
                failed_kinds.push("cmd");
            }
            if let Some(http) = &daemon.health_http {
                if http_client.is_none() {
                    http_client = Some(supervisor_http_client());
                }
                if let Some(client) = http_client.as_ref()
                    && !health_http_probe(&id, http, client).await
                {
                    failed_kinds.push("http");
                }
            }
            // A port that is None (unrendered template) skips the probe and
            // does not count as a failure.
            if let Some(health_port) = daemon.health_port.as_ref()
                && let Some(port) = health_port.as_port()
                && !health_port_probe(&id, port, effective_port_timeout(health_port)).await
            {
                failed_kinds.push("port");
            }

            if failed_kinds.is_empty() {
                consecutive_failures = 0;
                continue;
            }
            consecutive_failures += 1;
            warn!(
                "daemon {id} health check failed ({}): {consecutive_failures}/{retries} consecutive failures",
                failed_kinds.join("/"),
            );
            if consecutive_failures >= retries {
                let reason = format!(
                    "due to health check failure ({consecutive_failures} consecutive failures)"
                );
                self.kill_daemon_as_crash(&id, pid, &reason).await;
                return;
            }
        }
    }

    /// Read the daemon from state, returning it only while it is a valid
    /// health-check target: running, has a pid, and has at least one probe
    /// configured.
    async fn current_health_target(&self, id: &DaemonId) -> Option<Daemon> {
        let daemon = self.get_daemon(id).await?;
        if daemon.status.is_running()
            && daemon.pid.is_some()
            && (daemon.health_cmd.is_some()
                || daemon.health_http.is_some()
                || daemon.health_port.is_some())
        {
            Some(daemon)
        } else {
            None
        }
    }

    /// Kill a daemon without setting `Stopping`, so the monitor observes a
    /// non-zero exit and records `Errored` — making the death look exactly
    /// like a crash and letting the retry checker restart the daemon when
    /// configured. Shared by resource-limit and health-check enforcement.
    ///
    /// `reason` names the enforcement in log lines (e.g. "due to resource
    /// limit violation" or "due to health check failure (2 consecutive
    /// failures)").
    pub(crate) async fn kill_daemon_as_crash(&self, id: &DaemonId, pid: u32, reason: &str) {
        info!("killing daemon {id} (pid {pid}) {reason}");
        let daemon = self.get_daemon(id).await;
        // A probe can complete long after the daemon it watched stopped or
        // restarted, by which point `pid` may have been recycled. If the daemon
        // no longer records this pid, there is nothing of ours left to kill —
        // and signalling the recycled pid could hit an unrelated process group.
        if daemon.as_ref().and_then(|d| d.pid) != Some(pid) {
            warn!("daemon {id} no longer owns pid {pid}; not killing it {reason}");
            return;
        }
        // Never signal a process group that provably isn't the daemon's: a
        // recycled PID would mean killing an unrelated process tree.
        let recorded_start_time = daemon.as_ref().and_then(|d| d.start_time);
        if !signalling_pid_is_authorized(recorded_start_time, PROCS.start_time(pid)) {
            warn!(
                "pid {pid} recorded for daemon {id} belongs to another process now; not killing it {reason}"
            );
            // Leaving the record running would have the watchdog measure the
            // stranger's usage / probe it on every tick and try to kill it
            // again each time. The daemon died unobserved, so record that —
            // the same terminal state orphan reconciliation would reach, and
            // one that keeps the daemon eligible for retry.
            self.finalize_if_pid(
                id,
                pid,
                DaemonStatus::Errored(-1),
                ExitObservation::Unobserved,
            )
            .await;
            return;
        }
        let stop_cfg = daemon.and_then(|d| d.stop_signal).unwrap_or_default();
        let stop_signal: i32 = stop_cfg.signal.into();
        if let Err(e) = PROCS
            .kill_process_group_async(pid, stop_signal, stop_cfg.timeout)
            .await
        {
            error!("failed to kill daemon {id} (pid {pid}) {reason}: {e}");
        }
    }
}

/// Whether a daemon's process identity changed since the last probe, meaning
/// the failure budget must reset. A PID alone can be reused by the OS after a
/// restart, so the start time disambiguates process generations.
fn process_identity_changed(
    last_pid: Option<u32>,
    last_start_time: Option<u64>,
    current_pid: Option<u32>,
    current_start_time: Option<u64>,
) -> bool {
    last_pid != current_pid || last_start_time != current_start_time
}

/// Build the shared HTTP client used for daemon health probes.
///
/// No total timeout is set here: `health_http_probe` bounds each request with
/// `effective_http_timeout`, so a per-daemon `health_http.timeout` larger than
/// `supervisor.http_client_timeout` is honored rather than silently capped by
/// a shared client timeout.
pub(crate) fn supervisor_http_client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap_or_default()
}

/// Effective probe interval: the first health check that sets one, else the
/// default.
fn effective_interval(
    cmd: &Option<HealthCmd>,
    http: &Option<HealthHttp>,
    port: &Option<HealthPort>,
) -> Duration {
    cmd.as_ref()
        .and_then(|c| c.interval)
        .or_else(|| http.as_ref().and_then(|h| h.interval))
        .or_else(|| port.as_ref().and_then(|p| p.interval))
        .unwrap_or_else(|| settings().supervisor_health_check_interval())
}

/// Effective per-probe timeout for a command health check.
fn effective_cmd_timeout(cmd: &HealthCmd) -> Duration {
    cmd.timeout
        .unwrap_or_else(|| settings().supervisor_health_cmd_timeout())
}

/// Effective per-request timeout for an HTTP health check.
fn effective_http_timeout(http: &HealthHttp) -> Duration {
    http.timeout
        .unwrap_or_else(|| settings().supervisor_health_http_timeout())
}

/// Effective per-connect timeout for a TCP port health check.
fn effective_port_timeout(port: &HealthPort) -> Duration {
    port.timeout
        .unwrap_or_else(|| settings().supervisor_health_port_timeout())
}

/// Effective failure threshold. With multiple probe kinds configured, the
/// strictest budget wins so any kind can trigger the kill within its own
/// retries; with one kind, its value (or the default) applies.
fn effective_retries(
    cmd: &Option<HealthCmd>,
    http: &Option<HealthHttp>,
    port: &Option<HealthPort>,
) -> u32 {
    [
        cmd.as_ref().and_then(|c| c.retries),
        http.as_ref().and_then(|h| h.retries),
        port.as_ref().and_then(|p| p.retries),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or_else(|| {
        settings()
            .supervisor
            .health_check_retries
            .clamp(1, u32::MAX as i64) as u32
    })
}

/// Run one command health probe. Healthy = exit code 0; spawn failures, io
/// errors and timeouts all count as failed. On timeout the probe is cancelled
/// so its child process is killed.
async fn health_cmd_probe(id: &DaemonId, daemon: &Daemon, cmd: &HealthCmd) -> bool {
    let dir = daemon.dir.as_deref().unwrap_or_else(|| env::CWD.as_path());
    let probe = spawn_cmd_probe(
        id,
        &cmd.run,
        dir,
        daemon.retry_count,
        daemon.env.as_ref(),
        &daemon.resolved_port,
    );
    let timeout = effective_cmd_timeout(cmd);
    match tokio::time::timeout(timeout, probe.result_rx).await {
        Ok(Ok(Ok(status))) => status.success(),
        // Spawn failure (channel closed) or io error: treated as failed.
        Ok(_) => false,
        // Timed out: cancel the probe so the child process is killed.
        Err(_) => {
            let _ = probe.cancel_tx.send(());
            false
        }
    }
}

/// Run one HTTP health probe. Healthy = the response status is in the
/// configured list, or any 2xx when the list is empty. Connection errors and
/// timeouts count as failed.
async fn health_http_probe(id: &DaemonId, http: &HealthHttp, client: &reqwest::Client) -> bool {
    let timeout = effective_http_timeout(http);
    let response = match tokio::time::timeout(timeout, client.get(&http.url).send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => {
            debug!("daemon {id} health check (http) request failed: {e}");
            return false;
        }
        Err(_) => {
            debug!("daemon {id} health check (http) timed out after {timeout:?}");
            return false;
        }
    };
    let status = response.status().as_u16();
    if http.status.is_empty() {
        (200..300).contains(&status)
    } else {
        http.status.contains(&status)
    }
}

/// Run one TCP port health probe. Healthy = a connection to 127.0.0.1:<port>
/// succeeds; connection refused/reset counts as failed. The connect is bounded
/// by the per-daemon `health_port.timeout` (or `supervisor.health_port_timeout`)
/// so a saturated accept backlog cannot stall the probe loop indefinitely.
async fn health_port_probe(id: &DaemonId, port: u16, timeout: Duration) -> bool {
    match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(("127.0.0.1", port))).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => {
            debug!("daemon {id} health check (port) connect to {port} failed: {e}");
            false
        }
        Err(_) => {
            debug!("daemon {id} health check (port) connect to {port} timed out after {timeout:?}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health_cmd(
        run: &str,
        interval: Option<Duration>,
        timeout: Option<Duration>,
        retries: Option<u32>,
    ) -> Option<HealthCmd> {
        Some(HealthCmd {
            run: run.into(),
            interval,
            timeout,
            retries,
        })
    }

    fn health_http(
        url: &str,
        status: Vec<u16>,
        interval: Option<Duration>,
        timeout: Option<Duration>,
        retries: Option<u32>,
    ) -> Option<HealthHttp> {
        Some(HealthHttp {
            url: url.into(),
            status,
            interval,
            timeout,
            retries,
        })
    }

    fn health_port(
        port: u16,
        interval: Option<Duration>,
        retries: Option<u32>,
    ) -> Option<HealthPort> {
        Some(HealthPort {
            port: Some(port),
            template: None,
            interval,
            retries,
            timeout: None,
        })
    }

    #[test]
    fn effective_interval_defaults_to_10s() {
        assert_eq!(
            effective_interval(&None, &None, &None),
            settings().supervisor_health_check_interval()
        );
        assert_eq!(
            effective_interval(
                &health_cmd("true", None, None, None),
                &None,
                &health_port(8443, None, None)
            ),
            settings().supervisor_health_check_interval()
        );
    }

    #[test]
    fn effective_interval_prefers_cmd_then_http_then_port_override() {
        let cmd_interval = Duration::from_secs(2);
        let http_interval = Duration::from_secs(7);
        let port_interval = Duration::from_secs(11);
        assert_eq!(
            effective_interval(
                &health_cmd("true", Some(cmd_interval), None, None),
                &None,
                &None
            ),
            cmd_interval
        );
        assert_eq!(
            effective_interval(
                &None,
                &health_http("http://x", vec![], Some(http_interval), None, None),
                &health_port(8443, Some(port_interval), None),
            ),
            http_interval
        );
        // port wins when only it is set.
        assert_eq!(
            effective_interval(&None, &None, &health_port(8443, Some(port_interval), None),),
            port_interval
        );
        // cmd wins when all three are set — one sleep per loop, cmd listed first.
        assert_eq!(
            effective_interval(
                &health_cmd("true", Some(cmd_interval), None, None),
                &health_http("http://x", vec![], Some(http_interval), None, None),
                &health_port(8443, Some(port_interval), None),
            ),
            cmd_interval
        );
    }

    #[test]
    fn effective_cmd_timeout_defaults_and_overrides() {
        assert_eq!(
            effective_cmd_timeout(&HealthCmd::new("true")),
            settings().supervisor_health_cmd_timeout()
        );
        let override_ = Duration::from_secs(3);
        assert_eq!(
            effective_cmd_timeout(&HealthCmd {
                run: "true".into(),
                interval: None,
                timeout: Some(override_),
                retries: None,
            }),
            override_
        );
    }

    #[test]
    fn effective_http_timeout_defaults_and_overrides() {
        assert_eq!(
            effective_http_timeout(&HealthHttp::new("http://x")),
            settings().supervisor_health_http_timeout()
        );
        let override_ = Duration::from_secs(2);
        assert_eq!(
            effective_http_timeout(&HealthHttp {
                url: "http://x".into(),
                status: vec![],
                interval: None,
                timeout: Some(override_),
                retries: None,
            }),
            override_
        );
    }

    #[test]
    fn effective_retries_defaults_and_takes_strictest_budget() {
        assert_eq!(
            effective_retries(&None, &None, &None),
            settings().supervisor.health_check_retries.max(1) as u32
        );
        assert_eq!(
            effective_retries(&health_cmd("true", None, None, Some(2)), &None, &None),
            2
        );
        assert_eq!(
            effective_retries(
                &None,
                &health_http("http://x", vec![], None, None, Some(5)),
                &None
            ),
            5
        );
        assert_eq!(
            effective_retries(&None, &None, &health_port(8443, None, Some(4))),
            4
        );
        // Mixed kinds: the lowest threshold wins, so any probe can trigger
        // the kill within its own budget.
        assert_eq!(
            effective_retries(
                &health_cmd("true", None, None, Some(5)),
                &health_http("http://x", vec![], None, None, Some(2)),
                &health_port(8443, None, Some(3)),
            ),
            2
        );
        // A kind without retries still counts its default.
        assert_eq!(
            effective_retries(
                &health_cmd("true", None, None, None),
                &health_http("http://x", vec![], None, None, Some(1)),
                &None,
            ),
            1
        );
    }

    #[test]
    fn effective_port_timeout_defaults_and_overrides() {
        assert_eq!(
            effective_port_timeout(&HealthPort::new(8443)),
            settings().supervisor_health_port_timeout()
        );
        let override_ = Duration::from_secs(9);
        assert_eq!(
            effective_port_timeout(&HealthPort {
                port: Some(8443),
                template: None,
                interval: None,
                retries: None,
                timeout: Some(override_),
            }),
            override_
        );
    }

    #[test]
    fn process_identity_changed_resets_on_reused_pid() {
        // Same PID, different start time: a new process generation.
        assert!(process_identity_changed(
            Some(42),
            Some(100),
            Some(42),
            Some(200)
        ));
        // Same PID, same start time: the same process.
        assert!(!process_identity_changed(
            Some(42),
            Some(100),
            Some(42),
            Some(100)
        ));
        // Different PID: a new process regardless of start time.
        assert!(process_identity_changed(
            Some(42),
            Some(100),
            Some(43),
            Some(100)
        ));
        // First probe (no last identity yet): always a fresh budget.
        assert!(process_identity_changed(None, None, Some(42), Some(100)));
    }

    #[tokio::test]
    async fn health_port_probe_connects_to_listening_port() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(health_port_probe(&DaemonId::new("ns", "x"), port, Duration::from_secs(5)).await);

        // A closed port must count as failed.
        drop(listener);
        assert!(!health_port_probe(&DaemonId::new("ns", "x"), port, Duration::from_secs(5)).await);
    }
}
