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
use std::time::{Duration, Instant};
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

/// What config says about a daemon's cron schedule.
#[derive(Debug)]
enum ConfigCron {
    /// Config schedules the daemon like this.
    Scheduled(crate::pitchfork_toml::PitchforkTomlCron),
    /// Config was read and does not schedule the daemon: its `cron`, or the
    /// daemon itself, is gone.
    Unscheduled,
    /// The daemon's config could not be found or read, so nothing is known
    /// and its stored schedule must stay.
    Unknown,
}

/// Look up `id`'s schedule in the config under `dir` (the daemon's working
/// directory, where `pitchfork start` found it) and in `all`, the config of
/// every project the supervisor knows.
///
/// A daemon missing from both counts as removed only if its own project's
/// config was read — the config under `dir` still defines other daemons of
/// the same namespace. `all` cannot show that: a project that failed to load
/// is skipped by it rather than failing it, and another project it loaded
/// separately may share the namespace, so without the check an unreadable
/// config would look like deleted schedules.
fn config_cron(id: &DaemonId, dir: Option<&Path>, all: Option<&PitchforkToml>) -> ConfigCron {
    let own = dir.and_then(|dir| PitchforkToml::all_merged_from(dir).ok());
    config_cron_in(id, own.as_ref(), all)
}

