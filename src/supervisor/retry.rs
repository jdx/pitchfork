//! Retry logic with exponential backoff
//!
//! Handles automatic recovery of crashed daemons based on recovery configuration.
//! The `retry` config controls startup-phase retries; `recovery` controls runtime-phase retries.

use super::Supervisor;
use super::hooks::{HookType, fire_hook};
use crate::daemon_id::DaemonId;
use crate::supervisor::state::UpsertDaemonOpts;
use crate::{Result, env};

impl Supervisor {
    /// Check for daemons that need recovery and attempt to restart them
    pub(crate) async fn check_retry(&self) -> Result<()> {
        // Collect only IDs of daemons that need recovery (avoids cloning entire Daemon structs)
        let ids_to_retry: Vec<DaemonId> = {
            let state_file = self.state_file.lock().await;
            state_file
                .daemons
                .iter()
                .filter(|(_id, d)| {
                    // Daemon is errored, not currently running, and has recovery attempts remaining
                    d.status.is_errored()
                        && d.pid.is_none()
                        && d.recovery.count() > 0
                        && d.recovery_count < d.recovery.count()
                })
                .map(|(id, _d)| id.clone())
                .collect()
        };

        for id in ids_to_retry {
            // Look up daemon when needed and re-verify recovery criteria
            // (state may have changed since we collected IDs)
            let daemon = {
                let state_file = self.state_file.lock().await;
                match state_file.daemons.get(&id) {
                    Some(d)
                        if d.status.is_errored()
                            && d.pid.is_none()
                            && d.recovery.count() > 0
                            && d.recovery_count < d.recovery.count() =>
                    {
                        d.clone()
                    }
                    _ => continue, // Daemon was removed or no longer needs recovery
                }
            };
            info!(
                "recovering daemon {} ({}/{} attempts)",
                id,
                daemon.recovery_count + 1,
                daemon.recovery.count()
            );

            // Use the persisted command from daemon state
            let cmd = match daemon.cmd.clone() {
                Some(cmd) => cmd,
                None => {
                    warn!("no run command found in state for daemon {id}, cannot recover");
                    // Mark as exhausted to prevent infinite recovery loop, preserving error status
                    self.upsert_daemon(
                        UpsertDaemonOpts::builder(id)
                            .set(|o| {
                                o.status = daemon.status.clone();
                                o.recovery_count = Some(daemon.recovery.count());
                            })
                            .build(),
                    )
                    .await?;
                    continue;
                }
            };
            let dir = daemon.dir.clone().unwrap_or_else(|| env::CWD.clone());
            // on_recover in check_retry is always fire-and-forget to avoid
            // blocking the interval watcher.
            fire_hook(
                HookType::OnRecover,
                id.clone(),
                dir.clone(),
                daemon.retry_count,
                daemon.recovery_count + 1,
                daemon.env.clone(),
                vec![],
            )
            .await;
            let mut retry_opts = daemon.to_run_options(cmd);
            retry_opts.recovery_count = daemon.recovery_count + 1;
            if let Err(e) = self.run(retry_opts).await {
                error!("failed to recover daemon {id}: {e}");
            }
        }

        Ok(())
    }
}
