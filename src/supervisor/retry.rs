//! Retry logic with exponential backoff
//!
//! Handles automatic retrying of failed daemons based on retry configuration.

use super::{BackgroundRetryStarted, Supervisor};
use crate::Result;
use crate::daemon_id::DaemonId;

impl Supervisor {
    /// Check for daemons that need retrying and attempt to restart them
    pub(crate) async fn check_retry(&self) -> Result<()> {
        // Collect only IDs of daemons that need retrying (avoids cloning entire Daemon structs)
        let ids_to_retry: Vec<DaemonId> = {
            let state_file = self.state_file.lock().await;
            state_file
                .daemons
                .iter()
                .filter(|(_id, daemon)| daemon.needs_retry())
                .map(|(id, _d)| id.clone())
                .collect()
        };

        for id in ids_to_retry {
            match self.try_start_background_retry(&id).await {
                Ok(Some(BackgroundRetryStarted { attempt, limit })) => {
                    info!("started retry for daemon {id} ({attempt}/{limit} attempts)");
                }
                Ok(None) => {}
                Err(error) => error!("failed to retry daemon {id}: {error}"),
            }
        }

        Ok(())
    }
}
