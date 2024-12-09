use crate::cli::daemon::kill_or_stop;
use crate::state_file::StateFile;
use crate::{env, Result};
use duct::cmd;
use miette::IntoDiagnostic;

/// Starts the internal pitchfork daemon in the background
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Start {
    /// kill existing daemon
    #[clap(short, long)]
    force: bool,
}

impl Start {
    pub async fn run(&self) -> Result<()> {
        let pid_file = StateFile::read(&*env::PITCHFORK_STATE_FILE)?;
        if let Some(d) = pid_file.daemons.get("pitchfork") {
            if !(kill_or_stop(d.pid, self.force)?) {
                return Ok(());
            }
        }

        cmd!(&*env::BIN_PATH, "daemon", "run")
            .stdout_null()
            .stderr_null()
            .start()
            .into_diagnostic()?;

        Ok(())
    }
}
