use std::time::Duration;
use log::{info, warn};
use sysinfo::Pid;
use crate::{env, procs};
use crate::Result;
use tokio::time;
use crate::pid_file::PidFile;

#[derive(Debug, clap::Args)]
#[clap(hide = false)]
pub struct Daemon {
    #[clap(short, long)]
    force: bool,
}

impl Daemon {
    pub async fn run(&self) -> Result<()> {
        let pid_file = PidFile::read(&*env::PITCHFORK_PID_FILE)?;
        if let Some(process) = get_process(&system) {
            if self.force {
                sysinfo::Process::kill(process);
            } else {
                let existing_pid = process.pid();
                warn!("Pitchfork is already running with pid {existing_pid}. Kill it with `--force`");
                return Ok(());
            }
        }
        let pid = std::process::id();
        xx::file::write(&*env::PITCHFORK_PID_FILE, pid.to_string())?;

        let mut interval = time::interval(Duration::from_millis(1000));

        loop {
            interval.tick().await;
            info!("Daemon running");
        }
    }

    fn kill_or_stop(&self, existing_pid: u32) -> Result<bool> {
        if let Some(process) = procs::get_process(existing_pid) {

        }
    }
}
