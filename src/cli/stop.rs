use crate::cli::supervisor::kill_or_stop;
use crate::state_file::StateFile;
use crate::Result;

/// Sends a stop signal to a daemon
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "kill", verbatim_doc_comment)]
pub struct Stop {
    /// The name of the daemon to stop
    id: String,
}

impl Stop {
    pub async fn run(&self) -> Result<()> {
        let pid_file = StateFile::get();
        if let Some(d) = pid_file.daemons.get(&self.id) {
            if let Some(pid) = d.pid {
                info!("stopping {} with pid {}", self.id, pid);
                kill_or_stop(pid, true)?;
                return Ok(());
            }
        }
        warn!("{} is not running", self.id);
        Ok(())
    }
}
