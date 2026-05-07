//! Daemon lifecycle management - start/stop operations
//!
//! Contains the core `run()`, `run_once()`, and `stop()` methods for daemon process management.

use super::hooks::{self, HookType, fire_hook};
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
use miette::IntoDiagnostic;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
#[cfg(unix)]
use std::ffi::CString;
use std::iter::once;
use std::sync::atomic;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufWriter};
use tokio::select;
use tokio::sync::oneshot;
use tokio::time;

/// Cache for compiled regex patterns to avoid recompilation on daemon restarts
static REGEX_CACHE: Lazy<std::sync::Mutex<HashMap<String, Regex>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

#[cfg(unix)]
#[derive(Clone, Debug, PartialEq, Eq)]
enum RunIdentity {
    Inherit,
    Switch {
        uid: nix::unistd::Uid,
        gid: nix::unistd::Gid,
        username: Option<CString>,
    },
}

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
        if opts.wait_ready && opts.retry.count() > 0 {
            // Use saturating_add to avoid overflow when retry = u32::MAX (infinite)
            let max_attempts = opts.retry.count().saturating_add(1);
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
                        if attempt < opts.retry.count() {
                            let backoff_secs = 2u64.saturating_pow(attempt).min(3600);
                            info!(
                                "daemon {id} failed (attempt {}/{}), retrying in {}s",
                                attempt + 1,
                                max_attempts,
                                backoff_secs
                            );
                            fire_hook(
                                HookType::OnRetry,
                                id.clone(),
                                opts.dir.0.clone(),
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
        let expected_ports = opts
            .port
            .as_ref()
            .map(|p| p.expect.clone())
            .unwrap_or_default();
        let (resolved_ports, effective_ready_port) = if !expected_ports.is_empty() {
            let port_cfg = opts.port.as_ref().unwrap();
            match check_ports_available(
                &expected_ports,
                port_cfg.auto_bump(),
                port_cfg.max_bump_attempts(),
            )
            .await
            {
                Ok(resolved) => {
                    let ready_port = if let Some(configured_port) = opts.ready_port {
                        // If ready_port matches one of the expected ports, apply the same bump offset
                        let bump_offset = resolved
                            .first()
                            .unwrap_or(&0)
                            .saturating_sub(*expected_ports.first().unwrap_or(&0));
                        if expected_ports.contains(&configured_port) && bump_offset > 0 {
                            configured_port
                                .checked_add(bump_offset)
                                .or(Some(configured_port))
                        } else {
                            Some(configured_port)
                        }
                    } else if opts.ready_output.is_none()
                        && opts.ready_http.is_none()
                        && opts.ready_cmd.is_none()
                        && opts.ready_delay.is_none()
                    {
                        // No other ready check configured — use the first expected port as a
                        // TCP port readiness check so the daemon is considered ready once it
                        // starts listening.  Skip port 0 (ephemeral port request).
                        resolved.first().copied().filter(|&p| p != 0)
                    } else {
                        // Another ready check is configured (output/http/cmd/delay).
                        // Don't add an implicit TCP port check — it could race and fire
                        // before the daemon has produced any output.
                        None
                    };
                    info!("daemon {id}: ports {expected_ports:?} resolved to {resolved:?}");
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
            // When ready_port is set without expected_port, check that the port
            // is not already occupied.  If another process is listening on it,
            // the TCP readiness probe would immediately succeed and pitchfork
            // would falsely consider the daemon ready — routing proxy traffic to
            // the wrong process.
            if let Some(port) = opts.ready_port {
                if port > 0 {
                    if let Some((pid, process)) = detect_port_conflict(port).await {
                        return Ok(IpcResponse::PortConflict { port, process, pid });
                    }
                }
            }
            (Vec::new(), opts.ready_port)
        };

        let cmd: Vec<String> = if opts.mise.unwrap_or(settings().general.mise) {
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
        #[cfg(unix)]
        let run_identity = match resolve_effective_run_identity(opts.user.as_deref()) {
            Ok(identity) => identity,
            Err(e) => {
                return Ok(IpcResponse::DaemonFailed {
                    error: e.to_string(),
                });
            }
        };
        info!("run: spawning daemon {id} with args: {args:?}");

        // Allocate PTY if configured
        #[cfg(unix)]
        let pty_pair = if opts.pty.unwrap_or(false) {
            match super::pty::openpty() {
                Ok(pair) => {
                    info!("daemon {id}: allocated PTY (pty = true)");
                    Some(pair)
                }
                Err(e) => {
                    warn!("daemon {id}: failed to allocate PTY, falling back to pipes: {e}");
                    None
                }
            }
        } else {
            None
        };

        let mut cmd = tokio::process::Command::new("sh");

        #[cfg(unix)]
        if let Some(ref pair) = pty_pair {
            // PTY mode: connect both stdout and stderr to the slave PTY.
            // The child uses the slave for stdin/stdout/stderr, and we read
            // output from the master.
            let slave_file = std::fs::File::from(
                pair.slave
                    .try_clone()
                    .map_err(|e| miette::miette!("failed to dup slave PTY fd: {e}"))?,
            );
            cmd.stdin(std::process::Stdio::from(slave_file.try_clone().map_err(
                |e| miette::miette!("failed to clone slave PTY fd for stdin: {e}"),
            )?));
            cmd.stdout(std::process::Stdio::from(slave_file.try_clone().map_err(
                |e| miette::miette!("failed to clone slave PTY fd for stdout: {e}"),
            )?));
            cmd.stderr(std::process::Stdio::from(slave_file));
        } else {
            cmd.stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
        }

        #[cfg(not(unix))]
        {
            cmd.stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
        }

        cmd.args(&args).current_dir(&opts.dir);

        #[cfg(unix)]
        if pty_pair.is_none() {
            cmd.stdin(std::process::Stdio::null());
        }

        #[cfg(not(unix))]
        cmd.stdin(std::process::Stdio::null());

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
                cmd.env(format!("PORT{i}"), port.to_string());
            }
        }

        // Inject proxy-related environment variables
        inject_proxy_env(&mut cmd, &opts.slug);

        #[cfg(unix)]
        {
            let run_identity = run_identity.clone();
            let use_pty = pty_pair.is_some();
            unsafe {
                cmd.pre_exec(move || {
                    nix::unistd::setsid().map_err(nix_to_io_error)?;

                    // When using a PTY, set the slave as the controlling terminal.
                    // The slave FD has already been dup'd onto stdin/stdout/stderr
                    // by tokio, so we can use stdin (fd 0) for TIOCSCTTY.
                    if use_pty {
                        let ret = libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0);
                        if ret < 0 {
                            // Non-fatal: the process can still run without
                            // a controlling terminal.
                            #[cfg(target_os = "linux")]
                            eprintln!(
                                "pitchfork: TIOCSCTTY failed: {}",
                                std::io::Error::last_os_error()
                            );
                        }
                    }

                    apply_run_identity(&run_identity)?;
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
                        o.dir = Some(opts.dir.0.clone());
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
                        o.port = crate::config_types::PortConfig::from_parts(
                            expected_ports,
                            opts.port.as_ref().map(|p| p.bump).unwrap_or_default(),
                        );
                        o.resolved_port = resolved_ports;
                        o.depends = Some(opts.depends.clone());
                        o.env = opts.env.clone();
                        o.watch = Some(opts.watch.clone());
                        o.watch_mode = Some(opts.watch_mode);
                        o.watch_base_dir = opts.watch_base_dir.clone();
                        o.mise = opts.mise;
                        o.user = opts.user.clone();
                        o.memory_limit = opts.memory_limit;
                        o.cpu_limit = opts.cpu_limit;
                        o.stop_signal = opts.stop_signal;
                        o.pty = opts.pty;
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
        let daemon_dir = opts.dir.0.clone();
        let hook_retry_count = opts.retry_count;
        let hook_retry = opts.retry;
        let hook_daemon_env = opts.env.clone();
        let on_output_hook = opts.on_output_hook.clone();
        // Whether this daemon has any port-related config — used to skip the
        // active_port detection task for daemons that never bind a port (e.g. `sleep 60`).
        // When the proxy is enabled, only detect active_port for daemons that are
        // actually referenced by a registered slug, rather than blanket-polling every
        // daemon (which wastes ~7.5 s of listeners::get_all() calls per port-less daemon).
        let has_port_config = opts.port.as_ref().is_some_and(|p| !p.expect.is_empty())
            || (settings().proxy.enable && is_daemon_slug_target(id));
        let daemon_pid = pid;

        // Prepare output readers before spawning the monitoring task.
        // In PTY mode, we read from the PTY master FD.
        // In pipe mode, we read from separate stdout/stderr pipes.
        #[cfg(unix)]
        let pty_reader = pty_pair.map(|p| {
            tokio::io::BufReader::new(tokio::fs::File::from_std(std::fs::File::from(p.master)))
                .lines()
        });
        #[cfg(not(unix))]
        let pty_reader: Option<tokio::io::Lines<tokio::io::BufReader<tokio::fs::File>>> = None;
        let stdout_reader = if pty_reader.is_none() {
            child
                .stdout
                .take()
                .map(|s| tokio::io::BufReader::new(s).lines())
        } else {
            None
        };
        let stderr_reader = if pty_reader.is_none() {
            child
                .stderr
                .take()
                .map(|s| tokio::io::BufReader::new(s).lines())
        } else {
            None
        };

        if pty_reader.is_none() && (stdout_reader.is_none() || stderr_reader.is_none()) {
            error!("Failed to capture stdout/stderr for daemon {id}");
        }

        tokio::spawn(async move {
            let id = id_clone;

            // Merge all output sources (PTY master OR stdout+stderr) into a single channel.
            let (output_tx, mut output_rx) = tokio::sync::mpsc::channel::<String>(256);

            if let Some(mut reader) = pty_reader {
                // PTY mode: single merged stream from the master.
                // output_tx is moved into the spawn; when the reader ends the
                // channel closes automatically.
                tokio::spawn(async move {
                    while let Ok(Some(mut line)) = reader.next_line().await {
                        // PTY slave uses ONLCR: \n → \r\n; strip the trailing \r.
                        if line.ends_with('\r') {
                            line.pop();
                        }
                        if output_tx.send(line).await.is_err() {
                            break;
                        }
                    }
                });
            } else {
                // Pipe mode: stdout and stderr are merged into the same channel.
                // Both `ready_output` and `on_output_hook` patterns match against
                // lines from either stream, which is the expected behavior (a
                // "server ready" message may appear on stderr in some tools).
                if let Some(mut stdout) = stdout_reader {
                    let tx = output_tx.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = stdout.next_line().await {
                            if tx.send(line).await.is_err() {
                                break;
                            }
                        }
                    });
                }
                if let Some(mut stderr) = stderr_reader {
                    let tx = output_tx.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = stderr.next_line().await {
                            if tx.send(line).await.is_err() {
                                break;
                            }
                        }
                    });
                }
                // Drop the last sender so the channel closes when all readers finish.
                drop(output_tx);
            }
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
                let line_for_log = line;
                if line_for_log.starts_with(&format!("{id} ")) {
                    // mise tasks often already have the id printed
                    format!("{} {line_for_log}\n", now())
                } else {
                    format!("{} {id} {line_for_log}\n", now())
                }
            };

            // Setup readiness checking
            let mut ready_notified = false;
            let mut ready_tx = ready_tx;
            let ready_pattern = ready_output.as_ref().and_then(|p| get_or_compile_regex(p));
            // Track whether we've already spawned the active_port detection task
            let mut active_port_spawned = false;

            // Validate on_output config early; discard the hook on any error so
            // a bad regex does not silently fall through to the (None, None) => true
            // match arm and fire on every line.
            let on_output_hook = match on_output_hook {
                Some(ref hook) => match hook.validate(id.name()) {
                    Ok(()) => on_output_hook,
                    Err(e) => {
                        error!("{e}");
                        None
                    }
                },
                None => None,
            };

            // Compile the regex pattern after validation so we only attempt this
            // when the hook is known-good (validate() already checked the syntax).
            let on_output_pattern: Option<regex::Regex> = on_output_hook
                .as_ref()
                .and_then(|h| h.regex.as_deref().and_then(get_or_compile_regex));
            let on_output_debounce = on_output_hook
                .as_ref()
                .map(|h| h.debounce_duration())
                .unwrap_or(Duration::from_millis(1000));
            // Last time the on_output hook fired; None means it has never fired.
            let mut on_output_last_fired: Option<std::time::Instant> = None;

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
                // On non-Linux Unix (e.g. macOS) the zombie reaper may win the
                // race and consume the exit status via waitpid(None, WNOHANG)
                // before Tokio's child.wait() gets to it. When that happens,
                // Tokio returns an ECHILD io::Error. We recover by checking
                // REAPED_STATUSES for the stashed exit code.
                //
                // On Linux this is unnecessary because the reaper uses
                // waitid(WNOWAIT) to peek before reaping, which avoids the
                // race entirely.
                #[cfg(all(unix, not(target_os = "linux")))]
                let result = match &result {
                    Err(e) if e.raw_os_error() == Some(nix::libc::ECHILD) => {
                        if let Some(code) = super::REAPED_STATUSES.lock().await.remove(&child_pid) {
                            warn!(
                                "daemon pid {child_pid} wait() got ECHILD; \
                                 recovered exit code {code} from zombie reaper"
                            );
                            // Synthesize an ExitStatus from the stashed code.
                            // On Unix we can use `ExitStatus::from_raw()` with
                            // a wait-style status word (code << 8 for normal
                            // exit, or raw signal number for signal death).
                            use std::os::unix::process::ExitStatusExt;
                            if code >= 0 {
                                Ok(std::process::ExitStatus::from_raw(code << 8))
                            } else {
                                // Negative code means killed by signal (-sig)
                                Ok(std::process::ExitStatus::from_raw((-code) & 0x7f))
                            }
                        } else {
                            warn!(
                                "daemon pid {child_pid} wait() got ECHILD but no \
                                 stashed status found; reporting as error"
                            );
                            result
                        }
                    }
                    _ => result,
                };
                debug!("daemon pid {child_pid} wait() completed with result: {result:?}");
                let _ = exit_tx.send(result).await;
            });

            #[allow(unused_assignments)]
            // Initial None is a safety net; loop only exits via exit_rx.recv() which sets it
            let mut exit_status = None;

            // If there is no ready check of any kind and no delay, the daemon is
            // considered immediately ready and the active_port detection task would
            // never be triggered inside the select loop.  Kick it off right away so
            // that daemons without any readiness configuration still get their
            // active_port populated (needed for proxy routing).
            if has_port_config
                && ready_pattern.is_none()
                && ready_http.is_none()
                && ready_port.is_none()
                && ready_cmd.is_none()
                && delay_timer.is_none()
            {
                active_port_spawned = true;
                detect_and_store_active_port(id.clone(), daemon_pid);
            }

            loop {
                select! {
                    Some(line) = output_rx.recv() => {
                        let formatted = format_line(line.clone());
                        if let Err(e) = log_appender.write_all(formatted.as_bytes()).await {
                            error!("Failed to write to log for daemon {id}: {e}");
                        }
                        trace!("output: {id} {formatted}");

                        // Strip ANSI for pattern matching so user-written patterns
                        // work regardless of whether the process emits color codes.
                        let line_clean = console::strip_ansi_codes(&line).to_string();

                        // Check if output matches ready pattern
                        if !ready_notified
                            && let Some(ref pattern) = ready_pattern
                            && pattern.is_match(&line_clean)
                        {
                            info!("daemon {id} ready: output matched pattern");
                            ready_notified = true;
                            let _ = log_appender.flush().await;
                            if let Some(tx) = ready_tx.take() {
                                let _ = tx.send(Ok(()));
                            }
                            fire_hook(HookType::OnReady, id.clone(), daemon_dir.clone(), hook_retry_count, hook_daemon_env.clone(), vec![]).await;
                            if !active_port_spawned && has_port_config {
                                active_port_spawned = true;
                                detect_and_store_active_port(id.clone(), daemon_pid);
                            }
                        }

                        // Check on_output hook
                        if let Some(ref hook) = on_output_hook {
                            let matched = match (&hook.filter, &on_output_pattern) {
                                (Some(substr), _) => line_clean.contains(substr.as_str()),
                                (None, Some(re)) => re.is_match(&line_clean),
                                (None, None) => true,
                            };
                            if matched {
                                let now = std::time::Instant::now();
                                let elapsed = on_output_last_fired.map(|t| now.duration_since(t));
                                if elapsed.is_none_or(|e| e >= on_output_debounce) {
                                    on_output_last_fired = Some(now);
                                    hooks::fire_output_hook(id.clone(), daemon_dir.clone(), hook_retry_count, hook_daemon_env.clone(), hook.run.clone(), line_clean.clone()).await;
                                }
                            }
                        }
                    }
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
                    },
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
                                    let _ = log_appender.flush().await;
                                    if let Some(tx) = ready_tx.take() {
                                        let _ = tx.send(Ok(()));
                                    }
                                    fire_hook(HookType::OnReady, id.clone(), daemon_dir.clone(), hook_retry_count, hook_daemon_env.clone(), vec![]).await;
                                    http_check_interval = None;
                                    if !active_port_spawned && has_port_config {
                                        active_port_spawned = true;
                                        detect_and_store_active_port(id.clone(), daemon_pid);
                                    }
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
                                    if !active_port_spawned && has_port_config {
                                        active_port_spawned = true;
                                        detect_and_store_active_port(id.clone(), daemon_pid);
                                    }
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
                                    if !active_port_spawned && has_port_config {
                                        active_port_spawned = true;
                                        detect_and_store_active_port(id.clone(), daemon_pid);
                                    }
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
                        if !active_port_spawned && has_port_config {
                            active_port_spawned = true;
                            detect_and_store_active_port(id.clone(), daemon_pid);
                        }
                    }
                    _ = log_flush_interval.tick() => {
                        // Periodic flush to ensure logs are written to disk
                        if let Err(e) = log_appender.flush().await {
                            error!("Failed to flush log for daemon {id}: {e}");
                        }
                    }
                }
            }

            // Final flush to ensure all buffered logs are written
            if let Err(e) = log_appender.flush().await {
                error!("Failed to final flush log for daemon {id}: {e}");
            }

            // Clear active_port since the process is no longer running
            {
                let mut state_file = SUPERVISOR.state_file.lock().await;
                if let Some(d) = state_file.daemons.get_mut(&id) {
                    d.active_port = None;
                }
                if let Err(e) = state_file.write() {
                    debug!("Failed to write state after clearing active_port for {id}: {e}");
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
                _ if hook_retry_count >= hook_retry.count() => {
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
                    let stop_cfg = daemon.stop_signal.unwrap_or_default();
                    let stop_signal: i32 = stop_cfg.signal.into();
                    if let Err(e) = PROCS
                        .kill_process_group_async(pid, stop_signal, stop_cfg.timeout)
                        .await
                    {
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

#[cfg(unix)]
fn resolve_effective_run_identity(daemon_user: Option<&str>) -> Result<RunIdentity> {
    let settings_user = settings().supervisor.user.trim();
    let daemon_user = daemon_user.map(str::trim).filter(|user| !user.is_empty());
    let settings_user = (!settings_user.is_empty()).then_some(settings_user);
    let configured = daemon_user.or(settings_user);
    let current_uid = nix::unistd::Uid::effective().as_raw();
    let current_gid = nix::unistd::Gid::effective().as_raw();
    resolve_run_identity(
        configured,
        current_uid,
        current_gid,
        std::env::var("SUDO_UID").ok().as_deref(),
        std::env::var("SUDO_GID").ok().as_deref(),
    )
}

#[cfg(unix)]
fn resolve_run_identity(
    configured: Option<&str>,
    current_uid: u32,
    current_gid: u32,
    sudo_uid: Option<&str>,
    sudo_gid: Option<&str>,
) -> Result<RunIdentity> {
    let current_uid = nix::unistd::Uid::from_raw(current_uid);
    let current_gid = nix::unistd::Gid::from_raw(current_gid);
    if let Some(user) = configured {
        let identity = resolve_configured_user(user)?;
        ensure_can_use_identity(user, &identity, current_uid, current_gid)?;
        if identity.matches(current_uid, current_gid) {
            return Ok(RunIdentity::Inherit);
        }
        return Ok(identity);
    }

    if current_uid.is_root()
        && let Some(identity) = resolve_sudo_identity(sudo_uid, sudo_gid)
    {
        return Ok(identity);
    }

    Ok(RunIdentity::Inherit)
}

#[cfg(unix)]
fn resolve_configured_user(user: &str) -> Result<RunIdentity> {
    if user.chars().all(|c| c.is_ascii_digit()) {
        let uid = user
            .parse::<u32>()
            .map_err(|e| miette::miette!("invalid run user UID '{}': {}", user, e))?;
        let user_record = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
            .into_diagnostic()?
            .ok_or_else(|| miette::miette!("run user UID '{}' does not exist", user))?;
        return run_identity_from_user_record(user_record);
    }

    let user_record = nix::unistd::User::from_name(user)
        .into_diagnostic()?
        .ok_or_else(|| miette::miette!("run user '{}' does not exist", user))?;
    run_identity_from_user_record(user_record)
}

#[cfg(unix)]
fn run_identity_from_user_record(user: nix::unistd::User) -> Result<RunIdentity> {
    let username = CString::new(user.name)
        .map_err(|e| miette::miette!("run user name contains an interior nul byte: {}", e))?;
    Ok(RunIdentity::Switch {
        uid: user.uid,
        gid: user.gid,
        username: Some(username),
    })
}

#[cfg(unix)]
fn run_identity_from_raw_ids(uid: u32, gid: u32, username: Option<CString>) -> RunIdentity {
    RunIdentity::Switch {
        uid: nix::unistd::Uid::from_raw(uid),
        gid: nix::unistd::Gid::from_raw(gid),
        username,
    }
}

#[cfg(unix)]
fn resolve_sudo_identity(sudo_uid: Option<&str>, sudo_gid: Option<&str>) -> Option<RunIdentity> {
    let uid = sudo_uid?.parse::<u32>().ok()?;
    let gid = sudo_gid?.parse::<u32>().ok()?;
    let username = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
        .ok()
        .flatten()
        .and_then(|u| CString::new(u.name).ok());
    Some(run_identity_from_raw_ids(uid, gid, username))
}

#[cfg(unix)]
fn ensure_can_use_identity(
    configured_user: &str,
    identity: &RunIdentity,
    current_uid: nix::unistd::Uid,
    current_gid: nix::unistd::Gid,
) -> Result<()> {
    let RunIdentity::Switch { uid, gid, .. } = identity else {
        return Ok(());
    };
    if *uid == current_uid && *gid == current_gid {
        return Ok(());
    }
    if current_uid.is_root() {
        return Ok(());
    }
    Err(miette::miette!(
        "daemon is configured to run as '{}', but the supervisor is running as uid={} gid={}. Restart the supervisor with sudo to switch to uid={} gid={}, or choose a user matching the supervisor.",
        configured_user,
        current_uid.as_raw(),
        current_gid.as_raw(),
        uid.as_raw(),
        gid.as_raw()
    ))
}

#[cfg(unix)]
fn apply_run_identity(identity: &RunIdentity) -> std::io::Result<()> {
    let RunIdentity::Switch { uid, gid, username } = identity else {
        return Ok(());
    };
    if let Some(username) = username {
        initgroups_for_user(username, *gid)?;
    } else {
        setgroups_to_primary(*gid)?;
    }
    nix::unistd::setgid(*gid).map_err(nix_to_io_error)?;
    nix::unistd::setuid(*uid).map_err(nix_to_io_error)?;
    Ok(())
}

#[cfg(unix)]
impl RunIdentity {
    fn matches(&self, uid: nix::unistd::Uid, gid: nix::unistd::Gid) -> bool {
        matches!(self, RunIdentity::Switch { uid: u, gid: g, .. } if *u == uid && *g == gid)
    }
}

#[cfg(unix)]
fn setgroups_to_primary(gid: nix::unistd::Gid) -> std::io::Result<()> {
    let groups = [gid.as_raw() as libc::gid_t];
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let group_count = groups.len();
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let group_count = groups.len() as libc::c_int;
    let rc = unsafe { libc::setgroups(group_count, groups.as_ptr()) };
    if rc == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn initgroups_for_user(username: &CString, gid: nix::unistd::Gid) -> std::io::Result<()> {
    let gid = gid.as_raw();
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "watchos"
    ))]
    let base_gid = i32::try_from(gid)
        .map_err(|_| std::io::Error::other(format!("gid {gid} is out of range")))?;

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "watchos"
    )))]
    let base_gid = gid as libc::gid_t;

    // SAFETY: `username` is a valid nul-terminated C string and `base_gid`
    // is derived from a resolved system account or sudo-provided gid.
    let rc = unsafe { libc::initgroups(username.as_ptr(), base_gid) };
    if rc == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn nix_to_io_error(err: nix::errno::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(err as i32)
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

            // Use spawn_blocking to avoid blocking the async runtime during TCP bind checks.
            //
            // We check multiple addresses to avoid false-negatives caused by SO_REUSEADDR.
            // On macOS/BSD, Rust's TcpListener::bind sets SO_REUSEADDR by default, which
            // allows binding 0.0.0.0:port even when 127.0.0.1:port is already in use
            // (because 0.0.0.0 is technically a different address).  Most daemons bind
            // to localhost, so checking 127.0.0.1 is essential to detect real conflicts.
            // We also check [::1] to cover IPv6 loopback listeners.
            //
            // NOTE: This check has a time-of-check-to-time-of-use (TOCTOU) race condition.
            // Another process could grab the port between our check and the daemon actually
            // binding. This is inherent to the approach and acceptable for our use case
            // since we're primarily detecting conflicts with already-running daemons.
            if is_port_in_use(port).await {
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
                info!("ports {expected_ports:?} bumped by {bump_offset} to {candidate_ports:?}");
            }
            return Ok(candidate_ports);
        }

        // Port is in use
        if bump_offset == 0 && !auto_bump {
            if let Some(port) = conflicting_port {
                let (pid, process) = identify_port_owner(port).await;
                return Err(PortError::InUse { port, process, pid }.into());
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

/// Check whether a port is currently in use by attempting to bind on multiple addresses.
///
/// Returns `true` when at least one bind attempt gets `AddrInUse`, meaning another
/// process is listening.  Other errors (e.g. `AddrNotAvailable` on an address family
/// the OS doesn't support) are ignored so they don't produce false positives.
async fn is_port_in_use(port: u16) -> bool {
    tokio::task::spawn_blocking(move || {
        for &addr in &["0.0.0.0", "127.0.0.1", "::1"] {
            match std::net::TcpListener::bind((addr, port)) {
                Ok(listener) => drop(listener),
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return true,
                Err(_) => continue,
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

/// Best-effort lookup of the process occupying a port via `listeners::get_all()`.
///
/// Returns `(pid, process_name)`.  Falls back to `(0, "unknown")` when the
/// system call fails (permission error, unsupported OS, etc.).
async fn identify_port_owner(port: u16) -> (u32, String) {
    tokio::task::spawn_blocking(move || {
        listeners::get_all()
            .ok()
            .and_then(|list| {
                list.into_iter()
                    .find(|l| l.socket.port() == port)
                    .map(|l| (l.process.pid, l.process.name))
            })
            .unwrap_or((0, "unknown".to_string()))
    })
    .await
    .unwrap_or((0, "unknown".to_string()))
}

/// Detect whether a port is in use, and if so, identify the owning process.
///
/// Combines `is_port_in_use` (reliable bind probe) with `identify_port_owner`
/// (best-effort process lookup).  Returns `None` when the port is free.
async fn detect_port_conflict(port: u16) -> Option<(u32, String)> {
    if !is_port_in_use(port).await {
        return None;
    }
    Some(identify_port_owner(port).await)
}

/// Spawn a background task that detects the first port the daemon process is listening on
/// and stores it in the state file as `active_port`.
///
/// This is called once when the daemon becomes ready. The port is cleared when the daemon stops.
///
/// Port selection strategy:
/// 1. If the daemon has `expected_port` configured, prefer the first port from that list
///    (it is the port the operator explicitly designated as the primary service port).
/// 2. Otherwise, take the first port the process is actually listening on (in the order
///    returned by the OS), which is typically the port bound earliest.
///
/// Using `min()` (lowest port number) was previously used here but is incorrect: many
/// applications listen on multiple ports (e.g. HTTP + metrics) and the lowest-numbered
/// port is not necessarily the primary service port.
fn detect_and_store_active_port(id: DaemonId, pid: u32) {
    tokio::spawn(async move {
        // Retry with exponential backoff so that slow-starting daemons (JVM,
        // Node.js, Python, etc.) that take more than 500 ms to bind their port
        // are still detected.  Total wait budget: 500+1000+2000+4000 = 7.5 s.
        for delay_ms in [500u64, 1000, 2000, 4000] {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;

            // Read daemon state atomically: check if still alive and get expected_port
            // in a single lock acquisition to avoid TOCTOU and unnecessary lock overhead.
            let expected_port: Option<u16> = {
                let state_file = SUPERVISOR.state_file.lock().await;
                match state_file.daemons.get(&id) {
                    Some(d) if d.pid.is_none() => {
                        debug!("daemon {id}: aborting active_port detection — process exited");
                        return;
                    }
                    Some(d) => d
                        .port
                        .as_ref()
                        .and_then(|p| p.expect.first().copied())
                        .filter(|&p| p > 0),
                    None => None,
                }
            };

            let active_port = tokio::task::spawn_blocking(move || {
                let listeners = listeners::get_all().ok()?;
                let process_ports: Vec<u16> = listeners
                    .into_iter()
                    .filter(|listener| listener.process.pid == pid)
                    .map(|listener| listener.socket.port())
                    .filter(|&port| port > 0)
                    .collect();

                if process_ports.is_empty() {
                    return None;
                }

                // Prefer the configured expected_port if the process is actually
                // listening on it; otherwise fall back to the first port found.
                if let Some(ep) = expected_port {
                    if process_ports.contains(&ep) {
                        return Some(ep);
                    }
                }

                // No expected_port match — return the first port in the list.
                // The list order reflects the order the OS reports listeners,
                // which is generally the order they were bound (earliest first).
                // Do NOT sort: the lowest-numbered port is not necessarily the
                // primary service port (e.g. HTTP vs metrics).
                process_ports.into_iter().next()
            })
            .await
            .ok()
            .flatten();

            if let Some(port) = active_port {
                debug!("daemon {id} active_port detected: {port}");
                let mut state_file = SUPERVISOR.state_file.lock().await;
                if let Some(d) = state_file.daemons.get_mut(&id) {
                    // Guard against PID reuse: if the original process exited and the OS
                    // assigned the same PID to an unrelated process that happens to bind
                    // a port, we must not route proxy traffic to that unrelated service.
                    if d.pid == Some(pid) {
                        d.active_port = Some(port);
                    } else {
                        debug!(
                            "daemon {id}: skipping active_port write — PID mismatch \
                             (expected {pid}, current {:?})",
                            d.pid
                        );
                        return;
                    }
                }
                if let Err(e) = state_file.write() {
                    debug!("Failed to write state after detecting active_port for {id}: {e}");
                }
                return;
            }

            debug!("daemon {id}: no active port detected for pid {pid} (will retry)");
        }

        debug!("daemon {id}: active port detection exhausted all retries for pid {pid}");
    });
}

/// Check whether a daemon (by its qualified ID) is the target of any registered
/// slug in the global config.  This is used to decide whether to run the
/// `detect_and_store_active_port` polling task — only slug-targeted daemons need
/// it, avoiding wasted `listeners::get_all()` calls for port-less daemons.
///
/// Delegates to `proxy::server::is_slug_target()` which uses the same in-memory
/// slug cache as the proxy hot path, so this check is cheap.
fn is_daemon_slug_target(id: &DaemonId) -> bool {
    // read_global_slugs is called once per daemon start — acceptable cost.
    // We intentionally avoid making this async to keep has_port_config evaluation
    // simple and synchronous in run_once().
    let slugs = crate::pitchfork_toml::PitchforkToml::read_global_slugs();
    slugs.iter().any(|(slug, entry)| {
        let daemon_name = entry.daemon.as_deref().unwrap_or(slug);
        id.name() == daemon_name
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_run_identity_empty_without_sudo() {
        let identity = resolve_run_identity(None, 501, 20, None, None).unwrap();
        assert_eq!(identity, RunIdentity::Inherit);
    }

    #[test]
    fn test_resolve_run_identity_sudo_fallback() {
        let identity = resolve_run_identity(None, 0, 0, Some("501"), Some("20")).unwrap();
        let RunIdentity::Switch { uid, gid, .. } = identity else {
            panic!("expected identity switch");
        };
        assert_eq!(uid.as_raw(), 501);
        assert_eq!(gid.as_raw(), 20);
    }

    #[test]
    fn test_resolve_run_identity_ignores_stale_sudo_when_not_root() {
        let identity = resolve_run_identity(None, 501, 20, Some("0"), Some("0")).unwrap();
        assert_eq!(identity, RunIdentity::Inherit);
    }

    #[test]
    fn test_resolve_configured_user_root_name() {
        let identity = resolve_configured_user("root").unwrap();
        let RunIdentity::Switch { uid, username, .. } = identity else {
            panic!("expected identity switch");
        };
        assert_eq!(uid.as_raw(), 0);
        assert_eq!(
            username.as_deref().and_then(|s| s.to_str().ok()),
            Some("root")
        );
    }

    #[test]
    fn test_resolve_configured_user_root_uid() {
        let identity = resolve_configured_user("0").unwrap();
        let RunIdentity::Switch { uid, username, .. } = identity else {
            panic!("expected identity switch");
        };
        assert_eq!(uid.as_raw(), 0);
        assert_eq!(
            username.as_deref().and_then(|s| s.to_str().ok()),
            Some("root")
        );
    }

    #[test]
    fn test_resolve_configured_user_missing_user_fails() {
        let err = resolve_configured_user("pitchfork-user-that-should-not-exist")
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn test_resolve_run_identity_requires_root_for_user_switch() {
        let err = resolve_run_identity(Some("root"), 501, 20, None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Restart the supervisor with sudo"));
    }

    #[test]
    fn test_resolve_run_identity_same_user_is_noop() {
        let identity = resolve_run_identity(Some("root"), 0, 0, Some("501"), Some("20")).unwrap();
        assert_eq!(identity, RunIdentity::Inherit);
    }
}

/// Inject proxy-related environment variables into a daemon's command.
///
/// Adds:
/// - `HOST` — the address the daemon should bind to (`127.0.0.1`, omitted in LAN mode)
/// - `PITCHFORK_URL` — the public proxy URL for this daemon (if it has a slug)
/// - `NODE_EXTRA_CA_CERTS` — path to the pitchfork CA cert (if HTTPS enabled)
/// - `__VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS` — `.<tld>` for Vite host allowlisting
/// - `PITCHFORK_LAN` — set to `"1"` when LAN mode is active
fn inject_proxy_env(cmd: &mut tokio::process::Command, slug: &Option<String>) {
    let s = crate::settings::settings();
    let lan_enabled = s.proxy.lan || !s.proxy.lan_ip.is_empty();

    if should_force_loopback_host(slug) && !lan_enabled {
        // Only force loopback binding for daemons that are actually routed via a slug.
        // In LAN mode, daemons need to bind to 0.0.0.0 to be reachable from the network.
        cmd.env("HOST", "127.0.0.1");
    }

    // PITCHFORK_URL: the daemon's public proxy URL (only if it has a slug and proxy is enabled)
    if let Some(url) = build_pitchfork_url(slug, s) {
        cmd.env("PITCHFORK_URL", &url);
    }

    // NODE_EXTRA_CA_CERTS: let Node.js backends trust the pitchfork CA
    if s.proxy.enable && s.proxy.https {
        let ca_path = if s.proxy.tls_cert.is_empty() {
            crate::env::PITCHFORK_STATE_DIR.join("proxy").join("ca.pem")
        } else {
            std::path::PathBuf::from(&s.proxy.tls_cert)
        };
        if ca_path.exists() {
            cmd.env("NODE_EXTRA_CA_CERTS", ca_path.to_string_lossy().to_string());
        }
    }

    // __VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS: Vite host allowlisting
    if s.proxy.enable {
        let tld = if lan_enabled { "local" } else { &s.proxy.tld };
        cmd.env("__VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS", format!(".{tld}"));
    }

    // PITCHFORK_LAN: signal to daemons that LAN mode is active
    if lan_enabled {
        cmd.env("PITCHFORK_LAN", "1");
    }
}

fn should_force_loopback_host(slug: &Option<String>) -> bool {
    let Some(slug) = slug.as_deref() else {
        return false;
    };

    let s = crate::settings::settings();
    if !s.proxy.enable {
        return false;
    }

    let slugs = crate::pitchfork_toml::PitchforkToml::read_global_slugs();
    slugs.contains_key(slug)
}

/// Compute the public proxy URL for a daemon.
///
/// Returns `None` if the daemon has no slug or the proxy is not enabled.
fn build_pitchfork_url(slug: &Option<String>, s: &crate::settings::Settings) -> Option<String> {
    let slug = slug.as_ref()?;
    if !s.proxy.enable {
        return None;
    }
    let scheme = if s.proxy.https { "https" } else { "http" };
    let port = u16::try_from(s.proxy.port).ok().filter(|&p| p > 0)?;
    let port_suffix = if (scheme == "https" && port == 443) || (scheme == "http" && port == 80) {
        String::new()
    } else {
        format!(":{port}")
    };
    let lan_enabled = s.proxy.lan || !s.proxy.lan_ip.is_empty();
    let tld = if lan_enabled { "local" } else { &s.proxy.tld };
    Some(format!("{scheme}://{slug}.{tld}{port_suffix}",))
}
