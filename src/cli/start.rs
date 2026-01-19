use crate::Result;
use crate::cli::logs::print_startup_logs;
use crate::daemon::RunOptions;
use crate::deps::resolve_dependencies;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::{PitchforkToml, PitchforkTomlDaemon};
use chrono::{DateTime, Local};
use miette::ensure;
use std::collections::HashSet;
use std::sync::Arc;

/// Starts a daemon from a pitchfork.toml file
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "s",
    verbatim_doc_comment,
    long_about = "\
Starts a daemon from a pitchfork.toml file

Daemons are defined in pitchfork.toml with a `[daemons.<name>]` section.
The command waits for the daemon to be ready before returning.

Examples:
  pitchfork start api           Start a single daemon
  pitchfork start api worker    Start multiple daemons
  pitchfork start --all         Start all daemons in pitchfork.toml
  pitchfork start api -f        Restart daemon if already running
  pitchfork start api --delay 5 Wait 5 seconds for daemon to be ready
  pitchfork start api --output 'Listening on'
                                Wait for output pattern before ready
  pitchfork start api --http http://localhost:8080/health
                                Wait for HTTP endpoint to return 2xx
  pitchfork start api --port 8080
                                Wait for TCP port to be listening"
)]
pub struct Start {
    /// ID of the daemon(s) in pitchfork.toml to start
    id: Vec<String>,
    /// Start all daemons in all pitchfork.tomls
    #[clap(long, short)]
    all: bool,
    #[clap(long, hide = true)]
    shell_pid: Option<u32>,
    /// Stop the daemon if it is already running
    #[clap(short, long)]
    force: bool,
    /// Delay in seconds before considering daemon ready (default: 3 seconds)
    #[clap(long)]
    delay: Option<u64>,
    /// Wait until output matches this regex pattern before considering daemon ready
    #[clap(long)]
    output: Option<String>,
    /// Wait until HTTP endpoint returns 2xx status before considering daemon ready
    #[clap(long)]
    http: Option<String>,
    /// Wait until TCP port is listening before considering daemon ready
    #[clap(long)]
    port: Option<u16>,
    /// Suppress startup log output
    #[clap(short, long)]
    quiet: bool,
}

impl Start {
    pub async fn run(&self) -> Result<()> {
        ensure!(
            self.all || !self.id.is_empty(),
            "At least one daemon ID must be provided"
        );
        let pt = PitchforkToml::all_merged();
        let ipc = Arc::new(IpcClient::connect(true).await?);
        let disabled_daemons = ipc.get_disabled_daemons().await?;

        // Get requested daemon IDs
        let requested_ids: Vec<String> = if self.all {
            pt.daemons.keys().cloned().collect()
        } else {
            self.id.clone()
        };

        // Filter out disabled daemons from the requested list
        let requested_ids: Vec<String> = requested_ids
            .into_iter()
            .filter(|id| {
                if disabled_daemons.contains(id) {
                    warn!("Daemon {} is disabled", id);
                    false
                } else {
                    true
                }
            })
            .collect();

        if requested_ids.is_empty() {
            return Ok(());
        }

        // Resolve dependencies to get start order (levels)
        let dep_order = resolve_dependencies(&requested_ids, &pt.daemons)?;

        // Get currently running daemons
        let running_daemons: HashSet<String> = ipc
            .active_daemons()
            .await?
            .iter()
            .filter(|d| d.status.is_running() || d.status.is_waiting())
            .map(|d| d.id.clone())
            .collect();

        // Collect set of explicitly requested IDs for force restart check
        let explicitly_requested: HashSet<String> = if self.all {
            // When --all is used, all daemons are explicitly requested
            pt.daemons.keys().cloned().collect()
        } else {
            self.id.iter().cloned().collect()
        };

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
                        debug!("Skipping disabled dependency: {}", id);
                        return false;
                    }

                    // Skip already running daemons unless force is set AND they were explicitly requested
                    if running_daemons.contains(id) {
                        // Only force restart if --force was used AND this daemon was explicitly requested
                        if self.force && explicitly_requested.contains(id) {
                            debug!("Force restarting explicitly requested daemon: {}", id);
                            return true;
                        }
                        debug!("Daemon {} is already running, skipping", id);
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
                        warn!("Daemon {} not found", id);
                        continue;
                    }
                };

                let is_explicit = explicitly_requested.contains(&id);
                let task = self.spawn_daemon_task(&ipc, id, daemon_config, is_explicit);
                tasks.push(task);
            }

            // Wait for all daemons in this level to complete before moving to next level
            for task in tasks {
                match task.await {
                    Ok((id, start_time, exit_code)) => {
                        if exit_code.is_some() {
                            any_failed = true;
                            error!("Daemon {} failed to start", id);
                        } else if let Some(start_time) = start_time {
                            successful_daemons.push((id, start_time));
                        }
                    }
                    Err(e) => {
                        error!("Task panicked: {}", e);
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

        // Show startup logs for successful daemons (unless --quiet)
        if !self.quiet {
            for (id, start_time) in successful_daemons {
                if let Err(e) = print_startup_logs(&id, start_time) {
                    debug!("Failed to print startup logs for {}: {}", id, e);
                }
            }
        }

        if any_failed {
            std::process::exit(1);
        }
        Ok(())
    }

    fn spawn_daemon_task(
        &self,
        ipc: &Arc<IpcClient>,
        id: String,
        daemon_config: &PitchforkTomlDaemon,
        is_explicitly_requested: bool,
    ) -> tokio::task::JoinHandle<(String, Option<DateTime<Local>>, Option<i32>)> {
        let run = daemon_config.run.clone();
        let auto_stop = daemon_config
            .auto
            .contains(&crate::pitchfork_toml::PitchforkTomlAuto::Stop);
        let dir = daemon_config
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let cron_schedule = daemon_config.cron.as_ref().map(|c| c.schedule.clone());
        let cron_retrigger = daemon_config.cron.as_ref().map(|c| c.retrigger);
        let retry = daemon_config.retry.count();
        let ready_delay = daemon_config.ready_delay;
        let ready_output = daemon_config.ready_output.clone();
        let ready_http = daemon_config.ready_http.clone();
        let ready_port = daemon_config.ready_port;
        let depends = daemon_config.depends.clone();

        let ipc_clone = ipc.clone();
        let shell_pid = self.shell_pid;
        // Only force restart if explicitly requested
        let force = self.force && is_explicitly_requested;
        let delay = self.delay;
        let output = self.output.clone();
        let http = self.http.clone();
        let port = self.port;

        tokio::spawn(async move {
            let cmd = match shell_words::split(&run) {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to parse command for daemon {}: {}", id, e);
                    return (id, None, Some(1));
                }
            };

            let start_time = Local::now();

            let exit_code = match ipc_clone
                .run(RunOptions {
                    id: id.clone(),
                    cmd,
                    shell_pid,
                    force,
                    autostop: auto_stop,
                    dir,
                    cron_schedule,
                    cron_retrigger,
                    retry,
                    retry_count: 0,
                    ready_delay: delay.or(ready_delay).or(Some(3)),
                    ready_output: output.or(ready_output),
                    ready_http: http.or(ready_http),
                    ready_port: port.or(ready_port),
                    wait_ready: true,
                    depends,
                })
                .await
            {
                Ok((_started, exit_code)) => exit_code,
                Err(e) => {
                    error!("Failed to start daemon {}: {}", id, e);
                    Some(1)
                }
            };

            (id, Some(start_time), exit_code)
        })
    }
}
