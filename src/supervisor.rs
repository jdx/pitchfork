use crate::state_file::{StateFile, StateFileDaemon, StateFileDaemonStatus};
use crate::{async_watcher, env, Result};
use duct::cmd;
use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::{GenericFilePath, GenericNamespaced, ListenerOptions};
use std::io;
use std::path::PathBuf;
use std::process::exit;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};
use tokio::{fs, select, time, try_join};

pub struct Supervisor {
    state_file: StateFile,
    last_run: time::Instant,
}

const INTERVAL: Duration = Duration::from_secs(10);

enum Event {
    FileChange(Vec<PathBuf>),
    Signal(CrossPlatformSignal),
    Interval,
}

enum CrossPlatformSignal {
    Sigterm,
}

impl Supervisor {
    pub fn new(pid_file: StateFile) -> Self {
        Self { state_file: pid_file, last_run: time::Instant::now() }
    }

    pub async fn start(mut self) -> Result<()> {
        let pid = std::process::id();
        info!("Starting supervisor with pid {pid}");

        let _ = fs::remove_file(&*env::IPC_SOCK_PATH).await;
        let opts = ListenerOptions::new().name(env::IPC_SOCK_PATH.clone().to_fs_name::<GenericFilePath>()?);
        let listener = opts.create_tokio()?;
        
        self.state_file.daemons.insert("pitchfork".to_string(), StateFileDaemon { pid, status: StateFileDaemonStatus::Running });
        self.state_file.write()?;

        let mut interval = time::interval(INTERVAL);

        let (mut file_events, _debouncer) = async_watcher::async_debounce_watch(vec![
            (&*env::BIN_PATH, "nonrecursive"),
            (&self.state_file.path, "nonrecursive"),
        ]).await?;

        #[cfg(unix)]
        let mut sigterm = signal(SignalKind::terminate())?;

        self.refresh(Event::Interval).await?;

        loop {
            #[cfg(unix)]
            select! {
                _ = sigterm.recv() => {
                    if let Err(err) = self.refresh(Event::Signal(CrossPlatformSignal::Sigterm)).await {
                        error!("supervisor error: {:?}", err);
                    }
                },
                _ = interval.tick() => {
                    if let Err(err) = self.refresh(Event::Interval).await {
                        error!("supervisor error: {:?}", err);
                    }
                },
                conn = listener.accept() => {
                    let conn = match conn {
                        Ok(c) => c,
                        Err(e) => {
                            error!("failed to accept connection: {:?}", e);
                            continue;
                        }
                    };
                    
                    let mut recv = BufReader::new(&conn);
                    let mut send = &conn;
                    let mut buffer = String::with_capacity(1024);
                    let send = send.write_all(b"Hello, world!\n");
                    let recv = recv.read_line(&mut buffer);
                    try_join!(send, recv)?;
                    
                    println!("Received: {}", buffer.trim());
                }
                f = file_events.recv() => {
                    match f {
                        Some(Ok(event)) => {
                            let paths = event.into_iter().flat_map(|e| e.event.paths).collect();
                            if let Err(err) = self.refresh(Event::FileChange(paths)).await {
                                error!("supervisor error: {:?}", err);
                            }
                        }
                        Some(Err(e)) => {
                            warn!("watch error: {:?}", e);
                        }
                        None => {
                            warn!("watch channel closed");
                        }
                    }
                }
            }
            #[cfg(windows)]
            select! {
                _ = interval.tick() => {
                    if let Err(err) = self.refresh(Event::Interval).await {
                        error!("supervisor error: {:?}", err);
                    }
                },
                f = file_events.recv() => {
                    match f {
                        Some(Ok(event)) => {
                            let paths = event.into_iter().flat_map(|e| e.event.paths).collect();
                            if let Err(err) = self.refresh(Event::FileChange(paths)).await {
                                error!("supervisor error: {:?}", err);
                            }
                        }
                        Some(Err(e)) => {
                            warn!("watch error: {:?}", e);
                        }
                        None => {
                            warn!("watch channel closed");
                        }
                    }
                }
            }
        }
    }

    async fn refresh(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Interval => {
                if self.last_run.elapsed() < INTERVAL {
                    return Ok(());
                }
            }
            Event::FileChange(paths) => {
                if paths.contains(&*env::BIN_PATH) {
                    info!("pitchfork cli updated, restarting");
                    self.restart();
                }
                // TODO
                // if paths.contains(&self.pid_file.path) {
                //     self.pid_file = PidFile::read(&self.pid_file.path)?;
                // }
            }
            Event::Signal(CrossPlatformSignal::Sigterm) => {
                info!("received SIGTERM, stopping");
                exit(0);
            }
        }
        self.last_run = time::Instant::now();
        Ok(())
    }

    fn restart(&mut self) -> ! {
        self.state_file.daemons.remove("pitchfork");
        if let Err(err) = self.state_file.write() {
            warn!("failed to update state file: {:?}", err);
        }
        if !*env::PITCHFORK_EXEC || cfg!(windows) {
            if let Err(err) = cmd!(&*env::BIN_PATH, "daemon", "run", "--force").start() {
                panic!("failed to restart: {err:?}");
            }
        } else {
            let x = exec::execvp(&*env::BIN_PATH, &["daemon", "run", "--force"]);
            panic!("execvp returned unexpectedly: {x:?}");
        }
        exit(0);
    }
}
