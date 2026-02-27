//! Retry logic with exponential backoff
//!
//! Handles automatic retrying of failed daemons based on retry configuration.

use super::hooks::{HookType, fire_hook};
use super::{Supervisor, UpsertDaemonOpts};
use crate::daemon::RunOptions;
use crate::pitchfork_toml::PitchforkToml;
use crate::{Result, env};

impl Supervisor {
    /// Check for daemons that need retrying and attempt to restart them
    pub(crate) async fn check_retry(&self) -> Result<()> {
        // Collect only IDs of daemons that need retrying (avoids cloning entire Daemon structs)
        let ids_to_retry: Vec<String> = {
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
                        self.upsert_daemon(UpsertDaemonOpts {
                            id,
                            status: daemon.status.clone(),
                            retry_count: Some(daemon.retry),
                            ..Default::default()
                        })
                        .await?;
                        continue;
                    }
                };
                let dir = daemon.dir.unwrap_or_else(|| env::CWD.clone());
                fire_hook(
                    HookType::OnRetry,
                    id.clone(),
                    dir.clone(),
                    daemon.retry_count + 1,
                    daemon.env.clone(),
                    vec![],
                );
                let retry_opts = RunOptions {
                    id: id.clone(),
                    cmd,
                    force: false,
                    shell_pid: daemon.shell_pid,
                    dir,
                    autostop: daemon.autostop,
                    cron_schedule: daemon.cron_schedule,
                    cron_retrigger: daemon.cron_retrigger,
                    retry: daemon.retry,
                    retry_count: daemon.retry_count + 1,
                    ready_delay: daemon.ready_delay,
                    ready_output: daemon.ready_output.clone(),
                    ready_http: daemon.ready_http.clone(),
                    ready_port: daemon.ready_port,
                    ready_cmd: daemon.ready_cmd.clone(),
                    expected_port: daemon.original_port.clone(),
                    auto_bump_port: daemon.auto_bump_port,
                    wait_ready: false,
                    depends: daemon.depends.clone(),
                    env: daemon.env.clone(),
                    watch: daemon.watch.clone(),
                    watch_base_dir: daemon.watch_base_dir.clone(),
                };
                if let Err(e) = self.run(retry_opts).await {
                    error!("failed to retry daemon {id}: {e}");
                }
            } else {
                warn!("no run command found for daemon {id}, cannot retry");
                // Mark as exhausted
                self.upsert_daemon(UpsertDaemonOpts {
                    id,
                    retry_count: Some(daemon.retry),
                    ..Default::default()
                })
                .await?;
            }
        }

        Ok(())
    }

    /// Get the run command for a daemon from the pitchfork.toml configuration
    pub(crate) fn get_daemon_run_command(&self, id: &str) -> Option<String> {
        let pt = PitchforkToml::all_merged();
        pt.daemons.get(id).map(|d| d.run.clone())
    }
}
