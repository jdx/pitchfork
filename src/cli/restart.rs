use crate::Result;
use crate::cli::logs::{collect_startup_logs, print_startup_logs_block};
use crate::daemon_id::DaemonId;
use crate::ipc::batch::StartOptions;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use crate::ui::style::{ncyan, nstyle};
use itertools::Itertools;
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
  pitchfork restart -l            Restart all local daemons in pitchfork.toml
  pitchfork restart -g            Restart all global daemons in config.toml
  pitchfork restart api --delay 5 Wait 5 seconds for daemon to be ready"
)]
pub struct Restart {
    /// ID of the daemon(s) to restart
    #[clap(
        conflicts_with = "local",
        conflicts_with = "global",
        conflicts_with = "all"
    )]
    id: Vec<String>,
    /// Restart all running daemons
    #[clap(long, short, conflicts_with = "local", conflicts_with = "global")]
    all: bool,
    /// Restart all local daemons in pitchfork.toml
    #[clap(
        long,
        short = 'l',
        visible_alias = "all-local",
        conflicts_with = "all",
        conflicts_with = "global"
    )]
    local: bool,
    /// Restart all global daemons in ~/.config/pitchfork/config.toml and /etc/pitchfork/config.toml
    #[clap(
        long,
        short = 'g',
        visible_alias = "all-global",
        conflicts_with = "local",
        conflicts_with = "all"
    )]
    global: bool,
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
            self.local || self.global || self.all || !self.id.is_empty(),
            "At least one daemon ID or one of --all / --local / --global must be provided"
        );

        let ipc = Arc::new(IpcClient::connect(true).await?);

        let ids: Vec<DaemonId> = if self.all {
            ipc.get_running_daemons().await?
        } else if self.global || self.local {
            ipc.get_running_configured_daemons(self.global).await?
        } else {
            PitchforkToml::resolve_ids(&self.id)?
        };

        if ids.is_empty() {
            warn!("No daemons to restart");
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
            let all_ids: Vec<&DaemonId> = result.started.iter().map(|(id, _, _)| id).collect();
            let mut all_log_lines = vec![];
            for (id, start_time, resolved_ports) in &result.started {
                match collect_startup_logs(id, *start_time) {
                    Ok(lines) => all_log_lines.extend(lines),
                    Err(e) => debug!("Failed to collect startup logs for {id}: {e}"),
                }
                let display_name = id.styled_display_name(Some(all_ids.iter().copied()));
                if !resolved_ports.is_empty() {
                    let port_str = resolved_ports.iter().map(ToString::to_string).join(", ");
                    let port_label = if resolved_ports.len() == 1 {
                        "port"
                    } else {
                        "ports"
                    };
                    println!(
                        "{} {} restarted on {} {}",
                        nstyle("↻").green(),
                        display_name,
                        port_label,
                        ncyan(&port_str),
                    );
                } else {
                    println!("{} {} restarted", nstyle("↻").green(), display_name);
                }
            }
            print_startup_logs_block(&all_log_lines);
        }

        if result.any_failed {
            std::process::exit(1);
        }
        Ok(())
    }
}
