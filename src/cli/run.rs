use crate::cli::logs::print_startup_logs;
use crate::ipc::batch::StartOptions;
use crate::ipc::client::IpcClient;
use crate::{Result, env};
use miette::bail;

/// Runs a one-off daemon
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "r",
    verbatim_doc_comment,
    long_about = "\
Runs a one-off daemon

Runs a command as a managed daemon without needing a pitchfork.toml.
The daemon is tracked by pitchfork and can be monitored with 'pitchfork status'.

Examples:
  pitchfork run api -- npm run dev
                                Run npm as daemon named 'api'
  pitchfork run api -f -- npm run dev
                                Force restart if 'api' is running
  pitchfork run api --retry 3 -- ./server
                                Restart up to 3 times on failure
  pitchfork run api -d 5 -- ./server
                                Wait 5 seconds for ready check
  pitchfork run api -o 'Listening' -- ./server
                                Wait for output pattern before ready
  pitchfork run api --http http://localhost:8080/health -- ./server
                                Wait for HTTP endpoint to return 2xx
  pitchfork run api --port 8080 -- ./server
                                Wait for TCP port to be listening"
)]
pub struct Run {
    /// Name of the daemon to run
    id: String,
    /// Command and arguments to run (after --)
    #[clap(last = true)]
    run: Vec<String>,
    /// Stop the daemon if it is already running
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

impl Run {
    pub async fn run(&self) -> Result<()> {
        if self.run.is_empty() {
            bail!("No command provided");
        }

        let ipc = IpcClient::connect(true).await?;

        let opts = StartOptions {
            force: self.force,
            shell_pid: None,
            delay: self.delay,
            output: self.output.clone(),
            http: self.http.clone(),
            port: self.port,
            cmd: self.cmd.clone(),
            retry: Some(self.retry),
        };

        let result = ipc
            .run_adhoc(self.id.clone(), self.run.clone(), env::CWD.clone(), opts)
            .await?;

        if result.exit_code.is_some() {
            std::process::exit(1);
        }

        // Show startup logs on success (unless --quiet)
        if !self.quiet
            && result.started
            && let Err(e) = print_startup_logs(&self.id, result.start_time)
        {
            debug!("Failed to print startup logs: {e}");
        }

        Ok(())
    }
}
