//! High-level batch operations for daemon management.
//!
//! This module provides batch operations that can be used by CLI, TUI, and Web UI.

use crate::Result;
use crate::daemon::RunOptions;
use crate::deps::resolve_dependencies;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::{PitchforkToml, PitchforkTomlDaemon};
use chrono::{DateTime, Local};
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Result of a daemon run operation
#[derive(Debug, Clone)]
pub struct RunResult {
    pub started: bool,
    pub exit_code: Option<i32>,
    pub start_time: DateTime<Local>,
}

/// Result of batch start operation
#[derive(Debug)]
pub struct StartResult {
    /// Daemons that were successfully started (id, start_time)
    pub started: Vec<(String, DateTime<Local>)>,
    /// Whether any daemon failed to start
    pub any_failed: bool,
}

/// Result of batch stop operation
#[derive(Debug)]
pub struct StopResult {
    /// Whether any daemon failed to stop
    pub any_failed: bool,
}

/// Options for starting daemons
#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    /// Force restart if already running
    pub force: bool,
    /// Shell PID for autostop tracking
    pub shell_pid: Option<u32>,
    /// Override ready delay
    pub delay: Option<u64>,
    /// Override ready output pattern
    pub output: Option<String>,
    /// Override ready HTTP endpoint
    pub http: Option<String>,
    /// Override ready port
    pub port: Option<u16>,
    /// Override ready command
    pub cmd: Option<String>,
    /// Number of times to retry on failure (for ad-hoc daemons)
    pub retry: Option<u32>,
}

/// Build RunOptions from a daemon configuration and start options.
///
/// This is a shared helper used by both IpcClient batch operations and Web UI.
/// It handles:
/// - Command parsing from the config's run string
/// - Extracting all config values (cron, retry, ready checks, depends, etc.)
/// - Merging CLI/API overrides with config defaults
pub fn build_run_options(
    id: &str,
    daemon_config: &PitchforkTomlDaemon,
    opts: &StartOptions,
) -> std::result::Result<RunOptions, String> {
    let cmd = shell_words::split(&daemon_config.run)
        .map_err(|e| format!("Failed to parse command: {e}"))?;

    let dir = resolve_daemon_dir(daemon_config.dir.as_deref(), daemon_config.path.as_deref());

    Ok(RunOptions {
        id: id.to_string(),
        cmd,
        shell_pid: opts.shell_pid,
        force: opts.force,
        autostop: daemon_config
            .auto
            .contains(&crate::pitchfork_toml::PitchforkTomlAuto::Stop),
        dir,
        cron_schedule: daemon_config.cron.as_ref().map(|c| c.schedule.clone()),
        cron_retrigger: daemon_config.cron.as_ref().map(|c| c.retrigger),
        retry: daemon_config.retry.count(),
        retry_count: 0,
        ready_delay: opts.delay.or(daemon_config.ready_delay).or(Some(3)),
        ready_output: opts.output.clone().or(daemon_config.ready_output.clone()),
        ready_http: opts.http.clone().or(daemon_config.ready_http.clone()),
        ready_port: opts.port.or(daemon_config.ready_port),
        ready_cmd: opts.cmd.clone().or(daemon_config.ready_cmd.clone()),
        wait_ready: true,
        depends: daemon_config.depends.clone(),
        env: daemon_config.env.clone(),
        watch: daemon_config.watch.clone(),
        watch_base_dir: daemon_config.path.as_ref().and_then(|p| p.parent().map(|p| p.to_path_buf())),
    })
}

impl IpcClient {
    // =========================================================================
    // Helper functions for resolving daemon IDs
    // =========================================================================

    /// Get all configured daemon IDs from pitchfork.toml files
    pub fn get_all_configured_daemons() -> Vec<String> {
        PitchforkToml::all_merged()
            .daemons
            .keys()
            .cloned()
            .collect()
    }

    /// Get IDs of currently running daemons
    pub async fn get_running_daemons(&self) -> Result<Vec<String>> {
        Ok(self
            .active_daemons()
            .await?
            .iter()
            .filter(|d| d.status.is_running() || d.status.is_waiting())
            .map(|d| d.id.clone())
            .collect())
    }

