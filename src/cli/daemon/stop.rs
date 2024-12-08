use crate::cli::daemon::kill_or_stop;
use crate::state_file::StateFile;
use crate::{env, Result};
use duct::cmd;

/// Stops the internal pitchfork daemon running in the background
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Stop {}

impl Stop {
    pub async fn run(&self) -> Result<()> {
        let pid_file = StateFile::read(&*env::PITCHFORK_STATE_FILE)?;
        if let Some(d) = pid_file.daemons.get("pitchfork") {
            info!("Stopping pitchfork daemon with pid {}", d.pid);
            kill_or_stop(d.pid, true)?;
        } else {
            warn!("Pitchfork daemon is not running");
        }
        Ok(())
    }
}
