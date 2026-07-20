//! Autostop logic and boot daemon startup
//!
//! Handles automatic stopping of daemons when shells leave directories,
//! and starting daemons configured with `boot_start = true`.

use super::Supervisor;
use crate::Result;
use crate::daemon_id::DaemonId;
use crate::ipc::IpcResponse;
use crate::pitchfork_toml::PitchforkToml;
use crate::settings::settings;
use log::LevelFilter::Info;
use std::path::{Path, PathBuf};
use tokio::time;

/// Whether `path` is equal to or nested inside `base`.
///
/// Tries a direct `starts_with` first (fast path for paths that share the same
/// representation). Falls back to canonicalizing both sides to bridge
/// representation differences: the Windows verbatim (`\\?\`) prefix that
/// `canonicalize()` adds versus bare paths coming from `env::CWD` / config-file
/// parents, and symlink resolution (e.g. macOS `/tmp` -> `/private/tmp`).
/// `canonicalize` is only invoked when the cheap string comparison already
/// fails, so the common case stays allocation-free.
fn is_within(base: &Path, path: &Path) -> bool {
    if path.starts_with(base) {
        return true;
    }
    canonicalize_pair(base, path).is_some_and(|(b, p)| p.starts_with(&b))
}

/// Whether `a` and `b` are equal, or one contains the other (bidirectional).
fn dirs_overlap(a: &Path, b: &Path) -> bool {
    is_within(a, b) || is_within(b, a)
}

/// Canonicalize both paths, returning `None` if either fails (e.g. the path no
/// longer exists). Callers treat `None` as "no ancestry relationship known".
fn canonicalize_pair(a: &Path, b: &Path) -> Option<(PathBuf, PathBuf)> {
    Some((a.canonicalize().ok()?, b.canonicalize().ok()?))
}

impl Supervisor {
    /// Handle shell leaving a directory - schedule autostops for daemons
    pub(crate) async fn leave_dir(&self, dir: &Path) -> Result<()> {
        debug!("left dir {}", dir.display());
        let active_dirs = self.get_active_directories().await;
        debug!("active directories after leaving {dir:?}: {active_dirs:?}");
        let autostop_delay = settings().general_autostop_delay();

        for daemon in self.active_daemons().await {
            if !daemon.autostop {
                continue;
            }
            // if this daemon's dir is within the left dir
            // and no other active directory is within the daemon's dir
            // schedule the daemon for autostop
            if let Some(daemon_dir) = daemon.dir.as_ref() {
                let starts = is_within(dir, daemon_dir);
                let still_active = active_dirs.iter().any(|d| is_within(daemon_dir, d));
                debug!(
                    "leave_dir daemon={} daemon_dir={daemon_dir:?} starts_with_left={starts} still_active={still_active}",
                    daemon.id
                );
                if starts && !still_active {
                    if autostop_delay.is_zero() {
                        // No delay configured, stop immediately
                        info!("autostopping {daemon}");
                        self.stop(&daemon.id).await?;
                        self.add_notification(Info, format!("autostopped {daemon}"))
                            .await;
                    } else {
                        // Schedule autostop with delay
                        let stop_at = time::Instant::now() + autostop_delay;
                        let mut pending = self.pending_autostops.lock().await;
                        if !pending.contains_key(&daemon.id) {
                            info!(
                                "scheduling autostop for {} in {:?}",
                                daemon.id, autostop_delay
                            );
                            pending.insert(daemon.id.clone(), stop_at);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Cancel any pending autostop for daemons in the given directory
    /// Also cancels autostops for daemons in parent directories (e.g., entering /project/subdir
    /// cancels pending autostop for daemon in /project)
    pub(crate) async fn cancel_pending_autostops_for_dir(&self, dir: &Path) {
        let mut pending = self.pending_autostops.lock().await;
        let daemons_to_cancel: Vec<DaemonId> = {
            let state_file = self.state_file.lock().await;
            state_file
                .daemons
                .iter()
                .filter(|(_id, d)| {
                    d.dir.as_ref().is_some_and(|daemon_dir| {
                        // Cancel if entering a directory inside or equal to daemon's directory
                        // OR if daemon is in a subdirectory of the entered directory
                        dirs_overlap(dir, daemon_dir)
                    })
                })
                .map(|(id, _)| id.clone())
                .collect()
        };

        for daemon_id in daemons_to_cancel {
            if pending.remove(&daemon_id).is_some() {
                info!("cancelled pending autostop for {daemon_id}");
            }
        }
    }

    /// Process any pending autostops that have reached their scheduled time
    pub(crate) async fn process_pending_autostops(&self) -> Result<()> {
        let now = time::Instant::now();
        let to_stop: Vec<DaemonId> = {
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
                // Verify no active directory is in the daemon's directory
                let active_dirs = self.get_active_directories().await;
                if let Some(daemon_dir) = daemon.dir.as_ref() {
                    let still_active = active_dirs.iter().any(|d| is_within(daemon_dir, d));
                    debug!(
                        "process_pending_autostops daemon={daemon_id} daemon_dir={daemon_dir:?} active_dirs={active_dirs:?} still_active={still_active}"
                    );
                    if still_active {
                        debug!(
                            "process_pending_autostops: daemon={daemon_id} still has active directory, skipping"
                        );
                        continue;
                    }
                    info!("autostopping {daemon_id} (after delay)");
                    self.stop(&daemon_id).await?;
                    self.add_notification(Info, format!("autostopped {daemon_id}"))
                        .await;
                }
            }
        }
        Ok(())
    }

    /// Start daemons configured with `boot_start = true`
    pub(crate) async fn start_boot_daemons(&self) -> Result<()> {
        info!("Scanning for boot_start daemons");
        let pt = PitchforkToml::all_merged_all_namespaces()?;

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
            info!("Starting boot daemon: {id}");

            let cmd = match shell_words::split(&daemon.run) {
                Ok(cmd) => cmd,
                Err(e) => {
                    error!("failed to parse command for boot daemon {id}: {e}");
                    continue;
                }
            };
            let mut run_opts = daemon.to_run_options(id, cmd);
            run_opts.autostop = false; // Boot daemons should not autostop
            run_opts.wait_ready = false; // Don't block on boot daemons

            match self.run(run_opts).await {
                Ok(IpcResponse::DaemonStart { .. }) | Ok(IpcResponse::DaemonReady { .. }) => {
                    info!("Successfully started boot daemon: {id}");
                }
                Ok(IpcResponse::DaemonAlreadyRunning) => {
                    info!("Boot daemon already running: {id}");
                }
                Ok(other) => {
                    warn!("Unexpected response when starting boot daemon {id}: {other:?}");
                }
                Err(e) => {
                    error!("Failed to start boot daemon {id}: {e}");
                }
            }
        }

        Ok(())
    }
}
