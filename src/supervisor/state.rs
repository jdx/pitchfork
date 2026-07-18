//! State access layer for the supervisor
//!
//! All state getter/setter operations for daemons, shell directories, and notifications.

use super::Supervisor;
use crate::Result;
use crate::daemon::Daemon;
use crate::daemon::RunOptions;
use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::pitchfork_toml::CpuLimit;
use crate::pitchfork_toml::CronRetrigger;
use crate::pitchfork_toml::MemoryLimit;
use crate::pitchfork_toml::PitchforkToml;
use crate::pitchfork_toml::PortConfig;
use crate::pitchfork_toml::ReadyCmd;
use crate::pitchfork_toml::ReadyHttp;
use crate::pitchfork_toml::ReadyOutput;
use crate::pitchfork_toml::ReadyPort;
use crate::pitchfork_toml::Retry;
use crate::pitchfork_toml::StopConfig;
use crate::pitchfork_toml::WatchMode;
use crate::procs::PROCS;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Options for upserting a daemon's state.
///
/// Use `UpsertDaemonOpts::builder(id)` to create, then set fields directly and call `.build()`.
#[derive(Debug, Default)]
pub(crate) struct UpsertDaemonOpts {
    pub id: DaemonId,
    pub pid: Option<u32>,
    pub status: DaemonStatus,
    pub shell_pid: Option<u32>,
    pub dir: Option<PathBuf>,
    pub cmd: Option<Vec<String>>,
    pub run: Option<String>,
    pub autostop: bool,
    pub cron_schedule: Option<String>,
    pub cron_retrigger: Option<CronRetrigger>,
    pub cron_immediate: Option<bool>,
    pub last_exit_success: Option<bool>,
    pub retry: Option<Retry>,
    pub retry_count: Option<u32>,
    pub ready_delay: Option<u64>,
    pub ready_output: Option<ReadyOutput>,
    pub ready_http: Option<ReadyHttp>,
    pub ready_port: Option<ReadyPort>,
    pub ready_cmd: Option<ReadyCmd>,
    /// Port configuration
    pub port: Option<PortConfig>,
    /// Resolved ports actually used after auto-bump (may differ from expected)
    pub resolved_port: Vec<u16>,
    /// The first port the process is actually listening on (detected at runtime).
    pub active_port: Option<u16>,
    /// Optional stable slug alias for this daemon.
    pub slug: Option<String>,
    /// Whether to proxy this daemon (None = use global proxy.enable setting).
    pub proxy: Option<bool>,
    pub depends: Option<Vec<DaemonId>>,
    pub env: Option<IndexMap<String, String>>,
    pub watch: Option<Vec<String>>,
    pub watch_mode: Option<WatchMode>,
    pub watch_base_dir: Option<PathBuf>,
    pub mise: Option<bool>,
    /// Unix user to run this daemon as
    pub user: Option<String>,
    /// Memory limit for the daemon process
    pub memory_limit: Option<MemoryLimit>,
    /// CPU usage limit as a percentage
    pub cpu_limit: Option<CpuLimit>,
    /// Unix signal to send for graceful shutdown
    pub stop_signal: Option<StopConfig>,
    /// Archive hook command invoked before retention prunes this daemon's logs.
    pub archive_hook: Option<String>,
    /// Log format for this daemon.
    pub log_format: Option<String>,
    /// Allocate a pseudo-terminal for the daemon process.
    pub pty: Option<bool>,
    /// True for config-only cron daemons auto-registered into state.
    pub config_registered: bool,
}

/// Builder for UpsertDaemonOpts - ensures daemon ID is always provided.
///
/// # Example
/// ```ignore
/// let opts = UpsertDaemonOpts::builder(daemon_id)
///     .set(|o| {
///         o.pid = Some(pid);
///         o.status = DaemonStatus::Running;
///     })
///     .build();
/// ```
#[derive(Debug)]
pub(crate) struct UpsertDaemonOptsBuilder {
    pub opts: UpsertDaemonOpts,
}

impl UpsertDaemonOpts {
    /// Create a builder with the required daemon ID.
    pub fn builder(id: DaemonId) -> UpsertDaemonOptsBuilder {
        UpsertDaemonOptsBuilder {
            opts: UpsertDaemonOpts {
                id,
                ..Default::default()
            },
        }
    }

