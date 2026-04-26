use crate::Result;
use crate::daemon_id::DaemonId;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use miette::ensure;
use std::sync::Arc;

/// Sends a stop signal to a daemon
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "kill",
    verbatim_doc_comment,
    long_about = "\
Sends a stop signal to a daemon

Uses a graceful shutdown strategy:
1. Send SIGTERM and wait up to ~3 seconds for the process to exit (fast 10ms checks initially, then 50ms)
2. If still running, send SIGKILL to force termination

Most processes will exit immediately after the first SIGTERM. The escalation
ensures stubborn processes are eventually terminated while giving well-behaved
processes time to clean up resources.

When using --all/--local/--global, daemons are stopped in reverse dependency order:
dependents are stopped before the daemons they depend on.

Examples:
  pitchfork stop api           Stop a single daemon
  pitchfork stop api worker    Stop multiple daemons
  pitchfork stop --all         Stop all running daemons in dependency order
  pitchfork stop -l            Stop all local daemons in pitchfork.toml
  pitchfork stop -g            Stop all global daemons in config.toml
  pitchfork kill api           Same as 'stop' (alias)"
)]
pub struct Stop {
    /// The name of the daemon(s) to stop
    #[clap(
        conflicts_with = "local",
        conflicts_with = "global",
        conflicts_with = "all"
    )]
    id: Vec<String>,
    /// Stop all running daemons (in reverse dependency order)
    #[clap(long, short, conflicts_with = "local", conflicts_with = "global")]
    all: bool,
    /// Stop all local daemons in pitchfork.toml
    #[clap(
        long,
        short = 'l',
        visible_alias = "all-local",
        conflicts_with = "all",
        conflicts_with = "global"
    )]
    local: bool,
    /// Stop all global daemons in ~/.config/pitchfork/config.toml and /etc/pitchfork/config.toml
    #[clap(
        long,
        short = 'g',
        visible_alias = "all-global",
        conflicts_with = "local",
        conflicts_with = "all"
    )]
    global: bool,
}

impl Stop {
    pub async fn run(&self) -> Result<()> {
        ensure!(
            self.local || self.global || self.all || !self.id.is_empty(),
            "At least one daemon ID or one of --all / --local / --global must be provided"
        );

        let ipc = Arc::new(IpcClient::connect(false).await?);

        let ids: Vec<DaemonId> = if self.all {
            ipc.get_running_daemons().await?
        } else if self.global || self.local {
            ipc.get_running_configured_daemons(self.global).await?
        } else {
            PitchforkToml::resolve_ids(&self.id)?
        };

        if ids.is_empty() {
            warn!("No daemons to stop");
            return Ok(());
        }

        let result = ipc.stop_daemons(&ids).await?;

        if result.any_failed {
            std::process::exit(1);
        }
        Ok(())
    }
}
