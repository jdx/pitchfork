//! Supervisor module - daemon process supervisor
//!
//! This module is split into focused submodules:
//! - `state`: State access layer (get/set operations)
//! - `lifecycle`: Daemon start/stop operations
//! - `autostop`: Autostop logic and boot daemon startup
//! - `retry`: Retry logic with backoff
//! - `watchers`: Background tasks (interval, cron, file watching)
//! - `ipc_handlers`: IPC request dispatch

mod autostop;
mod hooks;
mod ipc_handlers;
mod lifecycle;
mod retry;
mod state;
mod watchers;

use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::deps::compute_reverse_stop_order;
use crate::ipc::server::{IpcServer, IpcServerHandle};

use crate::procs::PROCS;
use crate::settings::settings;
use crate::state_file::StateFile;
use crate::{Result, env};
use duct::cmd;
use miette::IntoDiagnostic;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::exit;
use std::sync::atomic;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::time::Duration;
#[cfg(unix)]
use tokio::signal::unix::SignalKind;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tokio::{signal, time};

/// Exit statuses reaped by the container-mode zombie reaper for managed daemon
/// PIDs. On non-Linux Unix platforms where `waitid(WNOWAIT)` is unavailable,
/// `waitpid(None, WNOHANG)` may race with Tokio's `child.wait()`. When the
/// zombie reaper wins, the exit status is stashed here so the monitoring task
/// in lifecycle.rs can recover it instead of treating the ECHILD as a failure.
///
/// On Linux this map is unused because the reaper uses `waitid` with `WNOWAIT`
/// to peek before reaping, which avoids the race entirely.
#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) static REAPED_STATUSES: Lazy<Mutex<HashMap<u32, i32>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

// Re-export types needed by other modules
pub(crate) use state::UpsertDaemonOpts;

