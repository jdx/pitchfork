use crate::Result;
use crate::cli::logs::print_startup_logs;
use crate::ipc::batch::StartOptions;
use crate::ipc::client::IpcClient;
use miette::ensure;
use std::sync::Arc;

/// Restarts a daemon (stops then starts it)
#[derive(Debug, clap::Args)]
#[clap(
    verbatim_doc_comment,
    long_about = "\
Restarts a daemon (stops then starts it)

Equivalent to 'start --force' - stops the daemon (SIGTERM) then starts it again
from the pitchfork.toml configuration with dependency resolution.

Examples:
  pitchfork restart api           Restart a single daemon
  pitchfork restart api worker    Restart multiple daemons
  pitchfork restart --all         Restart all running daemons
  pitchfork restart api --delay 5 Wait 5 seconds for daemon to be ready"
)]
pub struct Restart {
    /// ID of the daemon(s) to restart
    id: Vec<String>,
    /// Restart all running daemons
    #[clap(long, short)]
    all: bool,
    /// Delay in seconds before considering daemon ready (default: 3 seconds)
    #[clap(long)]
    delay: Option<u64>,
    /// Wait until output matches this regex pattern before considering daemon ready
    #[clap(long)]
    output: Option<String>,
    /// Wait until HTTP endpoint returns 2xx status before considering daemon ready
    #[clap(long)]
    http: Option<String>,
    /// Wait until TCP port is listening before considering daemon ready
    #[clap(long)]
    port: Option<u16>,
    /// Shell command to poll for readiness (exit code 0 = ready)
    #[clap(long)]
    cmd: Option<String>,
    /// Suppress startup log output
    #[clap(short, long)]
    quiet: bool,
}

impl Restart {
    pub async fn run(&self) -> Result<()> {
        ensure!(
            self.all || !self.id.is_empty(),
            "You must provide at least one daemon to restart, or use --all"
        );

        let ipc = Arc::new(IpcClient::connect(true).await?);

        // Compute daemon IDs to restart
        let ids: Vec<String> = if self.all {
            ipc.get_running_daemons().await?
        } else {
            self.id.clone()
        };

        if ids.is_empty() {
            info!("No daemons to restart");
            return Ok(());
        }

        let opts = StartOptions {
            force: true, // restart always forces
            delay: self.delay,
            output: self.output.clone(),
            http: self.http.clone(),
            port: self.port,
            cmd: self.cmd.clone(),
            ..Default::default()
        };

        // Restart is just start --force with dependency resolution
        let result = ipc.start_daemons(&ids, opts).await?;

        // Show startup logs for successful daemons (unless --quiet)
        if !self.quiet {
            for (id, start_time) in &result.started {
                if let Err(e) = print_startup_logs(id, *start_time) {
                    debug!("Failed to print startup logs for {id}: {e}");
                }
            }
        }

        if result.any_failed {
            std::process::exit(1);
        }
        Ok(())
    }
}
