use crate::daemon::{Daemon, RunOptions};
use crate::daemon_status::DaemonStatus;
use crate::ipc::server::IpcServer;
use crate::ipc::{IpcRequest, IpcResponse};
use crate::procs::PROCS;
use crate::state_file::StateFile;
use crate::{env, Result};
use duct::cmd;
use itertools::Itertools;
use log::LevelFilter::Info;
use miette::IntoDiagnostic;
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
use tokio::sync::oneshot;
use tokio::sync::Mutex;
use tokio::{select, signal, time};

pub struct Supervisor {
    state_file: Mutex<StateFile>,
    pending_notifications: Mutex<Vec<(log::LevelFilter, String)>>,
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
    debug!("starting supervisor in background");
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
            pending_notifications: Mutex::new(vec![]),
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
        self.cron_watch()?;
        self.signals()?;
        // self.file_watch().await?;

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

        self.check_retry().await?;

        Ok(())
    }

    async fn check_retry(&self) -> Result<()> {
        let state_file = self.state_file.lock().await;
        let daemons_to_retry: Vec<(String, Daemon)> = state_file
            .daemons
            .iter()
            .filter(|(_id, d)| {
                // Daemon is errored, not currently running, and has retries remaining
                d.status.is_errored() && d.pid.is_none() && d.retry > 0 && d.retry_count < d.retry
            })
            .map(|(id, d)| (id.clone(), d.clone()))
            .collect();
        drop(state_file);

        for (id, daemon) in daemons_to_retry {
            info!(
                "retrying daemon {} ({}/{} attempts)",
                id,
                daemon.retry_count + 1,
                daemon.retry
            );

            // Get command from pitchfork.toml
            if let Some(run_cmd) = self.get_daemon_run_command(&id) {
                let retry_opts = RunOptions {
                    id: id.clone(),
                    cmd: shell_words::split(&run_cmd).unwrap_or_default(),
                    force: false,
                    shell_pid: daemon.shell_pid,
                    dir: daemon.dir.unwrap_or_else(|| env::CWD.clone()),
                    autostop: daemon.autostop,
                    cron_schedule: daemon.cron_schedule,
                    cron_retrigger: daemon.cron_retrigger,
                    retry: daemon.retry,
                    retry_count: daemon.retry_count + 1,
                    ready_delay: daemon.ready_delay,
                    ready_output: daemon.ready_output.clone(),
                    wait_ready: false,
                };
                if let Err(e) = self.run(retry_opts).await {
                    error!("failed to retry daemon {}: {}", id, e);
                }
            } else {
                warn!("no run command found for daemon {}, cannot retry", id);
                // Mark as exhausted
                self.upsert_daemon(UpsertDaemonOpts {
                    id,
                    retry_count: Some(daemon.retry),
                    ..Default::default()
                })
                .await?;
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
                    self.add_notification(Info, format!("autostopped {daemon}"))
                        .await;
                }
            }
        }
        Ok(())
    }

    async fn run(&self, opts: RunOptions) -> Result<IpcResponse> {
        let id = &opts.id;
        let cmd = opts.cmd.clone();
        let daemon = self.get_daemon(id).await;
        if let Some(daemon) = daemon {
            // Stopping state is treated as "not running" - the monitoring task will clean it up
            // Only check for Running state with a valid PID
            if !daemon.status.is_stopping() && !daemon.status.is_stopped() {
                if let Some(pid) = daemon.pid {
                    if opts.force {
                        self.stop(id).await?;
                        info!("run: stop completed for daemon {id}");
                    } else {
                        warn!("daemon {id} already running with pid {pid}");
                        return Ok(IpcResponse::DaemonAlreadyRunning);
                    }
                }
            }
        }

        // If wait_ready is true and retry is configured, implement retry loop
        if opts.wait_ready && opts.retry > 0 {
            let max_attempts = opts.retry + 1; // initial attempt + retries
            for attempt in 0..max_attempts {
                let mut retry_opts = opts.clone();
                retry_opts.retry_count = attempt;
                retry_opts.cmd = cmd.clone();

                let result = self.run_once(retry_opts).await?;

                match result {
                    IpcResponse::DaemonReady { daemon } => {
                        return Ok(IpcResponse::DaemonReady { daemon });
                    }
                    IpcResponse::DaemonFailedWithCode { exit_code } => {
                        if attempt < opts.retry {
                            let backoff_secs = 2u64.pow(attempt);
                            info!(
                                "daemon {id} failed (attempt {}/{}), retrying in {}s",
                                attempt + 1,
                                max_attempts,
                                backoff_secs
                            );
                            time::sleep(Duration::from_secs(backoff_secs)).await;
                            continue;
                        } else {
                            info!("daemon {id} failed after {} attempts", max_attempts);
                            return Ok(IpcResponse::DaemonFailedWithCode { exit_code });
                        }
                    }
                    other => return Ok(other),
                }
            }
        }

        // No retry or wait_ready is false
        self.run_once(opts).await
    }

    async fn run_once(&self, opts: RunOptions) -> Result<IpcResponse> {
        let id = &opts.id;
        let cmd = opts.cmd;

        // Create channel for readiness notification if wait_ready is true
        let (ready_tx, ready_rx) = if opts.wait_ready {
            let (tx, rx) = oneshot::channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };

        let cmd = once("exec".to_string())
            .chain(cmd.into_iter())
            .collect_vec();
        let args = vec!["-c".to_string(), shell_words::join(&cmd)];
        let log_path = env::PITCHFORK_LOGS_DIR.join(id).join(format!("{id}.log"));
        xx::file::mkdirp(log_path.parent().unwrap())?;
        info!("run: spawning daemon {id} with args: {args:?}");
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .current_dir(&opts.dir);

        // Ensure daemon can find user tools by using the original PATH
        if let Some(ref path) = *env::ORIGINAL_PATH {
            cmd.env("PATH", path);
        }

        let mut child = cmd.spawn().into_diagnostic()?;
        let pid = child.id().unwrap();
        info!("started daemon {id} with pid {pid}");
        let daemon = self
            .upsert_daemon(UpsertDaemonOpts {
                id: id.to_string(),
                pid: Some(pid),
                status: DaemonStatus::Running,
                shell_pid: opts.shell_pid,
                dir: Some(opts.dir.clone()),
                autostop: opts.autostop,
                cron_schedule: opts.cron_schedule.clone(),
                cron_retrigger: opts.cron_retrigger,
                last_exit_success: None,
                retry: Some(opts.retry),
                retry_count: Some(opts.retry_count),
                ready_delay: opts.ready_delay,
                ready_output: opts.ready_output.clone(),
            })
            .await?;

        let id_clone = id.to_string();
        let ready_delay = opts.ready_delay;
        let ready_output = opts.ready_output.clone();

        tokio::spawn(async move {
            let id = id_clone;
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

            // Setup readiness checking
            let mut ready_notified = false;
            let mut ready_tx = ready_tx;
            let ready_pattern =
                ready_output
                    .as_ref()
                    .and_then(|pattern| match regex::Regex::new(pattern) {
                        Ok(re) => Some(re),
                        Err(e) => {
                            error!("invalid regex pattern for daemon {id}: {e}");
                            None
                        }
                    });

            let mut delay_timer =
                ready_delay.map(|secs| Box::pin(time::sleep(Duration::from_secs(secs))));

            // Use a channel to communicate process exit status
            let (exit_tx, mut exit_rx) =
                tokio::sync::mpsc::channel::<std::io::Result<std::process::ExitStatus>>(1);

            // Spawn a task to wait for process exit
            let child_pid = child.id().unwrap();
            tokio::spawn(async move {
                let result = child.wait().await;
                debug!(
                    "daemon pid {child_pid} wait() completed with result: {:?}",
                    result
                );
                let _ = exit_tx.send(result).await;
            });

            let mut exit_status = None;

            loop {
                select! {
                    Ok(Some(line)) = stdout.next_line() => {
                        let formatted = format_line(line.clone());
                        log_appender.write_all(formatted.as_bytes()).await.unwrap();
                        log_appender.flush().await.unwrap();
                        trace!("stdout: {id} {formatted}");

                        // Check if output matches ready pattern
                        if !ready_notified {
                            if let Some(ref pattern) = ready_pattern {
                                if pattern.is_match(&line) {
                                    info!("daemon {id} ready: output matched pattern");
                                    ready_notified = true;
                                    if let Some(tx) = ready_tx.take() {
                                        let _ = tx.send(Ok(()));
                                    }
                                }
                            }
                        }
                    }
                    Ok(Some(line)) = stderr.next_line() => {
                        let formatted = format_line(line.clone());
                        log_appender.write_all(formatted.as_bytes()).await.unwrap();
                        log_appender.flush().await.unwrap();
                        trace!("stderr: {id} {formatted}");

                        // Check if output matches ready pattern (also check stderr)
                        if !ready_notified {
                            if let Some(ref pattern) = ready_pattern {
                                if pattern.is_match(&line) {
                                    info!("daemon {id} ready: output matched pattern");
                                    ready_notified = true;
                                    if let Some(tx) = ready_tx.take() {
                                        let _ = tx.send(Ok(()));
                                    }
                                }
                            }
                        }
                    },
                    Some(result) = exit_rx.recv() => {
                        // Process exited - save exit status and notify if not ready yet
                        exit_status = Some(result);
                        debug!("daemon {id} process exited, exit_status: {:?}", exit_status);
                        if !ready_notified {
                            if let Some(tx) = ready_tx.take() {
                                let exit_code = exit_status.as_ref().and_then(|r| r.as_ref().ok().and_then(|s| s.code()));
                                debug!("daemon {id} not ready yet, sending failure notification with exit_code: {:?}", exit_code);
                                let _ = tx.send(Err(exit_code));
                            }
                        } else {
                            debug!("daemon {id} was already marked ready, not sending failure notification");
                        }
                        break;
                    }
                    _ = async {
                        if let Some(ref mut timer) = delay_timer {
                            timer.await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    } => {
                        if !ready_notified && ready_pattern.is_none() {
                            info!("daemon {id} ready: delay elapsed");
                            ready_notified = true;
                            if let Some(tx) = ready_tx.take() {
                                let _ = tx.send(Ok(()));
                            }
                        }
                        // Disable timer after it fires
                        delay_timer = None;
                    }
                    else => break,
                }
            }

            // Get the final exit status
            let exit_status = if let Some(status) = exit_status {
                status
            } else {
                // Streams closed but process hasn't exited yet, wait for it
                match exit_rx.recv().await {
                    Some(status) => status,
                    None => {
                        warn!("daemon {id} exit channel closed without receiving status");
                        Err(std::io::Error::other("exit channel closed"))
                    }
                }
            };
            let current_daemon = SUPERVISOR.get_daemon(&id).await;

            // Check if this monitoring task is for the current daemon process
            if current_daemon.is_none()
                || current_daemon.as_ref().is_some_and(|d| d.pid != Some(pid))
            {
                // Another process has taken over, don't update status
                return;
            }
            let is_stopping = current_daemon
                .as_ref()
                .is_some_and(|d| d.status.is_stopping());

            if current_daemon.is_some_and(|d| d.status.is_stopped()) {
                // was stopped by this supervisor so don't update status
                return;
            }
            if let Ok(status) = exit_status {
                info!("daemon {id} exited with status {status}");
                if status.success() || is_stopping {
                    // If stopping, always mark as Stopped with success
                    // This allows monitoring task to clear PID after stop() was called
                    SUPERVISOR
                        .upsert_daemon(UpsertDaemonOpts {
                            id: id.clone(),
                            pid: None, // Clear PID now that process has exited
                            status: DaemonStatus::Stopped,
                            last_exit_success: Some(status.success()),
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                } else {
                    // Handle error exit - mark for retry
                    // retry_count increment will be handled by interval_watch
                    SUPERVISOR
                        .upsert_daemon(UpsertDaemonOpts {
                            id: id.clone(),
                            pid: None,
                            status: DaemonStatus::Errored(status.code()),
                            last_exit_success: Some(false),
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                }
            } else {
                SUPERVISOR
                    .upsert_daemon(UpsertDaemonOpts {
                        id: id.clone(),
                        pid: None,
                        status: DaemonStatus::Errored(None),
                        last_exit_success: Some(false),
                        ..Default::default()
                    })
                    .await
                    .unwrap();
            }
        });

        // If wait_ready is true, wait for readiness notification
        if let Some(ready_rx) = ready_rx {
            match ready_rx.await {
                Ok(Ok(())) => {
                    info!("daemon {id} is ready");
                    Ok(IpcResponse::DaemonReady { daemon })
                }
                Ok(Err(exit_code)) => {
                    error!("daemon {id} failed before becoming ready");
                    Ok(IpcResponse::DaemonFailedWithCode { exit_code })
                }
                Err(_) => {
                    error!("readiness channel closed unexpectedly for daemon {id}");
                    Ok(IpcResponse::DaemonStart { daemon })
                }
            }
        } else {
            Ok(IpcResponse::DaemonStart { daemon })
        }
    }

    async fn stop(&self, id: &str) -> Result<IpcResponse> {
        info!("stopping daemon: {id}");
        if let Some(daemon) = self.get_daemon(id).await {
            trace!("daemon to stop: {daemon}");
            if let Some(pid) = daemon.pid {
                trace!("killing pid: {pid}");
                PROCS.refresh_processes();
                if PROCS.is_running(pid) {
                    // First set status to Stopping (keeps PID for monitoring task)
                    self.upsert_daemon(UpsertDaemonOpts {
                        id: id.to_string(),
                        status: DaemonStatus::Stopping,
                        ..Default::default()
                    })
                    .await?;

                    // Then kill the process
                    if let Err(e) = PROCS.kill_async(pid).await {
                        warn!("failed to kill pid {pid}: {e}");
                    }
                    PROCS.refresh_processes();
                    for child_pid in PROCS.all_children(pid) {
                        debug!("killing child pid: {child_pid}");
                        if let Err(e) = PROCS.kill_async(child_pid).await {
                            warn!("failed to kill child pid {child_pid}: {e}");
                        }
                    }
                    // Monitoring task will clear PID and set to Stopped when it detects exit
                } else {
                    debug!("pid {pid} not running");
                    // Process already dead, directly mark as stopped
                    self.upsert_daemon(UpsertDaemonOpts {
                        id: id.to_string(),
                        pid: None,
                        status: DaemonStatus::Stopped,
                        ..Default::default()
                    })
                    .await?;
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

    // async fn file_watch(&self) -> Result<()> {
    //     let state_file = self.state_file.lock().await.path.clone();
    //     task::spawn(async move {
    //         let mut wf = WatchFiles::new(Duration::from_secs(2)).unwrap();
    //
    //         wf.watch(&state_file, RecursiveMode::NonRecursive).unwrap();
    //
    //         while let Some(paths) = wf.rx.recv().await {
    //             if let Err(err) = SUPERVISOR.handle_file_change(paths).await {
    //                 error!("failed to handle file change: {err}");
    //             }
    //         }
    //     });
    //
    //     Ok(())
    // }

    // async fn handle_file_change(&self, paths: Vec<PathBuf>) -> Result<()> {
    //     debug!("file change: {:?}", paths);
    //     // let path = self.state_file.lock().await.path.clone();
    //     // if paths.contains(&path) {
    //     //     *self.state_file.lock().await = StateFile::read(&path)?;
    //     // }
    //     self.refresh().await
    // }

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

    fn cron_watch(&self) -> Result<()> {
        tokio::spawn(async move {
            // Check every minute for cron schedules
            // FIXME: need a better logic, what if the schedule gap is very short (30s, 1min)?
            let mut interval = time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                if let Err(err) = SUPERVISOR.check_cron_schedules().await {
                    error!("failed to check cron schedules: {err}");
                }
            }
        });
        Ok(())
    }

    async fn check_cron_schedules(&self) -> Result<()> {
        use cron::Schedule;
        use std::str::FromStr;

        let now = chrono::Local::now();
        let daemons = self.state_file.lock().await.daemons.clone();

        for (id, daemon) in daemons {
            if let Some(schedule_str) = &daemon.cron_schedule {
                if let Some(retrigger) = daemon.cron_retrigger {
                    // Parse the cron schedule
                    let schedule = match Schedule::from_str(schedule_str) {
                        Ok(s) => s,
                        Err(e) => {
                            warn!("invalid cron schedule for daemon {id}: {e}");
                            continue;
                        }
                    };

                    // Check if we should trigger now
                    let should_trigger = schedule.upcoming(chrono::Local).take(1).any(|next| {
                        // If the next execution is within the next minute, trigger it
                        let diff = next.signed_duration_since(now);
                        diff.num_seconds() < 60 && diff.num_seconds() >= 0
                    });

                    if should_trigger {
                        let should_run = match retrigger {
                            crate::pitchfork_toml::CronRetrigger::Finish => {
                                // Run if not currently running
                                daemon.pid.is_none()
                            }
                            crate::pitchfork_toml::CronRetrigger::Always => {
                                // Always run (force restart handled in run method)
                                true
                            }
                            crate::pitchfork_toml::CronRetrigger::Success => {
                                // Run only if previous command succeeded
                                daemon.pid.is_none() && daemon.last_exit_success.unwrap_or(false)
                            }
                            crate::pitchfork_toml::CronRetrigger::Fail => {
                                // Run only if previous command failed
                                daemon.pid.is_none() && !daemon.last_exit_success.unwrap_or(true)
                            }
                        };

                        if should_run {
                            info!("cron: triggering daemon {id} (retrigger: {retrigger:?})");
                            // Get the run command from pitchfork.toml
                            if let Some(run_cmd) = self.get_daemon_run_command(&id) {
                                let dir = daemon.dir.clone().unwrap_or_else(|| env::CWD.clone());
                                // Use force: true for Always retrigger to ensure restart
                                let force = matches!(
                                    retrigger,
                                    crate::pitchfork_toml::CronRetrigger::Always
                                );
                                let opts = RunOptions {
                                    id: id.clone(),
                                    cmd: shell_words::split(&run_cmd).unwrap_or_default(),
                                    force,
                                    shell_pid: None,
                                    dir,
                                    autostop: daemon.autostop,
                                    cron_schedule: Some(schedule_str.clone()),
                                    cron_retrigger: Some(retrigger),
                                    retry: daemon.retry,
                                    retry_count: daemon.retry_count,
                                    ready_delay: daemon.ready_delay,
                                    ready_output: daemon.ready_output.clone(),
                                    wait_ready: false,
                                };
                                if let Err(e) = self.run(opts).await {
                                    error!("failed to run cron daemon {id}: {e}");
                                }
                            } else {
                                warn!("no run command found for cron daemon {id}");
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn get_daemon_run_command(&self, id: &str) -> Option<String> {
        use crate::pitchfork_toml::PitchforkToml;
        let pt = PitchforkToml::all_merged();
        pt.daemons.get(id).map(|d| d.run.clone())
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
            IpcRequest::GetNotifications => {
                let notifications = self.get_notifications().await;
                IpcResponse::Notifications(notifications)
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

    async fn add_notification(&self, level: log::LevelFilter, message: String) {
        self.pending_notifications
            .lock()
            .await
            .push((level, message));
    }

    async fn get_notifications(&self) -> Vec<(log::LevelFilter, String)> {
        self.pending_notifications.lock().await.drain(..).collect()
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
            cron_schedule: opts
                .cron_schedule
                .or(existing.and_then(|d| d.cron_schedule.clone())),
            cron_retrigger: opts
                .cron_retrigger
                .or(existing.and_then(|d| d.cron_retrigger)),
            last_exit_success: opts
                .last_exit_success
                .or(existing.and_then(|d| d.last_exit_success)),
            retry: opts.retry.unwrap_or(existing.map(|d| d.retry).unwrap_or(0)),
            retry_count: opts
                .retry_count
                .unwrap_or(existing.map(|d| d.retry_count).unwrap_or(0)),
            ready_delay: opts.ready_delay.or(existing.and_then(|d| d.ready_delay)),
            ready_output: opts
                .ready_output
                .or(existing.and_then(|d| d.ready_output.clone())),
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
    cron_schedule: Option<String>,
    cron_retrigger: Option<crate::pitchfork_toml::CronRetrigger>,
    last_exit_success: Option<bool>,
    retry: Option<u32>,
    retry_count: Option<u32>,
    ready_delay: Option<u64>,
    ready_output: Option<String>,
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
            cron_schedule: None,
            cron_retrigger: None,
            last_exit_success: None,
            retry: None,
            retry_count: None,
            ready_delay: None,
            ready_output: None,
        }
    }
}
