//! Background watcher tasks
//!
//! Spawns background tasks for:
//! - Interval watching (periodic refresh)
//! - Cron scheduling
//! - File watching for daemon auto-restart

use super::{SUPERVISOR, Supervisor, interval_duration};
use crate::daemon::RunOptions;
use crate::ipc::IpcResponse;
use crate::pitchfork_toml::PitchforkToml;
use crate::watch_files::{WatchFiles, expand_watch_patterns, path_matches_patterns};
use crate::{Result, env};
use notify::RecursiveMode;
use std::time::Duration;
use tokio::time;

impl Supervisor {
    /// Start the interval watcher for periodic refresh
    pub(crate) fn interval_watch(&self) -> Result<()> {
        tokio::spawn(async move {
            let mut interval = time::interval(interval_duration());
            loop {
                interval.tick().await;
                if SUPERVISOR.last_refreshed_at.lock().await.elapsed() > interval_duration()
                    && let Err(err) = SUPERVISOR.refresh().await
                {
                    error!("failed to refresh: {err}");
                }
            }
        });
        Ok(())
    }

    /// Start the cron watcher for scheduled daemon execution
    pub(crate) fn cron_watch(&self) -> Result<()> {
        tokio::spawn(async move {
            // Check every 10 seconds to support sub-minute cron schedules
            let mut interval = time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                if let Err(err) = SUPERVISOR.check_cron_schedules().await {
                    error!("failed to check cron schedules: {err}");
                }
            }
        });
        Ok(())
    }

    /// Check cron schedules and trigger daemons as needed
    pub(crate) async fn check_cron_schedules(&self) -> Result<()> {
        use cron::Schedule;
        use std::str::FromStr;

        let now = chrono::Local::now();

        // Collect only IDs of daemons with cron schedules (avoids cloning entire HashMap)
        let cron_daemon_ids: Vec<String> = {
            let state_file = self.state_file.lock().await;
            state_file
                .daemons
                .iter()
                .filter(|(_id, d)| d.cron_schedule.is_some() && d.cron_retrigger.is_some())
                .map(|(id, _d)| id.clone())
                .collect()
        };

        for id in cron_daemon_ids {
            // Look up daemon when needed
            let daemon = {
                let state_file = self.state_file.lock().await;
                match state_file.daemons.get(&id) {
                    Some(d) => d.clone(),
                    None => continue,
                }
            };

            if let Some(schedule_str) = &daemon.cron_schedule
                && let Some(retrigger) = daemon.cron_retrigger
            {
                // Parse the cron schedule
                let schedule = match Schedule::from_str(schedule_str) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("invalid cron schedule for daemon {id}: {e}");
                        continue;
                    }
                };

                // Check if we should trigger: look for a scheduled time that has passed
                // since our last trigger (or last 10 seconds if never triggered)
                let check_since = daemon
                    .last_cron_triggered
                    .unwrap_or_else(|| now - chrono::Duration::seconds(10));

                // Find if there's a scheduled time between check_since and now
                let should_trigger = schedule
                    .after(&check_since)
                    .take_while(|t| *t <= now)
                    .next()
                    .is_some();

                if should_trigger {
                    // Update last_cron_triggered to prevent re-triggering the same event
                    {
                        let mut state_file = self.state_file.lock().await;
                        if let Some(d) = state_file.daemons.get_mut(&id) {
                            d.last_cron_triggered = Some(now);
                        }
                        if let Err(e) = state_file.write() {
                            error!("failed to update cron trigger time: {e}");
                        }
                    }

                    let should_run = match retrigger {
                        crate::pitchfork_toml::CronRetrigger::Finish => {
                            // Run if not currently running
                            daemon.pid.is_none()
                        }
                        crate::pitchfork_toml::CronRetrigger::Always => {
                            // Always run (force restart handled in run method)
                            true
                        }
                        crate::pitchfork_toml::CronRetrigger::Success => {
                            // Run only if previous command succeeded
                            daemon.pid.is_none() && daemon.last_exit_success.unwrap_or(false)
                        }
                        crate::pitchfork_toml::CronRetrigger::Fail => {
                            // Run only if previous command failed
                            daemon.pid.is_none() && !daemon.last_exit_success.unwrap_or(true)
                        }
                    };

                    if should_run {
                        info!("cron: triggering daemon {id} (retrigger: {retrigger:?})");
                        // Get the run command from pitchfork.toml
                        if let Some(run_cmd) = self.get_daemon_run_command(&id) {
                            let cmd = match shell_words::split(&run_cmd) {
                                Ok(cmd) => cmd,
                                Err(e) => {
                                    error!("failed to parse command for cron daemon {}: {}", id, e);
                                    continue;
                                }
                            };

                            // Re-resolve directory from config to handle config changes
                            let dir = {
                                let pt = crate::pitchfork_toml::PitchforkToml::all_merged();
                                if let Some(daemon_config) = pt.daemons.get(&id) {
                                    crate::pitchfork_toml::resolve_daemon_dir(
                                        daemon_config.dir.as_deref(),
                                        daemon_config.path.as_ref(),
                                    )
                                    .unwrap_or_else(|| env::CWD.clone())
                                } else {
                                    daemon.dir.clone().unwrap_or_else(|| env::CWD.clone())
                                }
                            };
                            // Use force: true for Always retrigger to ensure restart
                            let force =
                                matches!(retrigger, crate::pitchfork_toml::CronRetrigger::Always);
                            let opts = RunOptions {
                                id: id.clone(),
                                cmd,
                                force,
                                shell_pid: None,
                                dir,
                                autostop: daemon.autostop,
                                cron_schedule: Some(schedule_str.clone()),
                                cron_retrigger: Some(retrigger),
                                retry: daemon.retry,
                                retry_count: daemon.retry_count,
                                ready_delay: daemon.ready_delay,
                                ready_output: daemon.ready_output.clone(),
                                ready_http: daemon.ready_http.clone(),
                                ready_port: daemon.ready_port,
                                wait_ready: false,
                                depends: daemon.depends.clone(),
                            };
                            if let Err(e) = self.run(opts).await {
                                error!("failed to run cron daemon {id}: {e}");
                            }
                        } else {
                            warn!("no run command found for cron daemon {id}");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Watch files for daemons that have `watch` patterns configured.
    /// When a watched file changes, the daemon is automatically restarted.
    pub(crate) fn daemon_file_watch(&self) -> Result<()> {
        let pt = PitchforkToml::all_merged();

        // Collect all daemons with watch patterns and their base directories
        let watch_configs: Vec<(String, Vec<String>, std::path::PathBuf)> = pt
            .daemons
            .iter()
            .filter(|(_, d)| !d.watch.is_empty())
            .map(|(id, d)| {
                let base_dir = d
                    .path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| env::CWD.clone());
                (id.clone(), d.watch.clone(), base_dir)
            })
            .collect();

        if watch_configs.is_empty() {
            debug!("No daemons with watch patterns configured");
            return Ok(());
        }

        info!(
            "Setting up file watching for {} daemon(s)",
            watch_configs.len()
        );

        // Collect all directories to watch
        let mut all_dirs = std::collections::HashSet::new();
        for (id, patterns, base_dir) in &watch_configs {
            match expand_watch_patterns(patterns, base_dir) {
                Ok(dirs) => {
                    for dir in &dirs {
                        debug!("Watching {} for daemon {}", dir.display(), id);
                    }
                    all_dirs.extend(dirs);
                }
                Err(e) => {
                    warn!("Failed to expand watch patterns for {}: {}", id, e);
                }
            }
        }

        if all_dirs.is_empty() {
            debug!("No directories to watch after expanding patterns");
            return Ok(());
        }

        // Spawn the file watcher task
        tokio::spawn(async move {
            let mut wf = match WatchFiles::new(Duration::from_secs(1)) {
                Ok(wf) => wf,
                Err(e) => {
                    error!("Failed to create file watcher: {}", e);
                    return;
                }
            };

            // Register all directories with the watcher
            for dir in all_dirs {
                if let Err(e) = wf.watch(&dir, RecursiveMode::Recursive) {
                    warn!("Failed to watch directory {}: {}", dir.display(), e);
                }
            }

            info!("File watcher started");

            // Process file change events
            while let Some(changed_paths) = wf.rx.recv().await {
                debug!("File changes detected: {:?}", changed_paths);

                // Find which daemons should be restarted based on the changed paths
                let mut daemons_to_restart = std::collections::HashSet::new();

                for changed_path in &changed_paths {
                    for (id, patterns, base_dir) in &watch_configs {
                        if path_matches_patterns(changed_path, patterns, base_dir) {
                            info!(
                                "File {} matched pattern for daemon {}, scheduling restart",
                                changed_path.display(),
                                id
                            );
                            daemons_to_restart.insert(id.clone());
                        }
                    }
                }

                // Restart each affected daemon
                for id in daemons_to_restart {
                    if let Err(e) = SUPERVISOR.restart_watched_daemon(&id).await {
                        error!("Failed to restart daemon {} after file change: {}", id, e);
                    }
                }
            }
        });

        Ok(())
    }

    /// Restart a daemon that is being watched for file changes.
    /// Only restarts if the daemon is currently running.
    pub(crate) async fn restart_watched_daemon(&self, id: &str) -> Result<()> {
        // Check if daemon is running
        let daemon = self.get_daemon(id).await;
        let is_running = daemon
            .as_ref()
            .is_some_and(|d| d.pid.is_some() && d.status.is_running());

        if !is_running {
            debug!(
                "Daemon {} is not running, skipping restart on file change",
                id
            );
            return Ok(());
        }

        // Check if daemon is disabled
        let is_disabled = self.state_file.lock().await.disabled.contains(id);
        if is_disabled {
            debug!("Daemon {} is disabled, skipping restart on file change", id);
            return Ok(());
        }

        info!("Restarting daemon {} due to file change", id);

        // Get the daemon config to rebuild RunOptions
        let pt = PitchforkToml::all_merged();
        let Some(daemon_config) = pt.daemons.get(id) else {
            warn!("Daemon {} not found in config, cannot restart", id);
            return Ok(());
        };

        let dir = crate::pitchfork_toml::resolve_daemon_dir(
            daemon_config.dir.as_deref(),
            daemon_config.path.as_ref(),
        )
        .unwrap_or_else(|| env::CWD.clone());

        let cmd = match shell_words::split(&daemon_config.run) {
            Ok(cmd) => cmd,
            Err(e) => {
                error!("Failed to parse command for daemon {}: {}", id, e);
                return Ok(());
            }
        };

        // Extract values from daemon before stopping
        let shell_pid = daemon.as_ref().and_then(|d| d.shell_pid);
        let autostop = daemon.as_ref().map(|d| d.autostop).unwrap_or(false);

        // Stop the daemon first
        let _ = self.stop(id).await;

        // Small delay to allow the process to fully stop
        time::sleep(Duration::from_millis(100)).await;

        // Restart the daemon
        let run_opts = RunOptions {
            id: id.to_string(),
            cmd,
            force: true,
            shell_pid,
            dir,
            autostop,
            cron_schedule: daemon_config.cron.as_ref().map(|c| c.schedule.clone()),
            cron_retrigger: daemon_config.cron.as_ref().map(|c| c.retrigger),
            retry: daemon_config.retry.count(),
            retry_count: 0,
            ready_delay: daemon_config.ready_delay,
            ready_output: daemon_config.ready_output.clone(),
            ready_http: daemon_config.ready_http.clone(),
            ready_port: daemon_config.ready_port,
            wait_ready: false, // Don't block on file-triggered restarts
            depends: daemon_config.depends.clone(),
        };

        match self.run(run_opts).await {
            Ok(IpcResponse::DaemonStart { .. }) | Ok(IpcResponse::DaemonReady { .. }) => {
                info!("Successfully restarted daemon {} after file change", id);
            }
            Ok(other) => {
                warn!(
                    "Unexpected response when restarting daemon {}: {:?}",
                    id, other
                );
            }
            Err(e) => {
                error!("Failed to restart daemon {}: {}", id, e);
            }
        }

        Ok(())
    }
}