    /// Build an `UpsertDaemonOptsBuilder` from `RunOptions`, mapping all
    /// config-carried fields. Callers chain `.set()` to add runtime-specific
    /// fields (pid, resolved_port, etc.) and then call `.build()`.
    pub(crate) fn from_run_options(
        opts: &RunOptions,
        status: DaemonStatus,
    ) -> UpsertDaemonOptsBuilder {
        UpsertDaemonOpts::builder(opts.id.clone()).set(|o| {
            o.status = status;
            o.shell_pid = opts.shell_pid;
            o.dir = Some(opts.dir.0.clone());
            o.cmd = Some(opts.cmd.clone());
            o.run = opts.run.clone();
            o.autostop = opts.autostop;
            o.cron_schedule = opts.cron_schedule.clone();
            o.cron_retrigger = opts.cron_retrigger;
            o.cron_immediate = opts.cron_immediate;
            o.retry = Some(opts.retry);
            o.retry_count = Some(opts.retry_count);
            o.ready_delay = opts.ready_delay;
            o.ready_output = opts.ready_output.clone();
            o.ready_http = opts.ready_http.clone();
            o.ready_port = opts.ready_port.clone();
            o.ready_cmd = opts.ready_cmd.clone();
            o.port = opts.port.clone();
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
            o.archive_hook = opts.archive_hook.clone();
            o.log_format = opts.log_format.clone();
        })
    }
}

impl UpsertDaemonOptsBuilder {
    /// Modify opts fields with a closure.
    pub fn set<F: FnOnce(&mut UpsertDaemonOpts)>(mut self, f: F) -> Self {
        f(&mut self.opts);
        self
    }

    /// Build the UpsertDaemonOpts.
    pub fn build(self) -> UpsertDaemonOpts {
        self.opts
    }
}

impl Supervisor {
    /// Upsert a daemon's state, merging with existing values
    pub(crate) async fn upsert_daemon(&self, opts: UpsertDaemonOpts) -> Result<Daemon> {
        info!(
            "upserting daemon: {} pid: {} status: {}",
            opts.id,
            opts.pid.unwrap_or(0),
            opts.status
        );
        let mut state_file = self.state_file.lock().await;
        let existing = state_file.daemons.get(&opts.id);
        let daemon = Daemon {
            id: opts.id.clone(),
            title: opts.pid.and_then(|pid| PROCS.title(pid)),
            pid: opts.pid,
            status: opts.status,
            shell_pid: opts.shell_pid,
            autostop: opts.autostop || existing.is_some_and(|d| d.autostop),
            dir: opts.dir.or(existing.and_then(|d| d.dir.clone())),
            cmd: opts.cmd.or(existing.and_then(|d| d.cmd.clone())),
            run: opts.run.or(existing.and_then(|d| d.run.clone())),
            cron_schedule: opts
                .cron_schedule
                .or(existing.and_then(|d| d.cron_schedule.clone())),
            cron_retrigger: opts
                .cron_retrigger
                .or(existing.and_then(|d| d.cron_retrigger)),
            cron_immediate: opts
                .cron_immediate
                .or(existing.and_then(|d| d.cron_immediate)),
            last_cron_triggered: existing.and_then(|d| d.last_cron_triggered),
            last_exit_success: opts
                .last_exit_success
                .or(existing.and_then(|d| d.last_exit_success)),
            retry: opts
                .retry
                .unwrap_or_else(|| existing.map(|d| d.retry).unwrap_or_default()),
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
            ready_port: opts
                .ready_port
                .or(existing.and_then(|d| d.ready_port.clone())),
            ready_cmd: opts
                .ready_cmd
                .or(existing.and_then(|d| d.ready_cmd.clone())),
            port: opts.port.or_else(|| existing.and_then(|d| d.port.clone())),
            resolved_port: if opts.resolved_port.is_empty() {
                existing
                    .map(|d| d.resolved_port.clone())
                    .unwrap_or_default()
            } else {
                opts.resolved_port
            },
            depends: opts
                .depends
                .unwrap_or_else(|| existing.map(|d| d.depends.clone()).unwrap_or_default()),
            env: opts.env.or(existing.and_then(|d| d.env.clone())),
            watch: opts
                .watch
                .unwrap_or_else(|| existing.map(|d| d.watch.clone()).unwrap_or_default()),
            watch_mode: opts
                .watch_mode
                .unwrap_or_else(|| existing.map(|d| d.watch_mode).unwrap_or_default()),
            watch_base_dir: opts
                .watch_base_dir
                .or(existing.and_then(|d| d.watch_base_dir.clone())),
            mise: opts.mise.or(existing.and_then(|d| d.mise)),
            user: opts.user.or(existing.and_then(|d| d.user.clone())),
            proxy: opts.proxy.or(existing.and_then(|d| d.proxy)),
            // active_port is intentionally NOT inherited from the existing daemon.
            // When a daemon restarts, the new process has not yet bound a port, so
            // carrying over the old process's active_port would cause the proxy to
            // route to a port that is no longer listening.  The port will be
            // re-detected by detect_and_store_active_port once the new process is ready.
            active_port: opts.active_port,
            slug: opts.slug.or(existing.and_then(|d| d.slug.clone())),
            memory_limit: opts.memory_limit.or(existing.and_then(|d| d.memory_limit)),
            cpu_limit: opts.cpu_limit.or(existing.and_then(|d| d.cpu_limit)),
            stop_signal: opts.stop_signal.or(existing.and_then(|d| d.stop_signal)),
            archive_hook: opts
                .archive_hook
                .or(existing.and_then(|d| d.archive_hook.clone())),
            log_format: opts
                .log_format
                .or(existing.and_then(|d| d.log_format.clone())),
            pty: opts.pty.or(existing.and_then(|d| d.pty)),
            config_registered: opts.config_registered,
        };
        state_file.insert_daemon(&opts.id, daemon.clone());
        Ok(daemon)
    }

