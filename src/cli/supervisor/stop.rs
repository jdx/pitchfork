use crate::Result;
use crate::cli::supervisor::KillOrStopOutcome;
use crate::cli::supervisor::resolve_existing_supervisor;
use crate::daemon_id::DaemonId;
use crate::env;
use crate::state_file::StateFile;

/// Stops the internal pitchfork daemon running in the background
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Stop {}

impl Stop {
    pub async fn run(&self) -> Result<()> {
        let (existing_pid, outcome) = resolve_existing_supervisor(true).await?;
        let Some(pid) = existing_pid else {
            warn!("Pitchfork daemon is not running");
            return Ok(());
        };
        match outcome {
            KillOrStopOutcome::Killed => {
                info!("Stopped pitchfork daemon with pid {pid}");
            }
            KillOrStopOutcome::AlreadyDead => {
                // Clean up the stale entry so subsequent commands don't see it.
                if let Ok(mut sf) = StateFile::read(&*env::PITCHFORK_STATE_FILE) {
                    sf.daemons.remove(&DaemonId::pitchfork());
                    let _ = sf.write();
                }
                warn!("Pitchfork daemon with pid {pid} was already dead (cleaned up stale state)");
            }
            KillOrStopOutcome::StillRunning => {
                unreachable!("stop always passes force=true")
            }
        }
        Ok(())
    }
}
