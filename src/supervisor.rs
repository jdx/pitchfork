use crate::ipc::server::IpcServer;
use crate::ipc::IpcMessage;
use crate::state_file::{DaemonStatus, StateFile, StateFileDaemon};
use crate::watch_files::WatchFiles;
use crate::{env, Result};
use duct::cmd;
use miette::IntoDiagnostic;
use notify::RecursiveMode;
use std::fs;
use std::path::PathBuf;
use std::process::exit;
use std::sync::atomic;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
#[cfg(unix)]
use tokio::signal::unix::SignalKind;
use tokio::sync::mpsc::{channel, Sender};
use tokio::{signal, task, time};

pub struct Supervisor {
    state_file: StateFile,
    last_run: time::Instant,
    // ipc: IpcServer,
}

const INTERVAL: Duration = Duration::from_secs(10);

enum Event {
    FileChange(Vec<PathBuf>),
    Ipc(IpcMessage, Sender<IpcMessage>),
    Signal,
    Interval,
}

impl Supervisor {
    pub async fn new(pid_file: StateFile) -> Result<Self> {
        Ok(Self {
            state_file: pid_file,
            last_run: time::Instant::now(),
            // ipc: IpcServer::new().await?,
        })
    }

    pub async fn start(mut self) -> Result<()> {
        let pid = std::process::id();
        info!("Starting supervisor with pid {pid}");

        let daemon = StateFileDaemon {
            pid,
            status: DaemonStatus::Running,
        };
        self.state_file.daemons.insert("pitchfork".into(), daemon);
        self.state_file.write()?;

        let (tx, mut rx) = channel(1);
        self.interval_watch(tx.clone())?;
        self.signals(tx.clone())?;
        self.file_watch(tx.clone()).await?;
        self.conn_watch(tx.clone()).await?;
        self.handle(Event::Interval).await?;

        loop {
            let e = rx.recv().await.unwrap();
            if let Err(err) = self.handle(e).await {
                error!("supervisor error: {:?}", err);
            }
        }
    }

    async fn handle(&mut self, event: Event) -> Result<()> {
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
            Event::Ipc(msg, send) => {
                info!("received ipc message: {msg}");
                self.handle_ipc(msg, send).await?;
            }
            Event::Signal => {
                info!("received SIGTERM, stopping");
                self.close();
                exit(0);
            }
        }
        debug!("refreshing");
        self.last_run = time::Instant::now();
        Ok(())
    }

    async fn handle_ipc(&mut self, msg: IpcMessage, send: Sender<IpcMessage>) -> Result<()> {
        match msg {
            IpcMessage::Run(name, cmd) => {
                info!("received run message: {name:?} cmd: {cmd:?}");
                send.send(IpcMessage::Started(name))
                    .await
                    .into_diagnostic()?;
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
    fn signals(&self, tx: Sender<Event>) -> Result<()> {
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

    async fn file_watch(&self, tx: Sender<Event>) -> Result<()> {
        let bin_path = env::BIN_PATH.clone();
        let state_file = self.state_file.path.clone();
        task::spawn(async move {
            let mut wf = WatchFiles::new(Duration::from_secs(2)).unwrap();

            wf.watch(&bin_path, RecursiveMode::NonRecursive).unwrap();
            wf.watch(&state_file, RecursiveMode::NonRecursive).unwrap();

            while let Some(paths) = wf.rx.recv().await {
                tx.send(Event::FileChange(paths)).await.unwrap();
            }
        });

        Ok(())
    }

    fn interval_watch(&self, tx: Sender<Event>) -> Result<()> {
        tokio::spawn(async move {
            let mut interval = time::interval(INTERVAL);
            loop {
                interval.tick().await;
                tx.send(Event::Interval).await.unwrap();
            }
        });
        Ok(())
    }

    async fn conn_watch(&self, tx: Sender<Event>) -> Result<()> {
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
                tx.send(Event::Ipc(msg, send)).await.unwrap();
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