    // =========================================================================
    // High-level batch operations (for CLI, TUI, Web UI)
    // =========================================================================

    /// Start daemons by ID with dependency resolution
    ///
    /// Handles:
    /// - Dependency resolution (starts dependencies first)
    /// - Disabled daemon filtering
    /// - Already running daemon detection
    /// - Parallel execution within dependency levels
    /// - Ad-hoc daemon restart using saved commands
    pub async fn start_daemons(
        self: &Arc<Self>,
        ids: &[String],
        opts: StartOptions,
    ) -> Result<StartResult> {
        let pt = PitchforkToml::all_merged();
        let disabled_daemons = self.get_disabled_daemons().await?;

        // Get all active daemons for ad-hoc restart support
        let all_daemons = self.active_daemons().await?;
        let adhoc_daemons: HashMap<String, crate::daemon::Daemon> = all_daemons
            .into_iter()
            .filter(|d| !pt.daemons.contains_key(&d.id))
            .map(|d| (d.id.clone(), d))
            .collect();

        // Filter out disabled daemons from the requested list
        let requested_ids: Vec<String> = ids
            .iter()
            .filter(|id| {
                if disabled_daemons.contains(*id) {
                    warn!("Daemon {id} is disabled");
                    false
                } else {
                    true
                }
            })
            .cloned()
            .collect();

        if requested_ids.is_empty() {
            return Ok(StartResult {
                started: vec![],
                any_failed: false,
            });
        }

        // Separate config-based daemons from ad-hoc daemons
        let (config_ids, adhoc_ids): (Vec<String>, Vec<String>) = requested_ids
            .into_iter()
            .partition(|id| pt.daemons.contains_key(id));

        // Get currently running daemons
        let running_daemons: HashSet<String> = self
            .active_daemons()
            .await?
            .iter()
            .filter(|d| d.status.is_running() || d.status.is_waiting())
            .map(|d| d.id.clone())
            .collect();

        // Collect set of explicitly requested IDs for force restart check
        let explicitly_requested: HashSet<String> = ids.iter().cloned().collect();

        // Start daemons level by level
        let mut any_failed = false;
        let mut successful_daemons: Vec<(String, DateTime<Local>)> = Vec::new();

        // First, handle config-based daemons with dependency resolution
        if !config_ids.is_empty() {
            // Resolve dependencies to get start order (levels)
            let dep_order = resolve_dependencies(&config_ids, &pt.daemons)?;

            for level in dep_order.levels {
                // Filter daemons to start in this level
                let to_start: Vec<String> = level
                    .into_iter()
                    .filter(|id| {
                        // Skip disabled daemons (dependencies might be disabled)
                        if disabled_daemons.contains(id) {
                            debug!("Skipping disabled dependency: {id}");
                            return false;
                        }

                        // Skip already running daemons unless force is set AND they were explicitly requested
                        if running_daemons.contains(id) {
                            if opts.force && explicitly_requested.contains(id) {
                                debug!("Force restarting explicitly requested daemon: {id}");
                                return true;
                            }
                            if explicitly_requested.contains(id) {
                                info!("Daemon {id} is already running, use --force to restart");
                            } else {
                                debug!("Daemon {id} is already running, skipping");
                            }
                            return false;
                        }

                        true
                    })
                    .collect();

                if to_start.is_empty() {
                    continue;
                }

                // Start all daemons in this level concurrently
                let mut tasks = Vec::new();
                for id in to_start {
                    if let Some(daemon_config) = pt.daemons.get(&id) {
                        let is_explicit = explicitly_requested.contains(&id);
                        let task = Self::spawn_start_task(
                            self.clone(),
                            id,
                            daemon_config,
                            is_explicit,
                            &opts,
                        );
                        tasks.push(task);
                    }
                }

                // Wait for all daemons in this level to complete before moving to next level
                for task in tasks {
                    match task.await {
                        Ok((id, start_time, exit_code)) => {
                            if exit_code.is_some() {
                                any_failed = true;
                                error!("Daemon {id} failed to start");
                            } else if let Some(start_time) = start_time {
                                successful_daemons.push((id, start_time));
                            }
                        }
                        Err(e) => {
                            error!("Task panicked: {e}");
                            any_failed = true;
                        }
                    }
                }

                // If any daemon in this level failed, abort starting dependents
                if any_failed {
                    error!("Dependency failed, aborting remaining starts");
                    break;
                }
            }
        }

        // Then, handle ad-hoc daemons (no dependency resolution needed)
        if !any_failed && !adhoc_ids.is_empty() {
            let mut tasks = Vec::new();
            for id in adhoc_ids {
                // Skip already running daemons unless force is set
                if running_daemons.contains(&id) {
                    if opts.force && explicitly_requested.contains(&id) {
                        debug!("Force restarting ad-hoc daemon: {id}");
                    } else {
                        if explicitly_requested.contains(&id) {
                            info!("Ad-hoc daemon {id} is already running, use --force to restart");
                        }
                        continue;
                    }
                }

                if let Some(adhoc_daemon) = adhoc_daemons.get(&id) {
                    if let Some(ref cmd) = adhoc_daemon.cmd {
                        let is_explicit = explicitly_requested.contains(&id);
                        let task = Self::spawn_adhoc_start_task(
                            self.clone(),
                            id,
                            cmd.clone(),
                            adhoc_daemon.dir.clone().unwrap_or_default(),
                            adhoc_daemon.env.clone(),
                            is_explicit,
                            &opts,
                        );
                        tasks.push(task);
                    } else {
                        warn!("Ad-hoc daemon {id} has no saved command, cannot restart");
                    }
                } else {
                    warn!("Daemon {id} not found in config or state");
                }
            }

            // Wait for all ad-hoc daemons to complete
            for task in tasks {
                match task.await {
                    Ok((id, start_time, exit_code)) => {
                        if exit_code.is_some() {
                            any_failed = true;
                            error!("Ad-hoc daemon {id} failed to start");
                        } else if let Some(start_time) = start_time {
                            successful_daemons.push((id, start_time));
                        }
                    }
                    Err(e) => {
                        error!("Task panicked: {e}");
                        any_failed = true;
                    }
                }
            }
        }

        Ok(StartResult {
            started: successful_daemons,
            any_failed,
        })
    }

