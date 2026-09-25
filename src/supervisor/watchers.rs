//! Background watcher tasks
//!
//! Spawns background tasks for:
//! - Interval watching (periodic refresh)
//! - Cron scheduling
//! - File watching for daemon auto-restart

use super::{SUPERVISOR, Supervisor, UpsertDaemonOpts, interval_duration};
use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::ipc::IpcResponse;
use crate::log_store::sqlite::LOG_STORE;
use crate::log_store::{ArchiveHook, LogStore, RetentionPolicy};
use crate::pitchfork_toml::{PitchforkToml, WatchMode};
use crate::procs::PROCS;
use crate::settings::settings;
use crate::watch_files::{
    WatchEvents, WatchFiles, expand_watch_patterns, insert_watch_target, path_matches_patterns,
    watched_entries,
};
use crate::{Result, env};
use notify::RecursiveMode;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time;

type WatchConfig = (DaemonId, Vec<String>, PathBuf, WatchMode);

/// Build an optional archive hook from the configured settings.
fn build_archive_hook(config: &crate::settings::SettingsLogsArchiveHook) -> Option<ArchiveHook> {
    let command = config.command.trim();
    if command.is_empty() {
        return None;
    }
    Some(ArchiveHook {
        command: command.to_string(),
        batch_size: config.batch_size.max(1) as usize,
    })
}

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

fn watch_mode_of(dir: &Path, dir_modes: &HashMap<PathBuf, RecursiveMode>) -> RecursiveMode {
    dir_modes
        .get(dir)
        .copied()
        .unwrap_or(RecursiveMode::NonRecursive)
}

