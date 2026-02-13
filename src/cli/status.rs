use crate::Result;
use crate::pitchfork_toml::PitchforkToml;
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
        // Resolve the daemon ID to a qualified ID
        let qualified_id = PitchforkToml::resolve_id(&self.id)?;

        let daemon = StateFile::get().daemons.get(&qualified_id);
        if let Some(daemon) = daemon {
            // Display short name for cleaner output
            println!("Name: {}", qualified_id.name());
            if let Some(pid) = &daemon.pid {
                println!("PID: {pid}");
            }
            println!("Status: {}", daemon.status.style());
        } else {
            miette::bail!("Daemon {} not found", qualified_id.name());
        }
        Ok(())
    }
}