    /// Spawn a task to start a single daemon
    ///
    /// This encapsulates the start logic for one daemon, allowing parallel execution
    /// within the same dependency level. It handles:
    /// - Command parsing
    /// - Config merging (CLI options override config file)
    /// - IPC communication with supervisor
    fn spawn_start_task(
        ipc: Arc<Self>,
        id: String,
        daemon_config: &PitchforkTomlDaemon,
        is_explicitly_requested: bool,
        opts: &StartOptions,
    ) -> tokio::task::JoinHandle<(String, Option<DateTime<Local>>, Option<i32>)> {
        // Build options with force only if explicitly requested
        let mut start_opts = opts.clone();
        start_opts.force = opts.force && is_explicitly_requested;

        let run_opts = build_run_options(&id, daemon_config, &start_opts);

        tokio::spawn(async move {
            let run_opts = match run_opts {
                Ok(opts) => opts,
                Err(e) => {
                    error!("Failed to parse command for daemon {id}: {e}");
                    return (id, None, Some(1));
                }
            };

            let result = ipc.run(run_opts).await;

            match result {
                Ok(run_result) => {
                    if run_result.started {
                        (id, Some(run_result.start_time), run_result.exit_code)
                    } else {
                        (id, None, run_result.exit_code)
                    }
                }
                Err(e) => {
                    error!("Failed to start daemon {id}: {e}");
                    (id, None, Some(1))
                }
            }
        })
    }

