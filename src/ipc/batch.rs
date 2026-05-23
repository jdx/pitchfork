//! High-level batch operations for daemon management.
//!
//! This module provides batch operations that can be used by CLI, TUI, and Web UI.

use crate::Result;
use crate::cli::logs::{ReadyCheckType, create_ready_check_job, stream_startup_logs};
use crate::daemon::RunOptions;
use crate::daemon_id::DaemonId;
use crate::deps::{compute_reverse_stop_order, resolve_dependencies};
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::{
    PitchforkToml, PitchforkTomlDaemon, ReadyHttp, is_dot_config_pitchfork, is_global_config,
};
use chrono::{DateTime, Local};
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Result of a daemon run operation
#[derive(Debug, Clone)]
pub struct RunResult {
    pub started: bool,
    pub exit_code: Option<i32>,
    pub start_time: DateTime<Local>,
    pub resolved_ports: Vec<u16>,
    /// Error message if the daemon failed to start
    pub error_message: Option<String>,
}

/// Result of batch start operation
pub struct StartResult {
    /// Daemons that were successfully started (id, start_time, resolved_ports)
    pub started: Vec<(DaemonId, DateTime<Local>, Vec<u16>)>,
    /// Whether any daemon failed to start
    pub any_failed: bool,
    /// Deferred job status updates — caller must apply these
    /// after all tasks complete, before calling progress::stop()
    pub pending_job_updates: Vec<PendingJobUpdate>,
}

/// Result of batch stop operation
#[derive(Debug)]
pub struct StopResult {
    /// Whether any daemon failed to stop
    pub any_failed: bool,
}

/// A deferred progress job status update.
///
/// Created by spawn tasks, applied by the caller after all log streaming has stopped
/// to prevent `println()` from resetting the progress frame tracker (LINES).
pub struct PendingJobUpdate {
    pub job: Option<Arc<clx::progress::ProgressJob>>,
    pub id: DaemonId,
    pub run_result: std::result::Result<RunResult, miette::Report>,
}

/// Result of spawning a start task
pub struct SpawnTaskResult {
    /// Daemon ID
    pub id: DaemonId,
    /// Progress job for this daemon (None if --quiet)
    pub job: Option<Arc<clx::progress::ProgressJob>>,
    /// Raw IPC run result (used by caller to update job status after all tasks complete)
    pub run_result: std::result::Result<RunResult, miette::Report>,
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
    /// Ports the daemon is expected to bind to (None = not specified, use config)
    pub expected_port: Option<Vec<u16>>,
    /// Port auto-bump configuration (None = use config, Some = override)
    pub auto_bump_port: Option<crate::config_types::PortBump>,
    /// Number of times to retry on failure (for ad-hoc daemons)
    pub retry: Option<crate::config_types::Retry>,
    /// Suppress output (ready check hints, startup logs)
    pub quiet: bool,
}

/// Build RunOptions from a daemon configuration and start options.
///
/// This is a shared helper used by both IpcClient batch operations and Web UI.
/// It handles:
/// - Command parsing from the config's run string
/// - Extracting all config values (cron, retry, ready checks, depends, etc.)
/// - Merging CLI/API overrides with config defaults
pub fn build_run_options(
    id: &DaemonId,
    daemon_config: &PitchforkTomlDaemon,
    overrides: Option<&StartOptions>,
) -> std::result::Result<RunOptions, String> {
    let cmd = shell_words::split(&daemon_config.run)
        .map_err(|e| format!("Failed to parse command: {e}"))?;

    let mut run_opts = daemon_config.to_run_options(id, cmd);
    run_opts.wait_ready = true;
    run_opts.ready_delay = run_opts.ready_delay.or(Some(3));

    if let Some(opts) = overrides {
        run_opts.shell_pid = opts.shell_pid;
        run_opts.force = opts.force;
        run_opts.ready_delay = opts.delay.or(run_opts.ready_delay);
        run_opts.ready_output = opts.output.clone().or(run_opts.ready_output);
        run_opts.ready_http = merge_ready_http_override(run_opts.ready_http, opts.http.clone());
        run_opts.ready_port = opts.port.or(run_opts.ready_port);
        run_opts.ready_cmd = opts.cmd.clone().or(run_opts.ready_cmd);
        if let Some(ref expected) = opts.expected_port {
            run_opts.port.get_or_insert_with(Default::default).expect = expected.clone();
        }
        if let Some(bump) = opts.auto_bump_port {
            run_opts.port.get_or_insert_with(Default::default).bump = bump;
        }
    }

    Ok(run_opts)
}

