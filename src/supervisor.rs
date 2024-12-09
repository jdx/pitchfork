use crate::ipc::server::IpcServer;
use crate::ipc::IpcMessage;
use crate::procs::Procs;
use crate::state_file::{DaemonStatus, StateFile, StateFileDaemon};
use crate::watch_files::WatchFiles;
use crate::{env, Result};
use duct::cmd;
use itertools::Itertools;
use miette::IntoDiagnostic;
use notify::RecursiveMode;
use std::collections::HashMap;
use std::fs;
use std::iter::once;
use std::path::PathBuf;
use std::process::exit;
use std::sync::atomic;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::signal::unix::SignalKind;
use tokio::sync::broadcast;
use tokio::sync::mpsc::Sender;
use tokio::{select, signal, task, time};

pub struct Supervisor {
    state_file: StateFile,
    last_refreshed_at: time::Instant,
    active_pids: HashMap<u32, String>,
    event_tx: broadcast::Sender<Event>,
    event_rx: broadcast::Receiver<Event>,
    pitchfork_bin_file_size: u64,
    procs: Procs,
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
    DaemonStop(StateFileDaemon),
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
            procs: Procs::new(),
            pitchfork_bin_file_size: fs::metadata(&*env::BIN_PATH).into_diagnostic()?.len(),
            // ipc: IpcServer::new().await?,
        })
    }

    pub async fn start(mut self) -> Result<()> {
        let pid = std::process::id();
        info!("Starting supervisor with pid {pid}");

        let daemon = StateFileDaemon {
            name: "pitchfork".into(),
            pid: Some(pid),
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
                if paths.contains(&*env::BIN_PATH) && env::BIN_PATH.exists() {
                    let new_size = fs::metadata(&*env::BIN_PATH).into_diagnostic()?.len();
                    if new_size != self.pitchfork_bin_file_size {
                        info!("pitchfork cli updated, restarting");
                        self.restart();
                    }
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
                info!("received signal, stopping");
                self.close();
                exit(0)
            }
            Event::DaemonStop(daemon) => {
                self.active_pids.remove(&daemon.pid.unwrap());
                self.state_file
                    .daemons
                    .entry(daemon.name.clone())
                    .and_modify(|d| {
                        d.pid = None;
                        d.status = DaemonStatus::Stopped;
                    });
                self.state_file.write()?;
                Ok(())
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
        match msg {
            IpcMessage::Run(name, cmd) => {
                self.run(send, &name, cmd).await?;
            }
            IpcMessage::Stop(name) => {
                self.stop(send, &name).await?;
            }
            _ => {
                debug!("received unknown message: {msg}");
            }
        }
        Ok(())
    }

    async fn run(&mut self, send: Sender<IpcMessage>, name: &str, cmd: Vec<String>) -> Result<()> {
        let tx = self.event_tx.clone();
        let mut event_rx = self.event_tx.subscribe();
        info!("received run message: {name:?} cmd: {cmd:?}");
        if let Some(daemon) = self.state_file.daemons.get(name) {
            if let Some(pid) = daemon.pid {
                warn!("daemon {name} already running with pid {}", pid);
                if let Err(err) = send
                    .send(IpcMessage::DaemonAlreadyRunning(name.to_string()))
                    .await
                {
                    warn!("failed to send message: {err:?}");
                }
                return Ok(());
            }
        }
        task::spawn({
            let name = name.to_string();
            async move {
                while let Ok(ev) = event_rx.recv().await {
                    match ev {
                        Event::DaemonStart(daemon) => {
                            if daemon.name == name {
                                if let Err(err) = send.send(IpcMessage::DaemonStart(daemon)).await {
                                    error!("failed to send message: {err:?}");
                                }
                                return;
                            }
                        }
                        Event::DaemonFailed { name: n, error } => {
                            if n == name {
                                if let Err(err) =
                                    send.send(IpcMessage::DaemonFailed { name, error }).await
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
        let cmd = once("exec".to_string())
            .chain(cmd.into_iter())
            .collect_vec();
        let args = vec!["-c".to_string(), cmd.join(" ")];
        let log_path = env::PITCHFORK_LOGS_DIR
            .join(name)
            .join(format!("{name}.log"));
        xx::file::mkdirp(log_path.parent().unwrap())?;
        debug!("starting daemon: {name} with args: {args:?}");
        match tokio::process::Command::new("sh")
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                let pid = child.id().unwrap();
                self.active_pids.insert(pid, name.to_string());
                info!("started daemon {name} with pid {pid}");
                let daemon = StateFileDaemon {
                    name: name.to_string(),
                    pid: Some(pid),
                    status: DaemonStatus::Running,
                };
                self.state_file
                    .daemons
                    .insert(name.to_string(), daemon.clone());
                self.state_file.write()?;
                tx.send(Event::DaemonStart(daemon.clone())).unwrap();
                let name = name.to_string();
                tokio::spawn(async move {
                    let stdout = child.stdout.take().unwrap();
                    let stderr = child.stderr.take().unwrap();
                    let mut stdout = tokio::io::BufReader::new(stdout).lines();
                    let mut stderr = tokio::io::BufReader::new(stderr).lines();
                    let mut log_appender = tokio::fs::File::options()
                        .append(true)
                        .create(true)
                        .open(&log_path)
                        .await
                        .unwrap();
                    dbg!(&log_path);
                    loop {
                        select! {
                            Ok(Some(line)) = stdout.next_line() => {
                                debug!("stdout: {name} {line}");
                                log_appender.write_all(line.as_bytes()).await.unwrap();
                                log_appender.write_all(b"\n").await.unwrap();
                            }
                            Ok(Some(line)) = stderr.next_line() => {
                                debug!("stderr: {name} {line}");
                                log_appender.write_all(line.as_bytes()).await.unwrap();
                                log_appender.write_all(b"\n").await.unwrap();
                            },
                            else => break,
                        }
                    }
                    let status = child.wait().await.unwrap();
                    info!("daemon {name} exited with status {status}");
                    tx.send(Event::DaemonStop(daemon)).unwrap();
                });
            }
            Err(err) => {
                info!("failed to start daemon: {err:?}");
                self.event_tx
                    .send(Event::DaemonFailed {
                        name: name.to_string(),
                        error: format!("{err:?}"),
                    })
                    .unwrap();
            }
        }

        Ok(())
    }

    async fn stop(&mut self, send: Sender<IpcMessage>, name: &str) -> Result<()> {
        info!("received stop message: {name}");
        if let Some(daemon) = self.state_file.daemons.get(name) {
            if let Some(pid) = daemon.pid {
                self.procs.refresh_processes();
                if let Some(proc) = self.procs.get_process(pid) {
                    proc.kill();
                    proc.wait(); // TODO: no blocking
                }
                self.active_pids.remove(&pid);
                self.state_file
                    .daemons
                    .entry(name.to_string())
                    .and_modify(|d| {
                        d.pid = None;
                        d.status = DaemonStatus::Stopped;
                    });
                self.state_file.write()?;
                if let Err(err) = send
                    .send(IpcMessage::DaemonStop {
                        name: name.to_string(),
                    })
                    .await
                {
                    warn!("failed to send message: {err:?}");
                }
                return Ok(());
            }
        }
        if let Err(err) = send
            .send(IpcMessage::DaemonAlreadyStopped(name.to_string()))
            .await
        {
            warn!("failed to send message: {err:?}");
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
