use crate::cli::supervisor::kill_or_stop;
use crate::env;
use crate::state_file::StateFile;
use crate::supervisor::SUPERVISOR;
use crate::Result;

/// Runs the internal pitchfork daemon in the foreground
#[derive(Debug, clap::Args)]
pub struct Run {
    /// kill existing daemon
    #[clap(short, long)]
    force: bool,
    /// run as boot start (auto-start boot_start daemons)
    #[clap(long)]
    boot: bool,
}

impl Run {
    pub async fn run(&self) -> Result<()> {
        let pid_file = StateFile::read(&*env::PITCHFORK_STATE_FILE)?;
        if let Some(d) = pid_file.daemons.get("pitchfork") {
            if let Some(pid) = d.pid {
                if !(kill_or_stop(pid, self.force).await?) {
                    return Ok(());
                }
            }
        }

        SUPERVISOR.start(self.boot).await
    }
}