fn merge_ready_http_override(
    configured: Option<ReadyHttp>,
    override_url: Option<String>,
) -> Option<ReadyHttp> {
    match (configured, override_url) {
        (Some(mut ready_http), Some(url)) => {
            ready_http.url = url;
            Some(ready_http)
        }
        (None, Some(url)) => Some(ReadyHttp::new(url)),
        (ready_http, None) => ready_http,
    }
}

/// Determine the effective ready check type from merged RunOptions.
fn ready_check_type(opts: &RunOptions) -> ReadyCheckType {
    if let Some(ref pattern) = opts.ready_output {
        ReadyCheckType::Output(pattern.clone())
    } else if let Some(ref http) = opts.ready_http {
        ReadyCheckType::Http(http.url.clone())
    } else if let Some(port) = opts.ready_port {
        ReadyCheckType::Port(port)
    } else if let Some(ref cmd) = opts.ready_cmd {
        ReadyCheckType::Cmd(cmd.clone())
    } else if let Some(secs) = opts.ready_delay {
        ReadyCheckType::Delay(secs)
    } else {
        ReadyCheckType::Default
    }
}

/// Update a progress job's body and status based on the IPC run result.
///
/// Sets the body to a styled success or failure message and transitions
/// the job status to Done/Failed. For quiet mode (job = None), logs errors
/// to stderr instead.
///
/// Must be called after all log streaming has stopped to avoid `println()`
/// resetting the progress frame tracker (LINES), which causes duplicate output.
pub fn update_job_with_result(
    job: Option<&clx::progress::ProgressJob>,
    id: &DaemonId,
    result: &std::result::Result<RunResult, miette::Report>,
) {
    use clx::progress::ProgressStatus;

    let id_label = {
        let is_tty = std::io::stderr().is_terminal();
        let colors_enabled = is_tty && console::colors_enabled_stderr();
        crate::cli::logs::colored_id_label(&id.qualified(), colors_enabled)
    };

    let show_ts = crate::settings::settings().general.startup_log_timestamps;
    // When timestamps are off, the body uses {{spinner()}} which renders
    // as ✔/✗/spinner -- naturally 1 char wide, matching the "•" prefix
    // used by println.  When timestamps are on, we replace {{spinner()}}
    // with a dim timestamp string.
    let prefix = if show_ts {
        let now = chrono::Local::now();
        format!("{}", crate::ui::style::edim(now.format("%H:%M:%S")))
    } else {
        "{{spinner()}}".to_string()
    };

    if let Some(job) = job {
        match result {
            Ok(run_result) if run_result.started => {
                let body = if run_result.resolved_ports.is_empty() {
                    format!("{prefix} {id_label} started")
                } else {
                    let port_str = run_result
                        .resolved_ports
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    let port_label = if run_result.resolved_ports.len() == 1 {
                        "port"
                    } else {
                        "ports"
                    };
                    format!(
                        "{prefix} {id_label} started on {port_label} {}",
                        crate::ui::style::ncyan(&port_str)
                    )
                };
                job.set_body(body);
                job.set_status(ProgressStatus::Done);
            }
            Ok(run_result) => {
                if run_result.exit_code.is_none() && !run_result.started {
                    job.remove();
                    return;
                }
                let exit_info = run_result
                    .exit_code
                    .map(|c| format!(" (exit code {c})"))
                    .unwrap_or_default();
                let error_detail = run_result
                    .error_message
                    .as_ref()
                    .map(|msg| format!(": {msg}"))
                    .unwrap_or_default();
                job.set_body(format!(
                    "{prefix} {id_label} failed{exit_info}{error_detail}"
                ));
                job.set_status(ProgressStatus::Failed);
            }
            Err(e) => {
                job.set_body(format!("{prefix} {id_label} failed: {e}"));
                job.set_status(ProgressStatus::Failed);
            }
        }
    } else if let Ok(run_result) = result {
        if !run_result.started && run_result.exit_code.is_some() {
            if let Some(ref msg) = run_result.error_message {
                error!("{msg}");
            }
            if let Ok(lines) = crate::cli::logs::collect_startup_logs(id, run_result.start_time) {
                crate::cli::logs::print_error_logs_block(&lines);
            }
        }
    } else if let Err(e) = result {
        error!("Failed to start daemon {id}: {e}");
    }
}

