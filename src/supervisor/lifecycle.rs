//! Daemon lifecycle management - start/stop operations
//!
//! Contains the core `run()`, `run_once()`, and `stop()` methods for daemon process management.

use super::{SUPERVISOR, Supervisor, UpsertDaemonOpts};
use crate::daemon::RunOptions;
use crate::daemon_status::DaemonStatus;
use crate::ipc::IpcResponse;
use crate::procs::PROCS;
use crate::shell::Shell;
use crate::{Result, env};
use itertools::Itertools;
use miette::IntoDiagnostic;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::iter::once;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufWriter};
use tokio::select;
use tokio::sync::oneshot;
use tokio::time;

/// Cache for compiled regex patterns to avoid recompilation on daemon restarts
static REGEX_CACHE: Lazy<std::sync::Mutex<HashMap<String, Regex>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

/// Get or compile a regex pattern, caching the result for future use
pub(crate) fn get_or_compile_regex(pattern: &str) -> Option<Regex> {
    let mut cache = REGEX_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(re) = cache.get(pattern) {
        return Some(re.clone());
    }
    match Regex::new(pattern) {
        Ok(re) => {
            cache.insert(pattern.to_string(), re.clone());
            Some(re)
        }
        Err(e) => {
            error!("invalid regex pattern '{pattern}': {e}");
            None
        }
    }
}

