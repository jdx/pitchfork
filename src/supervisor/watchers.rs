//! Background watcher tasks
//!
//! Spawns background tasks for:
//! - Interval watching (periodic refresh)
//! - Cron scheduling
//! - File watching for daemon auto-restart

use super::{SUPERVISOR, Supervisor, interval_duration};
use crate::daemon_id::DaemonId;
use crate::ipc::IpcResponse;
use crate::pitchfork_toml::PitchforkToml;
use crate::procs::PROCS;
use crate::settings::settings;
use crate::watch_files::{WatchFiles, expand_watch_patterns, path_matches_patterns};
use crate::{Result, env};
use notify::RecursiveMode;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tokio::time;

impl Supervisor {
    /// Get all watch configurations from the current state of daemons.
    pub(crate) async fn get_all_watch_configs(&self) -> Vec<(DaemonId, Vec<String>, PathBuf)> {
        let state = self.state_file.lock().await;
        state
            .daemons
            .values()
            .filter(|d| !d.watch.is_empty())
            .map(|d| {
                let base_dir = d.watch_base_dir.clone().unwrap_or_else(|| env::CWD.clone());
                (d.id.clone(), d.watch.clone(), base_dir)
            })
            .collect()
    }

    /// Start the interval watcher for periodic refresh and resource monitoring
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
                // Check resource limits (CPU and memory) for all running daemons
                if let Err(err) = SUPERVISOR.check_resource_limits().await {
                    error!("failed to check resource limits: {err}");
                }
            }
        });
        Ok(())
    }

    /// Check resource limits (CPU and memory) for all running daemons.
    ///
    /// For each daemon with a `memory_limit` or `cpu_limit` configured, this method
    /// reads the current RSS / CPU% from sysinfo and kills the daemon if it exceeds
    /// the configured threshold. The kill is done without setting `Stopping` status,
    /// so the monitor task treats it as a failure (`Errored`), which allows retry
    /// logic to kick in if configured.
    async fn check_resource_limits(&self) -> Result<()> {
        // Quick check: does any daemon have resource limits configured?
        // This avoids acquiring the state lock on every tick when no limits are set.
        let daemons: Vec<_> = {
            let pitchfork_id = DaemonId::pitchfork();
            let state = self.state_file.lock().await;
            let has_any_limits = state.daemons.values().any(|d| {
                d.id != pitchfork_id && (d.memory_limit.is_some() || d.cpu_limit.is_some())
            });
            if !has_any_limits {
                return Ok(());
            }
            state
                .daemons
                .values()
                .filter(|d| {
                    d.id != pitchfork_id
                        && d.pid.is_some()
                        && d.status.is_running()
                        && (d.memory_limit.is_some() || d.cpu_limit.is_some())
                })
                .cloned()
                .collect()
        };

        if daemons.is_empty() {
            return Ok(());
        }

        // Refresh all processes so we can walk the process tree for each daemon.
        // This is necessary to aggregate stats across multi-process daemons
        // (e.g. gunicorn/nginx workers) where child processes may consume
        // significant resources beyond the root PID.
        PROCS.refresh_processes();

        for daemon in &daemons {
            let Some(pid) = daemon.pid else { continue };
            let Some(stats) = PROCS.get_group_stats(pid) else {
                continue;
            };

            // Check memory limit (RSS)
            if let Some(mem_limit) = daemon.memory_limit {
                if stats.memory_bytes > mem_limit.0 {
                    warn!(
                        "daemon {} (pid {}) exceeded memory limit: {} > {}, stopping",
                        daemon.id,
                        pid,
                        stats.memory_display(),
                        mem_limit,
                    );
                    self.stop_for_resource_violation(&daemon.id, pid).await;
                    continue; // Don't check CPU if we're already killing
                }
            }

            // Check CPU limit (percentage)
            if let Some(cpu_limit) = daemon.cpu_limit {
                if stats.cpu_percent > cpu_limit.0 {
                    warn!(
                        "daemon {} (pid {}) exceeded CPU limit: {:.1}% > {}%, stopping",
                        daemon.id, pid, stats.cpu_percent, cpu_limit.0,
                    );
                    self.stop_for_resource_violation(&daemon.id, pid).await;
                }
            }
        }

        Ok(())
    }

    /// Kill a daemon due to a resource limit violation.
    ///
    /// Unlike `stop()`, this does NOT set the daemon status to `Stopping` first.
    /// Instead, it kills the process group directly, which causes the monitor task
    /// to observe a non-zero exit and set the status to `Errored`. This allows
    /// the retry checker to restart the daemon if `retry` is configured.
    async fn stop_for_resource_violation(&self, id: &DaemonId, pid: u32) {
        info!("killing daemon {id} (pid {pid}) due to resource limit violation");
        if let Err(e) = PROCS.kill_process_group_async(pid).await {
            error!("failed to kill daemon {id} (pid {pid}) after resource violation: {e}");
        }
    }

    /// Start the cron watcher for scheduled daemon execution
    pub(crate) fn cron_watch(&self) -> Result<()> {
        tokio::spawn(async move {
            // Check every cron_check_interval to support sub-minute cron schedules
            let mut interval = time::interval(settings().supervisor_cron_check_interval());
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
        let cron_daemon_ids: Vec<DaemonId> = {
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
                                    error!("failed to parse command for cron daemon {id}: {e}");
                                    continue;
                                }
                            };
                            let dir = daemon.dir.clone().unwrap_or_else(|| env::CWD.clone());
                            // Use force: true for Always retrigger to ensure restart
                            let force =
                                matches!(retrigger, crate::pitchfork_toml::CronRetrigger::Always);
                            let mut opts = daemon.to_run_options(cmd);
                            opts.dir = dir;
                            opts.force = force;
                            opts.cron_schedule = Some(schedule_str.clone());
                            opts.cron_retrigger = Some(retrigger);
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
        let pt = PitchforkToml::all_merged()?;

        // Collect all daemons with watch patterns and their base directories
        let watch_configs: Vec<(DaemonId, Vec<String>, std::path::PathBuf)> = pt
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
                    warn!("Failed to expand watch patterns for {id}: {e}");
                }
            }
        }

        if all_dirs.is_empty() {
            debug!("No directories to watch after expanding patterns");
            return Ok(());
        }

        // Spawn the file watcher task
        tokio::spawn(async move {
            let mut wf = match WatchFiles::new(settings().supervisor_file_watch_debounce()) {
                Ok(wf) => wf,
                Err(e) => {
                    error!("Failed to create file watcher: {e}");
                    return;
                }
            };

            let mut watched_dirs = HashSet::new();
            info!("File watcher started");

            loop {
                // Refresh watch configurations from state
                let watch_configs = SUPERVISOR.get_all_watch_configs().await;

                // Collect all required directories and track which daemons need them
                let mut required_dirs = HashSet::new();
                let mut dir_to_daemons: HashMap<PathBuf, Vec<DaemonId>> = HashMap::new();

                for (id, patterns, base_dir) in &watch_configs {
                    match expand_watch_patterns(patterns, base_dir) {
                        Ok(dirs) => {
                            for dir in dirs {
                                required_dirs.insert(dir.clone());
                                dir_to_daemons.entry(dir).or_default().push(id.clone());
                            }
                        }
                        Err(e) => {
                            warn!("Failed to expand watch patterns for {id}: {e}");
                        }
                    }
                }

                // Unwatch directories that are no longer needed
                for dir in watched_dirs.difference(&required_dirs) {
                    debug!("Unwatching directory {}", dir.display());
                    if let Err(e) = wf.unwatch(dir) {
                        warn!("Failed to unwatch directory {}: {}", dir.display(), e);
                    }
                }

                // Watch new directories
                for dir in required_dirs.difference(&watched_dirs) {
                    let daemon_ids = dir_to_daemons
                        .get(dir)
                        .map(|ids| {
                            ids.iter()
                                .map(|id| id.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    debug!("Watching {} for daemon(s): {}", dir.display(), daemon_ids);
                    if let Err(e) = wf.watch(dir, RecursiveMode::Recursive) {
                        warn!("Failed to watch directory {}: {}", dir.display(), e);
                    }
                }

                // Update the set of watched directories
                watched_dirs = required_dirs;

                // Wait for file changes or a refresh interval
                let watch_interval = settings().supervisor_watch_interval();
                tokio::select! {
                    Some(changed_paths) = wf.rx.recv() => {
                        debug!("File changes detected: {changed_paths:?}");

                        // Find which daemons should be restarted based on the changed paths
                        let mut daemons_to_restart = HashSet::new();

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
                                error!("Failed to restart daemon {id} after file change: {e}");
                            }
                        }
                    }
                    _ = tokio::time::sleep(watch_interval) => {
                        // Periodically refresh watch configs to pick up new daemons
                        trace!("Refreshing file watch configurations");
                    }
                }
            }
        });

        Ok(())
    }

    /// Restart a daemon that is being watched for file changes.
    /// Only restarts if the daemon is currently running.
    pub(crate) async fn restart_watched_daemon(&self, id: &DaemonId) -> Result<()> {
        // Check if daemon is running
        let daemon = self.get_daemon(id).await;
        let Some(daemon) = daemon else {
            warn!("Daemon {id} not found in state, cannot restart");
            return Ok(());
        };

        let is_running = daemon.pid.is_some() && daemon.status.is_running();

        if !is_running {
            debug!("Daemon {id} is not running, skipping restart on file change");
            return Ok(());
        }

        // Check if daemon is disabled
        let is_disabled = self.state_file.lock().await.disabled.contains(id);
        if is_disabled {
            debug!("Daemon {id} is disabled, skipping restart on file change");
            return Ok(());
        }

        info!("Restarting daemon {id} due to file change");

        // Use values from the daemon state to rebuild RunOptions
        let cmd = match &daemon.cmd {
            Some(cmd) => cmd.clone(),
            None => {
                error!("Daemon {id} has no command in state, cannot restart");
                return Ok(());
            }
        };

        // Stop the daemon first
        let _ = self.stop(id).await;

        // Small delay to allow the process to fully stop
        time::sleep(settings().supervisor_restart_delay()).await;

        // Restart the daemon
        let mut run_opts = daemon.to_run_options(cmd);
        run_opts.force = true;
        run_opts.retry_count = 0;

        match self.run(run_opts).await {
            Ok(IpcResponse::DaemonStart { .. }) | Ok(IpcResponse::DaemonReady { .. }) => {
                info!("Successfully restarted daemon {id} after file change");
            }
            Ok(other) => {
                warn!("Unexpected response when restarting daemon {id}: {other:?}");
            }
            Err(e) => {
                error!("Failed to restart daemon {id}: {e}");
            }
        }

        Ok(())
    }
}
