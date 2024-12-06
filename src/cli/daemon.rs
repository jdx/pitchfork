use std::time::Duration;
use log::{info, warn};
use sysinfo::Pid;
use crate::env;
use crate::Result;
use tokio::time;

#[derive(Debug, clap::Args)]
#[clap(hide = false)]
pub struct Daemon {
    #[clap(short, long)]
    force: bool,
}

impl Daemon {
    pub async fn run(&self) -> Result<()> {
        let system = sysinfo::System::new_all();
        if let Some(process) = get_process(&system) {
            if self.force {
                sysinfo::Process::kill(process);
            } else {
                let existing_pid = process.pid();
                warn!("Pitchfork is already running with pid {existing_pid}. Kill it with `--force`");
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
}

fn get_process(system: &sysinfo::System) -> Option<&sysinfo::Process> {
    if let Some(existing_pid) = *env::PITCHFORK_PID {
        if let Some(process) = system.process(Pid::from_u32(existing_pid)) {
            return Some(process);
        }
    }
    None
}