/// The changed paths of a batch, plus the entries of directories created or
/// moved in below a recursive watch. Those entries produce no events of their
/// own, and the watch target (the recursive root) stays the same, so the
/// new-target scan does not see them either.
async fn changed_paths_with_new_subtrees(
    events: WatchEvents,
    watched: &HashMap<PathBuf, RecursiveMode>,
) -> Vec<PathBuf> {
    let WatchEvents { mut paths, created } = events;
    let new_subdirs = outermost_paths(
        created
            .into_iter()
            .filter(|p| {
                watched.iter().any(|(root, mode)| {
                    *mode == RecursiveMode::Recursive && p != root && p.starts_with(root)
                })
            })
            .collect(),
    );
    if new_subdirs.is_empty() {
        return paths;
    }
    let found = tokio::task::spawn_blocking(move || {
        new_subdirs
            .iter()
            .filter(|dir| dir.is_dir())
            .flat_map(|dir| watched_entries(dir, RecursiveMode::Recursive))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    paths.extend(found);
    paths
}

/// Drop paths that lie below another path in the list, so walking the result
/// visits each entry once.
fn outermost_paths(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    // Path ordering compares components, so descendants sort right after
    // their ancestor.
    paths.sort();
    let mut outermost: Vec<PathBuf> = vec![];
    for path in paths {
        if !outermost.last().is_some_and(|last| path.starts_with(last)) {
            outermost.push(path);
        }
    }
    outermost
}

/// Route a directory to the poll backend, keeping the recursion it needs there.
fn route_to_poll(
    dir: PathBuf,
    mode: RecursiveMode,
    target_poll_dirs: &mut HashSet<PathBuf>,
    poll_modes: &mut HashMap<PathBuf, RecursiveMode>,
) {
    insert_watch_target(poll_modes, dir.clone(), mode);
    target_poll_dirs.insert(dir);
}

/// Unwatch directories that are no longer targeted, or whose recursive mode
/// changed and so must be re-registered.
fn unwatch_removed_dirs(
    wf: &mut Option<WatchFiles>,
    watched: &HashMap<PathBuf, RecursiveMode>,
    target: &HashSet<PathBuf>,
    dir_modes: &HashMap<PathBuf, RecursiveMode>,
    backend: &str,
) {
    let Some(wf) = wf.as_mut() else { return };
    for (dir, mode) in watched {
        if target.contains(dir) && watch_mode_of(dir, dir_modes) == *mode {
            continue;
        }
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

#[allow(clippy::too_many_arguments)]
fn watch_new_dirs(
    wf: &mut Option<WatchFiles>,
    watched: &HashMap<PathBuf, RecursiveMode>,
    target: &HashSet<PathBuf>,
    dir_modes: &HashMap<PathBuf, RecursiveMode>,
    backend: &str,
    dir_to_daemons: &HashMap<PathBuf, Vec<DaemonId>>,
    auto_dirs: Option<&HashSet<PathBuf>>,
    failed_dirs: &mut HashSet<PathBuf>,
) -> HashSet<PathBuf> {
    let Some(wf) = wf.as_mut() else {
        return HashSet::new();
    };

    let mut fallback_dirs = HashSet::new();
    for dir in target {
        let mode = watch_mode_of(dir, dir_modes);
        if watched.get(dir) == Some(&mode) {
            continue;
        }
        let daemon_ids = daemon_ids_for_dir(dir, dir_to_daemons);
        debug!(
            "Watching {} ({mode:?}) for daemon(s) ({backend}): {}",
            dir.display(),
            daemon_ids
        );
        if let Err(e) = wf.watch(dir, mode) {
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
    /// Get the watch configurations of running daemons.
    ///
    /// Stopped daemons are skipped: a file change would not restart them, and
    /// their state records keep the `watch` patterns they were last started
    /// with even after those are removed from the config.
    pub(crate) async fn get_all_watch_configs(&self) -> Vec<WatchConfig> {
        let state = self.state_file.lock().await;
        state
            .daemons
            .values()
            .filter(|d| !d.watch.is_empty() && d.pid.is_some() && d.status.is_running())
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
            // Live per-daemon health-check tasks, spawned lazily once a daemon
            // is running with a health check configured.
            let mut health_tasks: HashMap<DaemonId, tokio::task::JoinHandle<()>> = HashMap::new();
            // Run log retention check no more than once per hour.
            let mut last_retention_check = tokio::time::Instant::now() - Duration::from_secs(3600);
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
                // Spawn/prune per-daemon health-check tasks.
                SUPERVISOR.manage_health_tasks(&mut health_tasks).await;
                // Stop proxy-started daemons that have gone idle. Here rather
                // than in `refresh`, which the shell hook also runs on every
                // `cd`.
                SUPERVISOR.check_idle_daemons().await;
                // Apply log retention policy if configured.
                if last_retention_check.elapsed() >= Duration::from_secs(3600) {
                    match SUPERVISOR.apply_log_retention().await {
                        Ok(removed) if removed > 0 => {
                            info!("log retention: pruned {removed} old entries")
                        }
                        Ok(_) => {}
                        Err(e) => warn!("log retention: failed to prune logs: {e}"),
                    }
                    last_retention_check = tokio::time::Instant::now();
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

        // Refresh process tree and collect stats for all running daemons
        // in a single pass.  O(N) instead of O(D × N) when calling
        // `get_group_stats` per daemon.
        let pids: Vec<u32> = daemons.iter().filter_map(|d| d.pid).collect();
        let stats_map: HashMap<u32, _> = PROCS.refresh_and_get_batch_stats(&pids);

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
            if let Some(mem_limit) = daemon.memory_limit
                && stats.memory_bytes > mem_limit.0
            {
                warn!(
                    "daemon {} (pid {}) exceeded memory limit: {} > {}, stopping",
                    daemon.id,
                    pid,
                    stats.memory_display(),
                    mem_limit,
                );
                cpu_violation_counts.remove(&daemon.id);
                self.stop_for_resource_violation(&daemon.id, pid, daemon.start_time)
                    .await;
                continue; // Don't check CPU if we're already killing
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
                        self.stop_for_resource_violation(&daemon.id, pid, daemon.start_time)
                            .await;
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

    /// Apply log retention policy globally and per-daemon.
    ///
    /// Builds the global retention policy from settings, then iterates over all
    /// daemons in the merged config and applies per-daemon overrides when set.
    async fn apply_log_retention(&self) -> Result<u64> {
        let settings = settings();
        let (global_age, global_age_warn) = if settings.logs.time_retention.is_empty() {
            (None, None)
        } else {
            match humantime::parse_duration(&settings.logs.time_retention) {
                Ok(d) => (Some(d), None),
                Err(_) => {
                    let msg = format!(
                        "invalid global logs.time_retention '{}': expected format like '7d', '24h', '30min'",
                        settings.logs.time_retention
                    );
                    (None, Some(msg))
                }
            }
        };
        let global_count = if settings.logs.line_retention <= 0 {
            None
        } else {
            Some(settings.logs.line_retention as u64)
        };

        let global_policy = RetentionPolicy {
            age: global_age.and_then(|std_dur| match chrono::Duration::from_std(std_dur) {
                Ok(d) => Some(d),
                Err(_) => {
                    warn!(
                        "global logs.time_retention duration {:?} out of range; ignoring",
                        std_dur
                    );
                    None
                }
            }),
            count: global_count,
        };

        let global_archive_hook = build_archive_hook(&settings.logs.archive_hook);

        if let Some(w) = global_age_warn {
            warn!("{w}");
        }

        // Collect per-daemon overrides from merged config.
        let mut per_daemon_warns: Vec<String> = Vec::new();
        let per_daemon_policies: Vec<(DaemonId, RetentionPolicy, Option<ArchiveHook>)> = {
            let config = PitchforkToml::all_merged()?;
            config
                .daemons
                .iter()
                .filter_map(|(id, d)| {
                    // Per-daemon retention: [daemons.x.logs] sub-table
                    // overrides top-level fields for backward compatibility.
                    let logs = d.logs.as_ref();
                    let age = logs
                        .and_then(|l| l.time_retention.as_deref())
                        .or(d.time_retention.as_deref())
                        .and_then(|s| {
                            if s.is_empty() {
                                None
                            } else {
                                match humantime::parse_duration(s) {
                                    Ok(d) => Some(d),
                                    Err(_) => {
                                        per_daemon_warns.push(format!(
                                            "invalid time_retention '{}' for daemon {}: expected format like '7d', '24h', '30min'",
                                            s, id
                                        ));
                                        None
                                    }
                                }
                            }
                        });
                    let count = logs
                        .and_then(|l| l.line_retention)
                        .or(d.line_retention)
                        .and_then(|n| if n > 0 { Some(n as u64) } else { None });
                    let hook = logs
                        .and_then(|l| l.archive_hook.as_deref())
                        .or(d.archive_hook.as_deref())
                        .filter(|cmd| !cmd.trim().is_empty())
                        .map(|cmd| ArchiveHook {
                            command: cmd.to_string(),
                            batch_size: settings.logs.archive_hook.batch_size.max(1) as usize,
                        });
                    if age.is_some() || count.is_some() || hook.is_some() {
                        Some((
                            id.clone(),
                            RetentionPolicy {
                                age: age.and_then(|std_dur| {
                                    match chrono::Duration::from_std(std_dur) {
                                        Ok(d) => Some(d),
                                        Err(_) => {
                                            warn!(
                                                "daemon {id} time_retention duration {:?} out of range; ignoring",
                                                std_dur
                                            );
                                            None
                                        }
                                    }
                                }),
                                count,
                            },
                            hook,
                        ))
                    } else {
                        None
                    }
                })
                .collect()
        };

        for w in per_daemon_warns {
            warn!("{w}");
        }

        // Apply global policy to all daemons *except* those that have their
        // own per-daemon overrides, so overrides are not silently overwritten.
        let excluded: Vec<DaemonId> = per_daemon_policies
            .iter()
            .map(|(id, _, _)| id.clone())
            .collect();

        // Offload blocking SQLite work to a dedicated thread.
        let total_removed = tokio::task::spawn_blocking(move || {
            let mut total = LOG_STORE.apply_retention(
                &global_policy,
                &excluded,
                global_archive_hook.as_ref(),
            )?;

            // For daemons with per-daemon overrides, apply their specific policy,
            // falling back to the global setting for any dimension they don't override.
            for (id, policy, hook) in per_daemon_policies {
                let effective_policy = RetentionPolicy {
                    age: policy.age.or(global_policy.age),
                    count: policy.count.or(global_policy.count),
                };
                let effective_hook = hook.as_ref().or(global_archive_hook.as_ref());
                total +=
                    LOG_STORE.apply_retention_for_daemon(&id, &effective_policy, effective_hook)?;
            }

            Ok::<u64, miette::Error>(total)
        })
        .await
        .map_err(|e| miette::miette!("retention task panicked: {e}"))??;

        Ok(total_removed)
    }

    /// Kill a daemon due to a resource limit violation.
    ///
    /// Unlike `stop()`, this does NOT set the daemon status to `Stopping` first.
    /// Instead, it kills the process group directly, which causes the monitor task
    /// to observe a non-zero exit and set the status to `Errored`. This allows
    /// the retry checker to restart the daemon if `retry` is configured.
    ///
    /// `expected_start_time` is the identity captured from the same daemon
    /// snapshot the violation was observed in: enforcement refuses if the
    /// daemon restarted in the meantime (see
    /// [`Supervisor::kill_daemon_as_crash`]).
    async fn stop_for_resource_violation(
        &self,
        id: &DaemonId,
        pid: u32,
        expected_start_time: Option<u64>,
    ) {
        self.kill_daemon_as_crash(
            id,
            pid,
            expected_start_time,
            "due to resource limit violation",
        )
        .await;
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

    /// Register config-only cron daemons into state and remove stale entries.
    ///
    /// Daemons defined in config with `cron` but never started are not in
    /// `state_file.daemons`, so the cron watcher cannot see them. This method
    /// scans the merged config and upserts any cron daemons that are missing
    /// from state, marking them with `config_registered = true` so that
    /// list/status/stats treat them as "available" rather than "stopped".
    ///
    /// Also removes stale `config_registered` entries for daemons whose cron
    /// config has been removed, so they stop firing.
    async fn register_config_cron_daemons(&self) -> Result<()> {
        let config = PitchforkToml::all_merged_all_namespaces()?;

        let config_cron_ids: HashSet<&DaemonId> = config
            .daemons
            .iter()
            .filter(|(_, d)| d.cron.is_some())
            .map(|(id, _)| id)
            .collect();

        // Remove stale config_registered entries no longer in config.
        let stale_ids: Vec<DaemonId> = {
            let state = self.state_file.lock().await;
            state
                .daemons
                .iter()
                .filter(|(id, d)| d.config_registered && !config_cron_ids.contains(*id))
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in &stale_ids {
            self.remove_daemon(id).await?;
            info!("removed stale config-only cron daemon {id} from state");
        }

        // Register config-only cron daemons not yet in state.
        let to_register: Vec<_> = {
            let state = self.state_file.lock().await;
            config
                .daemons
                .iter()
                .filter(|(id, d)| d.cron.is_some() && !state.daemons.contains_key(*id))
                .collect()
        };

        for (id, d) in to_register {
            let cmd = match shell_words::split(&d.run) {
                Ok(cmd) => cmd,
                Err(e) => {
                    error!("failed to parse command for cron daemon {id}: {e}");
                    continue;
                }
            };
            let run_opts = d.to_run_options(id, cmd);
            self.upsert_daemon(
                UpsertDaemonOpts::from_run_options(&run_opts, DaemonStatus::Stopped)
                    .set(|o| {
                        o.config_registered = true;
                    })
                    .build(),
            )
            .await?;
            info!("registered config-only cron daemon {id} into state");
        }

        Ok(())
    }

    /// Check cron schedules and trigger daemons as needed
    pub(crate) async fn check_cron_schedules(&self) -> Result<()> {
        use cron::Schedule;
        use std::str::FromStr;

        // Register config-only cron daemons into state so the cron watcher
        // can see them. Without this, daemons defined in config with `cron`
        // but never started (no `boot_start`, no manual `pitchfork start`)
        // are invisible to the cron checker.
        self.register_config_cron_daemons().await?;

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
                // since our last trigger.
                let check_since = match daemon.last_cron_triggered {
                    Some(t) => t,
                    None => {
                        if daemon.cron_immediate.unwrap_or(false) {
                            // immediate=true: restore the old look-back behavior.
                            // A scheduled time within the last 10 seconds before startup
                            // will trigger immediately.
                            now - chrono::Duration::seconds(10)
                        } else {
                            // immediate=false (default): anchor last_cron_triggered to now
                            // so the next scheduled time is picked up, without firing now.
                            let mut state_file = self.state_file.lock().await;
                            if state_file.set_last_cron_triggered(&id, now)
                                && let Err(e) = state_file.write()
                            {
                                error!(
                                    "failed to persist last_cron_triggered for daemon {id}: {e}"
                                );
                            }
                            continue;
                        }
                    }
                };

                // Find if there's a scheduled time between check_since and now
                let should_trigger = schedule
                    .after(&check_since)
                    .take_while(|t| *t <= now)
                    .next()
                    .is_some();

                if should_trigger {
                    // Update last_cron_triggered to prevent re-triggering the same event.
                    // This write is synchronous and critical: deferring it to the
                    // background flush task creates a window where a supervisor
                    // crash-then-restart will see the stale timestamp from disk and
                    // re-fire the cron job immediately.
                    {
                        let mut state_file = self.state_file.lock().await;
                        if state_file.set_last_cron_triggered(&id, now)
                            && let Err(e) = state_file.write()
                        {
                            error!("failed to persist last_cron_triggered for daemon {id}: {e}");
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
                            // Run if not currently running and the previous run
                            // succeeded. A never-started daemon (None) is allowed
                            // to fire its first run.
                            daemon.pid.is_none() && daemon.last_exit_success.unwrap_or(true)
                        }
                        crate::pitchfork_toml::CronRetrigger::Fail => {
                            // Run if not currently running and the previous run
                            // failed. A never-started daemon (None) is allowed
                            // to fire its first run.
                            daemon.pid.is_none() && !daemon.last_exit_success.unwrap_or(false)
                        }
                    };

                    if should_run {
                        info!("cron: triggering daemon {id} (retrigger: {retrigger:?})");
                        // Use the persisted command from daemon state
                        let cmd = match daemon.cmd.clone() {
                            Some(cmd) => cmd,
                            None => {
                                warn!("no run command found in state for cron daemon {id}");
                                continue;
                            }
                        };
                        let dir = daemon.dir.clone().unwrap_or_else(|| env::CWD.clone());
                        // Use force: true for Always retrigger to ensure restart
                        let force =
                            matches!(retrigger, crate::pitchfork_toml::CronRetrigger::Always);
                        let mut opts = daemon.to_run_options(cmd);
                        opts.dir = crate::config_types::Dir(dir);
                        opts.force = force;
                        opts.wait_ready = false;
                        opts.cron_schedule = Some(schedule_str.clone());
                        opts.cron_retrigger = Some(retrigger);
                        // A scheduled run is the configuration asking for the
                        // daemon, not the proxy: it is never stopped for
                        // inactivity, whatever started the previous run.
                        opts.proxy_idle_timeout_ms = None;
                        // `last_cron_run` is recorded by `run_once` at the
                        // moment a process is spawned, which is the only point
                        // that distinguishes a window that produced a run from
                        // one that did not.
                        opts.cron_started = true;
                        if let Err(e) = self.run(opts).await {
                            error!("failed to run cron daemon {id}: {e}");
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
        let pt = PitchforkToml::all_merged_all_namespaces()?;

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
            debug!("No daemons with watch patterns configured, watcher will start lazily");
            // Do NOT return early — spawn the watcher loop anyway so it can
            // pick up daemons added after supervisor startup (e.g. when the
            // supervisor is pre-started before tests create pitchfork.toml).
        } else {
            info!(
                "Setting up file watching for {} daemon(s)",
                watch_configs.len()
            );
        }

        // Spawn the file watcher task
        tokio::spawn(async move {
            let debounce = settings().supervisor_file_watch_debounce();
            let poll_interval = settings().supervisor_watch_poll_interval();

            let mut native_wf: Option<WatchFiles> = None;
            let mut poll_wf: Option<WatchFiles> = None;
            let mut native_creation_failed = false;
            let mut poll_creation_failed = false;
            let mut watched_native_dirs: HashMap<PathBuf, RecursiveMode> = HashMap::new();
            let mut watched_poll_dirs: HashMap<PathBuf, RecursiveMode> = HashMap::new();
            // Directories that previously failed native watch in auto mode and
            // are permanently tracked by the poll watcher. Maps dir → set of
            // daemon IDs that originally triggered the fallback, so entries for
            // removed daemons are pruned even when a different daemon uses the
            // same dir (which should get a fresh native-watch attempt).
            let mut auto_fallback_dirs: HashMap<PathBuf, HashSet<DaemonId>> = HashMap::new();
            // Each daemon's patterns and watch targets from the previous pass,
            // used to recognize directories created since then.
            let mut prev_expansions: HashMap<
                DaemonId,
                (Vec<String>, HashMap<PathBuf, RecursiveMode>),
            > = HashMap::new();
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
                // Recursion each backend needs per directory: a directory is
                // watched recursively by a backend if any daemon it serves needs it.
                let mut native_modes: HashMap<PathBuf, RecursiveMode> = HashMap::new();
                let mut poll_modes: HashMap<PathBuf, RecursiveMode> = HashMap::new();
                let mut auto_modes: HashMap<PathBuf, RecursiveMode> = HashMap::new();

                // Expanding patterns reads the filesystem, so keep it off the runtime.
                let configs = watch_configs.clone();
                let expanded = tokio::task::spawn_blocking(move || {
                    configs
                        .iter()
                        .map(|(_, patterns, base_dir, _)| expand_watch_patterns(patterns, base_dir))
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_else(|e| {
                    error!("Failed to expand watch patterns: {e}");
                    vec![]
                });

                // A target that newly appears for a daemon whose patterns did not
                // change is a directory created since the last pass. Entries created
                // inside it before its watch was added produced no events.
                let mut new_dirs: HashMap<PathBuf, RecursiveMode> = HashMap::new();
                let mut new_dir_configs: Vec<WatchConfig> = vec![];
                for (config, dirs) in watch_configs.iter().zip(&expanded) {
                    let (id, patterns, _, _) = config;
                    let Some((prev_patterns, prev_dirs)) = prev_expansions.get(id) else {
                        continue;
                    };
                    if prev_patterns != patterns {
                        continue;
                    }
                    let mut has_new = false;
                    for (dir, mode) in dirs {
                        if !prev_dirs.contains_key(dir) {
                            insert_watch_target(&mut new_dirs, dir.clone(), *mode);
                            has_new = true;
                        }
                    }
                    if has_new {
                        new_dir_configs.push(config.clone());
                    }
                }
                prev_expansions = watch_configs
                    .iter()
                    .zip(&expanded)
                    .map(|((id, patterns, _, _), dirs)| {
                        (id.clone(), (patterns.clone(), dirs.clone()))
                    })
                    .collect();

                for ((id, _, _, watch_mode), dirs) in watch_configs.iter().zip(expanded) {
                    for (dir, mode) in dirs {
                        dir_to_daemons
                            .entry(dir.clone())
                            .or_default()
                            .push(id.clone());
                        let (required, modes) = match watch_mode {
                            WatchMode::Native => (&mut required_native_dirs, &mut native_modes),
                            WatchMode::Poll => (&mut required_poll_dirs, &mut poll_modes),
                            WatchMode::Auto => (&mut required_auto_dirs, &mut auto_modes),
                        };
                        insert_watch_target(modes, dir.clone(), mode);
                        required.insert(dir);
                    }
                }
                // Auto directories are routed to native unless they fell back to poll.
                for (dir, mode) in &auto_modes {
                    if !auto_fallback_dirs.contains_key(dir) {
                        insert_watch_target(&mut native_modes, dir.clone(), *mode);
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
                            route_to_poll(
                                dir.clone(),
                                watch_mode_of(dir, &auto_modes),
                                &mut target_poll_dirs,
                                &mut poll_modes,
                            );
                        } else {
                            target_native_dirs.insert(dir.clone());
                        }
                    }
                }

                unwatch_removed_dirs(
                    &mut native_wf,
                    &watched_native_dirs,
                    &target_native_dirs,
                    &native_modes,
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
                            &native_modes,
                            "native",
                            &dir_to_daemons,
                            Some(&auto_only_dirs),
                            &mut failed_native_watch_dirs,
                        );
                    } else {
                        for dir in target_native_dirs.drain() {
                            let mode = watch_mode_of(&dir, &native_modes);
                            route_to_poll(dir, mode, &mut target_poll_dirs, &mut poll_modes);
                        }
                    }
                }

                if !new_fallback_dirs.is_empty() {
                    target_native_dirs.retain(|d| !new_fallback_dirs.contains(d));
                    for dir in &new_fallback_dirs {
                        let mode = watch_mode_of(dir, &native_modes);
                        route_to_poll(dir.clone(), mode, &mut target_poll_dirs, &mut poll_modes);
                        let daemon_ids = dir_to_daemons
                            .get(dir)
                            .cloned()
                            .unwrap_or_default()
                            .into_iter()
                            .collect::<HashSet<_>>();
                        auto_fallback_dirs.insert(dir.clone(), daemon_ids);
                    }
                }

                unwatch_removed_dirs(
                    &mut poll_wf,
                    &watched_poll_dirs,
                    &target_poll_dirs,
                    &poll_modes,
                    "poll",
                );

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
                            &poll_modes,
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
                let with_modes =
                    |dirs: HashSet<PathBuf>, modes: &HashMap<PathBuf, RecursiveMode>| {
                        dirs.into_iter()
                            .map(|d| {
                                let mode = watch_mode_of(&d, modes);
                                (d, mode)
                            })
                            .collect::<HashMap<_, _>>()
                    };
                watched_native_dirs = with_modes(target_native_dirs, &native_modes);
                watched_poll_dirs = with_modes(target_poll_dirs, &poll_modes);

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

                // Check what already exists in new directories now that they are
                // watched, then re-expand right away in case they contain
                // directories that are new targets too.
                if !new_dirs.is_empty() {
                    let found = tokio::task::spawn_blocking(move || {
                        new_dirs
                            .iter()
                            .flat_map(|(dir, mode)| watched_entries(dir, *mode))
                            .collect::<Vec<_>>()
                    })
                    .await
                    .unwrap_or_default();
                    if !found.is_empty() {
                        debug!("Entries found in new watched directories: {found:?}");
                        SUPERVISOR
                            .restart_for_changed_paths(found, &new_dir_configs)
                            .await;
                    }
                    continue;
                }

                // Wait for file changes or a refresh interval
                let watch_interval = settings().supervisor_watch_interval();
                tokio::select! {
                    native_changes = async {
                        match native_wf.as_mut() {
                            Some(wf) => wf.rx.recv().await,
                            None => std::future::pending::<Option<WatchEvents>>().await,
                        }
                    } => {
                        if let Some(events) = native_changes {
                            let changed_paths =
                                changed_paths_with_new_subtrees(events, &watched_native_dirs).await;
                            debug!("File changes detected (native): {changed_paths:?}");
                            SUPERVISOR
                                .restart_for_changed_paths(changed_paths, &watch_configs)
                                .await;
                        }
                    }
                    poll_changes = async {
                        match poll_wf.as_mut() {
                            Some(wf) => wf.rx.recv().await,
                            None => std::future::pending::<Option<WatchEvents>>().await,
                        }
                    } => {
                        if let Some(events) = poll_changes {
                            let changed_paths =
                                changed_paths_with_new_subtrees(events, &watched_poll_dirs).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outermost_paths() {
        let paths = [
            "/p/src/a/b",
            "/p/src/ab",
            "/p/src/a",
            "/p/src/a/b/c",
            "/p/lib",
        ]
        .map(PathBuf::from)
        .to_vec();

        assert_eq!(
            outermost_paths(paths),
            ["/p/lib", "/p/src/a", "/p/src/ab"]
                .map(PathBuf::from)
                .to_vec()
        );
    }
}