impl IpcClient {
    // =========================================================================
    // Helper functions for resolving daemon IDs
    // =========================================================================

    /// Get all configured daemon IDs from pitchfork.toml files (both local and global)
    pub fn get_all_configured_daemons() -> Result<Vec<DaemonId>> {
        Ok(PitchforkToml::all_merged()?
            .daemons
            .keys()
            .cloned()
            .collect())
    }

    /// Get local configured daemon IDs (from pitchfork.toml files in current directory hierarchy)
    ///
    /// This excludes daemons from global config files (~/.config/pitchfork/config.toml
    /// and /etc/pitchfork/config.toml), returning only daemons defined in project-level
    /// pitchfork.toml files.
    pub fn get_local_configured_daemons() -> Result<Vec<DaemonId>> {
        Self::get_configured_daemons_filtered(|id| id.namespace() != "global")
    }

    /// Get global configured daemon IDs (from ~/.config/pitchfork/config.toml and /etc/pitchfork/config.toml)
    ///
    /// This returns only daemons defined in global config files, excluding project-level
    /// pitchfork.toml files.
    pub fn get_global_configured_daemons() -> Result<Vec<DaemonId>> {
        Self::get_configured_daemons_filtered(|id| id.namespace() == "global")
    }

    /// Get configured daemon IDs filtered by a predicate
    fn get_configured_daemons_filtered<F>(predicate: F) -> Result<Vec<DaemonId>>
    where
        F: Fn(&DaemonId) -> bool,
    {
        Ok(PitchforkToml::all_merged()?
            .daemons
            .into_keys()
            .filter(predicate)
            .collect())
    }

    /// Get IDs of currently running daemons
    pub async fn get_running_daemons(&self) -> Result<Vec<DaemonId>> {
        Ok(self
            .active_daemons()
            .await?
            .iter()
            .filter(|d| d.status.is_running() || d.status.is_waiting())
            .map(|d| d.id.clone())
            .collect())
    }

    /// Get IDs of currently running daemons that are configured
    /// for stop / restart with --local or --global
    pub async fn get_running_configured_daemons(&self, global: bool) -> Result<Vec<DaemonId>> {
        let configured: HashSet<DaemonId> = if global {
            Self::get_global_configured_daemons()?
        } else {
            Self::get_local_configured_daemons()?
        }
        .into_iter()
        .collect();

        Ok(self
            .get_running_daemons()
            .await?
            .into_iter()
            .filter(|id| configured.contains(id))
            .collect())
    }

    // =========================================================================
    // High-level batch operations (for CLI, TUI, Web UI)
    // =========================================================================

