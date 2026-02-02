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

Sends SIGTERM to gracefully stop a running daemon. Use 'pitchfork status'
to check if the daemon has stopped.

When using --all, daemons are stopped in reverse dependency order:
dependents are stopped before the daemons they depend on.

Examples:
  pitchfork stop api           Stop a single daemon
  pitchfork stop api worker    Stop multiple daemons
  pitchfork stop --all         Stop all running daemons in dependency order
  pitchfork kill api           Same as 'stop' (alias)"
)]
pub struct Stop {
    /// The name of the daemon(s) to stop
    id: Vec<String>,
    /// Stop all running daemons (in reverse dependency order)
    #[clap(long, short)]
    all: bool,
}

impl Stop {
    pub async fn run(&self) -> Result<()> {
        ensure!(
            !self.all || self.id.is_empty(),
            "--all and daemon IDs cannot be used together"
        );
        ensure!(
            self.all || !self.id.is_empty(),
            "At least one daemon ID must be provided (or use --all)"
        );

        let ipc = Arc::new(IpcClient::connect(false).await?);

        // Compute daemon IDs to stop
        let ids: Vec<DaemonId> = if self.all {
            ipc.get_running_daemons().await?
        } else {
            PitchforkToml::resolve_ids(&self.id)?
        };

        let result = ipc.stop_daemons(&ids).await?;

        if result.any_failed {
            std::process::exit(1);
        }
        Ok(())
    }
}
