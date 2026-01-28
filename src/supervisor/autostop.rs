//! Autostop logic and boot daemon startup
//!
//! Handles automatic stopping of daemons when shells leave directories,
//! and starting daemons configured with `boot_start = true`.

use super::Supervisor;
use crate::daemon::RunOptions;
use crate::ipc::IpcResponse;
use crate::pitchfork_toml::PitchforkToml;
use crate::{Result, env};
use log::LevelFilter::Info;
use std::path::Path;
use std::time::Duration;
use tokio::time;

impl Supervisor {
    /// Handle shell leaving a directory - schedule autostops for daemons
    pub(crate) async fn leave_dir(&self, dir: &Path) -> Result<()> {
        debug!("left dir {}", dir.display());
        let shell_dirs = self.get_dirs_with_shell_pids().await;
        let shell_dirs = shell_dirs.keys().collect::<Vec<_>>();
        let delay_secs = *env::PITCHFORK_AUTOSTOP_DELAY;

        for daemon in self.active_daemons().await {
            if !daemon.autostop {
                continue;
            }
            // if this daemon's dir starts with the left dir
            // and no other shell pid has this dir as a prefix
            // schedule the daemon for autostop
            if let Some(daemon_dir) = daemon.dir.as_ref()
                && daemon_dir.starts_with(dir)
                && !shell_dirs.iter().any(|d| d.starts_with(daemon_dir))
            {
                if delay_secs == 0 {
                    // No delay configured, stop immediately
                    info!("autostopping {daemon}");
                    self.stop(&daemon.id).await?;
                    self.add_notification(Info, format!("autostopped {daemon}"))
                        .await;
                } else {
                    // Schedule autostop with delay
                    let stop_at = time::Instant::now() + Duration::from_secs(delay_secs);
                    let mut pending = self.pending_autostops.lock().await;
                    if !pending.contains_key(&daemon.id) {
                        info!("scheduling autostop for {} in {}s", daemon.id, delay_secs);
                        pending.insert(daemon.id.clone(), stop_at);
                    }
                }
            }
        }
        Ok(())
    }

    /// Cancel any pending autostop for daemons in the given directory
    /// Also cancels autostops for daemons in parent directories (e.g., entering /project/subdir
    /// cancels pending autostop for daemon in /project)
    pub(crate) async fn cancel_pending_autostops_for_dir(&self, dir: &Path) {
        let mut pending = self.pending_autostops.lock().await;
        let daemons_to_cancel: Vec<String> = {
            let state_file = self.state_file.lock().await;
            state_file
                .daemons
                .iter()
                .filter(|(_id, d)| {
                    d.dir.as_ref().is_some_and(|daemon_dir| {
                        // Cancel if entering a directory inside or equal to daemon's directory
                        // OR if daemon is in a subdirectory of the entered directory
                        dir.starts_with(daemon_dir) || daemon_dir.starts_with(dir)
                    })
                })
                .map(|(id, _)| id.clone())
                .collect()
        };

        for daemon_id in daemons_to_cancel {
            if pending.remove(&daemon_id).is_some() {
                info!("cancelled pending autostop for {daemon_id}");
            }
        }
    }

    /// Process any pending autostops that have reached their scheduled time
    pub(crate) async fn process_pending_autostops(&self) -> Result<()> {
        let now = time::Instant::now();
        let to_stop: Vec<String> = {
            let pending = self.pending_autostops.lock().await;
            pending
                .iter()
                .filter(|(_, stop_at)| now >= **stop_at)
                .map(|(id, _)| id.clone())
                .collect()
        };

        for daemon_id in to_stop {
            // Remove from pending first
            {
                let mut pending = self.pending_autostops.lock().await;
                pending.remove(&daemon_id);
            }

            // Check if daemon is still running and should be stopped
            if let Some(daemon) = self.get_daemon(&daemon_id).await
                && daemon.autostop
                && daemon.status.is_running()
            {
                // Verify no shell is in the daemon's directory
                let shell_dirs = self.get_dirs_with_shell_pids().await;
                let shell_dirs = shell_dirs.keys().collect::<Vec<_>>();
                if let Some(daemon_dir) = daemon.dir.as_ref()
                    && !shell_dirs.iter().any(|d| d.starts_with(daemon_dir))
                {
                    info!("autostopping {daemon_id} (after delay)");
                    self.stop(&daemon_id).await?;
                    self.add_notification(Info, format!("autostopped {daemon_id}"))
                        .await;
                }
            }
        }
        Ok(())
    }

    /// Start daemons configured with `boot_start = true`
    pub(crate) async fn start_boot_daemons(&self) -> Result<()> {
        info!("Scanning for boot_start daemons");
        let pt = PitchforkToml::all_merged();

        let boot_daemons: Vec<_> = pt
            .daemons
            .iter()
            .filter(|(_id, d)| d.boot_start.unwrap_or(false))
            .collect();

        if boot_daemons.is_empty() {
            info!("No daemons configured with boot_start = true");
            return Ok(());
        }

        info!("Found {} daemon(s) to start at boot", boot_daemons.len());

        for (id, daemon) in boot_daemons {
            info!("Starting boot daemon: {id}");

            let dir = daemon
                .path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| env::CWD.clone());

            let cmd = match shell_words::split(&daemon.run) {
                Ok(cmd) => cmd,
                Err(e) => {
                    error!("failed to parse command for boot daemon {id}: {e}");
                    continue;
                }
            };
            let run_opts = RunOptions {
                id: id.clone(),
                cmd,
                force: false,
                shell_pid: None,
                dir,
                autostop: false, // Boot daemons should not autostop
                cron_schedule: daemon.cron.as_ref().map(|c| c.schedule.clone()),
                cron_retrigger: daemon.cron.as_ref().map(|c| c.retrigger),
                retry: daemon.retry.count(),
                retry_count: 0,
                ready_delay: daemon.ready_delay,
                ready_output: daemon.ready_output.clone(),
                ready_http: daemon.ready_http.clone(),
                ready_port: daemon.ready_port,
                ready_cmd: daemon.ready_cmd.clone(),
                wait_ready: false, // Don't block on boot daemons
                depends: daemon.depends.clone(),
            };

            match self.run(run_opts).await {
                Ok(IpcResponse::DaemonStart { .. }) | Ok(IpcResponse::DaemonReady { .. }) => {
                    info!("Successfully started boot daemon: {id}");
                }
                Ok(IpcResponse::DaemonAlreadyRunning) => {
                    info!("Boot daemon already running: {id}");
                }
                Ok(other) => {
                    warn!("Unexpected response when starting boot daemon {id}: {other:?}");
                }
                Err(e) => {
                    error!("Failed to start boot daemon {id}: {e}");
                }
            }
        }

        Ok(())
    }
}
