use crate::cli::logs;
use crate::procs::PROCS;
use crate::state_file::StateFile;
use crate::Result;
use tokio::time;

/// Wait for a daemon to stop, tailing the logs along the way
///
/// Exits with the same status code as the daemon
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "w", verbatim_doc_comment)]
pub struct Wait {
    /// The name of the daemon to wait for
    id: String,
}

impl Wait {
    pub async fn run(&self) -> Result<()> {
        let sf = StateFile::get();
        let pid = if let Some(pid) = sf.daemons.get(&self.id).and_then(|d| d.pid) {
            pid
        } else {
            warn!("{} is not running", self.id);
            return Ok(());
        };

        let tail_names = vec![self.id.to_string()];
        tokio::spawn(async move {
            logs::tail_logs(&tail_names).await.unwrap_or_default();
        });

        let mut interval = time::interval(time::Duration::from_millis(100));
        loop {
            if !PROCS.is_running(pid) {
                break;
            }
            interval.tick().await;
            PROCS.refresh_processes();
        }

        Ok(())
    }
}
