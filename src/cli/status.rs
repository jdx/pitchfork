use crate::Result;
use crate::cli::json_output::{JsonStatusEntry, print_json};
use crate::cli::list::{build_proxy_url, proxy_tls_mode};
use crate::daemon::Daemon;
use crate::daemon_list::build_placeholder_daemon;
use crate::pitchfork_toml::PitchforkToml;
use crate::settings::settings;
use crate::state_file::StateFile;
use crate::ui::cron::format_at;

/// Display the status of a daemon
#[derive(Debug, usage_rs::Args)]
#[usage(
    verbatim_doc_comment,
    long_about = "\
Display the status of a daemon

Shows detailed information about a single daemon including its PID and
current status (running, stopped, failed, etc.).

Example:

    pitchfork status api

Output:

    Name: api
    PID: 12345
    Status: running"
)]
pub struct Status {
    /// Name of the daemon to check
    pub id: String,
    /// Output in JSON format
    #[usage(long)]
    json: bool,
}

/// The hostname to show for a daemon: its legacy slug, or the automatic
/// hostname derived from where its config lives.
fn daemon_host(
    id: &crate::daemon_id::DaemonId,
    global_slugs: &indexmap::IndexMap<String, crate::pitchfork_toml::SlugEntry>,
) -> Option<String> {
    let config = PitchforkToml::all_merged_all_namespaces().ok();
    crate::proxy::hostname::host_for_daemon(
        id,
        config.as_ref().and_then(|pt| pt.daemons.get(id)),
        global_slugs,
    )
}

/// The TLS mode the proxy uses for a daemon's hostname, resolved the way the
/// router resolves it rather than from the daemon's recorded state.
fn daemon_proxy_tls_mode(
    id: &crate::daemon_id::DaemonId,
    global_slugs: &indexmap::IndexMap<String, crate::pitchfork_toml::SlugEntry>,
) -> crate::pitchfork_toml::ProxyTlsMode {
    let config = PitchforkToml::all_merged_all_namespaces().ok();
    proxy_tls_mode(
        id,
        config.as_ref().and_then(|pt| pt.daemons.get(id)),
        global_slugs,
    )
}

impl Status {
    pub async fn run(&self) -> Result<()> {
        let qualified_id = PitchforkToml::resolve_id(&self.id)?;
        let global_slugs = settings()
            .proxy
            .enable
            .then(PitchforkToml::read_global_slugs)
            .unwrap_or_default();

        // Try state file first, then fall back to config for "available" daemons.
        let (daemon, is_available): (Daemon, bool) =
            match StateFile::get().daemons.get(&qualified_id) {
                Some(d) => (d.clone(), d.config_registered),
                None => {
                    let config = PitchforkToml::all_merged_all_namespaces()?;
                    match config.daemons.get(&qualified_id) {
                        Some(dc) => (build_placeholder_daemon(&qualified_id, dc), true),
                        None => miette::bail!("Daemon {} not found", qualified_id),
                    }
                }
            };

        if self.json {
            let s = settings();
            let proxy_url = if s.proxy.enable
                && (daemon.active_port.is_some() || !daemon.resolved_port.is_empty())
            {
                build_proxy_url(daemon_host(&qualified_id, &global_slugs).as_deref(), &s)
            } else {
                None
            };
            let proxy_tls = proxy_url
                .as_ref()
                .map(|_| daemon_proxy_tls_mode(&qualified_id, &global_slugs).to_string());
            let entry = JsonStatusEntry {
                id: qualified_id.qualified(),
                namespace: qualified_id.namespace().to_string(),
                name: qualified_id.name().to_string(),
                pid: daemon.pid,
                status: if is_available {
                    "available".to_string()
                } else {
                    daemon.status.to_string()
                },
                oneshot: daemon.oneshot,
                active_port: daemon.active_port,
                port: daemon.resolved_port.clone(),
                proxy_url: proxy_url.clone(),
                url: proxy_url,
                proxy_tls,
                cron_schedule: daemon.cron_schedule.clone(),
                cron_last_run: daemon.last_cron_run.map(|t| t.to_rfc3339()),
                cron_next_run: daemon
                    .next_cron_run(chrono::Local::now())
                    .map(|t| t.to_rfc3339()),
            };
            return print_json(&entry);
        }

        println!("Name: {qualified_id}");
        if let Some(pid) = &daemon.pid {
            println!("PID: {pid}");
        }
        if is_available {
            println!("Status: available");
        } else {
            println!("Status: {}", daemon.status.style());
        }
        if let Some(port) = daemon.active_port {
            println!("Port: {port} (active)");
        } else if !daemon.resolved_port.is_empty() {
            let ports = daemon
                .resolved_port
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!("Port: {ports}");
        }
        // Between runs a cron daemon is `stopped`, which says nothing about
        // whether the schedule is still live. These lines are what makes that
        // readable without opening the state file.
        if let Some(schedule) = &daemon.cron_schedule {
            let now = chrono::Local::now();
            println!("Cron: {schedule}");
            match daemon.last_cron_run {
                // Timestamp only. How the daemon last exited is already on
                // the `Status:` line above (`failed`, `errored`, `completed`),
                // attributed to the run it actually describes;
                // `last_exit_success` is the daemon's last exit, not
                // necessarily this run's, so pairing it with this timestamp
                // would claim an attribution the state does not carry.
                Some(t) => println!("Last run: {}", format_at(t, now, false)),
                None => println!("Last run: never"),
            }
            match daemon.next_cron_run(now) {
                Some(t) => println!("Next run: {}", format_at(t, now, true)),
                // Only an unparseable expression gets here; the watcher logs
                // the same problem and skips the daemon.
                None => println!("Next run: unknown (invalid schedule)"),
            }
        }
        let s = settings();
        if s.proxy.enable && (daemon.active_port.is_some() || !daemon.resolved_port.is_empty()) {
            match build_proxy_url(daemon_host(&qualified_id, &global_slugs).as_deref(), &s) {
                // Like `list`, only the non-default mode is called out.
                Some(url) => match daemon_proxy_tls_mode(&qualified_id, &global_slugs) {
                    mode if mode.is_passthrough() => println!("Proxy: {url} ({mode})"),
                    _ => println!("Proxy: {url}"),
                },
                // A daemon with a port but no hostname either opted out or lost
                // a label to a clash, which `proxy status` spells out.
                None => println!("Proxy: not routed (see `pitchfork proxy status`)"),
            }
        }
        Ok(())
    }
}
