use crate::Result;
use crate::ipc::client::IpcClient;
use miette::ensure;

/// Sends a stop signal to a daemon
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "kill",
    verbatim_doc_comment,
    long_about = "\
Sends a stop signal to a daemon

Sends SIGTERM to gracefully stop a running daemon. Use 'pitchfork status'
to check if the daemon has stopped.

Examples:
  pitchfork stop api           Stop a single daemon
  pitchfork stop api worker    Stop multiple daemons
  pitchfork kill api           Same as 'stop' (alias)"
)]
pub struct Stop {
    /// The name of the daemon to stop
    id: Vec<String>,
}

impl Stop {
    pub async fn run(&self) -> Result<()> {
        ensure!(
            !self.id.is_empty(),
            "You must provide at least one daemon to stop"
        );
        let ipc = IpcClient::connect(false).await?;
        for id in &self.id {
            ipc.stop(id.clone()).await?;
        }
        Ok(())
    }
}
