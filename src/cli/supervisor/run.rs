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
    /// Web UI port (default: 19876, tries up to 10 ports if in use)
    #[clap(long, env = "PITCHFORK_WEB_PORT", default_value = "19876")]
    web_port: u16,
    /// Disable the web UI
    #[clap(long, env = "PITCHFORK_NO_WEB")]
    no_web: bool,
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

        let web_port = if self.no_web {
            None
        } else {
            Some(self.web_port)
        };
        SUPERVISOR.start(self.boot, web_port).await
    }
}
