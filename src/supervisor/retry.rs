//! Background restart logic (tick-driven) with no backoff
//!
//! This module only handles background restarts driven by the `retry`
//! budget: the interval tick restarts errored daemons that still have budget
//! left. Startup retries (the `ready_retry` budget) live in the inline loop
//! inside `Supervisor::run`. The tick skips daemons with an in-flight
//! `run`, so it never starts a duplicate next to an active start/retry loop.

use super::Supervisor;
use super::hooks::{HookType, fire_hook};
use crate::daemon_id::DaemonId;
use crate::supervisor::state::UpsertDaemonOpts;
use crate::{Result, env};

impl Supervisor {
    /// Check for daemons that need retrying and attempt to restart them
    pub(crate) async fn check_retry(&self) -> Result<()> {
        // Collect only IDs of daemons that need retrying (avoids cloning entire Daemon structs)
        let mut ids_to_retry: Vec<DaemonId> = {
            let state_file = self.state_file.lock().await;
            state_file
                .daemons
                .iter()
                .filter(|(_id, d)| {
                    // Daemon is errored, not currently running, and has retries remaining
                    d.status.is_errored()
                        && d.pid.is_none()
                        && d.retry.count() > 0
                        && d.retry_count < d.retry.count()
                })
                .map(|(id, _d)| id.clone())
                .collect()
        };

        // Skip daemons whose start/retry loop is in flight: `run` holds a
        // reference count on the id for its whole duration (including inline
        // backoff sleeps) and re-checks state between attempts, so retrying
        // here would race it. A concurrent run adds its own count, so it can
        // exit early without clearing the first run's protection.
        let in_flight = self.in_flight_runs.lock().await.clone();
        ids_to_retry.retain(|id| !in_flight.contains_key(id));

        for id in ids_to_retry {
            // Look up daemon when needed and re-verify retry criteria
            // (state may have changed since we collected IDs)
            let daemon = {
                let state_file = self.state_file.lock().await;
                match state_file.daemons.get(&id) {
                    Some(d)
                        if d.status.is_errored()
                            && d.pid.is_none()
                            && d.retry.count() > 0
                            && d.retry_count < d.retry.count() =>
                    {
                        d.clone()
                    }
                    _ => continue, // Daemon was removed or no longer needs retry
                }
            };
            // Re-verify under the current tick: an in-flight start/retry loop
            // may have claimed this daemon since the collection pass.
            if self.in_flight_runs.lock().await.contains_key(&id) {
                continue; // an in-flight start/retry loop owns this daemon
            }
            info!(
                "retrying daemon {} ({}/{} attempts)",
                id,
                daemon.retry_count + 1,
                daemon.retry.count()
            );

            // Use the persisted command from daemon state
            let cmd = match daemon.cmd.clone() {
                Some(cmd) => cmd,
                None => {
                    warn!("no run command found in state for daemon {id}, cannot retry");
                    // Mark as exhausted to prevent infinite retry loop, preserving error status
                    self.upsert_daemon(
                        UpsertDaemonOpts::builder(id)
                            .set(|o| {
                                o.status = daemon.status.clone();
                                o.retry_count = Some(daemon.retry.count());
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
                // Per-attempt value: run_once clears the record when an
                // attempt resolves no ports, so a port-less child's retry
                // hook receives no port vars.
                daemon.resolved_port.clone(),
                vec![],
            )
            .await;
            let mut retry_opts = daemon.to_run_options(cmd);
            retry_opts.retry_count = daemon.retry_count + 1;
            if let Err(e) = self.run(retry_opts).await {
                error!("failed to retry daemon {id}: {e}");
            }
        }

        Ok(())
    }
}