pub struct Supervisor {
    pub(crate) state_file: Mutex<StateFile>,
    pub(crate) pending_notifications: Mutex<Vec<(log::LevelFilter, String)>>,
    pub(crate) last_refreshed_at: Mutex<time::Instant>,
    /// Map of daemon ID to scheduled autostop time
    pub(crate) pending_autostops: Mutex<HashMap<DaemonId, time::Instant>>,
    /// Handle for graceful IPC server shutdown
    pub(crate) ipc_shutdown: Mutex<Option<IpcServerHandle>>,
    /// Tracks in-flight hook tasks so shutdown can wait for them to complete
    pub(crate) hook_tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Number of monitoring tasks that are still running (between process exit
    /// and hook registration completion). Used by `close()` to know when it is
    /// safe to drain `hook_tasks`.
    pub(crate) active_monitors: AtomicU32,
    /// Signalled by each monitoring task after it finishes registering hooks
    /// (or decides it has nothing to register). `close()` waits on this.
    pub(crate) monitor_done: Notify,
}

pub(crate) fn interval_duration() -> Duration {
    settings().general_interval()
}

pub static SUPERVISOR: Lazy<Supervisor> =
    Lazy::new(|| Supervisor::new().expect("Error creating supervisor"));

pub fn start_if_not_running() -> Result<()> {
    let sf = StateFile::get();
    if let Some(d) = sf.daemons.get(&DaemonId::pitchfork())
        && let Some(pid) = d.pid
        && PROCS.is_running(pid)
    {
        return Ok(());
    }
    start_in_background()
}

pub fn start_in_background() -> Result<()> {
    debug!("starting supervisor in background");
    // Ensure the log directory exists so we can redirect stderr there.
    // Panics and other fatal errors from the background supervisor process
    // would otherwise be silently swallowed.
    let log_file = &*env::PITCHFORK_LOG_FILE;
    if let Some(parent) = log_file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let stderr_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
        .into_diagnostic()?;
    #[cfg(unix)]
    fix_state_dir_permissions();
    cmd!(&*env::PITCHFORK_BIN, "supervisor", "run")
        .stdout_null()
        .stderr_file(stderr_file)
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
            ipc_shutdown: Mutex::new(None),
            hook_tasks: Mutex::new(Vec::new()),
            active_monitors: AtomicU32::new(0),
            monitor_done: Notify::new(),
        })
    }

    pub async fn start(
        &self,
        is_boot: bool,
        container: bool,
        web_port: Option<u16>,
        web_path: Option<String>,
    ) -> Result<()> {
        // Ensure the state directory and its contents are accessible by non-root
        // users. This is needed when the supervisor is started with `sudo` — all
        // files it creates are owned by root, which prevents normal CLI clients
        // from reading/writing state or connecting to the IPC socket.
        #[cfg(unix)]
        fix_state_dir_permissions();

        let pid = std::process::id();
        // Determine container mode: CLI flag takes priority, then settings
        let container_mode = container || settings().supervisor.container;
        if container_mode {
            info!("Starting supervisor in container/PID1 mode with pid {pid}");
        } else {
            info!("Starting supervisor with pid {pid}");
        }

        self.upsert_daemon(
            UpsertDaemonOpts::builder(DaemonId::pitchfork())
                .set(|o| {
                    o.pid = Some(pid);
                    o.status = DaemonStatus::Running;
                })
                .build(),
        )
        .await?;
        #[cfg(unix)]
        fix_state_dir_permissions();

        // If this is a boot start, automatically start boot_start daemons
        if is_boot {
            info!("Boot start mode enabled, starting boot_start daemons");
            self.start_boot_daemons().await?;
        }

        self.interval_watch()?;
        self.cron_watch()?;
        self.signals()?;
        self.daemon_file_watch()?;

        // In container mode, install SIGCHLD handler to reap orphaned/zombie processes
        #[cfg(unix)]
        if container_mode {
            self.reap_zombies()?;
        }

        // Start web server: CLI --web-port takes priority, then settings.web.auto_start + bind_port
        let s = settings();
        let effective_port = web_port.or_else(|| {
            if s.web.auto_start {
                match u16::try_from(s.web.bind_port).ok().filter(|&p| p > 0) {
                    Some(p) => Some(p),
                    None => {
                        error!(
                            "web.bind_port {} is out of valid port range (1-65535), web UI disabled",
                            s.web.bind_port
                        );
                        None
                    }
                }
            } else {
                None
            }
        });
        // CLI --web-path takes priority, then settings.web.base_path
        let effective_path = web_path.or_else(|| {
            let bp = s.web.base_path.clone();
            if bp.is_empty() { None } else { Some(bp) }
        });
        if let Some(port) = effective_port {
            tokio::spawn(async move {
                if let Err(e) = crate::web::serve(port, effective_path).await {
                    error!("Web server error: {e}");
                }
            });
        }

        // Start reverse proxy server if enabled
        if s.proxy.enable {
            // Pre-generate the TLS certificate synchronously before spawning the proxy
            // task. This ensures the cert exists immediately after `sup start` returns,
            // so `proxy trust` can be run right away without waiting for the async task.
            #[cfg(feature = "proxy-tls")]
            if s.proxy.https {
                let proxy_dir = crate::env::PITCHFORK_STATE_DIR.join("proxy");
                let ca_cert_path = proxy_dir.join("ca.pem");
                let ca_key_path = proxy_dir.join("ca-key.pem");
                if !ca_cert_path.exists() || !ca_key_path.exists() {
                    match crate::proxy::server::generate_ca(&ca_cert_path, &ca_key_path) {
                        Ok(()) => {
                            info!(
                                "Generated local CA certificate at {}",
                                ca_cert_path.display()
                            );
                            info!("To trust the CA in your browser, run: pitchfork proxy trust");
                        }
                        Err(e) => {
                            error!("Failed to generate CA certificate: {e}");
                        }
                    }
                }
            }
            // Spawn the proxy server and wait for its bind result via a oneshot
            // channel.  This avoids the TOCTOU race of a pre-flight bind check
            // while still surfacing binding failures immediately.
            let (bind_tx, bind_rx) = tokio::sync::oneshot::channel();
            tokio::spawn(async {
                if let Err(e) = crate::proxy::server::serve(bind_tx).await {
                    error!("Proxy server error: {e}");
                }
            });
            match bind_rx.await {
                Ok(Ok(())) => {
                    // Proxy bound successfully — nothing to do.
                }
                Ok(Err(msg)) => {
                    error!("{msg}");
                    self.add_notification(log::LevelFilter::Error, msg).await;
                }
                Err(_) => {
                    // Sender dropped without sending — serve() panicked or
                    // returned before signalling.  Already logged by the
                    // spawn error handler above.
                }
            }
        }

        let (ipc, ipc_handle) = IpcServer::new()?;
        *self.ipc_shutdown.lock().await = Some(ipc_handle);
        self.conn_watch(ipc).await
    }

    pub(crate) async fn refresh(&self) -> Result<()> {
        trace!("refreshing");

        // Collect PIDs we need to check (shell PIDs only)
        // This is more efficient than refreshing all processes on the system
        let dirs_with_pids = self.get_dirs_with_shell_pids().await;
        let pids_to_check: Vec<u32> = dirs_with_pids.values().flatten().copied().collect();

        if pids_to_check.is_empty() {
            // No PIDs to check, skip the expensive refresh
            trace!("no shell PIDs to check, skipping process refresh");
        } else {
            PROCS.refresh_pids(&pids_to_check);
        }

        let mut last_refreshed_at = self.last_refreshed_at.lock().await;
        *last_refreshed_at = time::Instant::now();

        for (dir, pids) in dirs_with_pids {
            let to_remove = pids
                .iter()
                .filter(|pid| !PROCS.is_running(**pid))
                .collect::<Vec<_>>();
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

    /// Install a SIGCHLD handler that reaps orphaned zombie child processes.
    ///
    /// When running as PID 1 inside a container, orphaned processes are
    /// re-parented to PID 1. Without explicit reaping, they accumulate
    /// as zombies in the process table indefinitely.
    ///
    /// Only reaps processes that are NOT managed by the supervisor (i.e.
    /// not tracked in the state file). Managed daemon processes are reaped
    /// by their monitoring tasks via `child.wait()`.
    ///
    /// ## Strategy
    ///
    /// **Linux**: Uses `waitid(Id::All, WNOHANG | WNOWAIT | WEXITED)` to
    /// *peek* at the next zombie without consuming its status. If the PID
    /// belongs to a managed daemon, the reaper skips it so Tokio's
    /// `child.wait()` can collect the status normally. Only unmanaged
    /// orphans are actually reaped (via `waitpid(Pid, WNOHANG)`). This
    /// eliminates the race entirely.
    ///
    /// **Non-Linux Unix** (e.g. macOS — mainly for local development;
    /// container mode targets Linux): `waitid` is unavailable, so we fall
    /// back to `waitpid(None, WNOHANG)`. If the reaper accidentally
    /// consumes a managed PID's status, it stashes the exit code in
    /// [`REAPED_STATUSES`] for the monitoring task to recover.
    #[cfg(unix)]
    fn reap_zombies(&self) -> Result<()> {
        let mut stream = signal::unix::signal(SignalKind::child())
            .map_err(|e| miette::miette!("Failed to register SIGCHLD handler: {e}"))?;
        tokio::spawn(async move {
            loop {
                stream.recv().await;
                // Collect PIDs of managed daemons so we don't steal their exit status
                let managed_pids: HashSet<u32> = SUPERVISOR
                    .state_file
                    .lock()
                    .await
                    .daemons
                    .values()
                    .filter_map(|d| d.pid)
                    .collect();
                // Reap all available zombie children that are NOT managed
                Self::reap_unmanaged_zombies(&managed_pids).await;
            }
        });
        info!("container mode: SIGCHLD zombie reaper installed");
        Ok(())
    }

    /// Linux implementation: peek with `waitid(WNOWAIT)` then selectively reap.
    ///
    /// `WNOWAIT` leaves the zombie in the table so we can inspect its PID
    /// without consuming the exit status. Only if the PID is *not* managed
    /// do we call `waitpid(Pid, WNOHANG)` to actually reap it.
    #[cfg(target_os = "linux")]
    async fn reap_unmanaged_zombies(managed_pids: &HashSet<u32>) {
        use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid, waitpid};
        use nix::unistd::Pid;

        loop {
            // Peek at the next zombie without consuming it
            let peek_flags = WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT | WaitPidFlag::WEXITED;
            match waitid(Id::All, peek_flags) {
                Ok(WaitStatus::StillAlive) => break,
                Ok(status) => {
                    let Some(pid_raw) = status.pid().map(|p| p.as_raw() as u32) else {
                        break;
                    };
                    if managed_pids.contains(&pid_raw) {
                        // This is a managed daemon — leave it for Tokio's child.wait().
                        // We must break out of the loop because waitid(Id::All) would
                        // keep returning the same zombie if we don't consume it.
                        trace!(
                            "zombie reaper: skipping managed daemon pid {pid_raw}, \
                             leaving for Tokio to reap"
                        );
                        break;
                    }
                    // Not managed — actually reap it
                    match waitpid(Pid::from_raw(pid_raw as i32), Some(WaitPidFlag::WNOHANG)) {
                        Ok(s) => trace!("reaped orphaned zombie child: {s:?}"),
                        Err(nix::errno::Errno::ECHILD) => break,
                        Err(e) => {
                            trace!("waitpid error reaping pid {pid_raw}: {e}");
                            break;
                        }
                    }
                }
                Err(nix::errno::Errno::ECHILD) => break, // no children at all
                Err(e) => {
                    trace!("waitid error in zombie reaper: {e}");
                    break;
                }
            }
        }
    }

    /// Non-Linux fallback: blind `waitpid(None, WNOHANG)` with stash recovery.
    ///
    /// Since `waitid(WNOWAIT)` is not available, we cannot peek. If we
    /// accidentally reap a managed PID, we stash the exit code in
    /// [`REAPED_STATUSES`] so the monitoring task can recover it.
    #[cfg(all(unix, not(target_os = "linux")))]
    async fn reap_unmanaged_zombies(managed_pids: &HashSet<u32>) {
        use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};

        loop {
            match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::StillAlive) => break,
                Ok(status) => {
                    let Some(pid) = status.pid().map(|p| p.as_raw() as u32) else {
                        continue;
                    };
                    if managed_pids.contains(&pid) {
                        // Race lost — stash the exit code for lifecycle recovery
                        let exit_code = match status {
                            WaitStatus::Exited(_, code) => code,
                            WaitStatus::Signaled(_, sig, _) => -(sig as i32),
                            _ => -1,
                        };
                        warn!(
                            "zombie reaper reaped managed daemon pid {pid} \
                             (exit_code={exit_code}); stashing status for recovery"
                        );
                        REAPED_STATUSES.lock().await.insert(pid, exit_code);
                    } else {
                        trace!("reaped orphaned zombie child: {status:?}");
                    }
                }
                Err(nix::errno::Errno::ECHILD) => break, // no more children
                Err(e) => {
                    trace!("waitpid error in zombie reaper: {e}");
                    break;
                }
            }
        }
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
                    warn!("Failed to register signal handler for {signal:?}: {e}");
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

    pub(crate) async fn close(&self) {
        let pitchfork_id = DaemonId::pitchfork();
        let active = self.active_daemons().await;
        let active_ids: Vec<DaemonId> = active
            .iter()
            .filter(|d| d.id != pitchfork_id)
            .map(|d| d.id.clone())
            .collect();

        // Stop daemons in reverse dependency order.
        // If dependency resolution fails (e.g. config changed), fall back to
        // stopping in arbitrary order so we still shut down cleanly.
        // Daemons within the same level are stopped concurrently.
        let stop_levels = compute_reverse_stop_order(&active_ids);
        for level in &stop_levels {
            let mut tasks = Vec::new();
            for id in level {
                let id = id.clone();
                tasks.push(tokio::spawn(async move {
                    if let Err(err) = SUPERVISOR.stop(&id).await {
                        error!("failed to stop daemon {id}: {err}");
                    }
                }));
            }
            for task in tasks {
                let _ = task.await;
            }
        }
        let _ = self.remove_daemon(&pitchfork_id).await;

        // Signal IPC server to shut down gracefully
        if let Some(mut handle) = self.ipc_shutdown.lock().await.take() {
            handle.shutdown();
        }

        // Wait for all in-flight monitoring tasks to finish registering their
        // hook handles. Each monitoring task increments `active_monitors` when
        // its process exits, and decrements it (+ notifies `monitor_done`)
        // after all fire_hook() calls complete. This replaces the old
        // yield_now() approach which had a race window.
        let drain_timeout = time::sleep(Duration::from_secs(5));
        tokio::pin!(drain_timeout);
        loop {
            if self.active_monitors.load(atomic::Ordering::Acquire) == 0 {
                break;
            }
            tokio::select! {
                _ = self.monitor_done.notified() => {}
                _ = &mut drain_timeout => {
                    warn!("timed out waiting for monitoring tasks to register hooks, proceeding with shutdown");
                    break;
                }
            }
        }
        let handles: Vec<JoinHandle<()>> = std::mem::take(&mut *self.hook_tasks.lock().await);
        let hook_timeout = Duration::from_secs(30);
        for handle in handles {
            match time::timeout(hook_timeout, handle).await {
                Ok(_) => {} // Hook completed (success or error, doesn't matter)
                Err(_) => {
                    warn!(
                        "hook task did not complete within {hook_timeout:?} during shutdown, skipping"
                    );
                }
            }
        }

        let _ = fs::remove_dir_all(&*env::IPC_SOCK_DIR);
    }

    pub(crate) async fn add_notification(&self, level: log::LevelFilter, message: String) {
        self.pending_notifications
            .lock()
            .await
            .push((level, message));
    }
}

