use crate::ipc::server::IpcServer;
use crate::ipc::IpcMessage;
use crate::state_file::{DaemonStatus, StateFile, StateFileDaemon};
use crate::watch_files::WatchFiles;
use crate::{env, Result};
use duct::cmd;
use notify::RecursiveMode;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::exit;
use std::sync::atomic;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
#[cfg(unix)]
use tokio::signal::unix::SignalKind;
use tokio::sync::broadcast;
use tokio::sync::mpsc::Sender;
use tokio::{signal, task, time};

pub struct Supervisor {
    state_file: StateFile,
    last_refreshed_at: time::Instant,
    active_pids: HashMap<u32, String>,
    event_tx: broadcast::Sender<Event>,
    event_rx: broadcast::Receiver<Event>,
    // ipc: IpcServer,
}

const INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
enum Event {
    FileChange(Vec<PathBuf>),
    Ipc(IpcMessage, Sender<IpcMessage>),
    Signal,
    Interval,
    DaemonStart(StateFileDaemon),
    DaemonFailed { name: String, error: String },
}

impl Supervisor {
    pub async fn new(state_file: StateFile) -> Result<Self> {
        let (event_tx, event_rx) = broadcast::channel(1);
        Ok(Self {
            state_file,
            last_refreshed_at: time::Instant::now(),
            active_pids: Default::default(),
            event_tx,
            event_rx,
            // ipc: IpcServer::new().await?,
        })
    }

    pub async fn start(mut self) -> Result<()> {
        let pid = std::process::id();
        info!("Starting supervisor with pid {pid}");

        let daemon = StateFileDaemon {
            name: "pitchfork".into(),
            pid,
            status: DaemonStatus::Running,
        };
        self.state_file.daemons.insert("pitchfork".into(), daemon);
        self.state_file.write()?;

        self.interval_watch()?;
        self.signals()?;
        self.file_watch()?;
        self.conn_watch().await?;

        loop {
            let e = self.event_rx.recv().await.unwrap();
            if let Err(err) = self.handle(e).await {
                error!("supervisor error: {:?}", err);
            }
        }
    }

