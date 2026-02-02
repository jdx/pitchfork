use crate::Result;
use crate::cli::logs;
use crate::pitchfork_toml::PitchforkToml;
use crate::procs::PROCS;
use crate::state_file::StateFile;
use tokio::time;

/// Wait for a daemon to stop, tailing the logs along the way
///
/// Exits with the same status code as the daemon
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "w",
    verbatim_doc_comment,
    long_about = "\
Wait for a daemon to stop, tailing the logs along the way

Blocks until the specified daemon stops running, while displaying its
log output in real-time. Exits with the same status code as the daemon.

Useful in scripts that need to wait for a daemon to complete.

Examples:
  pitchfork wait api              Wait for 'api' to stop
  pitchfork w api                 Alias for 'wait'
  pitchfork wait api && echo done Run command after daemon stops"
)]
pub struct Wait {
    /// The name of the daemon to wait for
    id: String,
}

impl Wait {
    pub async fn run(&self) -> Result<()> {
        // Resolve the daemon ID to a qualified ID
        let qualified_id = PitchforkToml::resolve_id(&self.id)?;

        let sf = StateFile::get();
        let pid = if let Some(pid) = sf.daemons.get(&qualified_id).and_then(|d| d.pid) {
            pid
        } else {
            warn!("{} is not running", qualified_id);
            return Ok(());
        };

        let tail_names = vec![qualified_id.clone()];
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
