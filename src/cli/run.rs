use crate::{ipc, Result};
use eyre::bail;
use tokio::io::AsyncWriteExt;
use crate::ipc::client::IpcClient;

/// Runs a one-off daemon
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Run {
    /// Name of the daemon to run
    name: String,
    #[clap(trailing_var_arg = true)]
    cmd: Vec<String>,
    #[clap(short, long)]
    force: bool,
}

impl Run {
    pub async fn run(&self) -> Result<()> {
        info!("Running one-off daemon");
        if self.cmd.is_empty() {
            bail!("No command provided");
        }
        dbg!(&self);

        let _ipc = IpcClient::connect().await?;
        // ipc.send.write_all(b"Hello from client!\n").await?;
        Ok(())
    }
}
