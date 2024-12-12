use crate::cli::supervisor::kill_or_stop;
use crate::state_file::StateFile;
use crate::Result;
use miette::ensure;

/// Sends a stop signal to a daemon
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "kill", verbatim_doc_comment)]
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
        let pid_file = StateFile::get();
        for id in &self.id {
            if let Some(d) = pid_file.daemons.get(id) {
                if let Some(pid) = d.pid {
                    info!("stopping {} with pid {}", id, pid);
                    kill_or_stop(pid, true).await?;
                    continue;
                }
            }
            warn!("{} is not running", id);
        }
        Ok(())
    }
}