/// Fix ownership on the state directory so non-root users can access files
/// created by a `sudo`-started supervisor.
///
/// When `[settings.supervisor] user` or `SUDO_UID`/`SUDO_GID` are set, we
/// `chown` the state directory and safe subdirectories back to that non-root
/// runtime user. This is strictly better than `chmod 0o666` because it does not
/// widen the permission bits — the files stay owner-only (0o600/0o700) but the
/// *owner* is the user that daemon processes and CLI clients need to share.
///
/// **Security**: The `proxy/` subtree is intentionally skipped. It contains
/// `ca-key.pem` which must remain `0o600` and owned by the process that
/// generated it. Changing its ownership or permissions would expose the CA
/// private key to other local users.
///
/// If neither `user` nor `SUDO_UID`/`SUDO_GID` are available (e.g. direct
/// root login), we fall back to relaxing permissions on only the `sock/` and
/// `logs/` subdirectories (plus `state.toml`) so CLI clients can still function.
#[cfg(unix)]
fn fix_state_dir_permissions() {
    let state_dir = &*env::PITCHFORK_STATE_DIR;
    if let Some((uid, gid)) = state_owner_ids() {
        if !state_dir.exists()
            && let Err(err) = fs::create_dir_all(state_dir)
        {
            warn!(
                "failed to create state directory for ownership fix at {}: {err}",
                state_dir.display()
            );
            return;
        }

        // Best path: chown back to the runtime user. Permissions stay tight.
        chown_recursive(state_dir, uid, gid, true);
        debug!(
            "chowned state directory to uid={uid} gid={gid} at {}",
            state_dir.display()
        );
    } else {
        if !state_dir.exists() {
            return;
        }

        // Fallback: relax permissions on safe subdirectories only.
        // proxy/ is never touched.
        chmod_safe_subtrees(state_dir);
        debug!(
            "relaxed permissions on safe subtrees at {}",
            state_dir.display()
        );
    }
}

