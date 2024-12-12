use crate::ipc::client::IpcClient;
use crate::Result;

/// Starts the internal pitchfork daemon in the background
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Start {
    /// kill existing daemon
    #[clap(short, long)]
    force: bool,
}

impl Start {
    pub async fn run(&self) -> Result<()> {
        IpcClient::connect(true).await?;
        println!("Pitchfork daemon is running");

        Ok(())
    }
}
