use crate::Result;
use crate::cli::list::build_proxy_url;
use crate::cli::logs::{collect_startup_logs, print_startup_logs_block};
use crate::daemon_id::DaemonId;
use crate::ipc::batch::StartOptions;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use crate::settings::settings;
use crate::ui::style::{ncyan, ndim, nstyle};
use itertools::Itertools;
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
  pitchfork start -l            Start all local daemons in pitchfork.toml
  pitchfork start -g            Start all global daemons in config.toml
  pitchfork start -a            Start all daemons (local and global)
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
    #[clap(
        conflicts_with = "local",
        conflicts_with = "global",
        conflicts_with = "all"
    )]
    id: Vec<String>,
    /// Start all local daemons in pitchfork.toml
    #[clap(
        long,
        short = 'l',
        visible_alias = "all-local",
        conflicts_with = "all",
        conflicts_with = "global"
    )]
    local: bool,
    /// Start all global daemons in ~/.config/pitchfork/config.toml and /etc/pitchfork/config.toml
    #[clap(
        long,
        short = 'g',
        visible_alias = "all-global",
        conflicts_with = "local",
        conflicts_with = "all"
    )]
    global: bool,
    /// Start all daemons (both local and global)
    #[clap(long, short = 'a', conflicts_with = "local", conflicts_with = "global")]
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
    #[clap(long, num_args = 0..=1, value_name = "[BUMP]")]
    bump: Option<Option<u32>>,
    /// Suppress startup log output
    #[clap(short, long)]
    quiet: bool,
}

impl Start {
    pub async fn run(&self) -> Result<()> {
        ensure!(
            self.local || self.global || self.all || !self.id.is_empty(),
            "At least one daemon ID or one of --all / --local / --global must be provided"
        );

        let ipc = Arc::new(IpcClient::connect(true).await?);

        // Compute daemon IDs to start
        let ids: Vec<DaemonId> = if self.all {
            IpcClient::get_all_configured_daemons()?
        } else if self.global {
            IpcClient::get_global_configured_daemons()?
        } else if self.local {
            IpcClient::get_local_configured_daemons()?
        } else {
            PitchforkToml::resolve_ids(&self.id)?
        };

        let opts = StartOptions {
            force: self.force,
            shell_pid: self.shell_pid,
            delay: self.delay,
            output: self.output.clone(),
            http: self.http.clone(),
            port: self.port,
            cmd: self.cmd.clone(),
            expected_port: (!self.expected_port.is_empty()).then_some(self.expected_port.clone()),
            auto_bump_port: match self.bump {
                None => None,
                Some(None) => Some(crate::config_types::PortBump(
                    crate::settings::settings().default_port_bump_attempts(),
                )),
                Some(Some(n)) => Some(crate::config_types::PortBump(n)),
            },
            ..Default::default()
        };

        let result = ipc.start_daemons(&ids, opts).await?;
        let global_slugs = settings()
            .proxy
            .enable
            .then(PitchforkToml::read_global_slugs)
            .unwrap_or_default();

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
                        "{} {} started on {} {}",
                        nstyle("✔").green(),
                        display_name,
                        port_label,
                        ncyan(&port_str),
                    );
                } else {
                    println!("{} {} started", nstyle("✔").green(), display_name);
                }
                // Show proxy URL when the proxy is enabled and the daemon has a port.
                let s = settings();
                if s.proxy.enable && !resolved_ports.is_empty() {
                    let slug_name =
                        PitchforkToml::find_slug_for_daemon_in_registry(id, &global_slugs);
                    if let Some(proxy_url) = build_proxy_url(slug_name.as_deref(), s) {
                        println!("  {} {}", ndim("↳"), ncyan(&proxy_url).underlined(),);
                    }
                }
            }
            print_startup_logs_block(&all_log_lines);
        }

        // Surface any pending supervisor notifications (e.g. proxy bind failure)
        // so the user sees them immediately after starting daemons.
        super::drain_notifications(&ipc).await;

        if result.any_failed {
            std::process::exit(1);
        }
        Ok(())
    }
}
