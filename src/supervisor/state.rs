//! State access layer for the supervisor
//!
//! All state getter/setter operations for daemons, shell directories, and notifications.

use super::Supervisor;
use crate::Result;
use crate::daemon::Daemon;
use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::pitchfork_toml::CronRetrigger;
use crate::pitchfork_toml::PitchforkToml;
use crate::procs::PROCS;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::path::PathBuf;

/// Options for upserting a daemon's state.
///
/// Use `UpsertDaemonOpts::builder(id)` to create, then set fields directly and call `.build()`.
#[derive(Debug)]
pub(crate) struct UpsertDaemonOpts {
    pub id: DaemonId,
    pub pid: Option<u32>,
    pub status: DaemonStatus,
    pub shell_pid: Option<u32>,
    pub dir: Option<PathBuf>,
    pub cmd: Option<Vec<String>>,
    pub autostop: bool,
    pub cron_schedule: Option<String>,
    pub cron_retrigger: Option<CronRetrigger>,
    pub last_exit_success: Option<bool>,
    pub retry: Option<u32>,
    pub retry_count: Option<u32>,
    pub ready_delay: Option<u64>,
    pub ready_output: Option<String>,
    pub ready_http: Option<String>,
    pub ready_port: Option<u16>,
    pub ready_cmd: Option<String>,
    pub depends: Option<Vec<DaemonId>>,
    pub env: Option<IndexMap<String, String>>,
    pub watch: Option<Vec<String>>,
    pub watch_base_dir: Option<PathBuf>,
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
                pid: None,
                status: DaemonStatus::default(),
                shell_pid: None,
                dir: None,
                cmd: None,
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
                ready_cmd: None,
                depends: None,
                env: None,
                watch: None,
                watch_base_dir: None,
            },
        }
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
            cron_schedule: opts
                .cron_schedule
                .or(existing.and_then(|d| d.cron_schedule.clone())),
            cron_retrigger: opts
                .cron_retrigger
                .or(existing.and_then(|d| d.cron_retrigger)),
            last_cron_triggered: existing.and_then(|d| d.last_cron_triggered),
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
            ready_cmd: opts
                .ready_cmd
                .or(existing.and_then(|d| d.ready_cmd.clone())),
            depends: opts
                .depends
                .unwrap_or_else(|| existing.map(|d| d.depends.clone()).unwrap_or_default()),
            env: opts.env.or(existing.and_then(|d| d.env.clone())),
            watch: opts
                .watch
                .unwrap_or_else(|| existing.map(|d| d.watch.clone()).unwrap_or_default()),
            watch_base_dir: opts
                .watch_base_dir
                .or(existing.and_then(|d| d.watch_base_dir.clone())),
        };
        state_file.daemons.insert(opts.id.clone(), daemon.clone());
        if let Err(err) = state_file.write() {
            warn!("failed to update state file: {err:#}");
        }
        Ok(daemon)
    }

    /// Enable a daemon (remove from disabled set)
    pub async fn enable(&self, id: &DaemonId) -> Result<bool> {
        info!("enabling daemon: {id}");
        let config = PitchforkToml::all_merged()?;
        let mut state_file = self.state_file.lock().await;
        let exists = state_file.daemons.contains_key(id) || config.daemons.contains_key(id);
        if !exists {
            return Err(miette::miette!("daemon '{}' not found", id));
        }
        let result = state_file.disabled.remove(id);
        state_file.write()?;
        Ok(result)
    }

    /// Disable a daemon (add to disabled set)
    pub async fn disable(&self, id: &DaemonId) -> Result<bool> {
        info!("disabling daemon: {id}");
        let config = PitchforkToml::all_merged()?;
        let mut state_file = self.state_file.lock().await;
        let exists = state_file.daemons.contains_key(id) || config.daemons.contains_key(id);
        if !exists {
            return Err(miette::miette!("daemon '{}' not found", id));
        }
        let result = state_file.disabled.insert(id.clone());
        state_file.write()?;
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
        self.state_file.lock().await.daemons.remove(id);
        if let Err(err) = self.state_file.lock().await.write() {
            warn!("failed to update state file: {err:#}");
        }
        Ok(())
    }

    /// Set the shell's working directory
    pub(crate) async fn set_shell_dir(&self, shell_pid: u32, dir: PathBuf) -> Result<()> {
        let mut state_file = self.state_file.lock().await;
        state_file.shell_dirs.insert(shell_pid.to_string(), dir);
        state_file.write()?;
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
        if state_file
            .shell_dirs
            .remove(&shell_pid.to_string())
            .is_some()
        {
            state_file.write()?;
        }
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
        state_file.daemons.retain(|_id, d| d.pid.is_some());
        state_file.write()?;
        Ok(())
    }
}