#[cfg(unix)]
pub(crate) fn state_owner_ids() -> Option<(u32, u32)> {
    if !nix::unistd::Uid::effective().is_root() {
        return None;
    }

    let user = settings().supervisor.user.trim();
    if !user.is_empty() {
        return resolve_supervisor_user_ids(user).or_else(|| {
            warn!(
                "failed to resolve supervisor.user '{user}' for state ownership; falling back to SUDO_UID/SUDO_GID"
            );
            parse_sudo_ids()
        });
    }

    parse_sudo_ids()
}

#[cfg(unix)]
fn resolve_supervisor_user_ids(user: &str) -> Option<(u32, u32)> {
    let user_record = if user.chars().all(|c| c.is_ascii_digit()) {
        let uid = user.parse::<u32>().ok()?;
        nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
            .ok()
            .flatten()
    } else {
        nix::unistd::User::from_name(user).ok().flatten()
    }?;

    Some((user_record.uid.as_raw(), user_record.gid.as_raw()))
}

/// Parse `SUDO_UID` and `SUDO_GID` environment variables into numeric IDs.
///
/// Returns `None` unless the effective UID is 0 (root). This prevents stale
/// `SUDO_UID`/`SUDO_GID` values inherited into non-sudo environments from
/// triggering incorrect `chown` operations.
#[cfg(unix)]
fn parse_sudo_ids() -> Option<(u32, u32)> {
    if !nix::unistd::Uid::effective().is_root() {
        return None;
    }
    let uid: u32 = std::env::var("SUDO_UID").ok()?.parse().ok()?;
    let gid: u32 = std::env::var("SUDO_GID").ok()?.parse().ok()?;
    Some((uid, gid))
}

