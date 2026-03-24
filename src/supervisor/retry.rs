//! Retry logic with exponential backoff
//!
//! Handles automatic retrying of failed daemons based on retry configuration.

use super::Supervisor;
use super::hooks::{HookType, fire_hook};
use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::PitchforkToml;
use crate::supervisor::state::UpsertDaemonOpts;
use crate::{Result, env};

impl Supervisor {
    /// Check for daemons that need retrying and attempt to restart them
    pub(crate) async fn check_retry(&self) -> Result<()> {
        // Collect only IDs of daemons that need retrying (avoids cloning entire Daemon structs)
        let ids_to_retry: Vec<DaemonId> = {
            let state_file = self.state_file.lock().await;
            state_file
                .daemons
                .iter()
                .filter(|(_id, d)| {
                    // Daemon is errored, not currently running, and has retries remaining
                    d.status.is_errored()
                        && d.pid.is_none()
                        && d.retry > 0
                        && d.retry_count < d.retry
                })
                .map(|(id, _d)| id.clone())
                .collect()
        };

        for id in ids_to_retry {
            // Look up daemon when needed and re-verify retry criteria
            // (state may have changed since we collected IDs)
            let daemon = {
                let state_file = self.state_file.lock().await;
                match state_file.daemons.get(&id) {
                    Some(d)
                        if d.status.is_errored()
                            && d.pid.is_none()
                            && d.retry > 0
                            && d.retry_count < d.retry =>
                    {
                        d.clone()
                    }
                    _ => continue, // Daemon was removed or no longer needs retry
                }
            };
            info!(
                "retrying daemon {} ({}/{} attempts)",
                id,
                daemon.retry_count + 1,
                daemon.retry
            );

            // Get command from pitchfork.toml
            if let Some(run_cmd) = self.get_daemon_run_command(&id) {
                let cmd = match shell_words::split(&run_cmd) {
                    Ok(cmd) => cmd,
                    Err(e) => {
                        error!("failed to parse command for daemon {id}: {e}");
                        // Mark as exhausted to prevent infinite retry loop, preserving error status
                        self.upsert_daemon(
                            UpsertDaemonOpts::builder(id)
                                .set(|o| {
                                    o.status = daemon.status.clone();
                                    o.retry_count = Some(daemon.retry);
                                })
                                .build(),
                        )
                        .await?;
                        continue;
                    }
                };
                let dir = daemon.dir.clone().unwrap_or_else(|| env::CWD.clone());
                fire_hook(
                    HookType::OnRetry,
                    id.clone(),
                    dir.clone(),
                    daemon.retry_count + 1,
                    daemon.env.clone(),
                    vec![],
                )
                .await;
                let mut retry_opts = daemon.to_run_options(cmd);
                retry_opts.retry_count = daemon.retry_count + 1;
                if let Err(e) = self.run(retry_opts).await {
                    error!("failed to retry daemon {id}: {e}");
                }
            } else {
                warn!("no run command found for daemon {id}, cannot retry");
                // Mark as exhausted
                self.upsert_daemon(
                    UpsertDaemonOpts::builder(id)
                        .set(|o| {
                            o.retry_count = Some(daemon.retry);
                        })
                        .build(),
                )
                .await?;
            }
        }

        Ok(())
    }

    /// Get the run command for a daemon from the pitchfork.toml configuration
    pub(crate) fn get_daemon_run_command(&self, id: &DaemonId) -> Option<String> {
        let pt = PitchforkToml::all_merged().unwrap_or_else(|e| {
            warn!("Failed to load config for run-command lookup: {e}");
            crate::pitchfork_toml::PitchforkToml::default()
        });
        pt.daemons.get(id).map(|d| d.run.clone())
    }
}
