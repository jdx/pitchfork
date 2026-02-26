use crate::Result;
use crate::cli::logs::print_startup_logs;
use crate::ipc::batch::StartOptions;
use crate::ipc::client::IpcClient;
use miette::ensure;
use std::sync::Arc;

/// Starts a daemon from a pitchfork.toml file
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "s",
    verbatim_doc_comment,
    long_about = "\
Starts a daemon from a pitchfork.toml file

Daemons are defined in pitchfork.toml with a `[daemons.<name>]` section.
The command waits for the daemon to be ready before returning.

Examples:
  pitchfork start api           Start a single daemon
  pitchfork start api worker    Start multiple daemons
  pitchfork start --all         Start all daemons in pitchfork.toml
  pitchfork start api -f        Restart daemon if already running
  pitchfork start api --delay 5 Wait 5 seconds for daemon to be ready
  pitchfork start api --output 'Listening on'
                                Wait for output pattern before ready
  pitchfork start api --http http://localhost:8080/health
                                Wait for HTTP endpoint to return 2xx
  pitchfork start api --port 8080
                                Wait for TCP port to be listening"
)]
pub struct Start {
    /// ID of the daemon(s) in pitchfork.toml to start
    id: Vec<String>,
    /// Start all daemons in all pitchfork.tomls
    #[clap(long, short)]
    all: bool,
    #[clap(long, hide = true)]
    shell_pid: Option<u32>,
    /// Stop the daemon if it is already running
    #[clap(short, long)]
    force: bool,
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
    /// Ports the daemon is expected to bind to (can be specified multiple times)
    #[clap(long, value_delimiter = ',')]
    expected_port: Vec<u16>,
    /// Automatically find an available port if the expected port is in use
    #[clap(long)]
    auto_bump_port: bool,
    /// Suppress startup log output
    #[clap(short, long)]
    quiet: bool,
}

impl Start {
    pub async fn run(&self) -> Result<()> {
        ensure!(
            self.all || !self.id.is_empty(),
            "At least one daemon ID must be provided"
        );

        let ipc = Arc::new(IpcClient::connect(true).await?);

        // Compute daemon IDs to start
        let ids: Vec<String> = if self.all {
            IpcClient::get_all_configured_daemons()
        } else {
            self.id.clone()
        };

        let opts = StartOptions {
            force: self.force,
            shell_pid: self.shell_pid,
            delay: self.delay,
            output: self.output.clone(),
            http: self.http.clone(),
            port: self.port,
            cmd: self.cmd.clone(),
            expected_port: self.expected_port.clone(),
            auto_bump_port: self.auto_bump_port,
            ..Default::default()
        };

        let result = ipc.start_daemons(&ids, opts).await?;

        // Show startup logs for successful daemons (unless --quiet)
        if !self.quiet {
            for (id, start_time, resolved_ports) in &result.started {
                if let Err(e) = print_startup_logs(id, *start_time) {
                    debug!("Failed to print startup logs for {id}: {e}");
                }
                if !resolved_ports.is_empty() {
                    let port_str = resolved_ports
                        .iter()
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!("Daemon '{id}' started on port(s): {port_str}");
                }
            }
        }

        if result.any_failed {
            std::process::exit(1);
        }
        Ok(())
    }
}
