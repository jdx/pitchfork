//! Background watcher tasks
//!
//! Spawns background tasks for:
//! - Interval watching (periodic refresh)
//! - Cron scheduling
//! - File watching for daemon auto-restart

use super::{SUPERVISOR, Supervisor, interval_duration};
use crate::daemon_id::DaemonId;
use crate::ipc::IpcResponse;
use crate::pitchfork_toml::{PitchforkToml, WatchMode};
use crate::procs::PROCS;
use crate::settings::settings;
use crate::watch_files::{WatchFiles, expand_watch_patterns, path_matches_patterns};
use crate::{Result, env};
use notify::RecursiveMode;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::time;

type WatchConfig = (DaemonId, Vec<String>, PathBuf, WatchMode);

fn daemon_ids_for_dir(dir: &Path, dir_to_daemons: &HashMap<PathBuf, Vec<DaemonId>>) -> String {
    dir_to_daemons
        .get(dir)
        .map(|ids| {
            ids.iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn unwatch_removed_dirs(
    wf: &mut Option<WatchFiles>,
    watched: &HashSet<PathBuf>,
    target: &HashSet<PathBuf>,
    backend: &str,
) {
    let Some(wf) = wf.as_mut() else { return };
    for dir in watched.difference(target) {
        debug!("Unwatching directory {} ({backend})", dir.display());
        if let Err(e) = wf.unwatch(dir) {
            warn!(
                "Failed to unwatch directory {} ({backend}): {}",
                dir.display(),
                e
            );
        }
    }
}

fn watch_new_dirs(
    wf: &mut Option<WatchFiles>,
    watched: &HashSet<PathBuf>,
    target: &HashSet<PathBuf>,
    backend: &str,
    dir_to_daemons: &HashMap<PathBuf, Vec<DaemonId>>,
    auto_dirs: Option<&HashSet<PathBuf>>,
    failed_dirs: &mut HashSet<PathBuf>,
) -> HashSet<PathBuf> {
    let Some(wf) = wf.as_mut() else {
        return HashSet::new();
    };

    let mut fallback_dirs = HashSet::new();
    for dir in target.difference(watched) {
        let daemon_ids = daemon_ids_for_dir(dir, dir_to_daemons);
        debug!(
            "Watching {} for daemon(s) ({backend}): {}",
            dir.display(),
            daemon_ids
        );
        if let Err(e) = wf.watch(dir, RecursiveMode::Recursive) {
            let should_fallback = auto_dirs.is_some_and(|dirs| dirs.contains(dir));
            if should_fallback {
                warn!(
                    "{backend} watch failed for {} in auto mode, falling back to poll: {}",
                    dir.display(),
                    e
                );
                fallback_dirs.insert(dir.clone());
            } else if failed_dirs.insert(dir.clone()) {
                // Only log the first time; subsequent iterations are silenced.
                warn!(
                    "Failed to watch directory {} ({backend}): {}",
                    dir.display(),
                    e
                );
            }
        }
    }

    // Clear dirs that are no longer in target (they were unwatched) so they
    // get a fresh log if they reappear and fail again.
    failed_dirs.retain(|d| target.contains(d));

    fallback_dirs
}

impl Supervisor {
    /// Get all watch configurations from the current state of daemons.
    pub(crate) async fn get_all_watch_configs(&self) -> Vec<WatchConfig> {
        let state = self.state_file.lock().await;
        state
            .daemons
            .values()
            .filter(|d| !d.watch.is_empty())
            .map(|d| {
                let base_dir = d.watch_base_dir.clone().unwrap_or_else(|| env::CWD.clone());
                (d.id.clone(), d.watch.clone(), base_dir, d.watch_mode)
            })
            .collect()
    }

    async fn restart_for_changed_paths(
        &self,
        changed_paths: Vec<PathBuf>,
        watch_configs: &[WatchConfig],
    ) {
        let mut daemons_to_restart = HashSet::new();

        for changed_path in &changed_paths {
            for (id, patterns, base_dir, _) in watch_configs {
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

        for id in daemons_to_restart {
            if let Err(e) = self.restart_watched_daemon(&id).await {
                error!("Failed to restart daemon {id} after file change: {e}");
            }
        }
    }

    /// Start the interval watcher for periodic refresh and resource monitoring
    pub(crate) fn interval_watch(&self) -> Result<()> {
        tokio::spawn(async move {
            let mut interval = time::interval(interval_duration());
            // Track consecutive CPU-over-limit samples per daemon.
            // Kept outside the state file because it is ephemeral runtime data.
            let mut cpu_violation_counts: HashMap<DaemonId, u32> = HashMap::new();
            loop {
                interval.tick().await;
                if SUPERVISOR.last_refreshed_at.lock().await.elapsed() > interval_duration()
                    && let Err(err) = SUPERVISOR.refresh().await
                {
                    error!("failed to refresh: {err}");
                }
                // Check resource limits (CPU and memory) for all running daemons
                if let Err(err) = SUPERVISOR
                    .check_resource_limits(&mut cpu_violation_counts)
                    .await
                {
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
    async fn check_resource_limits(
        &self,
        cpu_violation_counts: &mut HashMap<DaemonId, u32>,
    ) -> Result<()> {
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

        // Collect all root PIDs and fetch stats in a single pass (O(N) instead
        // of O(D × N) when calling get_group_stats per daemon).
        let pids: Vec<u32> = daemons.iter().filter_map(|d| d.pid).collect();
        let batch_stats = PROCS.get_batch_group_stats(&pids);
        let stats_map: HashMap<u32, _> = batch_stats
            .into_iter()
            .filter_map(|(pid, stats)| stats.map(|s| (pid, s)))
            .collect();

        // Track which daemon IDs are still active so we can prune stale entries
        // from cpu_violation_counts at the end.
        let mut active_ids: HashSet<&DaemonId> = HashSet::new();

        for daemon in &daemons {
            let Some(pid) = daemon.pid else { continue };
            let Some(stats) = stats_map.get(&pid) else {
                continue;
            };
            active_ids.insert(&daemon.id);

            // Check memory limit (RSS) — immediate kill, no grace period.
            // Memory violations are not transient: once RSS exceeds the limit
            // the process is unlikely to release it without intervention.
            if let Some(mem_limit) = daemon.memory_limit {
                if stats.memory_bytes > mem_limit.0 {
                    warn!(
                        "daemon {} (pid {}) exceeded memory limit: {} > {}, stopping",
                        daemon.id,
                        pid,
                        stats.memory_display(),
                        mem_limit,
                    );
                    cpu_violation_counts.remove(&daemon.id);
                    self.stop_for_resource_violation(&daemon.id, pid).await;
                    continue; // Don't check CPU if we're already killing
                }
            }

            // Check CPU limit (percentage) with consecutive-sample threshold.
            // A single spike (JIT warm-up, burst response) should not kill the
            // daemon; only sustained over-limit usage triggers enforcement.
            if let Some(cpu_limit) = daemon.cpu_limit {
                let threshold = (settings().supervisor.cpu_violation_threshold).max(1) as u32;
                if stats.cpu_percent > cpu_limit.0 {
                    let count = cpu_violation_counts.entry(daemon.id.clone()).or_insert(0);
                    *count += 1;
                    if *count >= threshold {
                        warn!(
                            "daemon {} (pid {}) exceeded CPU limit for {} consecutive checks: \
                             {:.1}% > {}%, stopping",
                            daemon.id, pid, count, stats.cpu_percent, cpu_limit.0,
                        );
                        cpu_violation_counts.remove(&daemon.id);
                        self.stop_for_resource_violation(&daemon.id, pid).await;
                    } else {
                        debug!(
                            "daemon {} (pid {}) CPU {:.1}% > {}% ({}/{} consecutive violations)",
                            daemon.id, pid, stats.cpu_percent, cpu_limit.0, count, threshold,
                        );
                    }
                } else {
                    // Below limit — reset the counter
                    cpu_violation_counts.remove(&daemon.id);
                }
            }
        }

        // Prune counters for daemons that are no longer running/tracked
        cpu_violation_counts.retain(|id, _| active_ids.contains(id));

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
                            opts.wait_ready = false;
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
        let watch_configs: Vec<WatchConfig> = pt
            .daemons
            .iter()
            .filter(|(_, d)| !d.watch.is_empty())
            .map(|(id, d)| {
                let base_dir = crate::ipc::batch::resolve_config_base_dir(d.path.as_deref());
                (id.clone(), d.watch.clone(), base_dir, d.watch_mode)
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
        for (id, patterns, base_dir, _watch_mode) in &watch_configs {
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
            let debounce = settings().supervisor_file_watch_debounce();
            let poll_interval = settings().supervisor_watch_poll_interval();

            let mut native_wf: Option<WatchFiles> = None;
            let mut poll_wf: Option<WatchFiles> = None;
            let mut native_creation_failed = false;
            let mut poll_creation_failed = false;
            let mut watched_native_dirs = HashSet::new();
            let mut watched_poll_dirs = HashSet::new();
            // Directories that previously failed native watch in auto mode and
            // are permanently tracked by the poll watcher. Maps dir → set of
            // daemon IDs that originally triggered the fallback, so entries for
            // removed daemons are pruned even when a different daemon uses the
            // same dir (which should get a fresh native-watch attempt).
            let mut auto_fallback_dirs: HashMap<PathBuf, HashSet<DaemonId>> = HashMap::new();
            // Dirs for which wf.watch() has already failed; suppresses repeated
            // warn-level logs on every loop iteration.
            let mut failed_native_watch_dirs: HashSet<PathBuf> = HashSet::new();
            let mut failed_poll_watch_dirs: HashSet<PathBuf> = HashSet::new();

            info!("File watcher started");

            loop {
                // Refresh watch configurations from state
                let watch_configs = SUPERVISOR.get_all_watch_configs().await;

                // Collect required directories grouped by watch mode
                let mut required_native_dirs = HashSet::new();
                let mut required_poll_dirs = HashSet::new();
                let mut required_auto_dirs = HashSet::new();
                let mut dir_to_daemons: HashMap<PathBuf, Vec<DaemonId>> = HashMap::new();

                for (id, patterns, base_dir, watch_mode) in &watch_configs {
                    match expand_watch_patterns(patterns, base_dir) {
                        Ok(dirs) => {
                            for dir in dirs {
                                dir_to_daemons
                                    .entry(dir.clone())
                                    .or_default()
                                    .push(id.clone());
                                match watch_mode {
                                    WatchMode::Native => {
                                        required_native_dirs.insert(dir);
                                    }
                                    WatchMode::Poll => {
                                        required_poll_dirs.insert(dir);
                                    }
                                    WatchMode::Auto => {
                                        required_auto_dirs.insert(dir);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to expand watch patterns for {id}: {e}");
                        }
                    }
                }

                // Directories that are ONLY referenced by auto-mode daemons.
                // Shared directories (also referenced by native/poll daemons) must
                // not be silently downgraded — the explicit mode takes precedence.
                let auto_only_dirs: HashSet<PathBuf> = required_auto_dirs
                    .difference(&required_native_dirs)
                    .cloned()
                    .collect::<HashSet<_>>()
                    .difference(&required_poll_dirs)
                    .cloned()
                    .collect();

                // AUTO mode prefers native when available; otherwise use poll.
                let mut target_native_dirs = required_native_dirs;
                let mut target_poll_dirs = required_poll_dirs;

                if !required_auto_dirs.is_empty() {
                    // AUTO mode prefers native; route auto dirs to the native target
                    // and let the lazy-init logic below attempt to create the watcher.
                    // If creation fails, the else-branch further down moves them to poll.
                    // Directories that previously fell back to poll are routed there
                    // directly to avoid repeated native-watch failure + warn logging.
                    for dir in &required_auto_dirs {
                        if auto_fallback_dirs.contains_key(dir) {
                            target_poll_dirs.insert(dir.clone());
                        } else {
                            target_native_dirs.insert(dir.clone());
                        }
                    }
                }

                unwatch_removed_dirs(
                    &mut native_wf,
                    &watched_native_dirs,
                    &target_native_dirs,
                    "native",
                );

                // Watch new native directories (AUTO directories may fall back to poll on failure)
                let mut new_fallback_dirs = HashSet::new();
                if !target_native_dirs.is_empty() {
                    if native_wf.is_none() {
                        match WatchFiles::new(debounce, WatchMode::Native, poll_interval) {
                            Ok(wf) => {
                                native_wf = Some(wf);
                                native_creation_failed = false;
                            }
                            Err(e) => {
                                if native_creation_failed {
                                    debug!("Native file watcher still unavailable: {e}");
                                } else {
                                    native_creation_failed = true;
                                    error!("Failed to create native file watcher: {e}");
                                }
                            }
                        }
                    }
                    if native_wf.is_some() {
                        new_fallback_dirs = watch_new_dirs(
                            &mut native_wf,
                            &watched_native_dirs,
                            &target_native_dirs,
                            "native",
                            &dir_to_daemons,
                            Some(&auto_only_dirs),
                            &mut failed_native_watch_dirs,
                        );
                    } else {
                        target_poll_dirs.extend(target_native_dirs.iter().cloned());
                        target_native_dirs.clear();
                    }
                }

                if !new_fallback_dirs.is_empty() {
                    target_native_dirs.retain(|d| !new_fallback_dirs.contains(d));
                    target_poll_dirs.extend(new_fallback_dirs.iter().cloned());
                    for dir in &new_fallback_dirs {
                        let daemon_ids = dir_to_daemons
                            .get(dir)
                            .cloned()
                            .unwrap_or_default()
                            .into_iter()
                            .collect::<HashSet<_>>();
                        auto_fallback_dirs.insert(dir.clone(), daemon_ids);
                    }
                }

                unwatch_removed_dirs(&mut poll_wf, &watched_poll_dirs, &target_poll_dirs, "poll");

                // Watch new poll directories
                if !target_poll_dirs.is_empty() {
                    if poll_wf.is_none() {
                        match WatchFiles::new(debounce, WatchMode::Poll, poll_interval) {
                            Ok(wf) => {
                                poll_wf = Some(wf);
                                poll_creation_failed = false;
                            }
                            Err(e) => {
                                if poll_creation_failed {
                                    debug!("Poll file watcher still unavailable: {e}");
                                } else {
                                    poll_creation_failed = true;
                                    error!("Failed to create polling file watcher: {e}");
                                }
                            }
                        }
                    }

                    if poll_wf.is_some() {
                        let _ = watch_new_dirs(
                            &mut poll_wf,
                            &watched_poll_dirs,
                            &target_poll_dirs,
                            "poll",
                            &dir_to_daemons,
                            None,
                            &mut failed_poll_watch_dirs,
                        );
                    } else {
                        target_poll_dirs.clear();
                    }
                }

                // Only record dirs that were actually registered with an active watcher.
                // If native_wf is None, nothing was registered natively — clearing
                // target_native_dirs above ensures watched_native_dirs stays empty,
                // so the next iteration won't skip re-registration if native recovers.
                watched_native_dirs = target_native_dirs;
                watched_poll_dirs = target_poll_dirs;

                // Prune stale auto-fallback entries: keep a dir only if at least
                // one of the daemon IDs that originally triggered the fallback is
                // still watching that dir in auto mode. This prevents leaked poll
                // watches after daemon removal AND avoids pinning a new daemon to
                // poll just because a removed daemon had a native-watch failure for
                // the same directory.
                auto_fallback_dirs.retain(|dir, daemon_ids| {
                    daemon_ids.retain(|id| {
                        required_auto_dirs.contains(dir)
                            && dir_to_daemons.get(dir).is_some_and(|ids| ids.contains(id))
                    });
                    !daemon_ids.is_empty()
                });

                // Wait for file changes or a refresh interval
                let watch_interval = settings().supervisor_watch_interval();
                tokio::select! {
                    native_changes = async {
                        match native_wf.as_mut() {
                            Some(wf) => wf.rx.recv().await,
                            None => std::future::pending::<Option<Vec<PathBuf>>>().await,
                        }
                    } => {
                        if let Some(changed_paths) = native_changes {
                            debug!("File changes detected (native): {changed_paths:?}");
                            SUPERVISOR
                                .restart_for_changed_paths(changed_paths, &watch_configs)
                                .await;
                        }
                    }
                    poll_changes = async {
                        match poll_wf.as_mut() {
                            Some(wf) => wf.rx.recv().await,
                            None => std::future::pending::<Option<Vec<PathBuf>>>().await,
                        }
                    } => {
                        if let Some(changed_paths) = poll_changes {
                            debug!("File changes detected (poll): {changed_paths:?}");
                            SUPERVISOR
                                .restart_for_changed_paths(changed_paths, &watch_configs)
                                .await;
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
        run_opts.wait_ready = false; // Don't block on file-triggered restarts

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
