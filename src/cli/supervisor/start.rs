use crate::cli::supervisor::{kill_or_stop, start};
use crate::ipc::client::IpcClient;
use crate::state_file::StateFile;
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
        let sf = StateFile::get();
        let mut running = false;
        if let Some(d) = sf.daemons.get("pitchfork") {
            if let Some(pid) = d.pid {
                if !(kill_or_stop(pid, self.force)?) {
                    running = true;
                }
            }
        }

        if !running {
            start()?;
        }

        IpcClient::connect().await?;
        println!("Pitchfork daemon is running");

        Ok(())
    }
}
