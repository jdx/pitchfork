use crate::Result;
use crate::cli::list::build_proxy_url;
use crate::daemon_id::DaemonId;
use crate::ipc::batch::{StartOptions, update_job_with_result};
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use crate::settings::settings;
use crate::ui::style::{ncyan, ndim};
use std::sync::Arc;

/// Shared long help for the `start` command and its implicit fallback form.
pub(crate) const LONG_ABOUT: &str = "\
Starts a daemon from a pitchfork.toml file

Daemons are defined in pitchfork.toml with a `[daemons.<name>]` section.
The command waits for the daemon to be ready before returning.

Examples:

    pitchfork start api           Start a single daemon
    pitchfork start api worker    Start multiple daemons
    pitchfork start --group backend Start all daemons in the 'backend' group
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
                                  Wait for TCP port to be listening";

/// Starts a daemon from a pitchfork.toml file
#[derive(Debug, usage_rs::Args)]
#[usage(
    verbatim_doc_comment,
    long_about = LONG_ABOUT
)]
pub struct Start {
    /// ID of the daemon(s) in pitchfork.toml to start
    #[usage(conflicts = "local", conflicts = "global", conflicts = "all")]
    id: Vec<String>,
    /// Start all daemons in the named group
    #[usage(
        long,
        value_name = "GROUP",
        conflicts = "local",
        conflicts = "global",
        conflicts = "all"
    )]
    group: Option<String>,
    /// Start all local daemons in pitchfork.toml
    #[usage(
        long,
        short = 'l',
        visible_alias = "all-local",
        conflicts = "all",
        conflicts = "global"
    )]
    local: bool,
    /// Start all global daemons in ~/.config/pitchfork/config.toml and /etc/pitchfork/config.toml
    #[usage(
        long,
        short = 'g',
        visible_alias = "all-global",
        conflicts = "local",
        conflicts = "all"
    )]
    global: bool,
    /// Start all daemons (both local and global)
    #[usage(long, short = 'a', conflicts = "local", conflicts = "global")]
    all: bool,
    #[usage(long, hide = true)]
    shell_pid: Option<u32>,
    /// Stop the daemon if it is already running
    #[usage(short, long)]
    force: bool,
    /// Delay in seconds before considering daemon ready (default: 3 seconds)
    #[usage(long)]
    delay: Option<u64>,
    /// Wait until output matches this regex pattern before considering daemon ready
    #[usage(long)]
    output: Option<String>,
    /// Wait until HTTP endpoint returns 2xx status before considering daemon ready
    #[usage(long)]
    http: Option<String>,
    /// Wait until TCP port is listening before considering daemon ready
    #[usage(long)]
    port: Option<u16>,
    /// Shell command to poll for readiness (exit code 0 = ready)
    #[usage(long)]
    cmd: Option<String>,
    /// Shell command to poll for health (exit code 0 = healthy)
    #[usage(long)]
    health_cmd: Option<String>,
    /// HTTP endpoint URL to poll for health
    #[usage(long)]
    health_http: Option<String>,
    /// TCP port to probe for health (connection success = healthy)
    #[usage(long)]
    health_port: Option<u16>,
    /// Ports the daemon is expected to bind to (can be specified multiple times or comma-separated)
    #[usage(long, delimiter = ',')]
    expected_port: Vec<u16>,
    /// Automatically find an available port if the expected port is in use
    #[usage(long, value_name = "BUMP")]
    bump: Option<Option<u32>>,
    /// Suppress startup log output
    #[usage(short, long)]
    quiet: bool,
}

impl Start {
    pub async fn run(&self) -> Result<()> {
        let no_target =
            self.id.is_empty() && self.group.is_none() && !self.local && !self.global && !self.all;

        // When no target is specified, verify we're in an interactive terminal
        // before connecting to the supervisor (which may auto-start it).
        if no_target {
            super::interactive::require_interactive_terminal()?;
        }

        let ipc = Arc::new(IpcClient::connect(true).await?);

        // Compute daemon IDs to start
        let ids: Vec<DaemonId> = if self.all {
            IpcClient::get_all_configured_daemons()?
        } else if self.global {
            IpcClient::get_global_configured_daemons()?
        } else if self.local {
            IpcClient::get_local_configured_daemons()?
        } else if no_target {
            let all = IpcClient::get_all_configured_daemons()?;
            // Without --force, exclude daemons that are already running so the
            // user doesn't accidentally restart them.
            let candidates = if self.force {
                all
            } else {
                let running: std::collections::HashSet<DaemonId> =
                    ipc.get_running_daemons().await?.into_iter().collect();
                all.into_iter().filter(|id| !running.contains(id)).collect()
            };
            super::interactive::select_daemons_interactively(&candidates, "start")?
        } else {
            PitchforkToml::resolve_ids_and_group(&self.id, self.group.as_deref())?
        };

        if ids.is_empty() {
            warn!("No daemons to start");
            return Ok(());
        }

        let opts = StartOptions {
            force: self.force,
            shell_pid: self.shell_pid,
            delay: self.delay,
            output: self.output.clone(),
            http: self.http.clone(),
            port: self.port,
            cmd: self.cmd.clone(),
            health_cmd: self.health_cmd.clone(),
            health_http: self.health_http.clone(),
            health_port: self.health_port,
            expected_port: (!self.expected_port.is_empty()).then_some(self.expected_port.clone()),
            auto_bump_port: match self.bump {
                None => None,
                Some(None) => Some(crate::config_types::PortBump(
                    crate::settings::settings().default_port_bump_attempts(),
                )),
                Some(Some(n)) => Some(crate::config_types::PortBump(n)),
            },
            quiet: self.quiet,
            ..Default::default()
        };

        let result = ipc.start_daemons(&ids, opts).await?;

        // Apply all deferred job status updates.
        // Log streaming was already stopped inside each spawn task,
        // so println() won't race with the render thread here.
        for update in &result.pending_job_updates {
            update_job_with_result(update.job.as_deref(), &update.id, &update.run_result);
        }

        // Stop progress display (renders final frame with all job statuses)
        clx::progress::stop();
        clx::progress::clear_jobs();

        // Show proxy URLs for successful daemons (unless --quiet)
        if !self.quiet {
            let global_slugs = settings()
                .proxy
                .enable
                .then(PitchforkToml::read_global_slugs)
                .unwrap_or_default();
            for (id, _start_time, resolved_ports) in &result.started {
                let s = settings();
                if s.proxy.enable && !resolved_ports.is_empty() {
                    let slug_name =
                        PitchforkToml::find_slug_for_daemon_in_registry(id, &global_slugs);
                    if let Some(proxy_url) = build_proxy_url(slug_name.as_deref(), &s) {
                        let display_name = id.styled_qualified();
                        println!(
                            "  {} {} {}",
                            ndim("↳"),
                            display_name,
                            ncyan(&proxy_url).underlined()
                        );
                    }
                }
            }
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