impl Supervisor {
    /// Run a daemon, handling retries if configured
    pub async fn run(&self, opts: RunOptions) -> Result<IpcResponse> {
        let id = &opts.id;
        let cmd = opts.cmd.clone();

        // Clear any pending autostop for this daemon since it's being started
        {
            let mut pending = self.pending_autostops.lock().await;
            if pending.remove(id).is_some() {
                info!("cleared pending autostop for {id} (daemon starting)");
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
            // Use saturating_add to avoid overflow when retry = u32::MAX (infinite)
            let max_attempts = opts.retry.saturating_add(1);
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
                            info!("daemon {id} failed after {max_attempts} attempts");
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

    /// Run a daemon once (single attempt)
    pub(crate) async fn run_once(&self, opts: RunOptions) -> Result<IpcResponse> {
        let id = &opts.id;
        let original_cmd = opts.cmd.clone(); // Save original command for persistence
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
                cmd: Some(original_cmd),
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
                ready_cmd: opts.ready_cmd.clone(),
                depends: Some(opts.depends.clone()),
            })
            .await?;

        let id_clone = id.to_string();
        let ready_delay = opts.ready_delay;
        let ready_output = opts.ready_output.clone();
        let ready_http = opts.ready_http.clone();
        let ready_port = opts.ready_port;
        let ready_cmd = opts.ready_cmd.clone();

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
            let ready_pattern = ready_output.as_ref().and_then(|p| get_or_compile_regex(p));

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

            // Setup command readiness check interval (poll every 500ms)
            let mut cmd_check_interval = ready_cmd
                .as_ref()
                .map(|_| tokio::time::interval(Duration::from_millis(500)));

            // Setup periodic log flush interval (every 500ms - balances I/O reduction with responsiveness)
            let mut log_flush_interval = tokio::time::interval(Duration::from_millis(500));

            // Use a channel to communicate process exit status
            let (exit_tx, mut exit_rx) =
                tokio::sync::mpsc::channel::<std::io::Result<std::process::ExitStatus>>(1);

            // Spawn a task to wait for process exit
            let child_pid = child.id().unwrap_or(0);
            tokio::spawn(async move {
                let result = child.wait().await;
                debug!("daemon pid {child_pid} wait() completed with result: {result:?}");
                let _ = exit_tx.send(result).await;
            });

            #[allow(unused_assignments)]
            // Initial None is a safety net; loop only exits via exit_rx.recv() which sets it
            let mut exit_status = None;

            loop {
                select! {
                    Ok(Some(line)) = stdout.next_line() => {
                        let formatted = format_line(line.clone());
                        if let Err(e) = log_appender.write_all(formatted.as_bytes()).await {
                            error!("Failed to write to log for daemon {id}: {e}");
                        }
                        trace!("stdout: {id} {formatted}");

                        // Check if output matches ready pattern
                        if !ready_notified
                            && let Some(ref pattern) = ready_pattern
                                && pattern.is_match(&line) {
                                    info!("daemon {id} ready: output matched pattern");
                                    ready_notified = true;
                                    // Flush logs before notifying so clients see logs immediately
                                    let _ = log_appender.flush().await;
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
                        trace!("stderr: {id} {formatted}");

                        // Check if output matches ready pattern (also check stderr)
                        if !ready_notified
                            && let Some(ref pattern) = ready_pattern
                                && pattern.is_match(&line) {
                                    info!("daemon {id} ready: output matched pattern");
                                    ready_notified = true;
                                    // Flush logs before notifying so clients see logs immediately
                                    let _ = log_appender.flush().await;
                                    if let Some(tx) = ready_tx.take() {
                                        let _ = tx.send(Ok(()));
                                    }
                                }
                    },
                    Some(result) = exit_rx.recv() => {
                        // Process exited - save exit status and notify if not ready yet
                        exit_status = Some(result);
                        debug!("daemon {id} process exited, exit_status: {exit_status:?}");
                        // Flush logs before notifying so clients see logs immediately
                        let _ = log_appender.flush().await;
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
                                    debug!("daemon {id} exited with failure before ready check, sending failure notification with exit_code: {exit_code:?}");
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
                                    // Flush logs before notifying so clients see logs immediately
                                    let _ = log_appender.flush().await;
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
                                    // Flush logs before notifying so clients see logs immediately
                                    let _ = log_appender.flush().await;
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
                        if let Some(ref mut interval) = cmd_check_interval {
                            interval.tick().await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    }, if !ready_notified && ready_cmd.is_some() => {
                        if let Some(ref cmd) = ready_cmd {
                            // Run the readiness check command using the shell abstraction
                            let mut command = Shell::default_for_platform().command(cmd);
                            command
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null());
                            let result: std::io::Result<std::process::ExitStatus> = command.status().await;
                            match result {
                                Ok(status) if status.success() => {
                                    info!("daemon {id} ready: readiness command succeeded");
                                    ready_notified = true;
                                    let _ = log_appender.flush().await;
                                    if let Some(tx) = ready_tx.take() {
                                        let _ = tx.send(Ok(()));
                                    }
                                    // Stop checking once ready
                                    cmd_check_interval = None;
                                }
                                Ok(_) => {
                                    trace!("daemon {id} cmd check: command returned non-zero (not ready)");
                                }
                                Err(e) => {
                                    trace!("daemon {id} cmd check failed: {e}");
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
                        if !ready_notified && ready_pattern.is_none() && ready_http.is_none() && ready_port.is_none() && ready_cmd.is_none() {
                            info!("daemon {id} ready: delay elapsed");
                            ready_notified = true;
                            // Flush logs before notifying so clients see logs immediately
                            let _ = log_appender.flush().await;
                            if let Some(tx) = ready_tx.take() {
                                let _ = tx.send(Ok(()));
                            }
                        }
                        // Disable timer after it fires
                        delay_timer = None;
                    }
                    _ = log_flush_interval.tick() => {
                        // Periodic flush to ensure logs are written to disk
                        if let Err(e) = log_appender.flush().await {
                            error!("Failed to flush log for daemon {id}: {e}");
                        }
                    }
                    // Note: No `else => break` because log_flush_interval.tick() is always available,
                    // making the else branch unreachable. The loop exits via the exit_rx.recv() branch.
                }
            }

            // Final flush to ensure all buffered logs are written
            if let Err(e) = log_appender.flush().await {
                error!("Failed to final flush log for daemon {id}: {e}");
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
                    let status = match status.code() {
                        Some(code) => DaemonStatus::Errored(code),
                        None => DaemonStatus::ErroredUnknown,
                    };
                    if let Err(e) = SUPERVISOR
                        .upsert_daemon(UpsertDaemonOpts {
                            id: id.clone(),
                            pid: None,
                            status,
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
                    status: DaemonStatus::ErroredUnknown,
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

    /// Stop a running daemon
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
                    // First set status to Stopping (preserve PID for monitoring task)
                    self.upsert_daemon(UpsertDaemonOpts {
                        id: id.to_string(),
                        pid: Some(pid),
                        status: DaemonStatus::Stopping,
                        ..Default::default()
                    })
                    .await?;

                    // First kill child processes (should be ahead of parent)
                    for child_pid in PROCS.all_children(pid) {
                        debug!("killing child pid: {child_pid}");
                        if let Err(e) = PROCS.kill_async(child_pid).await {
                            warn!("failed to kill child pid {child_pid}: {e}");
                        }
                    }

                    // Then kill the parent process
                    if let Err(e) = PROCS.kill_async(pid).await {
                        debug!("failed to kill pid {pid}: {e}");
                        // Check if the process is actually stopped despite the error
                        PROCS.refresh_processes();
                        if PROCS.is_running(pid) {
                            // Process still running after kill attempt - set back to Running
                            debug!("failed to stop pid {pid}: process still running after kill");
                            self.upsert_daemon(UpsertDaemonOpts {
                                id: id.to_string(),
                                pid: Some(pid), // Preserve PID to avoid orphaning the process
                                status: DaemonStatus::Running,
                                ..Default::default()
                            })
                            .await?;
                            return Ok(IpcResponse::DaemonStopFailed {
                                error: format!(
                                    "process {pid} still running after kill attempt: {e}"
                                ),
                            });
                        }
                    }

                    // Verify process is fully terminated before returning
                    // This avoids race conditions where a new process starts before
                    // the old one has fully released resources (ports, files, etc.)
                    let mut process_terminated = false;
                    for i in 0..10 {
                        PROCS.refresh_pids(&[pid]);
                        if !PROCS.is_running(pid) {
                            process_terminated = true;
                            break;
                        }
                        if i < 9 {
                            debug!(
                                "waiting for process {pid} to fully terminate ({}/10)",
                                i + 1
                            );
                            time::sleep(Duration::from_millis(50)).await;
                        }
                    }

                    if !process_terminated {
                        warn!(
                            "Process {pid} for daemon {id} did not terminate within 500ms after SIGTERM. \
                             The process may take longer to release resources."
                        );
                    }

                    // Process successfully stopped
                    self.upsert_daemon(UpsertDaemonOpts {
                        id: id.to_string(),
                        pid: None,
                        status: DaemonStatus::Stopped,
                        last_exit_success: Some(true), // Manual stop is considered successful
                        ..Default::default()
                    })
                    .await?;
                } else {
                    debug!("pid {pid} not running, process may have exited unexpectedly");
                    // Process already dead, directly mark as stopped
                    // Note that the cleanup logic is handled in monitor task
                    self.upsert_daemon(UpsertDaemonOpts {
                        id: id.to_string(),
                        pid: None,
                        status: DaemonStatus::Stopped,
                        ..Default::default()
                    })
                    .await?;
                    return Ok(IpcResponse::DaemonWasNotRunning);
                }
                Ok(IpcResponse::Ok)
            } else {
                debug!("daemon {id} not running");
                Ok(IpcResponse::DaemonNotRunning)
            }
        } else {
            debug!("daemon {id} not found");
            Ok(IpcResponse::DaemonNotFound)
        }
    }
}
