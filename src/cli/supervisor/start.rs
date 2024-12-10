use crate::cli::supervisor::kill_or_stop;
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
        let sf = StateFile::get();
        if let Some(d) = sf.daemons.get("pitchfork") {
            if let Some(pid) = d.pid {
                if !(kill_or_stop(pid, self.force)?) {
                    return Ok(());
                }
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