    /// Start daemons by ID with dependency resolution
    ///
    /// Handles:
    /// - ID resolution (short ID to qualified ID based on current directory)
    /// - Dependency resolution (starts dependencies first)
    /// - Disabled daemon filtering
    /// - Already running daemon detection
    /// - Parallel execution within dependency levels
    /// - Ad-hoc daemon restart using saved commands
    pub async fn start_daemons(
        self: &Arc<Self>,
        ids: &[DaemonId],
        opts: StartOptions,
    ) -> Result<StartResult> {
        let pt = PitchforkToml::all_merged_all_namespaces()?;
        let disabled_daemons = self.get_disabled_daemons().await?;

        // Get all active daemons for ad-hoc restart support
        let all_daemons = self.active_daemons().await?;
        let adhoc_daemons: HashMap<DaemonId, crate::daemon::Daemon> = all_daemons
            .into_iter()
            .filter(|d| !pt.daemons.contains_key(&d.id))
            .map(|d| (d.id.clone(), d))
            .collect();

        // Filter out disabled daemons from the requested list
        let requested_ids: Vec<DaemonId> = ids
            .iter()
            .filter(|id| {
                if disabled_daemons.contains(id) {
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
                pending_job_updates: vec![],
            });
        }

        // Separate config-based daemons from ad-hoc daemons
        let (config_ids, adhoc_ids): (Vec<DaemonId>, Vec<DaemonId>) = requested_ids
            .into_iter()
            .partition(|id| pt.daemons.contains_key(id));

        // Get currently running daemons once and reuse the snapshot for both
        // restart checks and template context.
        let active_daemons = self.active_daemons().await?;
        let running_daemons: HashSet<DaemonId> = active_daemons
            .iter()
            .filter(|d| d.status.is_running() || d.status.is_waiting())
            .map(|d| d.id.clone())
            .collect();
        let running_ports_map: HashMap<DaemonId, Vec<u16>> = active_daemons
            .into_iter()
            .filter(|d| {
                (d.status.is_running() || d.status.is_waiting()) && !d.resolved_port.is_empty()
            })
            .map(|d| (d.id, d.resolved_port))
            .collect();

        // Collect set of explicitly requested IDs for force restart check
        let explicitly_requested: HashSet<DaemonId> = ids.iter().cloned().collect();

        // Start daemons level by level
        let mut any_failed = false;
        let mut successful_daemons: Vec<(DaemonId, DateTime<Local>, Vec<u16>)> = Vec::new();
        // Accumulated resolved ports from completed levels, available for template rendering
        let mut resolved_ports_map: std::collections::HashMap<DaemonId, Vec<u16>> =
            std::collections::HashMap::new();
        // Collect all task results for deferred job status updates
        let mut pending_job_updates: Vec<PendingJobUpdate> = Vec::new();

        // First, handle config-based daemons with dependency resolution
        if !config_ids.is_empty() {
            // Resolve dependencies to get start order (levels)
            let dep_order = resolve_dependencies(&config_ids, &pt.daemons)?;

            for (level_idx, level) in dep_order.levels.iter().enumerate() {
                let is_last_level = level_idx == dep_order.levels.len() - 1;
                let mut successful_this_level: Vec<(DaemonId, Vec<u16>)> = Vec::new();

                // Filter daemons to start in this level
                let to_start: Vec<DaemonId> = level
                    .iter()
                    .filter(|&id| {
                        // Skip disabled daemons (dependencies might be disabled)
                        if disabled_daemons.contains(id) {
                            warn!("Skipping disabled daemon {id} (dependency)");
                            return false;
                        }

                        // Skip already running daemons unless they are explicitly requested
                        // with force=true
                        if running_daemons.contains(id) {
                            if opts.force && explicitly_requested.contains(id) {
                                debug!("Force restarting explicitly requested daemon: {id}");
                                true // Allow restart if force is set AND explicitly requested
                            } else {
                                if explicitly_requested.contains(id) {
                                    info!("Daemon {id} is already running, use --force to restart");
                                } else {
                                    debug!("Skipping already running daemon {id}");
                                }
                                false
                            }
                        } else {
                            true
                        }
                    })
                    .cloned()
                    .collect();

                for id in level {
                    if let Some(ports) = running_ports_map.get(id) {
                        resolved_ports_map.insert(id.clone(), ports.clone());
                    }
                }

                if to_start.is_empty() {
                    continue;
                }

                // Start all daemons in this level concurrently
                let mut tasks = Vec::new();
                for id in to_start {
                    if let Some(daemon_config) = pt.daemons.get(&id) {
                        // Render Tera templates with context from previously started daemons
                        let mut rendered_config = daemon_config.clone();
                        let template_ctx = crate::template::TemplateContext::new(
                            &id,
                            daemon_config,
                            &resolved_ports_map,
                            &pt.daemons,
                        );
                        match crate::template::render_daemon_templates(
                            &mut rendered_config,
                            &template_ctx,
                        ) {
                            Ok(()) => {}
                            Err(e) => {
                                error!("Template render error for daemon {id}: {e}");
                                any_failed = true;
                                continue;
                            }
                        }

                        let is_explicit = explicitly_requested.contains(&id);
                        let task = Self::spawn_start_task(
                            self.clone(),
                            id,
                            &rendered_config,
                            is_explicit,
                            &opts,
                        );
                        tasks.push(task);
                    }
                }

                // Wait for all daemons in this level to complete before moving to next level
                for task in tasks {
                    match task.await {
                        Ok(result) => {
                            let SpawnTaskResult {
                                id,
                                job,
                                run_result,
                                ..
                            } = result;
                            match &run_result {
                                Ok(rr) if rr.started => {
                                    successful_this_level
                                        .push((id.clone(), rr.resolved_ports.clone()));
                                    successful_daemons.push((
                                        id.clone(),
                                        rr.start_time,
                                        rr.resolved_ports.clone(),
                                    ));
                                }
                                Ok(rr) => {
                                    if rr.exit_code.is_some() {
                                        any_failed = true;
                                        error!("Daemon {} failed to start", id);
                                    }
                                }
                                Err(_) => {
                                    any_failed = true;
                                }
                            }
                            pending_job_updates.push(PendingJobUpdate {
                                job: job.clone(),
                                id: id.clone(),
                                run_result,
                            });
                        }
                        Err(e) => {
                            error!("Task panicked: {e}");
                            any_failed = true;
                        }
                    }
                }

                // Daemons from this level become visible to later levels if they
                // either started successfully now or were already running.
                for (id, ports) in successful_this_level {
                    resolved_ports_map.insert(id, ports);
                }
                // If any daemon in this level failed, abort starting dependents
                if any_failed {
                    if !is_last_level {
                        error!("Dependency failed, aborting remaining starts");
                    }
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
                            adhoc_daemon.ready_http.clone(),
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
                    Ok(result) => {
                        let SpawnTaskResult {
                            id,
                            job,
                            run_result,
                            ..
                        } = result;
                        match &run_result {
                            Ok(rr) if rr.started => {
                                successful_daemons.push((
                                    id.clone(),
                                    rr.start_time,
                                    rr.resolved_ports.clone(),
                                ));
                            }
                            Ok(rr) => {
                                if rr.exit_code.is_some() {
                                    any_failed = true;
                                }
                            }
                            Err(_) => {
                                any_failed = true;
                            }
                        }
                        pending_job_updates.push(PendingJobUpdate {
                            job: job.clone(),
                            id: id.clone(),
                            run_result,
                        });
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
            pending_job_updates,
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
        id: DaemonId,
        daemon_config: &PitchforkTomlDaemon,
        is_explicitly_requested: bool,
        opts: &StartOptions,
    ) -> tokio::task::JoinHandle<SpawnTaskResult> {
        // Build options with force only if explicitly requested
        let mut start_opts = opts.clone();
        start_opts.force = opts.force && is_explicitly_requested;

        let run_opts = build_run_options(&id, daemon_config, Some(&start_opts));
        let quiet = opts.quiet;

        tokio::spawn(async move {
            let run_opts = match run_opts {
                Ok(opts) => opts,
                Err(e) => {
                    return SpawnTaskResult {
                        id,
                        job: None,
                        run_result: Err(miette::miette!("Failed to parse command: {e}")),
                    };
                }
            };

            let check_type = ready_check_type(&run_opts);

            let job = if !quiet {
                let job = create_ready_check_job(&id, &check_type);
                Some(job)
            } else {
                None
            };

            let start_time = chrono::Local::now();

            // Start streaming logs for this daemon
            let (log_stop_tx, log_handle) = if let Some(ref job) = job {
                let (tx, handle) = stream_startup_logs(&id, start_time, job.clone());
                (Some(tx), Some(handle))
            } else {
                (None, None)
            };

            let result = ipc.run(run_opts).await;

            // Stop log streaming and wait for the task to fully exit
            if let Some(tx) = &log_stop_tx {
                let _ = tx.send(true);
            }
            if let Some(handle) = log_handle {
                let _ = handle.await;
            }

            SpawnTaskResult {
                id,
                job,
                run_result: result,
            }
        })
    }

    /// Spawn a task to start an ad-hoc daemon using saved command
    ///
    /// This handles restarting ad-hoc daemons that were originally started
    /// via `pitchfork run` command.
    #[allow(clippy::too_many_arguments)]
    fn spawn_adhoc_start_task(
        ipc: Arc<Self>,
        id: DaemonId,
        cmd: Vec<String>,
        dir: PathBuf,
        env: Option<IndexMap<String, String>>,
        ready_http: Option<ReadyHttp>,
        is_explicitly_requested: bool,
        opts: &StartOptions,
    ) -> tokio::task::JoinHandle<SpawnTaskResult> {
        let force = opts.force && is_explicitly_requested;
        let delay = opts.delay;
        let output = opts.output.clone();
        let http = merge_ready_http_override(ready_http, opts.http.clone());
        let port = opts.port;
        let ready_cmd = opts.cmd.clone();
        let expected_port = opts.expected_port.clone();
        let auto_bump_port = opts.auto_bump_port;
        let retry = opts.retry.unwrap_or_default();
        let shell_pid = opts.shell_pid;
        let quiet = opts.quiet;

        tokio::spawn(async move {
            let run_opts = RunOptions {
                id: id.clone(),
                cmd,
                force,
                shell_pid,
                dir: crate::config_types::Dir(dir),
                retry,
                ready_delay: delay.or(Some(3)),
                ready_output: output,
                ready_http: http,
                ready_port: port,
                ready_cmd,
                port: crate::config_types::PortConfig::from_parts(
                    expected_port.unwrap_or_default(),
                    auto_bump_port.unwrap_or_default(),
                ),
                wait_ready: true,
                env,
                watch: vec![],
                watch_base_dir: None,
                mise: None,
                slug: None,
                proxy: None,
                ..RunOptions::default()
            };

            let check_type = ready_check_type(&run_opts);

            let job = if !quiet {
                let job = create_ready_check_job(&id, &check_type);
                Some(job)
            } else {
                None
            };

            let start_time = chrono::Local::now();

            // Start streaming logs for this daemon
            let (log_stop_tx, log_handle) = if let Some(ref job) = job {
                let (tx, handle) = stream_startup_logs(&id, start_time, job.clone());
                (Some(tx), Some(handle))
            } else {
                (None, None)
            };

            let result = ipc.run(run_opts).await;

            // Stop log streaming and wait for the task to fully exit
            if let Some(tx) = &log_stop_tx {
                let _ = tx.send(true);
            }
            if let Some(handle) = log_handle {
                let _ = handle.await;
            }

            SpawnTaskResult {
                id,
                job,
                run_result: result,
            }
        })
    }

    /// Spawn a task to stop a single daemon
    ///
    /// Similar to spawn_start_task, this allows parallel stopping of daemons
    /// within the same dependency level.
    fn spawn_stop_task(
        ipc: Arc<Self>,
        id: DaemonId,
    ) -> tokio::task::JoinHandle<(DaemonId, Result<()>)> {
        tokio::spawn(async move {
            let result = ipc.stop(id.clone()).await.map(|_| ());
            (id, result)
        })
    }
    // =========================================================================
    // Single-daemon operations (shared by CLI, TUI, and Web UI)
    // =========================================================================

    /// Start a single daemon with configuration from pitchfork.toml.
    ///
    /// Handles:
    /// - ID resolution
    /// - Config reading and command parsing
    /// - `RunOptions` construction with correct defaults (`wait_ready=true`, `ready_delay=Some(3)`)
    /// - IPC communication with supervisor
    ///
    /// `overrides` can be used by the CLI to override config defaults with command-line flags.
    /// Web callers pass `None` to use pure config defaults.
    pub async fn start_daemon(
        &self,
        id: &DaemonId,
        overrides: Option<&StartOptions>,
    ) -> Result<RunResult> {
        let pt = PitchforkToml::all_merged_all_namespaces()?;

        let daemon_config = pt
            .daemons
            .get(id)
            .cloned()
            .ok_or_else(|| miette::miette!("Daemon config not found for {id}"))?;

        let run_opts =
            build_run_options(id, &daemon_config, overrides).map_err(|e| miette::miette!("{e}"))?;

        self.run(run_opts).await
    }

    /// Restart a single daemon by stopping then starting it.
    ///
    /// Stop errors are ignored (daemon may not be running).
    pub async fn restart_daemon(
        &self,
        id: &DaemonId,
        overrides: Option<&StartOptions>,
    ) -> Result<RunResult> {
        let _ = self.stop(id.clone()).await;
        self.start_daemon(id, overrides).await
    }

    /// Stop daemons by ID with dependency resolution
    ///
    /// Handles:
    /// - ID resolution (short ID to qualified ID based on current directory)
    /// - Dependency resolution (stops dependents first, in reverse order)
    /// - Ad-hoc daemon handling (no dependencies)
    /// - Parallel execution within dependency levels
    pub async fn stop_daemons(self: &Arc<Self>, ids: &[DaemonId]) -> Result<StopResult> {
        // Get currently running daemons
        let running_daemons: HashSet<DaemonId> = self
            .active_daemons()
            .await?
            .iter()
            .filter(|d| d.status.is_running() || d.status.is_waiting())
            .map(|d| d.id.clone())
            .collect();

        // Filter to only running daemons
        let requested_ids: Vec<DaemonId> = ids
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

        let mut any_failed = false;

        // Use shared reverse dependency ordering
        let stop_levels = compute_reverse_stop_order(&requested_ids);

        for level in stop_levels {
            // Filter to only running daemons in this level
            let to_stop: Vec<DaemonId> = level
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

        Ok(StopResult { any_failed })
    }

    /// Run a one-off daemon (not from config)
    pub async fn run_adhoc(
        &self,
        id: DaemonId,
        cmd: Vec<String>,
        dir: PathBuf,
        opts: StartOptions,
    ) -> Result<RunResult> {
        self.run(RunOptions {
            id,
            cmd,
            shell_pid: opts.shell_pid,
            force: opts.force,
            dir: crate::config_types::Dir(dir),
            retry: opts.retry.unwrap_or_default(),
            ready_delay: opts.delay.or(Some(3)),
            ready_output: opts.output,
            ready_http: merge_ready_http_override(None, opts.http),
            ready_port: opts.port,
            ready_cmd: opts.cmd.clone(),
            port: crate::config_types::PortConfig::from_parts(
                opts.expected_port.unwrap_or_default(),
                opts.auto_bump_port.unwrap_or_default(),
            ),
            wait_ready: true,
            mise: None,
            slug: None,
            proxy: None,
            ..RunOptions::default()
        })
        .await
    }
}

/// Resolve the project base directory from a config file path.
///
/// For `.config/pitchfork.toml` and `.config/pitchfork.local.toml`, the project
/// directory is the grandparent (i.e. the directory containing `.config/`).
/// For all other config files, it is the parent directory.
pub fn resolve_config_base_dir(config_path: Option<&Path>) -> PathBuf {
    config_path
        .and_then(|p| {
            if is_global_config(p) {
                p.parent()
            } else if is_dot_config_pitchfork(p) {
                // .config/pitchfork.toml and .config/pitchfork.local.toml uses project directory (grandparent)
                p.parent().and_then(|p| p.parent())
            } else {
                p.parent()
            }
        })
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| crate::env::CWD.to_path_buf())
}

/// Resolve the working directory for a daemon.
///
/// If `dir` is set in config, resolve it relative to the project base directory.
/// Otherwise, use the project base directory directly.
pub fn resolve_daemon_dir(dir: Option<&str>, config_path: Option<&Path>) -> PathBuf {
    let base_dir = resolve_config_base_dir(config_path);
    match dir {
        Some(d) => base_dir.join(d),
        None => base_dir,
    }
}

#[cfg(test)]
mod tests {
    use crate::env;

    use super::*;

    #[test]
    fn http_override_preserves_configured_status_codes() {
        let configured = Some(ReadyHttp {
            url: "http://localhost:3000/original".to_string(),
            status: vec![401],
        });

        let ready_http =
            merge_ready_http_override(configured, Some("http://localhost:3000/health".to_string()))
                .unwrap();

        assert_eq!(ready_http.url, "http://localhost:3000/health");
        assert_eq!(ready_http.status, vec![401]);
        assert!(ready_http.accepts_status(401));
        assert!(!ready_http.accepts_status(200));
    }

    #[test]
    fn build_run_options_preserves_ready_http_status_for_cli_http_override() {
        let id = DaemonId::try_new("project", "api").unwrap();
        let daemon_config = PitchforkTomlDaemon {
            run: "echo ready".to_string(),
            ready_http: Some(ReadyHttp {
                url: "http://localhost:3000/original".to_string(),
                status: vec![401],
            }),
            ..PitchforkTomlDaemon::default()
        };
        let opts = StartOptions {
            http: Some("http://localhost:3000/health".to_string()),
            ..StartOptions::default()
        };

        let run_opts = build_run_options(&id, &daemon_config, Some(&opts)).unwrap();
        let ready_http = run_opts.ready_http.unwrap();

        assert_eq!(ready_http.url, "http://localhost:3000/health");
        assert_eq!(ready_http.status, vec![401]);
    }

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

    #[test]
    fn test_resolve_daemon_dir_dot_config_none() {
        // .config/pitchfork.toml should resolve to project directory (not .config)
        let result = resolve_daemon_dir(
            None,
            Some(Path::new("/projects/myapp/.config/pitchfork.toml")),
        );
        assert_eq!(
            result,
            PathBuf::from("/projects/myapp"),
            ".config/pitchfork.toml should resolve to project dir"
        );
    }

    #[test]
    fn test_resolve_daemon_dir_dot_config_local_none() {
        // .config/pitchfork.toml should resolve to project directory (not .config)
        let result = resolve_daemon_dir(
            None,
            Some(Path::new("/projects/myapp/.config/pitchfork.local.toml")),
        );
        assert_eq!(
            result,
            PathBuf::from("/projects/myapp"),
            ".config/pitchfork.local.toml should resolve to project dir"
        );
    }

    #[test]
    fn test_resolve_daemon_dir_dot_config_relative() {
        // Relative dir from .config/pitchfork.toml -> project dir + relative
        let result = resolve_daemon_dir(
            Some("frontend"),
            Some(Path::new("/projects/myapp/.config/pitchfork.toml")),
        );
        assert_eq!(
            result,
            PathBuf::from("/projects/myapp/frontend"),
            "Relative dir should resolve from project dir"
        );
    }

    #[test]
    fn test_resolve_daemon_dir_dot_config_local_relative() {
        // Relative dir from .config/pitchfork.toml -> project dir + relative
        let result = resolve_daemon_dir(
            Some("frontend"),
            Some(Path::new("/projects/myapp/.config/pitchfork.local.toml")),
        );
        assert_eq!(
            result,
            PathBuf::from("/projects/myapp/frontend"),
            "Relative dir should resolve from project dir"
        );
    }

    #[test]
    fn test_resolve_daemon_dir_dot_config_absolute() {
        // Absolute dir overrides project dir
        let result = resolve_daemon_dir(
            Some("/opt/service"),
            Some(Path::new("/projects/myapp/.config/pitchfork.toml")),
        );
        assert_eq!(
            result,
            PathBuf::from("/opt/service"),
            "Absolute dir should override project dir"
        );
    }

    #[test]
    fn test_resolve_daemon_dir_dot_config_local_absolute() {
        // Absolute dir overrides project dir
        let result = resolve_daemon_dir(
            Some("/opt/service"),
            Some(Path::new("/projects/myapp/.config/pitchfork.local.toml")),
        );
        assert_eq!(
            result,
            PathBuf::from("/opt/service"),
            "Absolute dir should override project dir"
        );
    }

    #[test]
    fn test_resolve_daemon_dir_global_config_normal() {
        // Global config (~/.config/pitchfork/config.toml) should use normal resolution (parent)
        let global_path = env::PITCHFORK_GLOBAL_CONFIG_USER.as_path();
        let result = resolve_daemon_dir(None, Some(global_path));
        assert_eq!(
            result,
            global_path.parent().unwrap(),
            "Global config should use parent directory"
        );
    }
}