/// [`config_cron`] over configs already read: `own`, the config under the
/// daemon's directory, and `all`.
fn config_cron_in(
    id: &DaemonId,
    own: Option<&PitchforkToml>,
    all: Option<&PitchforkToml>,
) -> ConfigCron {
    for pt in own.into_iter().chain(all) {
        if let Some(daemon) = pt.daemons.get(id) {
            return match &daemon.cron {
                Some(cron) => ConfigCron::Scheduled(cron.clone()),
                None => ConfigCron::Unscheduled,
            };
        }
    }
    let project_read = own.is_some_and(|pt| {
        pt.daemons
            .keys()
            .any(|other| other.namespace() == id.namespace())
    });
    if project_read {
        ConfigCron::Unscheduled
    } else {
        ConfigCron::Unknown
    }
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
            // A symlink moved in is not walked into its target
            .filter(|dir| dir.symlink_metadata().is_ok_and(|m| m.is_dir()))
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

/// Delay before the first retry of a failed watch registration. It doubles
/// with each further failure, up to `WATCH_RETRY_MAX`.
const WATCH_RETRY_BASE: Duration = Duration::from_secs(10);
const WATCH_RETRY_MAX: Duration = Duration::from_secs(300);

/// A directory whose watch registration failed, e.g. from an exhausted inotify
/// limit or a directory that briefly did not exist. It is retried with
/// exponential backoff rather than on every pass: a recursive watch walks the
/// whole tree on each attempt, which would spin on a large tree.
#[derive(Debug)]
struct FailedWatch {
    mode: RecursiveMode,
    failures: u32,
    retry_at: Instant,
}

fn watch_retry_delay(failures: u32) -> Duration {
    let exp = failures.saturating_sub(1).min(16);
    WATCH_RETRY_BASE
        .saturating_mul(1 << exp)
        .min(WATCH_RETRY_MAX)
}

/// Unwatch directories that are no longer targeted, or whose recursive mode
/// changed and so must be re-registered. Failed registrations are dropped the
/// same way, releasing any part of a recursive watch that was added before
/// the failure.
fn unwatch_removed_dirs(
    wf: &mut Option<WatchFiles>,
    watched: &mut HashMap<PathBuf, RecursiveMode>,
    failed: &mut HashMap<PathBuf, FailedWatch>,
    target: &HashSet<PathBuf>,
    dir_modes: &HashMap<PathBuf, RecursiveMode>,
    backend: &str,
) {
    let Some(wf) = wf.as_mut() else { return };
    watched.retain(|dir, mode| {
        if target.contains(dir) && watch_mode_of(dir, dir_modes) == *mode {
            return true;
        }
        debug!("Unwatching directory {} ({backend})", dir.display());
        if let Err(e) = wf.unwatch(dir) {
            warn!(
                "Failed to unwatch directory {} ({backend}): {}",
                dir.display(),
                e
            );
        }
        false
    });
    failed.retain(|dir, failure| {
        let targeted = target.contains(dir);
        if targeted && watch_mode_of(dir, dir_modes) == failure.mode {
            return true;
        }
        // Usually nothing was registered, so an error here is expected.
        if let Err(e) = wf.unwatch(dir) {
            trace!(
                "No partial watch to remove for {} ({backend}): {e}",
                dir.display()
            );
        }
        // A directory whose mode changed keeps its entry so a failure of the
        // new watch is not reported again, but is retried right away.
        targeted
    });
}

#[allow(clippy::too_many_arguments)]
fn watch_new_dirs(
    wf: &mut Option<WatchFiles>,
    watched: &mut HashMap<PathBuf, RecursiveMode>,
    failed: &mut HashMap<PathBuf, FailedWatch>,
    target: &HashSet<PathBuf>,
    dir_modes: &HashMap<PathBuf, RecursiveMode>,
    backend: &str,
    dir_to_daemons: &HashMap<PathBuf, Vec<DaemonId>>,
    auto_dirs: Option<&HashSet<PathBuf>>,
    now: Instant,
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
        if failed
            .get(dir)
            .is_some_and(|f| f.mode == mode && now < f.retry_at)
        {
            continue;
        }
        let daemon_ids = daemon_ids_for_dir(dir, dir_to_daemons);
        debug!(
            "Watching {} ({mode:?}) for daemon(s) ({backend}): {}",
            dir.display(),
            daemon_ids
        );
        match wf.watch(dir, mode) {
            Ok(()) => {
                if let Some(f) = failed.remove(dir) {
                    info!(
                        "Watching directory {} ({backend}) after {} failed attempt(s)",
                        dir.display(),
                        f.failures
                    );
                }
                watched.insert(dir.clone(), mode);
            }
            Err(e) if auto_dirs.is_some_and(|dirs| dirs.contains(dir)) => {
                warn!(
                    "{backend} watch failed for {} in auto mode, falling back to poll: {}",
                    dir.display(),
                    e
                );
                fallback_dirs.insert(dir.clone());
            }
            Err(e) => {
                let failures = failed.get(dir).map_or(0, |f| f.failures) + 1;
                let delay = watch_retry_delay(failures);
                if failures == 1 {
                    // Only warn the first time; retries log at debug level.
                    warn!(
                        "Failed to watch directory {} ({backend}), retrying in {}: {}",
                        dir.display(),
                        humantime::format_duration(delay),
                        e
                    );
                } else {
                    debug!(
                        "Failed to watch directory {} ({backend}) after {failures} attempts, \
                         retrying in {}: {}",
                        dir.display(),
                        humantime::format_duration(delay),
                        e
                    );
                }
                failed.insert(
                    dir.clone(),
                    FailedWatch {
                        mode,
                        failures,
                        retry_at: now + delay,
                    },
                );
            }
        }
    }

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

        // Remove stale config_registered entries no longer in config. A
        // daemon only the schedule has run no longer carries
        // `config_registered`, but it came from config just the same, so it
        // goes too once it is not running: left in state, its stored schedule
        // would keep starting it.
        let stale_ids: Vec<DaemonId> = {
            let state = self.state_file.lock().await;
            state
                .daemons
                .iter()
                .filter(|(id, d)| {
                    (d.config_registered || (d.scheduled_from_config && d.pid.is_none()))
                        && !config_cron_ids.contains(*id)
                })
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
            // Registered as written. Templates are rendered at each scheduled
            // run instead (see `scheduled_from_config`), when the daemons
            // whose ports they name are more likely to be running, and with
            // those ports as they are then. The command is parsed there too,
            // after rendering: a template can make it parse only once
            // rendered, so failing to parse it here must not stop the
            // daemon being scheduled. What is stored is only shown.
            let cmd = d.run.argv().unwrap_or_default();
            let run_opts = d.to_run_options(id, cmd);
            self.upsert_daemon(
                UpsertDaemonOpts::from_run_options(&run_opts, DaemonStatus::Stopped)
                    .set(|o| {
                        o.config_registered = true;
                        o.scheduled_from_config = Some(true);
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

        // Bring the schedules stored in state in line with config first, so
        // the checks below fire each daemon by the schedule it has now.
        self.sync_cron_schedules_with_config().await;

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
                        let mut opts = match self.scheduled_run_from_config(&id, &daemon).await {
                            Some(Ok(opts)) => opts,
                            Some(Err(e)) => {
                                error!("failed to run cron daemon {id}: {e}");
                                continue;
                            }
                            None => {
                                // Use the persisted command from daemon state
                                let cmd = match daemon.cmd.clone() {
                                    Some(cmd) => cmd,
                                    None => {
                                        warn!("no run command found in state for cron daemon {id}");
                                        continue;
                                    }
                                };
                                let dir = daemon.dir.clone().unwrap_or_else(|| env::CWD.clone());
                                let mut opts = daemon.to_run_options(cmd);
                                opts.dir = crate::config_types::Dir(dir);
                                opts
                            }
                        };
                        // Use force: true for Always retrigger to ensure restart
                        let force =
                            matches!(retrigger, crate::pitchfork_toml::CronRetrigger::Always);
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

    /// Run options for a scheduled run of a daemon only the schedule has
    /// started, built from its current config with templates rendered.
    ///
    /// `None` when a client has started the daemon, so its stored options are
    /// to be used instead. An error when its schedule is no longer in config:
    /// it is not run from what was stored, and is removed from state once it
    /// has stopped.
    async fn scheduled_run_from_config(
        &self,
        id: &DaemonId,
        daemon: &crate::daemon::Daemon,
    ) -> Option<Result<crate::daemon::RunOptions>> {
        if !daemon.scheduled_from_config {
            return None;
        }
        // Reading config walks the filesystem, so it runs on a blocking
        // worker rather than holding up the other daemons' cron checks.
        let pt = match tokio::task::spawn_blocking(PitchforkToml::all_merged_all_namespaces).await {
            Ok(Ok(pt)) => pt,
            Ok(Err(e)) => return Some(Err(e)),
            Err(e) => {
                return Some(Err(miette::miette!(
                    "reading config for cron daemon {id} panicked: {e}"
                )));
            }
        };
        let Some(config) = pt.daemons.get(id).filter(|d| d.cron.is_some()) else {
            return Some(Err(miette::miette!(
                "its cron schedule is no longer in config"
            )));
        };
        Some(self.run_options_from_config(id, config, &pt).await)
    }

    /// Update the cron schedules stored in state to what config says now.
    ///
    /// `pitchfork start` stores a daemon's schedule in state, and the checks
    /// fire from that copy, so without this an edit to `cron` in config — a new
    /// expression, a new `retrigger`, removing it, adding it back — would not
    /// reach a daemon that had been started until it was started again.
    /// Ad-hoc runs have no schedule, so every stored one came from config.
    async fn sync_cron_schedules_with_config(&self) {
        let daemons: Vec<(DaemonId, Option<PathBuf>)> = {
            let state = self.state_file.lock().await;
            state
                .daemons
                .iter()
                .map(|(id, d)| (id.clone(), d.dir.clone()))
                .collect()
        };
        // Reading config walks the filesystem, so it runs on a blocking worker
        // rather than holding up the watcher.
        let found = match tokio::task::spawn_blocking(move || {
            let all = PitchforkToml::all_merged_all_namespaces().ok();
            daemons
                .into_iter()
                .map(|(id, dir)| {
                    let cron = config_cron(&id, dir.as_deref(), all.as_ref());
                    (id, cron)
                })
                .collect::<Vec<_>>()
        })
        .await
        {
            Ok(found) => found,
            Err(e) => {
                warn!("cron: reading config panicked ({e}); keeping stored schedules");
                return;
            }
        };

        let mut state = self.state_file.lock().await;
        let mut changed = false;
        for (id, cron) in found {
            let Some(daemon) = state.daemons.get_mut(&id) else {
                continue;
            };
            match cron {
                ConfigCron::Scheduled(cron) => {
                    if daemon.cron_schedule.as_deref() != Some(cron.schedule.as_str()) {
                        // A new schedule starts afresh: its first check is
                        // decided by `immediate`, not by when the old one last
                        // fired.
                        daemon.cron_schedule = Some(cron.schedule.clone());
                        daemon.cron_retrigger = Some(cron.retrigger);
                        daemon.cron_immediate = Some(cron.immediate);
                        daemon.last_cron_triggered = None;
                        info!(
                            "cron: {id} is now scheduled by config as {:?}",
                            cron.schedule
                        );
                        changed = true;
                    } else {
                        if daemon.cron_retrigger != Some(cron.retrigger) {
                            daemon.cron_retrigger = Some(cron.retrigger);
                            info!(
                                "cron: {id} now retriggers {:?} as config says",
                                cron.retrigger
                            );
                            changed = true;
                        }
                        if daemon.cron_immediate != Some(cron.immediate) {
                            daemon.cron_immediate = Some(cron.immediate);
                            changed = true;
                        }
                    }
                }
                ConfigCron::Unscheduled => {
                    if daemon.cron_schedule.is_some() {
                        // A run in progress is left to finish; the daemon is
                        // just not started by the schedule again.
                        daemon.cron_schedule = None;
                        daemon.cron_retrigger = None;
                        info!("cron: {id} is no longer scheduled in config; dropped its schedule");
                        changed = true;
                    }
                }
                ConfigCron::Unknown => {}
            }
        }
        if changed && let Err(e) = state.write() {
            error!("failed to persist cron schedules updated from config: {e}");
        }
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
            // Directories registered with each watcher. Only successful
            // registrations are recorded, so failed ones are retried.
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
            // Dirs for which wf.watch() failed, retried with backoff. Also
            // suppresses repeated warn-level logs while the failure persists.
            let mut failed_native_watch_dirs: HashMap<PathBuf, FailedWatch> = HashMap::new();
            let mut failed_poll_watch_dirs: HashMap<PathBuf, FailedWatch> = HashMap::new();

            info!("File watcher started");

            loop {
                // Refresh watch configurations from state
                let watch_configs = SUPERVISOR.get_all_watch_configs().await;

                // Collect required directories grouped by watch mode
                let mut required_native_dirs = HashSet::new();
                let mut required_poll_dirs = HashSet::new();
                let mut required_auto_dirs = HashSet::new();
                let mut dir_to_daemons: HashMap<PathBuf, Vec<DaemonId>> = HashMap::new();
                // Auto-mode daemons per directory, which alone decide its fallback.
                let mut auto_dir_daemons: HashMap<PathBuf, HashSet<DaemonId>> = HashMap::new();
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
                        if *watch_mode == WatchMode::Auto {
                            auto_dir_daemons
                                .entry(dir.clone())
                                .or_default()
                                .insert(id.clone());
                        }
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

                // Auto-mode directories that may fall back to polling when a native
                // watch fails. Directories also referenced by native daemons must
                // not be silently downgraded — the explicit mode takes precedence.
                // Directories shared with poll daemons may: polling them already,
                // the fallback only adds the recursion auto daemons need there.
                let auto_fallback_candidates: HashSet<PathBuf> = required_auto_dirs
                    .difference(&required_native_dirs)
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
                    &mut watched_native_dirs,
                    &mut failed_native_watch_dirs,
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
                            &mut watched_native_dirs,
                            &mut failed_native_watch_dirs,
                            &target_native_dirs,
                            &native_modes,
                            "native",
                            &dir_to_daemons,
                            Some(&auto_fallback_candidates),
                            Instant::now(),
                        );
                    } else {
                        for dir in target_native_dirs.drain() {
                            let mode = watch_mode_of(&dir, &native_modes);
                            route_to_poll(dir, mode, &mut target_poll_dirs, &mut poll_modes);
                        }
                    }
                }

                if !new_fallback_dirs.is_empty() {
                    for dir in &new_fallback_dirs {
                        let mode = watch_mode_of(dir, &native_modes);
                        route_to_poll(dir.clone(), mode, &mut target_poll_dirs, &mut poll_modes);
                        // Only auto daemons pin the fallback; a poll daemon sharing
                        // the directory must not keep it after they are gone.
                        let daemon_ids = auto_dir_daemons.get(dir).cloned().unwrap_or_default();
                        auto_fallback_dirs.insert(dir.clone(), daemon_ids);
                    }
                }

                unwatch_removed_dirs(
                    &mut poll_wf,
                    &mut watched_poll_dirs,
                    &mut failed_poll_watch_dirs,
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

                    watch_new_dirs(
                        &mut poll_wf,
                        &mut watched_poll_dirs,
                        &mut failed_poll_watch_dirs,
                        &target_poll_dirs,
                        &poll_modes,
                        "poll",
                        &dir_to_daemons,
                        None,
                        Instant::now(),
                    );
                }

                // Prune stale auto-fallback entries: keep a dir only if at least
                // one of the daemon IDs that originally triggered the fallback is
                // still watching that dir in auto mode. This prevents leaked poll
                // watches after daemon removal AND avoids pinning a new daemon to
                // poll just because a removed daemon had a native-watch failure for
                // the same directory.
                auto_fallback_dirs.retain(|dir, daemon_ids| {
                    daemon_ids.retain(|id| {
                        auto_dir_daemons
                            .get(dir)
                            .is_some_and(|ids| ids.contains(id))
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

    fn project(toml: &str) -> PitchforkToml {
        PitchforkToml::parse_str(toml, Path::new("/tmp/proj/pitchfork.toml")).unwrap()
    }

    #[test]
    fn config_cron_follows_the_config_that_defines_the_daemon() {
        let id = DaemonId::new("proj", "job");
        let pt = project(
            "[daemons.job]\nrun = \"echo\"\ncron = { schedule = \"0 0 0 1 1 *\", retrigger = \"always\" }\n",
        );
        match config_cron_in(&id, Some(&pt), None) {
            ConfigCron::Scheduled(cron) => {
                assert_eq!(cron.schedule, "0 0 0 1 1 *");
                assert_eq!(cron.retrigger, crate::pitchfork_toml::CronRetrigger::Always);
            }
            other => panic!("expected a schedule, got {other:?}"),
        }

        let pt = project("[daemons.job]\nrun = \"echo\"\n");
        assert!(matches!(
            config_cron_in(&id, Some(&pt), None),
            ConfigCron::Unscheduled
        ));
    }

    #[test]
    fn config_cron_counts_a_daemon_as_removed_only_if_its_project_was_read() {
        let id = DaemonId::new("proj", "job");
        // The project was read and still defines another daemon: `job` was
        // deleted from it.
        let pt = project("[daemons.other]\nrun = \"echo\"\n");
        assert!(matches!(
            config_cron_in(&id, Some(&pt), None),
            ConfigCron::Unscheduled
        ));

        // Nothing of the project was read — it failed to load, or is not
        // one the supervisor knows — so nothing may be dropped.
        let elsewhere = PitchforkToml::parse_str(
            "[daemons.other]\nrun = \"echo\"\n",
            Path::new("/tmp/elsewhere/pitchfork.toml"),
        )
        .unwrap();
        assert!(matches!(
            config_cron_in(&id, Some(&elsewhere), None),
            ConfigCron::Unknown
        ));
        assert!(matches!(
            config_cron_in(&id, None, None),
            ConfigCron::Unknown
        ));

        // The project failed to load, and another project loaded on its own
        // shares its namespace: that says nothing about this project.
        assert!(matches!(
            config_cron_in(&id, None, Some(&pt)),
            ConfigCron::Unknown
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_changed_paths_with_new_subtrees() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let root = temp_dir.path().join("src");
        let outside = temp_dir.path().join("outside");
        std::fs::create_dir_all(root.join("moved/nested")).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(root.join("moved/nested/lib.rs"), "").unwrap();
        std::fs::write(outside.join("other.rs"), "").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        let watched = HashMap::from([(root.clone(), RecursiveMode::Recursive)]);
        let events = WatchEvents {
            paths: vec![root.join("moved"), root.join("link")],
            created: vec![
                root.join("moved"),
                root.join("moved/nested"),
                root.join("link"),
            ],
        };
        let mut paths = changed_paths_with_new_subtrees(events, &watched).await;
        paths.sort();

        assert_eq!(
            paths,
            vec![
                root.join("link"),
                root.join("moved"),
                root.join("moved/nested"),
                root.join("moved/nested/lib.rs"),
            ]
        );
    }

    #[test]
    fn test_watch_retry_delay() {
        let delays = (1..=7).map(watch_retry_delay).collect::<Vec<_>>();
        assert_eq!(
            delays,
            [10, 20, 40, 80, 160, 300, 300].map(Duration::from_secs)
        );
        assert_eq!(watch_retry_delay(u32::MAX), WATCH_RETRY_MAX);
    }

    /// A native watch on a missing directory fails, is retried only once its
    /// backoff has elapsed, and is recorded as watched once it succeeds.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn test_failed_watch_is_retried_with_backoff() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let dir = temp_dir.path().join("later");
        let target = HashSet::from([dir.clone()]);
        let modes = HashMap::new();
        let dir_to_daemons = HashMap::new();
        let mut wf = Some(
            WatchFiles::new(
                Duration::from_millis(50),
                WatchMode::Native,
                Duration::from_millis(50),
            )
            .unwrap(),
        );
        let mut watched = HashMap::new();
        let mut failed = HashMap::new();
        let mut pass = |now, watched: &mut _, failed: &mut _| {
            watch_new_dirs(
                &mut wf,
                watched,
                failed,
                &target,
                &modes,
                "native",
                &dir_to_daemons,
                None,
                now,
            )
        };

        let start = Instant::now();
        pass(start, &mut watched, &mut failed);
        assert!(watched.is_empty());
        assert_eq!(failed[&dir].failures, 1);
        assert_eq!(failed[&dir].retry_at, start + WATCH_RETRY_BASE);

        // Still failing: the next attempt waits twice as long.
        pass(start + WATCH_RETRY_BASE, &mut watched, &mut failed);
        assert_eq!(failed[&dir].failures, 2);
        let retry_at = failed[&dir].retry_at;
        assert_eq!(retry_at, start + WATCH_RETRY_BASE * 3);

        // Not retried before the backoff elapses, even once it would succeed.
        std::fs::create_dir(&dir).unwrap();
        pass(retry_at - Duration::from_secs(1), &mut watched, &mut failed);
        assert!(watched.is_empty());
        assert_eq!(failed[&dir].failures, 2);

        pass(retry_at, &mut watched, &mut failed);
        assert_eq!(watched, HashMap::from([(dir, RecursiveMode::NonRecursive)]));
        assert!(failed.is_empty());
    }

    #[test]
    fn test_unwatch_removed_dirs_drops_failed_watches() {
        let targeted = PathBuf::from("/p/targeted");
        let changed_mode = PathBuf::from("/p/changed");
        let removed = PathBuf::from("/p/removed");
        let failed_watch = |mode| FailedWatch {
            mode,
            failures: 3,
            retry_at: Instant::now(),
        };
        let mut failed = HashMap::from([
            (targeted.clone(), failed_watch(RecursiveMode::NonRecursive)),
            (
                changed_mode.clone(),
                failed_watch(RecursiveMode::NonRecursive),
            ),
            (removed.clone(), failed_watch(RecursiveMode::NonRecursive)),
        ]);
        let target = HashSet::from([targeted.clone(), changed_mode.clone()]);
        let modes = HashMap::from([(changed_mode.clone(), RecursiveMode::Recursive)]);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut wf = Some(
            WatchFiles::new(
                Duration::from_millis(50),
                WatchMode::Poll,
                Duration::from_secs(60),
            )
            .unwrap(),
        );

        unwatch_removed_dirs(
            &mut wf,
            &mut HashMap::new(),
            &mut failed,
            &target,
            &modes,
            "poll",
        );

        // Removed targets are forgotten, so they warn again if they fail after
        // coming back. A mode change keeps the entry for the watch_new_dirs
        // mode check to retry at once.
        assert_eq!(
            failed.keys().collect::<HashSet<_>>(),
            target.iter().collect()
        );
    }

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
