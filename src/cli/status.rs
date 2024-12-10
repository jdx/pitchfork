use crate::state_file::StateFile;
use crate::Result;

/// Display the status of a daemon
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "stat", verbatim_doc_comment)]
pub struct Status {
    pub id: String,
}

impl Status {
    pub async fn run(&self) -> Result<()> {
        let daemon = StateFile::get().daemons.get(&self.id);
        if let Some(daemon) = daemon {
            println!("Name: {}", self.id);
            println!("PID: {}", daemon.pid);
            println!("Status: {}", daemon.status);
        } else {
            warn!("Daemon {} not found", self.id);
        }
        Ok(())
    }
}
