use crate::Result;
use crate::cli::supervisor::kill_or_stop;
use crate::state_file::StateFile;

/// Stops the internal pitchfork daemon running in the background
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Stop {}

impl Stop {
    pub async fn run(&self) -> Result<()> {
        let pid_file = StateFile::get();
        if let Some(d) = pid_file.daemons.get("pitchfork")
            && let Some(pid) = d.pid
        {
            info!("Stopping pitchfork daemon with pid {}", pid);
            if kill_or_stop(pid, true).await? {
                return Ok(());
            }
        }
        warn!("Pitchfork daemon is not running");
        Ok(())
    }
}
