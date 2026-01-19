use crate::Result;
use crate::cli::logs::print_startup_logs;
use crate::daemon::RunOptions;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use chrono::{DateTime, Local};
use miette::ensure;
use std::sync::Arc;

/// Restarts a daemon (stops then starts it)
#[derive(Debug, clap::Args)]
#[clap(
    verbatim_doc_comment,
    long_about = "\
Restarts a daemon (stops then starts it)

Sends SIGTERM to stop the daemon, then starts it again from the
pitchfork.toml configuration.

Examples:
  pitchfork restart api           Restart a single daemon
  pitchfork restart api worker    Restart multiple daemons
  pitchfork restart --all         Restart all running daemons"
)]
pub struct Restart {
    /// ID of the daemon(s) to restart
    id: Vec<String>,
    /// Restart all running daemons
    #[clap(long, short)]
    all: bool,
    /// Suppress startup log output
    #[clap(short, long)]
    quiet: bool,
}

impl Restart {
    pub async fn run(&self) -> Result<()> {
        ensure!(
            self.all || !self.id.is_empty(),
            "You must provide at least one daemon to restart, or use --all"
        );

        let pt = PitchforkToml::all_merged();
        let ipc = Arc::new(IpcClient::connect(true).await?);

        // Determine which daemons to restart
        let disabled_daemons = ipc.get_disabled_daemons().await?;
        let ids: Vec<String> = if self.all {
            // Get all running daemons that are in config
            // (ad-hoc daemons started via `pitchfork run` cannot be restarted from config)
            let active = ipc.active_daemons().await?;
            active
                .into_iter()
                .filter(|d| d.pid.is_some() && pt.daemons.contains_key(&d.id))
                .map(|d| d.id)
                .collect()
        } else {
            // Validate all specified daemons BEFORE stopping any of them
            // to avoid terminating daemons that cannot be restarted
            let mut valid_ids = Vec::new();
            for id in &self.id {
                if !pt.daemons.contains_key(id) {
                    warn!(
                        "Daemon {} not found in config (ad-hoc daemons cannot be restarted), skipping",
                        id
                    );
                    continue;
                }
                if disabled_daemons.contains(id) {
                    warn!("Daemon {} is disabled, skipping", id);
                    continue;
                }
                valid_ids.push(id.clone());
            }
            valid_ids
        };

        if ids.is_empty() {
            info!("No daemons to restart");
            return Ok(());
        }

        // Stop all daemons first (all have been validated as restartable)
        for id in &ids {
            if let Err(e) = ipc.stop(id.clone()).await {
                warn!("Failed to stop daemon {}: {}", id, e);
            }
        }

        // Brief delay to allow processes to terminate
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Start all daemons
        let mut tasks = Vec::new();

        for id in ids {
            // Already validated above, but --all path still needs this check
            if self.all && disabled_daemons.contains(&id) {
                continue;
            }

            let daemon_data = match pt.daemons.get(&id) {
                Some(d) => {
                    let run = d.run.clone();
                    let auto_stop = d
                        .auto
                        .contains(&crate::pitchfork_toml::PitchforkTomlAuto::Stop);
                    let dir = d
                        .path
                        .as_ref()
                        .and_then(|p| p.parent())
                        .map(|p| p.to_path_buf())
                        .unwrap_or_default();
                    let cron_schedule = d.cron.as_ref().map(|c| c.schedule.clone());
                    let cron_retrigger = d.cron.as_ref().map(|c| c.retrigger);
                    let retry = d.retry.count();
                    let ready_delay = d.ready_delay;
                    let ready_output = d.ready_output.clone();
                    let ready_http = d.ready_http.clone();
                    let ready_port = d.ready_port;
                    let depends = d.depends.clone();

                    (
                        run,
                        auto_stop,
                        dir,
                        cron_schedule,
                        cron_retrigger,
                        retry,
                        ready_delay,
                        ready_output,
                        ready_http,
                        ready_port,
                        depends,
                    )
                }
                None => {
                    warn!("Daemon {} not found in config, skipping", id);
                    continue;
                }
            };

            let (
                run,
                auto_stop,
                dir,
                cron_schedule,
                cron_retrigger,
                retry,
                ready_delay,
                ready_output,
                ready_http,
                ready_port,
                depends,
            ) = daemon_data;

            let ipc_clone = ipc.clone();

            let task = tokio::spawn(async move {
                let cmd = match shell_words::split(&run) {
                    Ok(c) => c,
                    Err(e) => {
                        error!("Failed to parse command for daemon {}: {}", id, e);
                        return (id, None, Some(1));
                    }
                };

                let start_time = Local::now();

                let (actually_started, exit_code) = match ipc_clone
                    .run(RunOptions {
                        id: id.clone(),
                        cmd,
                        shell_pid: None,
                        force: false,
                        autostop: auto_stop,
                        dir,
                        cron_schedule,
                        cron_retrigger,
                        retry,
                        retry_count: 0,
                        ready_delay: ready_delay.or(Some(3)),
                        ready_output,
                        ready_http,
                        ready_port,
                        wait_ready: true,
                        depends,
                    })
                    .await
                {
                    Ok((started, exit_code)) => (!started.is_empty(), exit_code),
                    Err(e) => {
                        error!("Failed to start daemon {}: {}", id, e);
                        (false, Some(1))
                    }
                };

                // Only report success if daemon was actually started
                if !actually_started {
                    warn!("Daemon {} was not restarted (may still be running)", id);
                    return (id, None, Some(1));
                }

                (id, Some(start_time), exit_code)
            });

            tasks.push(task);
        }

        // Wait for all tasks to complete
        let mut any_failed = false;
        let mut successful_daemons: Vec<(String, DateTime<Local>)> = Vec::new();

        for task in tasks {
            match task.await {
                Ok((id, start_time, exit_code)) => {
                    if exit_code.is_some() {
                        any_failed = true;
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
}
