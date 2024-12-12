use crate::daemon::{Daemon, RunOptions};
use crate::daemon_status::DaemonStatus;
use crate::ipc::server::IpcServer;
use crate::ipc::{IpcRequest, IpcResponse};
use crate::procs::PROCS;
use crate::state_file::StateFile;
use crate::watch_files::WatchFiles;
use crate::{env, Result};
use duct::cmd;
use itertools::Itertools;
use miette::IntoDiagnostic;
use notify::RecursiveMode;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::fs;
use std::iter::once;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::atomic;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufWriter};
#[cfg(unix)]
use tokio::signal::unix::SignalKind;
use tokio::sync::Mutex;
use tokio::{select, signal, task, time};

pub struct Supervisor {
    state_file: Mutex<StateFile>,
    last_refreshed_at: Mutex<time::Instant>,
}

const INTERVAL: Duration = Duration::from_secs(10);

pub static SUPERVISOR: Lazy<Supervisor> =
    Lazy::new(|| Supervisor::new().expect("Error creating supervisor"));

pub fn start_if_not_running() -> Result<()> {
    let sf = StateFile::get();
    if let Some(d) = sf.daemons.get("pitchfork") {
        if let Some(pid) = d.pid {
            if PROCS.is_running(pid) {
                return Ok(());
            }
        }
    }
    start_in_background()
}

pub fn start_in_background() -> Result<()> {
    cmd!(&*env::PITCHFORK_BIN, "supervisor", "run")
        .stdout_null()
        .stderr_null()
        .start()
        .into_diagnostic()?;
    Ok(())
}

impl Supervisor {
    pub fn new() -> Result<Self> {
        Ok(Self {
            state_file: Mutex::new(StateFile::new(env::PITCHFORK_STATE_FILE.clone())),
            last_refreshed_at: Mutex::new(time::Instant::now()),
        })
    }

    pub async fn start(&self) -> Result<()> {
        let pid = std::process::id();
        info!("Starting supervisor with pid {pid}");

        self.upsert_daemon(UpsertDaemonOpts {
            id: "pitchfork".to_string(),
            pid: Some(pid),
            status: DaemonStatus::Running,
            ..Default::default()
        })
        .await?;

        self.interval_watch()?;
        self.signals()?;
        self.file_watch().await?;

        let ipc = IpcServer::new()?;
        self.conn_watch(ipc).await
    }

    async fn refresh(&self) -> Result<()> {
        trace!("refreshing");
        PROCS.refresh_processes();
        let mut last_refreshed_at = self.last_refreshed_at.lock().await;
        *last_refreshed_at = time::Instant::now();

        for (dir, pids) in self.get_dirs_with_shell_pids().await {
            let to_remove = pids
                .iter()
                .filter(|pid| !PROCS.is_running(**pid))
                .collect_vec();
            for pid in &to_remove {
                self.remove_shell_pid(**pid).await?
            }
            if to_remove.len() == pids.len() {
                self.leave_dir(&dir).await?;
            }
        }
        Ok(())
    }

    async fn leave_dir(&self, dir: &Path) -> Result<()> {
        debug!("left dir {}", dir.display());
        let shell_dirs = self.get_dirs_with_shell_pids().await;
        let shell_dirs = shell_dirs.keys().collect_vec();
        for daemon in self.active_daemons().await {
            if !daemon.autostop {
                continue;
            }
            // if this daemon's dir starts with the left dir
            // and no other shell pid has this dir as a prefix
            // stop the daemon
            if let Some(daemon_dir) = daemon.dir.as_ref() {
                if daemon_dir.starts_with(dir)
                    && !shell_dirs.iter().any(|d| d.starts_with(daemon_dir))
                {
                    info!("autostopping {daemon}");
                    self.stop(&daemon.id).await?;
                }
            }
        }
        Ok(())
    }

