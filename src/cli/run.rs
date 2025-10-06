use crate::daemon::RunOptions;
use crate::ipc::client::IpcClient;
use crate::{env, Result};
use miette::bail;

/// Runs a one-off daemon
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "r", verbatim_doc_comment)]
pub struct Run {
    /// Name of the daemon to run
    id: String,
    #[clap(last = true)]
    run: Vec<String>,
    #[clap(short, long)]
    force: bool,
    /// Number of times to retry on error exit
    #[clap(long, default_value = "0")]
    retry: u32,
    /// Delay in seconds before considering daemon ready (default: 3 seconds)
    #[clap(short, long)]
    delay: Option<u64>,
    /// Wait until output matches this regex pattern before considering daemon ready
    #[clap(short, long)]
    output: Option<String>,
}

impl Run {
    pub async fn run(&self) -> Result<()> {
        info!("Running one-off daemon");
        if self.run.is_empty() {
            bail!("No command provided");
        }

        let ipc = IpcClient::connect(true).await?;

        let (started, exit_code) = ipc
            .run(RunOptions {
                id: self.id.clone(),
                cmd: self.run.clone(),
                shell_pid: None,
                force: self.force,
                dir: env::CWD.clone(),
                autostop: false,
                cron_schedule: None,
                cron_retrigger: None,
                retry: self.retry,
                retry_count: 0,
                ready_delay: self.delay.or(Some(3)),
                ready_output: self.output.clone(),
                wait_ready: true,
            })
            .await?;

        if !started.is_empty() {
            info!("started {}", started.join(", "));
        }

        if let Some(code) = exit_code {
            std::process::exit(code);
        }
        Ok(())
    }
}
