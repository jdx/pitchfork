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
mod ipc_handlers;
mod lifecycle;
mod retry;
mod state;
mod watchers;

use crate::daemon_status::DaemonStatus;
use crate::ipc::server::{IpcServer, IpcServerHandle};
use crate::procs::PROCS;
use crate::state_file::StateFile;
use crate::{Result, env};
use duct::cmd;
use miette::IntoDiagnostic;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::fs;
use std::process::exit;
use std::sync::atomic;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
#[cfg(unix)]
use tokio::signal::unix::SignalKind;
use tokio::sync::Mutex;
use tokio::{signal, time};

// Re-export types needed by other modules
pub(crate) use state::UpsertDaemonOpts;

pub struct Supervisor {
    pub(crate) state_file: Mutex<StateFile>,
    pub(crate) pending_notifications: Mutex<Vec<(log::LevelFilter, String)>>,
    pub(crate) last_refreshed_at: Mutex<time::Instant>,
    /// Map of daemon ID to scheduled autostop time
    pub(crate) pending_autostops: Mutex<HashMap<String, time::Instant>>,
    /// Handle for graceful IPC server shutdown
    pub(crate) ipc_shutdown: Mutex<Option<IpcServerHandle>>,
}

pub(crate) fn interval_duration() -> Duration {
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
            ipc_shutdown: Mutex::new(None),
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
        self.daemon_file_watch()?;

        // Start web server if port is configured
        if let Some(port) = web_port {
            tokio::spawn(async move {
                if let Err(e) = crate::web::serve(port).await {
                    error!("Web server error: {e}");
                }
            });
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
        for daemon in self.active_daemons().await {
            if daemon.id == "pitchfork" {
                continue;
            }
            if let Err(err) = self.stop(&daemon.id).await {
                error!("failed to stop daemon {daemon}: {err}");
            }
        }
        let _ = self.remove_daemon("pitchfork").await;

        // Signal IPC server to shut down gracefully
        if let Some(mut handle) = self.ipc_shutdown.lock().await.take() {
            handle.shutdown();
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