    /// Enable a daemon (remove from disabled set)
    pub async fn enable(&self, id: &DaemonId) -> Result<bool> {
        info!("enabling daemon: {id}");
        let config = PitchforkToml::all_merged_all_namespaces()?;
        let mut state_file = self.state_file.lock().await;
        let exists = state_file.daemons.contains_key(id) || config.daemons.contains_key(id);
        if !exists {
            return Err(miette::miette!("daemon '{}' not found", id));
        }
        let result = state_file.enable_daemon(id);
        Ok(result)
    }

    /// Disable a daemon (add to disabled set)
    pub async fn disable(&self, id: &DaemonId) -> Result<bool> {
        info!("disabling daemon: {id}");
        let config = PitchforkToml::all_merged_all_namespaces()?;
        let mut state_file = self.state_file.lock().await;
        let exists = state_file.daemons.contains_key(id) || config.daemons.contains_key(id);
        if !exists {
            return Err(miette::miette!("daemon '{}' not found", id));
        }
        let result = state_file.disable_daemon(id);
        Ok(result)
    }

    /// Get a daemon by ID
    pub(crate) async fn get_daemon(&self, id: &DaemonId) -> Option<Daemon> {
        self.state_file.lock().await.daemons.get(id).cloned()
    }

    /// Get all active daemons (those with PIDs, excluding pitchfork itself)
    pub(crate) async fn active_daemons(&self) -> Vec<Daemon> {
        let pitchfork_id = DaemonId::pitchfork();
        self.state_file
            .lock()
            .await
            .daemons
            .values()
            .filter(|d| d.pid.is_some() && d.id != pitchfork_id)
            .cloned()
            .collect()
    }

    /// Remove a daemon from state
    pub(crate) async fn remove_daemon(&self, id: &DaemonId) -> Result<()> {
        let mut state_file = self.state_file.lock().await;
        state_file.remove_daemon(id);
        Ok(())
    }

    /// Set the shell's working directory
    pub(crate) async fn set_shell_dir(&self, shell_pid: u32, dir: PathBuf) -> Result<()> {
        let mut state_file = self.state_file.lock().await;
        state_file.set_shell_dir(shell_pid, dir);
        Ok(())
    }

    /// Get the shell's working directory
    pub(crate) async fn get_shell_dir(&self, shell_pid: u32) -> Option<PathBuf> {
        self.state_file
            .lock()
            .await
            .shell_dirs
            .get(&shell_pid.to_string())
            .cloned()
    }

