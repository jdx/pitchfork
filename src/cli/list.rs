use crate::Result;
use crate::daemon_id::DaemonId;
use crate::daemon_list::get_all_daemons;
use crate::daemon_status::DaemonStatus;
use crate::ipc::client::IpcClient;
use crate::settings::settings;
use crate::ui::table::print_table;
use comfy_table::{Cell, Color, ContentArrangement, Table};

/// List all daemons
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "ls",
    verbatim_doc_comment,
    long_about = "\
List all daemons

Displays a table of all tracked daemons with their PIDs, status,
whether they are disabled, and any error messages.

This command shows both:
- Active daemons (currently running or stopped)
- Available daemons (defined in config but not yet started)

Example:
  pitchfork list
  pitchfork ls                    Alias for 'list'
  pitchfork list --hide-header    Output without column headers

Output:
  Name    PID    Status     Error
  api     12345  running
  worker         available
  db             errored    exit code 127"
)]
pub struct List {
    /// Hide the table header row
    #[clap(long)]
    hide_header: bool,
}

impl List {
    pub async fn run(&self) -> Result<()> {
        let client = IpcClient::connect(true).await?;

        let s = settings();
        let mut table = Table::new();
        table
            .load_preset(comfy_table::presets::NOTHING)
            .set_content_arrangement(ContentArrangement::Dynamic);
        if !self.hide_header && console::user_attended() {
            if s.proxy.enable {
                table.set_header(vec!["Name", "PID", "Status", "", "Proxy URL", "Error"]);
            } else {
                table.set_header(vec!["Name", "PID", "Status", "", "Error"]);
            }
        }

        let entries = get_all_daemons(&client).await?;

        // Collect all IDs for display name resolution (clone to avoid borrow issues)
        let all_ids: Vec<DaemonId> = entries.iter().map(|e| e.id.clone()).collect();

        for entry in entries {
            let display_name = entry.id.styled_display_name(Some(all_ids.iter()));

            let status_text = if entry.is_available {
                "available".to_string()
            } else {
                entry.daemon.status.to_string()
            };

            let status_color = if entry.is_available {
                Color::Cyan
            } else {
                match entry.daemon.status {
                    DaemonStatus::Failed(_) => Color::Red,
                    DaemonStatus::Waiting => Color::Yellow,
                    DaemonStatus::Running => Color::Green,
                    DaemonStatus::Stopping => Color::Yellow,
                    DaemonStatus::Stopped => Color::DarkGrey,
                    DaemonStatus::Errored(_) => Color::Red,
                }
            };

            let disabled_marker = if entry.is_disabled { "disabled" } else { "" };

            let error_msg = entry.daemon.status.error_message().unwrap_or_default();

            let error_cell = if error_msg.is_empty() {
                Cell::new("")
            } else {
                Cell::new(&error_msg).fg(Color::Red)
            };

            let pid_str = entry.daemon.pid.map(|p| p.to_string()).unwrap_or_default();
            let mut row = vec![
                Cell::new(&display_name),
                Cell::new(pid_str),
                Cell::new(&status_text).fg(status_color),
                Cell::new(disabled_marker),
            ];
            if s.proxy.enable {
                let proxy_cell = match build_proxy_url(&entry.id, entry.daemon.slug.as_deref(), s) {
                    Some(proxy_url)
                        if entry.daemon.proxy.unwrap_or(s.proxy.enable)
                            && (entry.daemon.active_port.is_some()
                                || !entry.daemon.resolved_port.is_empty()) =>
                    {
                        Cell::new(&proxy_url).fg(Color::Cyan)
                    }
                    _ => Cell::new(""), // no port yet, proxy disabled, or invalid proxy.port config
                };
                row.push(proxy_cell);
            }
            row.push(error_cell);
            table.add_row(row);
        }

        print_table(table)
    }
}

/// Build the proxy URL for a daemon based on its ID, optional slug, and proxy settings.
///
/// If a `slug` is provided, it takes precedence over the `<id>.<namespace>` pattern,
/// producing a shorter URL like `<slug>.<tld>:<port>`.
///
/// Returns `None` if `proxy.port` is invalid (out of range or zero), so callers
/// can skip displaying a URL rather than showing one that points to a port the
/// proxy server is not actually listening on.  This mirrors the behaviour of
/// `serve()`, which bails out with an error for the same invalid-port condition.
pub fn build_proxy_url(
    id: &DaemonId,
    slug: Option<&str>,
    s: &crate::settings::Settings,
) -> Option<String> {
    let scheme = if s.proxy.https { "https" } else { "http" };
    let tld = &s.proxy.tld;
    let standard_port = if s.proxy.https { 443u16 } else { 80u16 };

    // Return None for an invalid port so callers don't display a broken URL.
    let effective_port = u16::try_from(s.proxy.port).ok().filter(|&p| p > 0)?;

    let host = if let Some(slug) = slug {
        // Slug overrides the id.namespace pattern
        format!("{slug}.{tld}")
    } else if id.namespace() == "global" {
        format!("{}.{tld}", id.name())
    } else {
        format!("{}.{}.{tld}", id.name(), id.namespace())
    };

    // Omit port for standard ports (80 for http, 443 for https)
    Some(if effective_port == standard_port {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{effective_port}")
    })
}
