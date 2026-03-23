use crate::Result;
use crate::ipc::client::IpcClient;

/// Gets the status of the pitchfork daemon
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Status {}

impl Status {
    pub async fn run(&self) -> Result<()> {
        IpcClient::connect(false).await?;
        // NOTE: info! routes to stderr (via eprintln! in Logger::log), not stdout.
        // Use println! for user-facing messages that should appear on stdout.
        println!("Pitchfork daemon is running");
        Ok(())
    }
}
