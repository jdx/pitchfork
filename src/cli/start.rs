use crate::daemon::RunOptions;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use crate::Result;
use miette::{ensure, IntoDiagnostic};
use std::collections::HashSet;

/// Starts a daemon from a pitchfork.toml file
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "s", verbatim_doc_comment)]
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
}

impl Start {
    pub async fn run(&self) -> Result<()> {
        ensure!(
            self.all || !self.id.is_empty(),
            "At least one daemon ID must be provided"
        );
        let pt = PitchforkToml::all_merged();
        let ipc = IpcClient::connect(true).await?;
        let disabled_daemons = ipc.get_disabled_daemons().await?;
        let active_daemons: HashSet<String> = ipc
            .active_daemons()
            .await?
            .into_iter()
            .map(|d| d.id)
            .collect();
        let ids = if self.all {
            pt.daemons.keys().cloned().collect()
        } else {
            self.id.clone()
        };
        let mut any_failed = false;
        let mut last_exit_code = 0;

        for id in &ids {
            if disabled_daemons.contains(id) {
                warn!("Daemon {} is disabled", id);
                continue;
            }
            if !self.force && active_daemons.contains(id) {
                warn!("Daemon {} is already running", id);
                continue;
            }
            let daemon = pt.daemons.get(id);
            if let Some(daemon) = daemon {
                info!("Starting daemon {}", id);
                let start_time = chrono::Local::now();
                let cmd = shell_words::split(&daemon.run).into_diagnostic()?;
                let (started, exit_code) = ipc
                    .run(RunOptions {
                        id: id.clone(),
                        cmd,
                        shell_pid: self.shell_pid,
                        force: self.force,
                        autostop: daemon
                            .auto
                            .contains(&crate::pitchfork_toml::PitchforkTomlAuto::Stop),
                        dir: daemon
                            .path
                            .as_ref()
                            .unwrap()
                            .parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_default(),
                        cron_schedule: daemon.cron.as_ref().map(|c| c.schedule.clone()),
                        cron_retrigger: daemon.cron.as_ref().map(|c| c.retrigger),
                        retry: daemon.retry,
                        retry_count: 0,
                        ready_delay: self.delay.or(daemon.ready_delay).or(Some(3)),
                        ready_output: self.output.clone().or(daemon.ready_output.clone()),
                        wait_ready: true,
                    })
                    .await?;
                if !started.is_empty() {
                    info!("started {}", started.join(", "));
                }
                if let Some(code) = exit_code {
                    any_failed = true;
                    last_exit_code = code;
                    error!("daemon {} failed with exit code {}", id, code);
                    
                    // Print logs from the time we started this specific daemon
                    if let Err(e) = crate::cli::logs::print_logs_for_time_range(
                        id,
                        start_time,
                        None,
                    ) {
                        error!("Failed to print logs: {}", e);
                    }
                }
            } else {
                warn!("Daemon {} not found", id);
            }
        }

        if any_failed {
            if last_exit_code != 0 {
                error!("Process exited with code {}", last_exit_code);
            }
            std::process::exit(last_exit_code);
        }
        Ok(())
    }
}
