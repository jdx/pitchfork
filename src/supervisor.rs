use crate::state_file::{DaemonStatus, StateFile, StateFileDaemon};
use crate::{env, ipc, Result};
use duct::cmd;
use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::{GenericFilePath, ListenerOptions};
use notify_debouncer_mini::{new_debouncer, notify::*, DebounceEventResult, Debouncer};
use std::path::PathBuf;
use std::process::exit;
use std::sync::atomic;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::signal::unix::SignalKind;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::{fs, select, signal, time, try_join};

pub struct Supervisor {
    state_file: StateFile,
    last_run: time::Instant,
}

const INTERVAL: Duration = Duration::from_secs(10);

enum Event {
    FileChange(Vec<PathBuf>),
    Signal,
    Interval,
}

impl Supervisor {
    pub fn new(pid_file: StateFile) -> Self {
        Self {
            state_file: pid_file,
            last_run: time::Instant::now(),
        }
    }

    pub async fn start(mut self) -> Result<()> {
        let pid = std::process::id();
        info!("Starting supervisor with pid {pid}");

        let listener = ipc::server::listen().await?;

        self.state_file.daemons.insert(
            "pitchfork".to_string(),
            StateFileDaemon {
                pid,
                status: DaemonStatus::Running,
            },
        );
        self.state_file.write()?;

        let (tx, mut rx) = channel(1);
        self.interval_watch(tx.clone())?;
        self.signals(tx.clone())?;
        let _file_watcher = self.file_watch(tx.clone())?;
        self.refresh(Event::Interval).await?;

        loop {
            select! {
                e = rx.recv() => {
                    if let Some(e) = e {
                        if let Err(err) = self.refresh(e).await {
                            error!("supervisor error: {:?}", err);
                        }
                    }
                }
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
                debug!("file change: {:?}", paths);
                if paths.contains(&*env::BIN_PATH) {
                    info!("pitchfork cli updated, restarting");
                    self.restart();
                }
                // TODO
                // if paths.contains(&self.pid_file.path) {
                //     self.pid_file = PidFile::read(&self.pid_file.path)?;
                // }
            }
            Event::Signal => {
                info!("received SIGTERM, stopping");
                exit(0);
            }
        }
        debug!("refreshing");
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

    #[cfg(unix)]
    fn signals(&self, tx: Sender<Event>) -> Result<()> {
        let signals = [
            SignalKind::terminate(),
            SignalKind::alarm(),
            SignalKind::interrupt(),
            SignalKind::quit(),
            SignalKind::hangup(),
            SignalKind::pipe(),
            SignalKind::user_defined1(),
            SignalKind::user_defined2(),
        ];
        static RECEIVED_SIGNAL: AtomicBool = AtomicBool::new(false);
        for signal in signals {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut stream = signal::unix::signal(signal).unwrap();
                loop {
                    stream.recv().await;
                    if RECEIVED_SIGNAL.swap(true, atomic::Ordering::SeqCst) {
                        exit(1);
                    } else {
                        tx.send(Event::Signal).await.unwrap();
                    }
                }
            });
        }
        Ok(())
    }

    #[cfg(windows)]
    fn signals(&self) -> Result<()> {
        tokio::spawn(async move {
            let mut stream = signal::ctrl_c().unwrap();
            loop {
                stream.recv().await;
                tx.send(Event::Signal).await.unwrap();
            }
        });
        Ok(())
    }

    fn file_watch(&self, tx: Sender<Event>) -> Result<Debouncer<RecommendedWatcher>> {
        let h = tokio::runtime::Handle::current();
        let mut debouncer =
            new_debouncer(Duration::from_secs(2), move |res: DebounceEventResult| {
                let tx = tx.clone();
                h.spawn(async move {
                    if let Ok(ev) = res {
                        let paths = ev.into_iter().map(|e| e.path).collect();
                        tx.send(Event::FileChange(paths)).await.unwrap();
                    }
                });
            })?;

        debouncer
            .watcher()
            .watch(&env::BIN_PATH, RecursiveMode::NonRecursive)?;
        debouncer
            .watcher()
            .watch(&self.state_file.path, RecursiveMode::NonRecursive)?;

        Ok(debouncer)
    }

    fn interval_watch(&self, tx: Sender<Event>) -> Result<()> {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(INTERVAL);
            loop {
                interval.tick().await;
                tx.send(Event::Interval).await.unwrap();
            }
        });
        Ok(())
    }
}