    /// Spawn a task to start an ad-hoc daemon using saved command
    ///
    /// This handles restarting ad-hoc daemons that were originally started
    /// via `pitchfork run` command.
    fn spawn_adhoc_start_task(
        ipc: Arc<Self>,
        id: String,
        cmd: Vec<String>,
        dir: PathBuf,
        env: Option<IndexMap<String, String>>,
        is_explicitly_requested: bool,
        opts: &StartOptions,
    ) -> tokio::task::JoinHandle<(String, Option<DateTime<Local>>, Option<i32>)> {
        let force = opts.force && is_explicitly_requested;
        let delay = opts.delay;
        let output = opts.output.clone();
        let http = opts.http.clone();
        let port = opts.port;
        let ready_cmd = opts.cmd.clone();
        let retry = opts.retry.unwrap_or(0);
        let shell_pid = opts.shell_pid;

        tokio::spawn(async move {
            let run_opts = RunOptions {
                id: id.clone(),
                cmd,
                force,
                shell_pid,
                dir,
                autostop: false,
                cron_schedule: None,
                cron_retrigger: None,
                retry,
                retry_count: 0,
                ready_delay: delay.or(Some(3)),
                ready_output: output,
                ready_http: http,
                ready_port: port,
                ready_cmd,
                wait_ready: true,
                depends: vec![],
                env,
                watch: vec![],
                watch_base_dir: None,
            };

            let result = ipc.run(run_opts).await;

            match result {
                Ok(run_result) => {
                    if run_result.started {
                        (id, Some(run_result.start_time), run_result.exit_code)
                    } else {
                        (id, None, run_result.exit_code)
                    }
                }
                Err(e) => {
                    error!("Failed to start ad-hoc daemon {id}: {e}");
                    (id, None, Some(1))
                }
            }
        })
    }

    /// Spawn a task to stop a single daemon
    ///
    /// Similar to spawn_start_task, this allows parallel stopping of daemons
    /// within the same dependency level.
    fn spawn_stop_task(
        ipc: Arc<Self>,
        id: String,
    ) -> tokio::task::JoinHandle<(String, Result<()>)> {
        tokio::spawn(async move {
            let result = ipc.stop(id.clone()).await.map(|_| ());
            (id, result)
        })
    }

    /// Stop daemons by ID with dependency resolution
    ///
    /// Handles:
    /// - Dependency resolution (stops dependents first, in reverse order)
    /// - Ad-hoc daemon handling (no dependencies)
    /// - Parallel execution within dependency levels
    pub async fn stop_daemons(self: &Arc<Self>, ids: &[String]) -> Result<StopResult> {
        let pt = PitchforkToml::all_merged();

        // Get currently running daemons
        let running_daemons: HashSet<String> = self
            .active_daemons()
            .await?
            .iter()
            .filter(|d| d.status.is_running() || d.status.is_waiting())
            .map(|d| d.id.clone())
            .collect();

        // Filter to only running daemons
        let requested_ids: Vec<String> = ids
            .iter()
            .filter(|id| {
                if !running_daemons.contains(*id) {
                    warn!("Daemon {id} is not running");
                    false
                } else {
                    true
                }
            })
            .cloned()
            .collect();

        if requested_ids.is_empty() {
            info!("No running daemons to stop");
            return Ok(StopResult { any_failed: false });
        }

        // Separate config-based daemons from ad-hoc daemons
        let (config_daemons, adhoc_daemons): (Vec<String>, Vec<String>) = requested_ids
            .into_iter()
            .partition(|id| pt.daemons.contains_key(id));

        let mut any_failed = false;

        // Stop config-based daemons with dependency resolution (reverse order)
        if !config_daemons.is_empty() {
            let dep_order = resolve_dependencies(&config_daemons, &pt.daemons)?;

            // Reverse the levels: stop dependents first, then their dependencies
            let reversed_levels: Vec<Vec<String>> = dep_order.levels.into_iter().rev().collect();

            for level in reversed_levels {
                // Filter to only running daemons in this level
                let to_stop: Vec<String> = level
                    .into_iter()
                    .filter(|id| running_daemons.contains(id))
                    .collect();

                if to_stop.is_empty() {
                    continue;
                }

                // Stop all daemons in this level concurrently
                let mut tasks = Vec::new();
                for id in to_stop {
                    let task = Self::spawn_stop_task(self.clone(), id);
                    tasks.push(task);
                }

                // Wait for all stops in this level to complete
                for task in tasks {
                    match task.await {
                        Ok((id, Ok(()))) => {
                            debug!("Successfully stopped daemon {id}");
                        }
                        Ok((id, Err(e))) => {
                            error!("Failed to stop daemon {id}: {e}");
                            any_failed = true;
                        }
                        Err(e) => {
                            error!("Stop task panicked: {e}");
                            any_failed = true;
                        }
                    }
                }
            }
        }

        // Stop ad-hoc daemons (no dependency info) concurrently
        if !adhoc_daemons.is_empty() {
            debug!("Stopping ad-hoc daemons: {adhoc_daemons:?}");
            let mut tasks = Vec::new();
            for id in adhoc_daemons {
                let task = Self::spawn_stop_task(self.clone(), id);
                tasks.push(task);
            }

            for task in tasks {
                match task.await {
                    Ok((id, Ok(()))) => {
                        debug!("Successfully stopped ad-hoc daemon {id}");
                    }
                    Ok((id, Err(e))) => {
                        error!("Failed to stop ad-hoc daemon {id}: {e}");
                        any_failed = true;
                    }
                    Err(e) => {
                        error!("Stop task panicked: {e}");
                        any_failed = true;
                    }
                }
            }
        }

        Ok(StopResult { any_failed })
    }