    async fn handle(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Interval => {
                if self.last_refreshed_at.elapsed() < INTERVAL {
                    return Ok(());
                }
                self.refresh().await
            }
            Event::FileChange(paths) => {
                debug!("file change: {:?}", paths);
                if paths.contains(&*env::BIN_PATH) {
                    info!("pitchfork cli updated, restarting");
                    self.restart();
                }
                if paths.contains(&self.state_file.path) {
                    self.state_file = StateFile::read(&self.state_file.path)?;
                }
                self.refresh().await
            }
            Event::Ipc(msg, send) => {
                info!("received ipc message: {msg}");
                self.handle_ipc(msg, send).await
            }
            Event::Signal => {
                info!("received SIGTERM, stopping");
                self.close();
                exit(0)
            }
            _ => Ok(()),
        }
    }

    async fn refresh(&mut self) -> Result<()> {
        debug!("refreshing");
        self.last_refreshed_at = time::Instant::now();
        Ok(())
    }

    async fn handle_ipc(&mut self, msg: IpcMessage, send: Sender<IpcMessage>) -> Result<()> {
        let mut event_rx = self.event_tx.subscribe();
        match msg {
            IpcMessage::Run(name, cmd) => {
                info!("received run message: {name:?} cmd: {cmd:?}");
                task::spawn({
                    let name = name.clone();
                    async move {
                        while let Ok(ev) = event_rx.recv().await {
                            match ev {
                                Event::DaemonStart(daemon) => {
                                    if daemon.name == name {
                                        if let Err(err) =
                                            send.send(IpcMessage::DaemonStart(daemon)).await
                                        {
                                            error!("failed to send message: {err:?}");
                                        }
                                        return;
                                    }
                                }
                                Event::DaemonFailed { name: n, error } => {
                                    if n == name {
                                        if let Err(err) = send
                                            .send(IpcMessage::DaemonFailed { name, error })
                                            .await
                                        {
                                            error!("failed to send message: {err:?}");
                                        }
                                        return;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                });
                let program = cmd[0].clone();
                let args = cmd[1..].to_vec();
                let log_path = env::PITCHFORK_LOGS_DIR
                    .join(&name)
                    .join(format!("{name}.log"));
                match duct::cmd(&program, &args)
                    .stderr_to_stdout()
                    .stdout_path(log_path)
                    .reader()
                {
                    Ok(child) => {
                        let pid = *child.pids().first().unwrap();
                        info!("started daemon {name} with pid {pid}");
                        let daemon = StateFileDaemon {
                            name: name.clone(),
                            pid,
                            status: DaemonStatus::Running,
                        };
                        self.state_file.daemons.insert(name, daemon.clone());
                        self.state_file.write()?;
                        self.event_tx.send(Event::DaemonStart(daemon)).unwrap();
                    }
                    Err(err) => {
                        info!("failed to start daemon: {err:?}");
                        self.event_tx
                            .send(Event::DaemonFailed {
                                name,
                                error: format!("{err:?}"),
                            })
                            .unwrap();
                    }
                }
            }
            _ => {
                debug!("received unknown message: {msg}");
            }
        }
        Ok(())
    }

    fn restart(&mut self) -> ! {
        debug!("restarting");
        self.close();
        if *env::PITCHFORK_EXEC || cfg!(windows) {
            if let Err(err) = cmd!(&*env::BIN_PATH, "supervisor", "run", "--force").start() {
                panic!("failed to restart: {err:?}");
            }
        } else {
            let x = exec::execvp(
                &*env::BIN_PATH,
                &["pitchfork", "supervisor", "run", "--force"],
            );
            panic!("execvp returned unexpectedly: {x:?}");
        }
        exit(0);
    }

    #[cfg(unix)]
    fn signals(&self) -> Result<()> {
        let tx = self.event_tx.clone();
        let signals = [
            SignalKind::terminate(),
            SignalKind::alarm(),
            SignalKind::interrupt(),
            SignalKind::quit(),
            SignalKind::hangup(),
            SignalKind::user_defined1(),
            SignalKind::user_defined2(),
        ];
        static RECEIVED_SIGNAL: AtomicBool = AtomicBool::new(false);
        let mut pipe_stream = signal::unix::signal(SignalKind::pipe()).unwrap();
        tokio::spawn(async move {
            pipe_stream.recv().await;
            debug!("received SIGPIPE");
        });
        for signal in signals {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut stream = signal::unix::signal(signal).unwrap();
                loop {
                    stream.recv().await;
                    if RECEIVED_SIGNAL.swap(true, atomic::Ordering::SeqCst) {
                        exit(1);
                    } else {
                        tx.send(Event::Signal).unwrap();
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

    fn file_watch(&self) -> Result<()> {
        let bin_path = env::BIN_PATH.clone();
        let state_file = self.state_file.path.clone();
        let tx = self.event_tx.clone();
        task::spawn(async move {
            let mut wf = WatchFiles::new(Duration::from_secs(2)).unwrap();

            wf.watch(&bin_path, RecursiveMode::NonRecursive).unwrap();
            wf.watch(&state_file, RecursiveMode::NonRecursive).unwrap();

            while let Some(paths) = wf.rx.recv().await {
                tx.send(Event::FileChange(paths)).unwrap();
            }
        });

        Ok(())
    }

    fn interval_watch(&self) -> Result<()> {
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(INTERVAL);
            loop {
                interval.tick().await;
                event_tx.send(Event::Interval).unwrap();
            }
        });
        Ok(())
    }

    async fn conn_watch(&self) -> Result<()> {
        let tx = self.event_tx.clone();
        // TODO: reuse self.ipc
        let mut ipc = IpcServer::new().await?;
        tokio::spawn(async move {
            loop {
                let (msg, send) = match ipc.read().await {
                    Ok(msg) => msg,
                    Err(e) => {
                        error!("failed to accept connection: {:?}", e);
                        continue;
                    }
                };
                debug!("received message: {:?}", msg);
                tx.send(Event::Ipc(msg, send)).unwrap();
            }
        });
        Ok(())
    }

    fn close(&mut self) {
        self.state_file.daemons.remove("pitchfork");
        if let Err(err) = self.state_file.write() {
            warn!("failed to update state file: {:?}", err);
        }
        let _ = fs::remove_dir_all(&*env::IPC_SOCK_DIR);
        // TODO: move this to self.ipc
        // self.ipc.close();
    }
}
