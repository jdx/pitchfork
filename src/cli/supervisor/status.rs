use crate::Result;
use crate::ipc::client::IpcClient;

/// Gets the status of the pitchfork daemon
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Status {}

impl Status {
    pub async fn run(&self) -> Result<()> {
        IpcClient::connect(false).await?;
        println!("Pitchfork daemon is running");
        Ok(())
    }
}
