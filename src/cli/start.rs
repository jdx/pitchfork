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
                let cmd = shell_words::split(&daemon.run).into_diagnostic()?;
                let started = ipc
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
                    })
                    .await?;
                if !started.is_empty() {
                    info!("started {}", started.join(", "));
                }
            } else {
                warn!("Daemon {} not found", id);
            }
        }
        Ok(())
    }
}