    /// Remove a shell PID from tracking
    pub(crate) async fn remove_shell_pid(&self, shell_pid: u32) -> Result<()> {
        let mut state_file = self.state_file.lock().await;
        state_file.remove_shell_dir(shell_pid);
        Ok(())
    }

    /// Get all directories with their associated shell PIDs
    pub(crate) async fn get_dirs_with_shell_pids(&self) -> HashMap<PathBuf, Vec<u32>> {
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

    /// Get pending notifications and clear the queue
    pub(crate) async fn get_notifications(&self) -> Vec<(log::LevelFilter, String)> {
        self.pending_notifications.lock().await.drain(..).collect()
    }

    /// Clean up daemons that have no PID
    pub(crate) async fn clean(&self) -> Result<()> {
        let mut state_file = self.state_file.lock().await;
        state_file.retain_daemons(|_id, d| d.pid.is_some());
        Ok(())
    }

    /// Return the union of active directories from shell tracking and project
    /// sessions. These are the directories that should keep auto-stop daemons
    /// alive.
    pub(crate) async fn get_active_directories(&self) -> Vec<PathBuf> {
        let state = self.state_file.lock().await;
        let mut dirs: HashSet<PathBuf> = state.shell_dirs.values().cloned().collect();
        for (_, dir, _) in state.iter_project_sessions() {
            dirs.insert(dir.clone());
        }
        dirs.into_iter().collect()
    }

    /// Collect all project sessions as `(pid, dir, liveness_title)`. Every
    /// project session carries a host PID in its key, so every session is a
    /// liveness session. Used by the refresh loop to clean up stale sessions.
    pub(crate) async fn get_liveness_sessions(&self) -> Vec<(u32, PathBuf, Option<String>)> {
        self.state_file
            .lock()
            .await
            .iter_project_sessions()
            .into_iter()
            .filter_map(|(pid_str, dir, session)| {
                pid_str
                    .parse()
                    .ok()
                    .map(|pid| (pid, dir.clone(), session.liveness_title.clone()))
            })
            .collect()
    }

    /// Build a snapshot of all project sessions with live liveness status
    /// filled in from `PROCS`. Used to answer `GetProjectSessions` IPC
    /// requests.
    pub(crate) async fn get_project_sessions_info(&self) -> Vec<crate::ipc::ProjectSessionInfo> {
        let sessions: Vec<(u32, PathBuf, Option<String>)> = self
            .state_file
            .lock()
            .await
            .iter_project_sessions()
            .into_iter()
            .filter_map(|(pid_str, dir, session)| {
                pid_str
                    .parse()
                    .ok()
                    .map(|pid| (pid, dir.clone(), session.liveness_title.clone()))
            })
            .collect();
        let pids: Vec<u32> = sessions.iter().map(|(pid, _, _)| *pid).collect();
        if !pids.is_empty() {
            PROCS.refresh_pids(&pids);
        }
        sessions
            .into_iter()
            .map(
                |(pid, directory, liveness_title)| crate::ipc::ProjectSessionInfo {
                    pid,
                    directory,
                    liveness_title,
                    alive: PROCS.is_running(pid),
                    current_title: PROCS.title(pid),
                },
            )
            .collect()
    }

    /// Atomically enter (or replace) a project session for `(pid, dir)`.
    /// Returns the previous session, if any, so the caller can evaluate the
    /// previous entry for autostop.
    pub(crate) async fn enter_project_session(
        &self,
        pid: u32,
        dir: PathBuf,
    ) -> Result<Option<crate::state_file::ProjectSession>> {
        PROCS.refresh_pids(&[pid]);
        let liveness_title = PROCS.title(pid);
        let mut state_file = self.state_file.lock().await;
        let previous = state_file.set_project_session(
            pid,
            dir,
            crate::state_file::ProjectSession { liveness_title },
        );
        Ok(previous)
    }

    /// Atomically remove a project session for `(pid, dir)`. Returns the
    /// directory of the removed session so the caller can evaluate it for
    /// autostop.
    pub(crate) async fn leave_project_session(
        &self,
        pid: u32,
        dir: &std::path::Path,
    ) -> Result<Option<PathBuf>> {
        let mut state_file = self.state_file.lock().await;
        if state_file.remove_project_session(pid, dir).is_some() {
            Ok(Some(dir.to_path_buf()))
        } else {
            Ok(None)
        }
    }
}