/// Recursively `chown` a directory tree. If `skip_proxy` is true, the `proxy/`
/// subdirectory is skipped entirely to protect the CA private key.
#[cfg(unix)]
fn chown_recursive(dir: &std::path::Path, uid: u32, gid: u32, skip_proxy: bool) {
    // chown the directory itself
    let _ = chown_path(dir, uid, gid);

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip proxy/ at the top level of the state directory
            if skip_proxy {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name == "proxy" {
                        continue;
                    }
                }
            }
            chown_recursive(&path, uid, gid, false);
        } else {
            let _ = chown_path(&path, uid, gid);
        }
    }
}

/// `chown` a single path using libc. Returns Ok(()) on success.
#[cfg(unix)]
fn chown_path(path: &std::path::Path, uid: u32, gid: u32) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let ret = unsafe { libc::chown(c_path.as_ptr(), uid, gid) };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Fallback: relax permissions on safe subdirectories only (sock/, logs/, and
/// state.toml). The proxy/ subtree is never touched.
#[cfg(unix)]
fn chmod_safe_subtrees(state_dir: &std::path::Path) {
    // The state directory itself needs to be traversable
    let _ = fs::set_permissions(state_dir, fs::Permissions::from_mode(0o755));

    // state.toml — needs to be readable by CLI clients
    let state_file = state_dir.join("state.toml");
    if state_file.exists() {
        let _ = fs::set_permissions(&state_file, fs::Permissions::from_mode(0o644));
    }

    // Safe subdirectories: sock/ and logs/
    for subdir_name in &["sock", "logs"] {
        let subdir = state_dir.join(subdir_name);
        if subdir.is_dir() {
            chmod_recursive(&subdir);
        }
    }
}

/// Recursively chmod: directories → 0o755, files → 0o644.
#[cfg(unix)]
fn chmod_recursive(dir: &std::path::Path) {
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            chmod_recursive(&path);
        } else {
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o644));
        }
    }
}
