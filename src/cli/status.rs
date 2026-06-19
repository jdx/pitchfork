use crate::Result;
use crate::cli::json_output::{JsonStatusEntry, print_json};
use crate::cli::list::build_proxy_url;
use crate::pitchfork_toml::PitchforkToml;
use crate::settings::settings;
use crate::state_file::StateFile;

/// Display the status of a daemon
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "stat",
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
    #[clap(long)]
    json: bool,
}

impl Status {
    pub async fn run(&self) -> Result<()> {
        let qualified_id = PitchforkToml::resolve_id(&self.id)?;
        let global_slugs = settings()
            .proxy
            .enable
            .then(PitchforkToml::read_global_slugs)
            .unwrap_or_default();

        let daemon = StateFile::get().daemons.get(&qualified_id);
        if let Some(daemon) = daemon {
            if self.json {
                let s = settings();
                let proxy_url = if s.proxy.enable
                    && (daemon.active_port.is_some() || !daemon.resolved_port.is_empty())
                {
                    let slug = PitchforkToml::find_slug_for_daemon_in_registry(
                        &qualified_id,
                        &global_slugs,
                    );
                    build_proxy_url(slug.as_deref(), &s)
                } else {
                    None
                };
                let entry = JsonStatusEntry {
                    id: qualified_id.qualified(),
                    namespace: qualified_id.namespace().to_string(),
                    name: qualified_id.name().to_string(),
                    pid: daemon.pid,
                    status: daemon.status.to_string(),
                    port: daemon.resolved_port.clone(),
                    proxy_url,
                };
                return print_json(&entry);
            }
            println!("Name: {qualified_id}");
            if let Some(pid) = &daemon.pid {
                println!("PID: {pid}");
            }
            println!("Status: {}", daemon.status.style());
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
            let s = settings();
            if s.proxy.enable && (daemon.active_port.is_some() || !daemon.resolved_port.is_empty())
            {
                let slug =
                    PitchforkToml::find_slug_for_daemon_in_registry(&qualified_id, &global_slugs);
                if let Some(url) = build_proxy_url(slug.as_deref(), &s) {
                    println!("Proxy: {url}");
                }
            }
        } else {
            miette::bail!("Daemon {} not found", qualified_id);
        }
        Ok(())
    }
}
