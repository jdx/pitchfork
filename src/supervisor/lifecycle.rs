//! Daemon lifecycle management - start/stop operations
//!
//! Contains the core `run()`, `run_once()`, and `stop()` methods for daemon process management.

use super::hooks::{HookType, fire_hook};
use super::{SUPERVISOR, Supervisor};
use crate::daemon::RunOptions;
use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::error::PortError;
use crate::ipc::IpcResponse;
use crate::procs::PROCS;
use crate::settings::settings;
use crate::shell::Shell;
use crate::supervisor::state::UpsertDaemonOpts;
use crate::{Result, env};
use itertools::Itertools;
use miette::{IntoDiagnostic, WrapErr};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::iter::once;
use std::net::TcpListener;
use std::sync::atomic;
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
                            fire_hook(
                                HookType::OnRetry,
                                id.clone(),
                                opts.dir.clone(),
                                attempt + 1,
                                opts.env.clone(),
                                vec![],
                            )
                            .await;
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

        // Check port availability and apply auto-bump if configured
        let expected_ports = opts.expected_port.clone();
        let (resolved_ports, effective_ready_port) = if !opts.expected_port.is_empty() {
            match check_ports_available(
                &opts.expected_port,
                opts.auto_bump_port,
                opts.port_bump_attempts,
            )
            .await
            {
                Ok(resolved) => {
                    let ready_port = if let Some(configured_port) = opts.ready_port {
                        // If ready_port matches one of the expected ports, apply the same bump offset
                        let bump_offset = resolved
                            .first()
                            .unwrap_or(&0)
                            .saturating_sub(*opts.expected_port.first().unwrap_or(&0));
                        if opts.expected_port.contains(&configured_port) && bump_offset > 0 {
                            configured_port
                                .checked_add(bump_offset)
                                .or(Some(configured_port))
                        } else {
                            Some(configured_port)
                        }
                    } else {
                        // Don't use port 0 for readiness checks - it's a special value
                        // that requests an ephemeral port from the OS
                        resolved.first().copied().filter(|&p| p != 0)
                    };
                    info!(
                        "daemon {id}: ports {:?} resolved to {:?}",
                        opts.expected_port, resolved
                    );
                    (resolved, ready_port)
                }
                Err(e) => {
                    error!("daemon {id}: port check failed: {e}");
                    // Convert PortError to structured IPC response
                    if let Some(port_error) = e.downcast_ref::<PortError>() {
                        match port_error {
                            PortError::InUse { port, process, pid } => {
                                return Ok(IpcResponse::PortConflict {
                                    port: *port,
                                    process: process.clone(),
                                    pid: *pid,
                                });
                            }
                            PortError::NoAvailablePort {
                                start_port,
                                attempts,
                            } => {
                                return Ok(IpcResponse::NoAvailablePort {
                                    start_port: *start_port,
                                    attempts: *attempts,
                                });
                            }
                        }
                    }
                    return Ok(IpcResponse::DaemonFailed {
                        error: e.to_string(),
                    });
                }
            }
        } else {
            (Vec::new(), opts.ready_port)
        };

        let cmd: Vec<String> = if opts.mise {
            match settings().resolve_mise_bin() {
                Some(mise_bin) => {
                    let mise_bin_str = mise_bin.to_string_lossy().to_string();
                    info!("daemon {id}: wrapping command with mise ({mise_bin_str})");
                    once("exec".to_string())
                        .chain(once(mise_bin_str))
                        .chain(once("x".to_string()))
                        .chain(once("--".to_string()))
                        .chain(cmd)
                        .collect_vec()
                }
                None => {
                    warn!("daemon {id}: mise=true but mise binary not found, running without mise");
                    once("exec".to_string()).chain(cmd).collect_vec()
                }
            }
        } else {
            once("exec".to_string()).chain(cmd).collect_vec()
        };
        let args = vec!["-c".to_string(), shell_words::join(&cmd)];
        let log_path = id.log_path();
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

        // Apply custom environment variables from config
        if let Some(ref env_vars) = opts.env {
            cmd.envs(env_vars);
        }

        // Inject pitchfork metadata env vars AFTER user env so they can't be overwritten
        cmd.env("PITCHFORK_DAEMON_ID", id.qualified());
        cmd.env("PITCHFORK_DAEMON_NAMESPACE", id.namespace());
        cmd.env("PITCHFORK_RETRY_COUNT", opts.retry_count.to_string());

        // Inject the resolved ports for the daemon to use
        if !resolved_ports.is_empty() {
            // Set PORT to the first port for backward compatibility
            // When there's only one port, both PORT and PORT0 will be set to the same value.
            // This follows the convention used by many deployment platforms (Heroku, etc.).
            cmd.env("PORT", resolved_ports[0].to_string());
            // Set individual ports as PORT0, PORT1, etc.
            for (i, port) in resolved_ports.iter().enumerate() {
                cmd.env(format!("PORT{}", i), port.to_string());
            }
        }

        // Put each daemon in its own session/process group so we can kill the
        // entire tree atomically with a single signal to the group.
        #[cfg(unix)]
        {
            let memory_limit_bytes = opts.memory_limit.map(|ml| ml.0);
            let cpu_time_limit_secs = opts.cpu_time_limit.map(|ct| {
                // Round up so sub-second values like "500ms" become 1 instead of 0.
                // RLIMIT_CPU = 0 would kill the process immediately.
                ct.0.as_secs().max(1) + if ct.0.subsec_nanos() > 0 { 1 } else { 0 }
            });
            unsafe {
                cmd.pre_exec(move || {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    // Apply memory limit via RLIMIT_AS (virtual address space)
                    if let Some(limit) = memory_limit_bytes {
                        let rlim = libc::rlimit {
                            rlim_cur: limit,
                            rlim_max: limit,
                        };
                        if libc::setrlimit(libc::RLIMIT_AS, &rlim) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    // Apply CPU time limit via RLIMIT_CPU (total CPU seconds)
                    // Set the soft limit slightly below the hard limit so the process
                    // receives SIGXCPU first and has a grace window to clean up before
                    // SIGKILL arrives at the hard limit.
                    if let Some(secs) = cpu_time_limit_secs {
                        let grace = if secs <= 5 { 1 } else { 5 };
                        let soft = secs.saturating_sub(grace).max(1);
                        let rlim = libc::rlimit {
                            rlim_cur: soft,
                            rlim_max: secs,
                        };
                        if libc::setrlimit(libc::RLIMIT_CPU, &rlim) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    Ok(())
                });
            }
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
            .upsert_daemon(
                UpsertDaemonOpts::builder(id.clone())
                    .set(|o| {
                        o.pid = Some(pid);
                        o.status = DaemonStatus::Running;
                        o.shell_pid = opts.shell_pid;
                        o.dir = Some(opts.dir.clone());
                        o.cmd = Some(original_cmd);
                        o.autostop = opts.autostop;
                        o.cron_schedule = opts.cron_schedule.clone();
                        o.cron_retrigger = opts.cron_retrigger;
                        o.retry = Some(opts.retry);
                        o.retry_count = Some(opts.retry_count);
                        o.ready_delay = opts.ready_delay;
                        o.ready_output = opts.ready_output.clone();
                        o.ready_http = opts.ready_http.clone();
                        o.ready_port = effective_ready_port;
                        o.ready_cmd = opts.ready_cmd.clone();
                        o.expected_port = expected_ports;
                        o.resolved_port = resolved_ports;
                        o.auto_bump_port = Some(opts.auto_bump_port);
                        o.port_bump_attempts = Some(opts.port_bump_attempts);
                        o.depends = Some(opts.depends.clone());
                        o.env = opts.env.clone();
                        o.watch = Some(opts.watch.clone());
                        o.watch_base_dir = opts.watch_base_dir.clone();
                        o.mise = Some(opts.mise);
                        o.memory_limit = opts.memory_limit;
                        o.cpu_time_limit = opts.cpu_time_limit;
                    })
                    .build(),
            )
            .await?;

        let id_clone = id.clone();
        let ready_delay = opts.ready_delay;
        let ready_output = opts.ready_output.clone();
        let ready_http = opts.ready_http.clone();
        let ready_port = effective_ready_port;
        let ready_cmd = opts.ready_cmd.clone();
        let daemon_dir = opts.dir.clone();
        let hook_retry_count = opts.retry_count;
        let hook_retry = opts.retry;
        let hook_daemon_env = opts.env.clone();

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

            // Get settings for intervals
            let s = settings();
            let ready_check_interval = s.supervisor_ready_check_interval();
            let http_client_timeout = s.supervisor_http_client_timeout();
            let log_flush_interval_duration = s.supervisor_log_flush_interval();

            // Setup HTTP readiness check interval
            let mut http_check_interval = ready_http
                .as_ref()
                .map(|_| tokio::time::interval(ready_check_interval));
            let http_client = ready_http.as_ref().map(|_| {
                reqwest::Client::builder()
                    .timeout(http_client_timeout)
                    .build()
                    .unwrap_or_default()
            });

            // Setup TCP port readiness check interval
            let mut port_check_interval =
                ready_port.map(|_| tokio::time::interval(ready_check_interval));

            // Setup command readiness check interval
            let mut cmd_check_interval = ready_cmd
                .as_ref()
                .map(|_| tokio::time::interval(ready_check_interval));

            // Setup periodic log flush interval
            let mut log_flush_interval = tokio::time::interval(log_flush_interval_duration);

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
                                    fire_hook(HookType::OnReady, id.clone(), daemon_dir.clone(), hook_retry_count, hook_daemon_env.clone(), vec![]).await;
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
                                    fire_hook(HookType::OnReady, id.clone(), daemon_dir.clone(), hook_retry_count, hook_daemon_env.clone(), vec![]).await;
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
                                    fire_hook(HookType::OnReady, id.clone(), daemon_dir.clone(), hook_retry_count, hook_daemon_env.clone(), vec![]).await;
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
                                    fire_hook(HookType::OnReady, id.clone(), daemon_dir.clone(), hook_retry_count, hook_daemon_env.clone(), vec![]).await;
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
                                .current_dir(&daemon_dir)
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
                                    fire_hook(HookType::OnReady, id.clone(), daemon_dir.clone(), hook_retry_count, hook_daemon_env.clone(), vec![]).await;
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
                            fire_hook(HookType::OnReady, id.clone(), daemon_dir.clone(), hook_retry_count, hook_daemon_env.clone(), vec![]).await;
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

            // Signal that this monitoring task is processing its exit path.
            // The RAII guard will decrement the counter and notify close()
            // when the task finishes (including all fire_hook registrations),
            // regardless of which return path is taken.
            SUPERVISOR
                .active_monitors
                .fetch_add(1, atomic::Ordering::Release);
            struct MonitorGuard;
            impl Drop for MonitorGuard {
                fn drop(&mut self) {
                    SUPERVISOR
                        .active_monitors
                        .fetch_sub(1, atomic::Ordering::Release);
                    SUPERVISOR.monitor_done.notify_waiters();
                }
            }
            let _monitor_guard = MonitorGuard;
            // Check if this monitoring task is for the current daemon process.
            // Allow Stopped/Stopping daemons through: stop() clears pid atomically,
            // so d.pid != Some(pid) would be true, but we still need the is_stopped()
            // branch below to fire on_stop/on_exit hooks.
            if current_daemon.is_none()
                || current_daemon.as_ref().is_some_and(|d| {
                    d.pid != Some(pid) && !d.status.is_stopped() && !d.status.is_stopping()
                })
            {
                // Another process has taken over, don't update status
                return;
            }
            // Capture the intentional-stop flag BEFORE any state changes.
            // stop() transitions Stopping → Stopped and clears pid. If stop() wins the race
            // and sets Stopped before this task runs, we still need to fire on_stop/on_exit.
            // Treat both Stopping and Stopped as "intentional stop by pitchfork".
            let already_stopped = current_daemon
                .as_ref()
                .is_some_and(|d| d.status.is_stopped());
            let is_stopping = already_stopped
                || current_daemon
                    .as_ref()
                    .is_some_and(|d| d.status.is_stopping());

            // --- Phase 1: Determine exit_code, exit_reason, and update daemon state ---
            let (exit_code, exit_reason) = match (&exit_status, is_stopping) {
                (Ok(status), true) => {
                    // Intentional stop (by pitchfork). status.code() returns None
                    // on Unix when killed by signal (e.g. SIGTERM); use -1 to
                    // distinguish from a clean exit code 0.
                    (status.code().unwrap_or(-1), "stop")
                }
                (Ok(status), false) if status.success() => (status.code().unwrap_or(-1), "exit"),
                (Ok(status), false) => (status.code().unwrap_or(-1), "fail"),
                (Err(_), true) => {
                    // child.wait() error while stopping (e.g. sysinfo reaped the process)
                    (-1, "stop")
                }
                (Err(_), false) => (-1, "fail"),
            };

            // Update daemon state unless stop() already did it (won the race).
            if !already_stopped {
                if let Ok(status) = &exit_status {
                    info!("daemon {id} exited with status {status}");
                }
                let (new_status, last_exit_success) = match exit_reason {
                    "stop" | "exit" => (
                        DaemonStatus::Stopped,
                        exit_status.as_ref().map(|s| s.success()).unwrap_or(true),
                    ),
                    _ => (DaemonStatus::Errored(exit_code), false),
                };
                if let Err(e) = SUPERVISOR
                    .upsert_daemon(
                        UpsertDaemonOpts::builder(id.clone())
                            .set(|o| {
                                o.pid = None;
                                o.status = new_status;
                                o.last_exit_success = Some(last_exit_success);
                            })
                            .build(),
                    )
                    .await
                {
                    error!("Failed to update daemon state for {id}: {e}");
                }
            }

            // --- Phase 2: Fire hooks ---
            let hook_extra_env = vec![
                ("PITCHFORK_EXIT_CODE".to_string(), exit_code.to_string()),
                ("PITCHFORK_EXIT_REASON".to_string(), exit_reason.to_string()),
            ];

            // Determine which hooks to fire based on exit reason
            let hooks_to_fire: Vec<HookType> = match exit_reason {
                "stop" => vec![HookType::OnStop, HookType::OnExit],
                "exit" => vec![HookType::OnExit],
                // "fail": fire on_fail + on_exit only when retries are exhausted
                _ if hook_retry_count >= hook_retry => {
                    vec![HookType::OnFail, HookType::OnExit]
                }
                _ => vec![],
            };

            for hook_type in hooks_to_fire {
                fire_hook(
                    hook_type,
                    id.clone(),
                    daemon_dir.clone(),
                    hook_retry_count,
                    hook_daemon_env.clone(),
                    hook_extra_env.clone(),
                )
                .await;
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
    pub async fn stop(&self, id: &DaemonId) -> Result<IpcResponse> {
        let pitchfork_id = DaemonId::pitchfork();
        if *id == pitchfork_id {
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
                    self.upsert_daemon(
                        UpsertDaemonOpts::builder(id.clone())
                            .set(|o| {
                                o.pid = Some(pid);
                                o.status = DaemonStatus::Stopping;
                            })
                            .build(),
                    )
                    .await?;

                    // Kill the entire process group atomically (daemon PID == PGID
                    // because we called setsid() at spawn time)
                    if let Err(e) = PROCS.kill_process_group_async(pid).await {
                        debug!("failed to kill pid {pid}: {e}");
                        // Check if the process is actually stopped despite the error
                        PROCS.refresh_processes();
                        if PROCS.is_running(pid) {
                            // Process still running after kill attempt - set back to Running
                            debug!("failed to stop pid {pid}: process still running after kill");
                            self.upsert_daemon(
                                UpsertDaemonOpts::builder(id.clone())
                                    .set(|o| {
                                        o.pid = Some(pid); // Preserve PID to avoid orphaning the process
                                        o.status = DaemonStatus::Running;
                                    })
                                    .build(),
                            )
                            .await?;
                            return Ok(IpcResponse::DaemonStopFailed {
                                error: format!(
                                    "process {pid} still running after kill attempt: {e}"
                                ),
                            });
                        }
                    }

                    // Process successfully stopped
                    // Note: kill_async uses SIGTERM -> wait ~3s -> SIGKILL strategy,
                    // and also detects zombie processes, so by the time it returns,
                    // the process should be fully terminated.
                    self.upsert_daemon(
                        UpsertDaemonOpts::builder(id.clone())
                            .set(|o| {
                                o.pid = None;
                                o.status = DaemonStatus::Stopped;
                                o.last_exit_success = Some(true); // Manual stop is considered successful
                            })
                            .build(),
                    )
                    .await?;
                } else {
                    debug!("pid {pid} not running, process may have exited unexpectedly");
                    // Process already dead, directly mark as stopped
                    // Note that the cleanup logic is handled in monitor task
                    self.upsert_daemon(
                        UpsertDaemonOpts::builder(id.clone())
                            .set(|o| {
                                o.pid = None;
                                o.status = DaemonStatus::Stopped;
                            })
                            .build(),
                    )
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

/// Check if multiple ports are available and optionally auto-bump to find available ports.
///
/// All ports are bumped by the same offset to maintain relative port spacing.
/// Returns the resolved ports (either the original or bumped ones).
/// Returns an error if any port is in use and auto_bump is disabled,
/// or if no available ports can be found after max attempts.
async fn check_ports_available(
    expected_ports: &[u16],
    auto_bump: bool,
    max_attempts: u32,
) -> Result<Vec<u16>> {
    if expected_ports.is_empty() {
        return Ok(Vec::new());
    }

    for bump_offset in 0..=max_attempts {
        // Use wrapping_add to handle overflow correctly - ports wrap around at 65535
        let candidate_ports: Vec<u16> = expected_ports
            .iter()
            .map(|&p| p.wrapping_add(bump_offset as u16))
            .collect();

        // Check if all ports in this set are available
        let mut all_available = true;
        let mut conflicting_port = None;

        for &port in &candidate_ports {
            // Port 0 is a special case - it requests an ephemeral port from the OS.
            // Skip the availability check for port 0 since binding to it always succeeds.
            if port == 0 {
                continue;
            }

            // Use spawn_blocking to avoid blocking the async runtime during TCP bind checks
            // Bind to 0.0.0.0 to detect conflicts on all interfaces, not just localhost
            //
            // NOTE: This check has a time-of-check-to-time-of-use (TOCTOU) race condition.
            // Another process could grab the port between our check and the daemon actually
            // binding. This is inherent to the approach and acceptable for our use case
            // since we're primarily detecting conflicts with already-running daemons.
            let port_check =
                tokio::task::spawn_blocking(move || match TcpListener::bind(("0.0.0.0", port)) {
                    Ok(listener) => {
                        drop(listener);
                        true
                    }
                    Err(_) => false,
                })
                .await
                .into_diagnostic()
                .wrap_err("failed to check port availability")?;

            if !port_check {
                all_available = false;
                conflicting_port = Some(port);
                break;
            }
        }

        if all_available {
            // Check for overflow (port wrapped around to 0 due to wrapping_add)
            // If any candidate port is 0 but the original expected port wasn't 0,
            // it means we've wrapped around and should stop
            if candidate_ports.contains(&0) && !expected_ports.contains(&0) {
                return Err(PortError::NoAvailablePort {
                    start_port: expected_ports[0],
                    attempts: bump_offset + 1,
                }
                .into());
            }
            if bump_offset > 0 {
                info!(
                    "ports {:?} bumped by {} to {:?}",
                    expected_ports, bump_offset, candidate_ports
                );
            }
            return Ok(candidate_ports);
        }

        // Port is in use
        if bump_offset == 0 {
            // First attempt - try to get process info using lsof
            if let Some(port) = conflicting_port {
                if let Some((pid, process_name)) = get_process_using_port(port).await {
                    if !auto_bump {
                        return Err(PortError::InUse {
                            port,
                            process: process_name,
                            pid,
                        }
                        .into());
                    }
                } else if !auto_bump {
                    // Couldn't identify process, but port is definitely in use
                    return Err(PortError::InUse {
                        port,
                        process: "unknown".to_string(),
                        pid: 0,
                    }
                    .into());
                }
            }
        }
    }

    // No available ports found after max attempts
    Err(PortError::NoAvailablePort {
        start_port: expected_ports[0],
        attempts: max_attempts + 1,
    }
    .into())
}

/// Get the process using a specific port.
///
/// Returns (pid, process_name) if found, None otherwise.
async fn get_process_using_port(port: u16) -> Option<(u32, String)> {
    tokio::task::spawn_blocking(move || {
        listeners::get_all()
            .ok()?
            .into_iter()
            .find(|listener| listener.socket.port() == port)
            .map(|listener| (listener.process.pid, listener.process.name))
    })
    .await
    .ok()
    .flatten()
}
