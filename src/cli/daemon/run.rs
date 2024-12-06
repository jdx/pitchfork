use std::time::Duration;
use log::{info, warn};
use crate::{env, procs};
use crate::Result;
use tokio::time;
use crate::pid_file::PidFile;

#[derive(Debug, clap::Args)]
pub struct Run {
    #[clap(short, long)]
    force: bool,
}

impl Run {
    pub async fn run(&self) -> Result<()> {
        let mut pid_file = PidFile::read(&*env::PITCHFORK_PID_FILE)?;
        if let Some(existing_pid) = pid_file.get("pitchfork") {
            if self.kill_or_stop(*existing_pid)? == false {
                return Ok(());
            }
        }
        let pid = std::process::id();
        pid_file.set("pitchfork".to_string(), pid);
        pid_file.write(&*env::PITCHFORK_PID_FILE)?;

        let mut interval = time::interval(Duration::from_millis(1000));

        loop {
            interval.tick().await;
            info!("Daemon running");
        }
    }

    /// if --force is passed, will kill existing process
    /// Returns false if existing pid is running and --force was not passed (so we should cancel starting the daemon)
    fn kill_or_stop(&self, existing_pid: u32) -> Result<bool> {
        if let Some(process) = procs::get_process(existing_pid) {
            if self.force {
                sysinfo::Process::kill(process);
                Ok(true)
            } else {
                let existing_pid = process.pid();
                warn!("Pitchfork is already running with pid {existing_pid}. Kill it with `--force`");
                Ok(false)
            }
        } else {
            Ok(true)
        }
    }
}
