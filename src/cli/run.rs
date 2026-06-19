use crate::cli::logs::{ReadyCheckType, create_ready_check_job, stream_startup_logs};
use crate::ipc::batch::{StartOptions, update_job_with_result};
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use crate::settings::settings;
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
  pitchfork run api -o 'Listening' --fail-output 'EADDRINUSE|failed' -- ./server
                                  Fail startup if bad output appears before ready
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
    /// Fail startup if output matches this regex pattern before readiness
    #[clap(long = "fail-output")]
    fail_output: Option<String>,
    /// Wait until HTTP endpoint returns 2xx status before considering daemon ready
    #[clap(long)]
    http: Option<String>,
    /// Wait until TCP port is listening before considering daemon ready
    #[clap(long)]
    port: Option<u16>,
    /// Port(s) the daemon is expected to bind to (can be specified multiple times or comma-separated)
    #[clap(long = "expected-port", value_delimiter = ',')]
    expected_port: Vec<u16>,
    /// Automatically find an available port if the expected port is in use
    #[clap(long, num_args = 0..=1, value_name = "[BUMP]")]
    bump: Option<Option<u32>>,
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
            fail_output: self.fail_output.clone(),
            http: self.http.clone(),
            port: self.port,
            cmd: self.cmd.clone(),
            expected_port: (!self.expected_port.is_empty()).then_some(self.expected_port.clone()),
            auto_bump_port: match self.bump {
                None => None,
                Some(None) => Some(crate::config_types::PortBump(
                    settings().default_port_bump_attempts(),
                )),
                Some(Some(n)) => Some(crate::config_types::PortBump(n)),
            },
            retry: Some(crate::config_types::Retry(self.retry)),
            quiet: self.quiet,
        };

        // Resolve ID, allowing unconfigured short IDs as ad-hoc global daemons.
        let daemon_id = PitchforkToml::resolve_id_allow_adhoc(&self.id)?;

        // Create progress job for ready check (unless --quiet)
        let job = if !self.quiet {
            let check_type = if let Some(ref pattern) = self.output {
                ReadyCheckType::Output(pattern.clone())
            } else if let Some(ref url) = self.http {
                ReadyCheckType::Http(url.clone())
            } else if let Some(port) = self.port {
                ReadyCheckType::Port(port)
            } else if let Some(ref cmd) = self.cmd {
                ReadyCheckType::Cmd(cmd.clone())
            } else if let Some(secs) = self.delay {
                ReadyCheckType::Delay(secs)
            } else {
                ReadyCheckType::Default
            };
            Some(create_ready_check_job(&daemon_id, &check_type))
        } else {
            None
        };

        let start_time = chrono::Local::now();

        // Start streaming logs for this daemon
        let (log_stop_tx, log_handle) = if let Some(ref job) = job {
            let (tx, handle) = stream_startup_logs(&daemon_id, start_time, job.clone());
            (Some(tx), Some(handle))
        } else {
            (None, None)
        };

        let result = ipc
            .run_adhoc(daemon_id.clone(), self.run.clone(), env::CWD.clone(), opts)
            .await;

        match result {
            Ok(result) => {
                // Stop log streaming and wait for the task to fully exit
                if let Some(tx) = &log_stop_tx {
                    let _ = tx.send(true);
                }
                if let Some(handle) = log_handle {
                    let _ = handle.await;
                }

                // Update progress job and stop display
                update_job_with_result(job.as_deref(), &daemon_id, &Ok(result.clone()));
                clx::progress::stop();
                clx::progress::clear_jobs();

                if result.exit_code.is_some() {
                    std::process::exit(1);
                }
            }
            Err(e) => {
                // Stop log streaming and wait for the task to fully exit
                if let Some(tx) = &log_stop_tx {
                    let _ = tx.send(true);
                }
                if let Some(handle) = log_handle {
                    let _ = handle.await;
                }

                update_job_with_result(job.as_deref(), &daemon_id, &Err(e));
                clx::progress::stop();
                clx::progress::clear_jobs();
                std::process::exit(1);
            }
        }

        Ok(())
    }
}
