//! High-level batch operations for daemon management.
//!
//! This module provides batch operations that can be used by CLI, TUI, and Web UI.

use crate::Result;
use crate::daemon::RunOptions;
use crate::deps::resolve_dependencies;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::{PitchforkToml, PitchforkTomlDaemon};
use chrono::{DateTime, Local};
use std::collections::HashSet;
use std::path::PathBuf;
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

    let dir = daemon_config
        .path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_default();

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
    pub async fn start_daemons(
        self: &Arc<Self>,
        ids: &[String],
        opts: StartOptions,
    ) -> Result<StartResult> {
        let pt = PitchforkToml::all_merged();
        let disabled_daemons = self.get_disabled_daemons().await?;

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

        // Resolve dependencies to get start order (levels)
        let dep_order = resolve_dependencies(&requested_ids, &pt.daemons)?;

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
                let daemon_config = match pt.daemons.get(&id) {
                    Some(d) => d,
                    None => {
                        warn!("Daemon {id} not found in config");
                        continue;
                    }
                };

                let is_explicit = explicitly_requested.contains(&id);
                let task =
                    Self::spawn_start_task(self.clone(), id, daemon_config, is_explicit, &opts);
                tasks.push(task);
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
        opts: StartOptions,
    ) -> Result<RunResult> {
        self.run(RunOptions {
            id,
            cmd,
            shell_pid: opts.shell_pid,
            force: opts.force,
            dir,
            autostop: false,
            cron_schedule: None,
            cron_retrigger: None,
            retry: opts.retry.unwrap_or(0),
            retry_count: 0,
            ready_delay: opts.delay.or(Some(3)),
            ready_output: opts.output,
            ready_http: opts.http,
            ready_port: opts.port,
            ready_cmd: opts.cmd,
            wait_ready: true,
            depends: vec![],
        })
        .await
    }
}