    /// Run a one-off daemon (not from config)
    pub async fn run_adhoc(
        &self,
        id: String,
        cmd: Vec<String>,
        dir: PathBuf,
        _opts: StartOptions,
    ) -> Result<RunResult> {
        self.run(RunOptions {
            id,
            cmd,
            force: true,
            shell_pid: None,
            dir,
            autostop: false,
            cron_schedule: None,
            cron_retrigger: None,
            retry: 0,
            retry_count: 0,
            ready_delay: Some(0),
            ready_output: None,
            ready_http: None,
            ready_port: None,
            ready_cmd: None,
            wait_ready: true,
            depends: vec![],
            env: None,
            watch: vec![],
            watch_base_dir: None,
        })
        .await
    }
}

/// Resolve the working directory for a daemon.
///
/// If `dir` is set in config, resolve it (absolute or relative to pitchfork.toml parent).
/// Otherwise, use the pitchfork.toml parent directory.
pub fn resolve_daemon_dir(dir: Option<&str>, config_path: Option<&Path>) -> PathBuf {
    let base_dir = config_path
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| crate::env::CWD.to_path_buf());
    match dir {
        Some(d) => base_dir.join(d),
        None => base_dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_daemon_dir_none() {
        // No dir set, config at /projects/myapp/pitchfork.toml -> /projects/myapp
        let result = resolve_daemon_dir(None, Some(Path::new("/projects/myapp/pitchfork.toml")));
        assert_eq!(result, PathBuf::from("/projects/myapp"));
    }

    #[test]
    fn test_resolve_daemon_dir_relative() {
        // Relative dir "frontend" from /projects/myapp/pitchfork.toml -> /projects/myapp/frontend
        let result = resolve_daemon_dir(
            Some("frontend"),
            Some(Path::new("/projects/myapp/pitchfork.toml")),
        );
        assert_eq!(result, PathBuf::from("/projects/myapp/frontend"));
    }

    #[test]
    fn test_resolve_daemon_dir_absolute() {
        // Absolute dir overrides config path entirely
        let result = resolve_daemon_dir(
            Some("/opt/myapp"),
            Some(Path::new("/projects/myapp/pitchfork.toml")),
        );
        assert_eq!(result, PathBuf::from("/opt/myapp"));
    }

    #[test]
    fn test_resolve_daemon_dir_no_config_path() {
        // No config path -> defaults to CWD
        let result = resolve_daemon_dir(None, None);
        assert_eq!(result, crate::env::CWD.to_path_buf());
    }

    #[test]
    fn test_resolve_daemon_dir_relative_no_config_path() {
        // Relative dir but no config path -> relative to CWD
        let result = resolve_daemon_dir(Some("subdir"), None);
        assert_eq!(result, crate::env::CWD.join("subdir"));
    }

    #[test]
    fn test_resolve_daemon_dir_nested_relative() {
        // Nested relative path
        let result = resolve_daemon_dir(
            Some("services/api"),
            Some(Path::new("/projects/myapp/pitchfork.toml")),
        );
        assert_eq!(result, PathBuf::from("/projects/myapp/services/api"));
    }
}