    async fn run(&self, opts: RunOptions) -> Result<IpcResponse> {
        let id = &opts.id;
        let cmd = opts.cmd;
        let daemon = self.get_daemon(id).await;
        if let Some(daemon) = daemon {
            if let Some(pid) = daemon.pid {
                if opts.force {
                    self.stop(id).await?;
                } else {
                    warn!("daemon {id} already running with pid {pid}");
                    return Ok(IpcResponse::DaemonAlreadyRunning);
                }
            }
        }
        let cmd = once("exec".to_string())
            .chain(cmd.into_iter())
            .collect_vec();
        let args = vec!["-c".to_string(), cmd.join(" ")];
        let log_path = env::PITCHFORK_LOGS_DIR.join(id).join(format!("{id}.log"));
        xx::file::mkdirp(log_path.parent().unwrap())?;
        debug!("starting daemon: {id} with args: {args:?}");
        let mut child = tokio::process::Command::new("sh")
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .current_dir(&opts.dir)
            .spawn()
            .into_diagnostic()?;
        let pid = child.id().unwrap();
        info!("started daemon {id} with pid {pid}");
        let daemon = self
            .upsert_daemon(UpsertDaemonOpts {
                id: id.to_string(),
                pid: Some(pid),
                status: DaemonStatus::Running,
                shell_pid: opts.shell_pid,
                dir: Some(opts.dir),
                autostop: opts.autostop,
            })
            .await?;
        let id = id.to_string();
        tokio::spawn(async move {
            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();
            let mut stdout = tokio::io::BufReader::new(stdout).lines();
            let mut stderr = tokio::io::BufReader::new(stderr).lines();
            let mut log_appender = BufWriter::new(
                tokio::fs::File::options()
                    .append(true)
                    .create(true)
                    .open(&log_path)
                    .await
                    .unwrap(),
            );
            let now = || chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let format_line = |line: String| {
                if line.starts_with(&format!("{id} ")) {
                    // mise tasks often already have the id printed
                    format!("{} {line}\n", now())
                } else {
                    format!("{} {id} {line}\n", now())
                }
            };
            loop {
                select! {
                    Ok(Some(line)) = stdout.next_line() => {
                        let line = format_line(line);
                        log_appender.write_all(line.as_bytes()).await.unwrap();
                        log_appender.flush().await.unwrap();
                        trace!("stdout: {id} {line}");
                    }
                    Ok(Some(line)) = stderr.next_line() => {
                        let line = format_line(line);
                        log_appender.write_all(line.as_bytes()).await.unwrap();
                        log_appender.flush().await.unwrap();
                        trace!("stderr: {id} {line}");
                    },
                    else => break,
                }
            }
            let exit_status = child.wait().await;
            if SUPERVISOR
                .get_daemon(&id)
                .await
                .is_some_and(|d| d.status.is_stopped())
            {
                // was stopped by this supervisor so don't update status
                return;
            }
            if let Ok(status) = exit_status {
                info!("daemon {id} exited with status {status}");
                if status.success() {
                    SUPERVISOR
                        .upsert_daemon(UpsertDaemonOpts {
                            id: id.clone(),
                            status: DaemonStatus::Stopped,
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                } else {
                    SUPERVISOR
                        .upsert_daemon(UpsertDaemonOpts {
                            id: id.clone(),
                            status: DaemonStatus::Errored(status.code()),
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                }
            } else {
                SUPERVISOR
                    .upsert_daemon(UpsertDaemonOpts {
                        id: id.clone(),
                        status: DaemonStatus::Errored(None),
                        ..Default::default()
                    })
                    .await
                    .unwrap();
            }
        });

        Ok(IpcResponse::DaemonStart { daemon })
    }

    async fn stop(&self, id: &str) -> Result<IpcResponse> {
        info!("stopping daemon: {id}");
        if let Some(daemon) = self.get_daemon(id).await {
            trace!("daemon to stop: {daemon}");
            if let Some(pid) = daemon.pid {
                trace!("killing pid: {pid}");
                PROCS.refresh_processes();
                if PROCS.is_running(pid) {
                    self.upsert_daemon(UpsertDaemonOpts {
                        id: id.to_string(),
                        status: DaemonStatus::Stopped,
                        ..Default::default()
                    })
                    .await?;
                    PROCS.kill_async(pid).await?;
                } else {
                    debug!("pid {pid} not running");
                }
                return Ok(IpcResponse::Ok);
            } else {
                debug!("daemon {id} not running");
            }
        } else {
            debug!("daemon {id} not found");
        }
        Ok(IpcResponse::DaemonAlreadyStopped)
    }

    #[cfg(unix)]
    fn signals(&self) -> Result<()> {
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
            tokio::spawn(async move {
                let mut stream = signal::unix::signal(signal).unwrap();
                loop {
                    stream.recv().await;
                    if RECEIVED_SIGNAL.swap(true, atomic::Ordering::SeqCst) {
                        exit(1);
                    } else {
                        SUPERVISOR.handle_signal().await;
                    }
                }
            });
        }
        Ok(())
    }

    #[cfg(windows)]
    fn signals(&self) -> Result<()> {
        tokio::spawn(async move {
            static RECEIVED_SIGNAL: AtomicBool = AtomicBool::new(false);
            loop {
                signal::ctrl_c().await.unwrap();
                if RECEIVED_SIGNAL.swap(true, atomic::Ordering::SeqCst) {
                    exit(1);
                } else {
                    SUPERVISOR.handle_signal().await;
                }
            }
        });
        Ok(())
    }

    async fn handle_signal(&self) {
        info!("received signal, stopping");
        self.close().await;
        exit(0)
    }

    async fn file_watch(&self) -> Result<()> {
        let state_file = self.state_file.lock().await.path.clone();
        task::spawn(async move {
            let mut wf = WatchFiles::new(Duration::from_secs(2)).unwrap();

            wf.watch(&state_file, RecursiveMode::NonRecursive).unwrap();

            while let Some(paths) = wf.rx.recv().await {
                if let Err(err) = SUPERVISOR.handle_file_change(paths).await {
                    error!("failed to handle file change: {err}");
                }
            }
        });

        Ok(())
    }

    async fn handle_file_change(&self, paths: Vec<PathBuf>) -> Result<()> {
        debug!("file change: {:?}", paths);
        // let path = self.state_file.lock().await.path.clone();
        // if paths.contains(&path) {
        //     *self.state_file.lock().await = StateFile::read(&path)?;
        // }
        self.refresh().await
    }

    fn interval_watch(&self) -> Result<()> {
        tokio::spawn(async move {
            let mut interval = time::interval(INTERVAL);
            loop {
                interval.tick().await;
                if SUPERVISOR.last_refreshed_at.lock().await.elapsed() > INTERVAL {
                    if let Err(err) = SUPERVISOR.refresh().await {
                        error!("failed to refresh: {err}");
                    }
                }
            }
        });
        Ok(())
    }

    async fn conn_watch(&self, mut ipc: IpcServer) -> ! {
        loop {
            let (msg, send) = match ipc.read().await {
                Ok(msg) => msg,
                Err(e) => {
                    error!("failed to accept connection: {:?}", e);
                    continue;
                }
            };
            debug!("received message: {:?}", msg);
            tokio::spawn(async move {
                let rsp = SUPERVISOR
                    .handle_ipc(msg)
                    .await
                    .unwrap_or_else(|err| IpcResponse::Error(err.to_string()));
                if let Err(err) = send.send(rsp).await {
                    debug!("failed to send message: {:?}", err);
                }
            });
        }
    }

    async fn handle_ipc(&self, req: IpcRequest) -> Result<IpcResponse> {
        let rsp = match req {
            IpcRequest::Connect => {
                debug!("received connect message");
                IpcResponse::Ok
            }
            IpcRequest::Stop { id } => self.stop(&id).await?,
            IpcRequest::Run(opts) => self.run(opts).await?,
            IpcRequest::Enable { id } => {
                if self.enable(id).await? {
                    IpcResponse::Yes
                } else {
                    IpcResponse::No
                }
            }
            IpcRequest::Disable { id } => {
                if self.disable(id).await? {
                    IpcResponse::Yes
                } else {
                    IpcResponse::No
                }
            }
            IpcRequest::GetActiveDaemons => {
                let daemons = self.active_daemons().await;
                IpcResponse::ActiveDaemons(daemons)
            }
            IpcRequest::UpdateShellDir { shell_pid, dir } => {
                let prev = self.get_shell_dir(shell_pid).await;
                self.set_shell_dir(shell_pid, dir).await?;
                if let Some(prev) = prev {
                    self.leave_dir(&prev).await?;
                }
                self.refresh().await?;
                IpcResponse::Ok
            }
            IpcRequest::Clean => {
                self.clean().await?;
                IpcResponse::Ok
            }
            IpcRequest::GetDisabledDaemons => {
                let disabled = self.state_file.lock().await.disabled.clone();
                IpcResponse::DisabledDaemons(disabled.into_iter().collect())
            }
        };
        Ok(rsp)
    }

    async fn close(&self) {
        for daemon in self.active_daemons().await {
            if daemon.id == "pitchfork" {
                continue;
            }
            if let Err(err) = self.stop(&daemon.id).await {
                error!("failed to stop daemon {daemon}: {err}");
            }
        }
        let _ = self.remove_daemon("pitchfork").await;
        let _ = fs::remove_dir_all(&*env::IPC_SOCK_DIR);
        // TODO: cleanly stop ipc server
    }

    async fn active_daemons(&self) -> Vec<Daemon> {
        self.state_file
            .lock()
            .await
            .daemons
            .values()
            .filter(|d| d.pid.is_some())
            .cloned()
            .collect()
    }

    async fn remove_daemon(&self, id: &str) -> Result<()> {
        self.state_file.lock().await.daemons.remove(id);
        if let Err(err) = self.state_file.lock().await.write() {
            warn!("failed to update state file: {err:#}");
        }
        Ok(())
    }

    async fn upsert_daemon(&self, opts: UpsertDaemonOpts) -> Result<Daemon> {
        info!(
            "upserting daemon: {} pid: {} status: {}",
            opts.id,
            opts.pid.unwrap_or(0),
            opts.status
        );
        let mut state_file = self.state_file.lock().await;
        let existing = state_file.daemons.get(&opts.id);
        let daemon = Daemon {
            id: opts.id.to_string(),
            title: opts.pid.and_then(|pid| PROCS.title(pid)),
            pid: opts.pid,
            status: opts.status,
            shell_pid: opts.shell_pid,
            autostop: opts.autostop || existing.is_some_and(|d| d.autostop),
            dir: opts.dir.or(existing.and_then(|d| d.dir.clone())),
        };
        state_file
            .daemons
            .insert(opts.id.to_string(), daemon.clone());
        if let Err(err) = state_file.write() {
            warn!("failed to update state file: {err:#}");
        }
        Ok(daemon)
    }

    async fn enable(&self, id: String) -> Result<bool> {
        info!("enabling daemon: {id}");
        let mut state_file = self.state_file.lock().await;
        let result = state_file.disabled.remove(&id);
        state_file.write()?;
        Ok(result)
    }

    async fn disable(&self, id: String) -> Result<bool> {
        info!("disabling daemon: {id}");
        let mut state_file = self.state_file.lock().await;
        let result = state_file.disabled.insert(id);
        state_file.write()?;
        Ok(result)
    }

    async fn get_daemon(&self, id: &str) -> Option<Daemon> {
        self.state_file.lock().await.daemons.get(id).cloned()
    }

    async fn set_shell_dir(&self, shell_pid: u32, dir: PathBuf) -> Result<()> {
        let mut state_file = self.state_file.lock().await;
        state_file.shell_dirs.insert(shell_pid.to_string(), dir);
        state_file.write()?;
        Ok(())
    }

    async fn get_shell_dir(&self, shell_pid: u32) -> Option<PathBuf> {
        self.state_file
            .lock()
            .await
            .shell_dirs
            .get(&shell_pid.to_string())
            .cloned()
    }

    async fn remove_shell_pid(&self, shell_pid: u32) -> Result<()> {
        let mut state_file = self.state_file.lock().await;
        if state_file
            .shell_dirs
            .remove(&shell_pid.to_string())
            .is_some()
        {
            state_file.write()?;
        }
        Ok(())
    }

    async fn get_dirs_with_shell_pids(&self) -> HashMap<PathBuf, Vec<u32>> {
        self.state_file.lock().await.shell_dirs.iter().fold(
            HashMap::new(),
            |mut acc, (pid, dir)| {
                acc.entry(dir.clone())
                    .or_default()
                    .push(pid.parse().unwrap());
                acc
            },
        )
    }

    async fn clean(&self) -> Result<()> {
        let mut state_file = self.state_file.lock().await;
        state_file.daemons.retain(|_id, d| d.pid.is_some());
        state_file.write()?;
        Ok(())
    }
}

#[derive(Debug)]
struct UpsertDaemonOpts {
    id: String,
    pid: Option<u32>,
    status: DaemonStatus,
    shell_pid: Option<u32>,
    dir: Option<PathBuf>,
    autostop: bool,
}

impl Default for UpsertDaemonOpts {
    fn default() -> Self {
        Self {
            id: "".to_string(),
            pid: None,
            status: DaemonStatus::Stopped,
            shell_pid: None,
            dir: None,
            autostop: false,
        }
    }
}
