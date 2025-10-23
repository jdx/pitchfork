use crate::daemon::RunOptions;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use crate::Result;
use miette::ensure;
use std::sync::Arc;

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
        let ipc = Arc::new(IpcClient::connect(true).await?);
        let disabled_daemons = ipc.get_disabled_daemons().await?;
        let ids = if self.all {
            pt.daemons.keys().cloned().collect()
        } else {
            self.id.clone()
        };
        // launch all tasks concurrently
        let mut tasks = Vec::new();

        for id in ids {
            if disabled_daemons.contains(&id) {
                warn!("Daemon {} is disabled", id);
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
                    let retry = d.retry;
                    let ready_delay = d.ready_delay;
                    let ready_output = d.ready_output.clone();

                    (
                        run,
                        auto_stop,
                        dir,
                        cron_schedule,
                        cron_retrigger,
                        retry,
                        ready_delay,
                        ready_output,
                    )
                }
                None => {
                    warn!("Daemon {} not found", id);
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
            ) = daemon_data;

            let ipc_clone = ipc.clone();
            let shell_pid = self.shell_pid;
            let force = self.force;
            let delay = self.delay;
            let output = self.output.clone();

            let task = tokio::spawn(async move {
                let cmd = match shell_words::split(&run) {
                    Ok(c) => c,
                    Err(e) => {
                        error!("Failed to parse command for daemon {}: {}", id, e);
                        return Some(1);
                    }
                };

                match ipc_clone
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
                        wait_ready: true,
                    })
                    .await
                {
                    Ok((_started, exit_code)) => exit_code,
                    Err(e) => {
                        error!("Failed to start daemon {}: {}", id, e);
                        Some(1)
                    }
                }
            });

            tasks.push(task);
        }

        // wait for all tasks to complete
        let mut any_failed = false;

        for task in tasks {
            match task.await {
                Ok(exit_code) => {
                    if exit_code.is_some() {
                        any_failed = true;
                    }
                }
                Err(e) => {
                    error!("Task panicked: {}", e);
                    any_failed = true;
                }
            }
        }

        if any_failed {
            std::process::exit(1);
        }
        Ok(())
    }
}
