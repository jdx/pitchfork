use crate::daemon::{Daemon, RunOptions};
use crate::daemon_status::DaemonStatus;
use crate::ipc::server::IpcServer;
use crate::ipc::{IpcRequest, IpcResponse};
use crate::procs::PROCS;
use crate::state_file::StateFile;
use crate::{Result, env};
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
use tokio::sync::Mutex;
use tokio::sync::oneshot;
use tokio::{select, signal, time};

pub struct Supervisor {
    state_file: Mutex<StateFile>,
    pending_notifications: Mutex<Vec<(log::LevelFilter, String)>>,
    last_refreshed_at: Mutex<time::Instant>,
    /// Map of daemon ID to scheduled autostop time
    pending_autostops: Mutex<HashMap<String, time::Instant>>,
}

fn interval_duration() -> Duration {
    Duration::from_secs(*env::PITCHFORK_INTERVAL_SECS)
}

pub static SUPERVISOR: Lazy<Supervisor> =
    Lazy::new(|| Supervisor::new().expect("Error creating supervisor"));

pub fn start_if_not_running() -> Result<()> {
    let sf = StateFile::get();
    if let Some(d) = sf.daemons.get("pitchfork")
        && let Some(pid) = d.pid
        && PROCS.is_running(pid)
    {
        return Ok(());
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
            pending_autostops: Mutex::new(HashMap::new()),
        })
    }

    pub async fn start(&self, is_boot: bool, web_port: Option<u16>) -> Result<()> {
        let pid = std::process::id();
        info!("Starting supervisor with pid {pid}");

        self.upsert_daemon(UpsertDaemonOpts {
            id: "pitchfork".to_string(),
            pid: Some(pid),
            status: DaemonStatus::Running,
            ..Default::default()
        })
        .await?;

        // If this is a boot start, automatically start boot_start daemons
        if is_boot {
            info!("Boot start mode enabled, starting boot_start daemons");
            self.start_boot_daemons().await?;
        }

        self.interval_watch()?;
        self.cron_watch()?;
        self.signals()?;
        // self.file_watch().await?;

        // Start web server if port is configured
        if let Some(port) = web_port {
            tokio::spawn(async move {
                if let Err(e) = crate::web::serve(port).await {
                    error!("Web server error: {}", e);
                }
            });
        }

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
        self.process_pending_autostops().await?;

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
                    ready_http: daemon.ready_http.clone(),
                    ready_port: daemon.ready_port,
                    wait_ready: false,
                    depends: daemon.depends.clone(),
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
        let delay_secs = *env::PITCHFORK_AUTOSTOP_DELAY;

        for daemon in self.active_daemons().await {
            if !daemon.autostop {
                continue;
            }
            // if this daemon's dir starts with the left dir
            // and no other shell pid has this dir as a prefix
            // schedule the daemon for autostop
            if let Some(daemon_dir) = daemon.dir.as_ref()
                && daemon_dir.starts_with(dir)
                && !shell_dirs.iter().any(|d| d.starts_with(daemon_dir))
            {
                if delay_secs == 0 {
                    // No delay configured, stop immediately
                    info!("autostopping {daemon}");
                    self.stop(&daemon.id).await?;
                    self.add_notification(Info, format!("autostopped {daemon}"))
                        .await;
                } else {
                    // Schedule autostop with delay
                    let stop_at = time::Instant::now() + Duration::from_secs(delay_secs);
                    let mut pending = self.pending_autostops.lock().await;
                    if !pending.contains_key(&daemon.id) {
                        info!("scheduling autostop for {} in {}s", daemon.id, delay_secs);
                        pending.insert(daemon.id.clone(), stop_at);
                    }
                }
            }
        }
        Ok(())
    }

    /// Cancel any pending autostop for daemons in the given directory
    /// Also cancels autostops for daemons in parent directories (e.g., entering /project/subdir
    /// cancels pending autostop for daemon in /project)
    async fn cancel_pending_autostops_for_dir(&self, dir: &Path) {
        let mut pending = self.pending_autostops.lock().await;
        let daemons_to_cancel: Vec<String> = {
            let state_file = self.state_file.lock().await;
            state_file
                .daemons
                .iter()
                .filter(|(_id, d)| {
                    d.dir.as_ref().is_some_and(|daemon_dir| {
                        // Cancel if entering a directory inside or equal to daemon's directory
                        // OR if daemon is in a subdirectory of the entered directory
                        dir.starts_with(daemon_dir) || daemon_dir.starts_with(dir)
                    })
                })
                .map(|(id, _)| id.clone())
                .collect()
        };

        for daemon_id in daemons_to_cancel {
            if pending.remove(&daemon_id).is_some() {
                info!("cancelled pending autostop for {}", daemon_id);
            }
        }
    }

    /// Process any pending autostops that have reached their scheduled time
    async fn process_pending_autostops(&self) -> Result<()> {
        let now = time::Instant::now();
        let to_stop: Vec<String> = {
            let pending = self.pending_autostops.lock().await;
            pending
                .iter()
                .filter(|(_, stop_at)| now >= **stop_at)
                .map(|(id, _)| id.clone())
                .collect()
        };

        for daemon_id in to_stop {
            // Remove from pending first
            {
                let mut pending = self.pending_autostops.lock().await;
                pending.remove(&daemon_id);
            }

            // Check if daemon is still running and should be stopped
            if let Some(daemon) = self.get_daemon(&daemon_id).await
                && daemon.autostop
                && daemon.status.is_running()
            {
                // Verify no shell is in the daemon's directory
                let shell_dirs = self.get_dirs_with_shell_pids().await;
                let shell_dirs = shell_dirs.keys().collect_vec();
                if let Some(daemon_dir) = daemon.dir.as_ref()
                    && !shell_dirs.iter().any(|d| d.starts_with(daemon_dir))
                {
                    info!("autostopping {} (after delay)", daemon_id);
                    self.stop(&daemon_id).await?;
                    self.add_notification(Info, format!("autostopped {daemon_id}"))
                        .await;
                }
            }
        }
        Ok(())
    }

    async fn start_boot_daemons(&self) -> Result<()> {
        use crate::pitchfork_toml::PitchforkToml;

        info!("Scanning for boot_start daemons");
        let pt = PitchforkToml::all_merged();

        let boot_daemons: Vec<_> = pt
            .daemons
            .iter()
            .filter(|(_id, d)| d.boot_start.unwrap_or(false))
            .collect();

        if boot_daemons.is_empty() {
            info!("No daemons configured with boot_start = true");
            return Ok(());
        }

        info!("Found {} daemon(s) to start at boot", boot_daemons.len());

        for (id, daemon) in boot_daemons {
            info!("Starting boot daemon: {}", id);

            let dir = daemon
                .path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| env::CWD.clone());

            let run_opts = RunOptions {
                id: id.clone(),
                cmd: shell_words::split(&daemon.run).unwrap_or_default(),
                force: false,
                shell_pid: None,
                dir,
                autostop: false, // Boot daemons should not autostop
                cron_schedule: daemon.cron.as_ref().map(|c| c.schedule.clone()),
                cron_retrigger: daemon.cron.as_ref().map(|c| c.retrigger),
                retry: daemon.retry,
                retry_count: 0,
                ready_delay: daemon.ready_delay,
                ready_output: daemon.ready_output.clone(),
                ready_http: daemon.ready_http.clone(),
                ready_port: daemon.ready_port,
                wait_ready: false, // Don't block on boot daemons
                depends: daemon.depends.clone(),
            };

            match self.run(run_opts).await {
                Ok(IpcResponse::DaemonStart { .. }) | Ok(IpcResponse::DaemonReady { .. }) => {
                    info!("Successfully started boot daemon: {}", id);
                }
                Ok(IpcResponse::DaemonAlreadyRunning) => {
                    info!("Boot daemon already running: {}", id);
                }
                Ok(other) => {
                    warn!(
                        "Unexpected response when starting boot daemon {}: {:?}",
                        id, other
                    );
                }
                Err(e) => {
                    error!("Failed to start boot daemon {}: {}", id, e);
                }
            }
        }

        Ok(())
    }

    pub async fn run(&self, opts: RunOptions) -> Result<IpcResponse> {
        let id = &opts.id;
        let cmd = opts.cmd.clone();

        // Clear any pending autostop for this daemon since it's being started
        {
            let mut pending = self.pending_autostops.lock().await;
            if pending.remove(id).is_some() {
                info!("cleared pending autostop for {} (daemon starting)", id);
            }
        }

        let daemon = self.get_daemon(id).await;
        if let Some(daemon) = daemon {
            // Stopping state is treated as "not running" - the monitoring task will clean it up
            // Only check for Running state with a valid PID
            if !daemon.status.is_stopping()
                && !daemon.status.is_stopped()
                && let Some(pid) = daemon.pid
            {
                if opts.force {
                    self.stop(id).await?;
                    info!("run: stop completed for daemon {id}");
                } else {
                    warn!("daemon {id} already running with pid {pid}");
                    return Ok(IpcResponse::DaemonAlreadyRunning);
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
        if let Some(parent) = log_path.parent() {
            xx::file::mkdirp(parent)?;
        }
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
        let pid = match child.id() {
            Some(p) => p,
            None => {
                warn!("Daemon {id} exited before PID could be captured");
                return Ok(IpcResponse::DaemonFailed {
                    error: "Process exited immediately".to_string(),
                });
            }
        };
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
                ready_http: opts.ready_http.clone(),
                ready_port: opts.ready_port,
                depends: Some(opts.depends.clone()),
            })
            .await?;

        let id_clone = id.to_string();
        let ready_delay = opts.ready_delay;
        let ready_output = opts.ready_output.clone();
        let ready_http = opts.ready_http.clone();
        let ready_port = opts.ready_port;

        tokio::spawn(async move {
            let id = id_clone;
            let (stdout, stderr) = match (child.stdout.take(), child.stderr.take()) {
                (Some(out), Some(err)) => (out, err),
                _ => {
                    error!("Failed to capture stdout/stderr for daemon {id}");
                    return;
                }
            };
            let mut stdout = tokio::io::BufReader::new(stdout).lines();
            let mut stderr = tokio::io::BufReader::new(stderr).lines();
            let log_file = match tokio::fs::File::options()
                .append(true)
                .create(true)
                .open(&log_path)
                .await
            {
                Ok(f) => f,
                Err(e) => {
                    error!("Failed to open log file for daemon {id}: {e}");
                    return;
                }
            };
            let mut log_appender = BufWriter::new(log_file);

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

            // Setup HTTP readiness check interval (poll every 500ms)
            let mut http_check_interval = ready_http
                .as_ref()
                .map(|_| tokio::time::interval(Duration::from_millis(500)));
            let http_client = ready_http.as_ref().map(|_| {
                reqwest::Client::builder()
                    .timeout(Duration::from_secs(5))
                    .build()
                    .unwrap_or_default()
            });

            // Setup TCP port readiness check interval (poll every 500ms)
            let mut port_check_interval =
                ready_port.map(|_| tokio::time::interval(Duration::from_millis(500)));

            // Use a channel to communicate process exit status
            let (exit_tx, mut exit_rx) =
                tokio::sync::mpsc::channel::<std::io::Result<std::process::ExitStatus>>(1);

            // Spawn a task to wait for process exit
            let child_pid = child.id().unwrap_or(0);
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
                        if let Err(e) = log_appender.write_all(formatted.as_bytes()).await {
                            error!("Failed to write to log for daemon {id}: {e}");
                        }
                        if let Err(e) = log_appender.flush().await {
                            error!("Failed to flush log for daemon {id}: {e}");
                        }
                        trace!("stdout: {id} {formatted}");

                        // Check if output matches ready pattern
                        if !ready_notified
                            && let Some(ref pattern) = ready_pattern
                                && pattern.is_match(&line) {
                                    info!("daemon {id} ready: output matched pattern");
                                    ready_notified = true;
                                    if let Some(tx) = ready_tx.take() {
                                        let _ = tx.send(Ok(()));
                                    }
                                }
                    }
                    Ok(Some(line)) = stderr.next_line() => {
                        let formatted = format_line(line.clone());
                        if let Err(e) = log_appender.write_all(formatted.as_bytes()).await {
                            error!("Failed to write to log for daemon {id}: {e}");
                        }
                        if let Err(e) = log_appender.flush().await {
                            error!("Failed to flush log for daemon {id}: {e}");
                        }
                        trace!("stderr: {id} {formatted}");

                        // Check if output matches ready pattern (also check stderr)
                        if !ready_notified
                            && let Some(ref pattern) = ready_pattern
                                && pattern.is_match(&line) {
                                    info!("daemon {id} ready: output matched pattern");
                                    ready_notified = true;
                                    if let Some(tx) = ready_tx.take() {
                                        let _ = tx.send(Ok(()));
                                    }
                                }
                    },
                    Some(result) = exit_rx.recv() => {
                        // Process exited - save exit status and notify if not ready yet
                        exit_status = Some(result);
                        debug!("daemon {id} process exited, exit_status: {:?}", exit_status);
                        if !ready_notified {
                            if let Some(tx) = ready_tx.take() {
                                // Check if process exited successfully
                                let is_success = exit_status.as_ref()
                                    .and_then(|r| r.as_ref().ok())
                                    .map(|s| s.success())
                                    .unwrap_or(false);

                                if is_success {
                                    debug!("daemon {id} exited successfully before ready check, sending success notification");
                                    let _ = tx.send(Ok(()));
                                } else {
                                    let exit_code = exit_status.as_ref()
                                        .and_then(|r| r.as_ref().ok())
                                        .and_then(|s| s.code());
                                    debug!("daemon {id} exited with failure before ready check, sending failure notification with exit_code: {:?}", exit_code);
                                    let _ = tx.send(Err(exit_code));
                                }
                            }
                        } else {
                            debug!("daemon {id} was already marked ready, not sending notification");
                        }
                        break;
                    }
                    _ = async {
                        if let Some(ref mut interval) = http_check_interval {
                            interval.tick().await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    }, if !ready_notified && ready_http.is_some() => {
                        if let (Some(url), Some(client)) = (&ready_http, &http_client) {
                            match client.get(url).send().await {
                                Ok(response) if response.status().is_success() => {
                                    info!("daemon {id} ready: HTTP check passed (status {})", response.status());
                                    ready_notified = true;
                                    if let Some(tx) = ready_tx.take() {
                                        let _ = tx.send(Ok(()));
                                    }
                                    // Stop checking once ready
                                    http_check_interval = None;
                                }
                                Ok(response) => {
                                    trace!("daemon {id} HTTP check: status {} (not ready)", response.status());
                                }
                                Err(e) => {
                                    trace!("daemon {id} HTTP check failed: {e}");
                                }
                            }
                        }
                    }
                    _ = async {
                        if let Some(ref mut interval) = port_check_interval {
                            interval.tick().await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    }, if !ready_notified && ready_port.is_some() => {
                        if let Some(port) = ready_port {
                            match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                                Ok(_) => {
                                    info!("daemon {id} ready: TCP port {port} is listening");
                                    ready_notified = true;
                                    if let Some(tx) = ready_tx.take() {
                                        let _ = tx.send(Ok(()));
                                    }
                                    // Stop checking once ready
                                    port_check_interval = None;
                                }
                                Err(_) => {
                                    trace!("daemon {id} port check: port {port} not listening yet");
                                }
                            }
                        }
                    }
                    _ = async {
                        if let Some(ref mut timer) = delay_timer {
                            timer.await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    } => {
                        if !ready_notified && ready_pattern.is_none() && ready_http.is_none() && ready_port.is_none() {
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
                    if let Err(e) = SUPERVISOR
                        .upsert_daemon(UpsertDaemonOpts {
                            id: id.clone(),
                            pid: None, // Clear PID now that process has exited
                            status: DaemonStatus::Stopped,
                            last_exit_success: Some(status.success()),
                            ..Default::default()
                        })
                        .await
                    {
                        error!("Failed to update daemon state for {id}: {e}");
                    }
                } else {
                    // Handle error exit - mark for retry
                    // retry_count increment will be handled by interval_watch
                    if let Err(e) = SUPERVISOR
                        .upsert_daemon(UpsertDaemonOpts {
                            id: id.clone(),
                            pid: None,
                            status: DaemonStatus::Errored(status.code()),
                            last_exit_success: Some(false),
                            ..Default::default()
                        })
                        .await
                    {
                        error!("Failed to update daemon state for {id}: {e}");
                    }
                }
            } else if let Err(e) = SUPERVISOR
                .upsert_daemon(UpsertDaemonOpts {
                    id: id.clone(),
                    pid: None,
                    status: DaemonStatus::Errored(None),
                    last_exit_success: Some(false),
                    ..Default::default()
                })
                .await
            {
                error!("Failed to update daemon state for {id}: {e}");
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

    pub async fn stop(&self, id: &str) -> Result<IpcResponse> {
        if id == "pitchfork" {
            return Ok(IpcResponse::Error(
                "Cannot stop supervisor via stop command".into(),
            ));
        }
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
            let stream = match signal::unix::signal(signal) {
                Ok(s) => s,
                Err(e) => {
                    warn!("Failed to register signal handler for {:?}: {}", signal, e);
                    continue;
                }
            };
            tokio::spawn(async move {
                let mut stream = stream;
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
                if let Err(e) = signal::ctrl_c().await {
                    error!("Failed to wait for ctrl-c: {}", e);
                    return;
                }
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
            let mut interval = time::interval(interval_duration());
            loop {
                interval.tick().await;
                if SUPERVISOR.last_refreshed_at.lock().await.elapsed() > interval_duration()
                    && let Err(err) = SUPERVISOR.refresh().await
                {
                    error!("failed to refresh: {err}");
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
            if let Some(schedule_str) = &daemon.cron_schedule
                && let Some(retrigger) = daemon.cron_retrigger
            {
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
                            let force =
                                matches!(retrigger, crate::pitchfork_toml::CronRetrigger::Always);
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
                                ready_http: daemon.ready_http.clone(),
                                ready_port: daemon.ready_port,
                                wait_ready: false,
                                depends: daemon.depends.clone(),
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
                self.set_shell_dir(shell_pid, dir.clone()).await?;
                // Cancel any pending autostops for daemons in the new directory
                self.cancel_pending_autostops_for_dir(&dir).await;
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
            .filter(|d| d.pid.is_some() && d.id != "pitchfork")
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
            ready_http: opts
                .ready_http
                .or(existing.and_then(|d| d.ready_http.clone())),
            ready_port: opts.ready_port.or(existing.and_then(|d| d.ready_port)),
            depends: opts
                .depends
                .unwrap_or_else(|| existing.map(|d| d.depends.clone()).unwrap_or_default()),
        };
        state_file
            .daemons
            .insert(opts.id.to_string(), daemon.clone());
        if let Err(err) = state_file.write() {
            warn!("failed to update state file: {err:#}");
        }
        Ok(daemon)
    }

    pub async fn enable(&self, id: String) -> Result<bool> {
        info!("enabling daemon: {id}");
        let mut state_file = self.state_file.lock().await;
        let result = state_file.disabled.remove(&id);
        state_file.write()?;
        Ok(result)
    }

    pub async fn disable(&self, id: String) -> Result<bool> {
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
                if let Ok(pid) = pid.parse() {
                    acc.entry(dir.clone()).or_default().push(pid);
                }
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
    ready_http: Option<String>,
    ready_port: Option<u16>,
    depends: Option<Vec<String>>,
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
            ready_http: None,
            ready_port: None,
            depends: None,
        }
    }
}
