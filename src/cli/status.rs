use crate::Result;
use crate::state_file::StateFile;

/// Display the status of a daemon
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "stat",
    verbatim_doc_comment,
    long_about = "\
Display the status of a daemon

Shows detailed information about a single daemon including its PID and
current status (running, stopped, failed, etc.).

Example:
  pitchfork status api

Output:
  Name: api
  PID: 12345
  Status: running"
)]
pub struct Status {
    /// Name of the daemon to check
    pub id: String,
}

impl Status {
    pub async fn run(&self) -> Result<()> {
        let daemon = StateFile::get().daemons.get(&self.id);
        if let Some(daemon) = daemon {
            println!("Name: {}", self.id);
            if let Some(pid) = &daemon.pid {
                println!("PID: {pid}");
            }
            println!("Status: {}", daemon.status.style());
        } else {
            warn!("Daemon {} not found", self.id);
        }
        Ok(())
    }
}
