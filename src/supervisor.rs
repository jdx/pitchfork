use std::time::Duration;
use log::info;
use tokio::time;
use crate::pid_file::PidFile;
use crate::{env, Result};

pub struct Supervisor{
    pid_file: PidFile,
}

impl Supervisor {
    pub fn new(pid_file: PidFile) -> Self {
        Self { pid_file }
    }

    pub async fn start(mut self) -> Result<()> {
        let pid = std::process::id();
        self.pid_file.set("pitchfork".to_string(), pid);
        self.pid_file.write(&*env::PITCHFORK_PID_FILE)?;
        println!("Start");

        let mut interval = time::interval(Duration::from_millis(1000));

        loop {
            interval.tick().await;
            info!("Daemon running");
        }
    }
}
