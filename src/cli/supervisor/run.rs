use crate::Result;
use crate::cli::supervisor::kill_or_stop;
use crate::env;
use crate::state_file::StateFile;
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
    /// Enable web UI on specified port (tries up to 10 ports if in use)
    #[clap(long, env = "PITCHFORK_WEB_PORT")]
    web_port: Option<u16>,
    /// Serve web UI under a path prefix (e.g. "ps" serves at /ps/)
    #[clap(long, env = "PITCHFORK_WEB_PATH")]
    web_path: Option<String>,
}

impl Run {
    pub async fn run(&self) -> Result<()> {
        let pid_file = StateFile::read(&*env::PITCHFORK_STATE_FILE)?;
        if let Some(d) = pid_file.daemons.get("pitchfork")
            && let Some(pid) = d.pid
            && !(kill_or_stop(pid, self.force).await?)
        {
            return Ok(());
        }

        SUPERVISOR
            .start(self.boot, self.web_port, self.web_path.clone())
            .await
    }
}
