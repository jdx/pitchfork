use std::path::PathBuf;
use std::time::Duration;
use crate::pid_file::PidFile;
use crate::{async_watcher, env, Result};
use tokio::{select, time};

pub struct Supervisor {
    pid_file: PidFile,
}

const INTERVAL: Duration = Duration::from_secs(10);

impl Supervisor {
    pub fn new(pid_file: PidFile) -> Self {
        Self { pid_file }
    }

    pub async fn start(mut self) -> Result<()> {
        let pid = std::process::id();
        debug!("Starting supervisor with pid {pid}");
        self.pid_file.set("pitchfork".to_string(), pid);
        self.pid_file.write()?;

        let mut interval = time::interval(INTERVAL);

        let (mut file_events, _debouncer) = async_watcher::async_debounce_watch(vec![
            (&*env::BIN_PATH, "nonrecursive"),
            (&self.pid_file.path, "nonrecursive"),
        ]).await?;
        self.refresh(vec![]).await?;

        let mut last_run = time::Instant::now();
        loop {
            select! {
                _ = interval.tick() => {
                    if last_run.elapsed() < INTERVAL {
                        continue;
                    }
                    if let Err(err) = self.refresh(vec![]).await {
                        error!("interval error: {:?}", err);
                    }
                },
                f = file_events.recv() => {
                    match f {
                        Some(Ok(event)) => {
                            let paths = event.into_iter().flat_map(|e| e.event.paths).collect();
                            if let Err(err) = self.refresh(paths).await {
                                error!("watch error: {:?}", err);
                            }
                        }
                        Some(Err(e)) => {
                            error!("watch error: {:?}", e);
                        }
                        None => {
                            error!("watch channel closed");
                        }
                    }
                }
            }
            last_run = time::Instant::now();
        }
    }

    async fn refresh(&mut self, paths: Vec<PathBuf>) -> Result<()> {
        dbg!(paths);
        Ok(())
    }
}
