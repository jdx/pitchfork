use crate::Result;
use crate::cli::supervisor::{KillOrStopOutcome, resolve_existing_supervisor};
use crate::supervisor::SUPERVISOR;

/// Runs the internal pitchfork daemon in the foreground
#[derive(Debug, clap::Args)]
pub struct Run {
    /// kill existing daemon
    #[clap(short, long)]
    force: bool,
    /// run as boot start (auto-start boot_start daemons)
    #[clap(long)]
    boot: bool,
    /// Enable container/PID1 mode (reap zombies, forward signals)
    #[clap(long, env = "PITCHFORK_CONTAINER")]
    container: bool,
    /// Enable web UI on specified port (tries up to 10 ports if in use)
    #[clap(long, env = "PITCHFORK_WEB_PORT")]
    web_port: Option<u16>,
    /// Serve web UI under a path prefix (e.g. "ps" serves at /ps/)
    #[clap(long, env = "PITCHFORK_WEB_PATH")]
    web_path: Option<String>,
}

impl Run {
    pub async fn run(&self) -> Result<()> {
        let (existing_pid, outcome) = resolve_existing_supervisor(self.force).await?;
        match outcome {
            KillOrStopOutcome::StillRunning => {
                let pid = existing_pid.expect("StillRunning implies a pid exists");
                warn!(
                    "Pitchfork supervisor is already running with pid {pid}. Use `--force` to replace it."
                );
                return Ok(());
            }
            KillOrStopOutcome::Killed => {
                let pid = existing_pid.expect("Killed implies a pid exists");
                info!("Killed existing supervisor with pid {pid}");
            }
            KillOrStopOutcome::AlreadyDead => {}
        }

        SUPERVISOR
            .start(
                self.boot,
                self.container,
                self.web_port,
                self.web_path.clone(),
            )
            .await
    }
}
