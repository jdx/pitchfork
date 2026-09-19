use crate::Result;
use crate::cli::json_output::{JsonListEntry, print_json};
use crate::daemon_list::{NamespaceFilter, get_all_daemons};
use crate::daemon_status::DaemonStatus;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use crate::settings::settings;
use crate::ui::table::print_table;
use comfy_table::{Cell, Color, ContentArrangement, Table};

/// Status values accepted by `list --status`.
///
/// `available` and `disabled` are not `DaemonStatus` variants — they filter on
/// the list entry's flags instead. The remaining values match the corresponding
/// `DaemonStatus` variant (only on non-available entries, since an available
/// daemon displays as "available" regardless of its underlying status).
#[derive(Clone, Debug, usage_rs::ValueEnum)]
#[usage(rename_all = "snake_case")]
enum StatusFilter {
    Running,
    Stopped,
    Waiting,
    Stopping,
    Failed,
    Errored,
    Completed,
    Available,
    Disabled,
}

/// List all daemons
#[derive(Debug, usage_rs::Args)]
#[usage(
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
    pitchfork list --status running  Show only running daemons
    pitchfork ls --status available --status stopped
                                    Show daemons that are available OR stopped
    pitchfork list --namespace frontend
                                    Show only daemons in the 'frontend' namespace
    pitchfork list --project        Show only the current project's daemons

Output:

    Name    Status
    api     running    https://api.localhost
    worker  available
    db      errored    exit code 127"
)]
pub struct List {
    /// Hide the table header row
    #[usage(long)]
    hide_header: bool,

    /// Output in JSON format
    #[usage(long)]
    json: bool,

    /// Filter daemons by status (repeatable for OR logic)
    ///
    /// Values: running, stopped, waiting, stopping, failed, errored, completed, available, disabled
    #[usage(long, value_enum)]
    status: Vec<StatusFilter>,

    /// Only show daemons in this namespace (repeatable for OR logic)
    #[usage(long)]
    namespace: Vec<String>,

    /// Only show daemons in the current project's namespace
    ///
    /// The namespace is resolved from the current directory the same way
    /// short daemon IDs are: the nearest config file's namespace, falling
    /// back to 'global' when no config file is found.
    #[usage(long)]
    project: bool,
}

impl List {
    pub async fn run(&self) -> Result<()> {
        let client = IpcClient::connect(true).await?;

        let s = settings();
        let ns_filter = NamespaceFilter::from_flags(&self.namespace, self.project)?;
        let mut entries = get_all_daemons(&client, &ns_filter).await?;
        let global_slugs = PitchforkToml::read_global_slugs();
        // Hostnames are derived from where each daemon's config lives, so the
        // full cross-namespace config is needed to build them.
        let host_config = s
            .proxy
            .enable
            .then(PitchforkToml::all_merged_all_namespaces)
            .and_then(|r| r.ok());

        if !self.status.is_empty() {
            entries.retain(|entry| {
                self.status.iter().any(|filter| match filter {
                    StatusFilter::Available => entry.is_available,
                    StatusFilter::Disabled => entry.is_disabled,
                    StatusFilter::Running => {
                        !entry.is_available && matches!(entry.daemon.status, DaemonStatus::Running)
                    }
                    StatusFilter::Stopped => {
                        !entry.is_available && matches!(entry.daemon.status, DaemonStatus::Stopped)
                    }
                    StatusFilter::Waiting => {
                        !entry.is_available && matches!(entry.daemon.status, DaemonStatus::Waiting)
                    }
                    StatusFilter::Stopping => {
                        !entry.is_available && matches!(entry.daemon.status, DaemonStatus::Stopping)
                    }
                    StatusFilter::Failed => {
                        !entry.is_available
                            && matches!(entry.daemon.status, DaemonStatus::Failed(_))
                    }
                    StatusFilter::Errored => {
                        !entry.is_available
                            && matches!(entry.daemon.status, DaemonStatus::Errored(_))
                    }
                    StatusFilter::Completed => {
                        !entry.is_available
                            && matches!(entry.daemon.status, DaemonStatus::Completed)
                    }
                })
            });
        }

        if self.json {
            let json_entries: Vec<JsonListEntry> = entries
                .iter()
                .map(|entry| {
                    let status_text = if entry.is_available {
                        "available".to_string()
                    } else {
                        entry.daemon.status.to_string()
                    };
                    let proxy_url = if s.proxy.enable
                        && (entry.daemon.active_port.is_some()
                            || !entry.daemon.resolved_port.is_empty())
                    {
                        let host = crate::proxy::hostname::host_for_daemon(
                            &entry.id,
                            host_config
                                .as_ref()
                                .and_then(|pt| pt.daemons.get(&entry.id)),
                            &global_slugs,
                        );
                        build_proxy_url(host.as_deref(), &s)
                    } else {
                        None
                    };
                    JsonListEntry {
                        id: entry.id.qualified(),
                        namespace: entry.id.namespace().to_string(),
                        name: entry.id.name().to_string(),
                        pid: entry.daemon.pid,
                        status: status_text,
                        oneshot: entry.daemon.oneshot,
                        disabled: entry.is_disabled,
                        available: entry.is_available,
                        proxy_url: proxy_url.clone(),
                        url: proxy_url,
                        error: entry.daemon.status.error_message(),
                        active_port: entry.daemon.active_port,
                        port: entry.daemon.resolved_port.clone(),
                    }
                })
                .collect();
            return print_json(&json_entries);
        }

        let mut table = Table::new();
        table
            .load_style(comfy_table::presets::NOTHING)
            .set_content_arrangement(ContentArrangement::Disabled);
        if !self.hide_header && console::user_attended() {
            table.set_header(vec!["Name", "Status", ""]);
        }

        for entry in entries {
            let display_name = entry.id.styled_qualified();

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
                    DaemonStatus::Completed => Color::DarkGreen,
                    DaemonStatus::Errored(_) => Color::Red,
                }
            };

            // Merged "extra" column: disabled marker, proxy URL, and error
            // message combined into a single headerless cell. These rarely
            // co-occur, so color follows priority: error > disabled > proxy.
            let error_msg = entry.daemon.status.error_message().unwrap_or_default();
            let proxy_url = if s.proxy.enable {
                let host = crate::proxy::hostname::host_for_daemon(
                    &entry.id,
                    host_config
                        .as_ref()
                        .and_then(|pt| pt.daemons.get(&entry.id)),
                    &global_slugs,
                );
                build_proxy_url(host.as_deref(), &s).filter(|_| {
                    entry.daemon.active_port.is_some() || !entry.daemon.resolved_port.is_empty()
                })
            } else {
                None
            };

            let mut extra_parts: Vec<String> = Vec::new();
            if entry.is_disabled {
                extra_parts.push("disabled".to_string());
            }
            if let Some(url) = &proxy_url {
                extra_parts.push(url.clone());
            }
            if !error_msg.is_empty() {
                extra_parts.push(error_msg.clone());
            }
            let extra_text = extra_parts.join("  ");

            let extra_cell = if extra_text.is_empty() {
                Cell::new("")
            } else if !error_msg.is_empty() {
                Cell::new(&extra_text).fg(Color::Red)
            } else if entry.is_disabled {
                Cell::new(&extra_text).fg(Color::DarkGrey)
            } else {
                Cell::new(&extra_text).fg(Color::Cyan)
            };

            table.add_row(vec![
                Cell::new(&display_name),
                Cell::new(&status_text).fg(status_color),
                extra_cell,
            ]);
        }

        print_table(table)
    }
}

/// Build the proxy URL for a daemon's hostname.
///
/// Re-exported here because the CLI display paths grew up around this name; the
/// implementation lives in [`crate::proxy::build_proxy_url`].
pub fn build_proxy_url(host: Option<&str>, s: &crate::settings::Settings) -> Option<String> {
    crate::proxy::build_proxy_url(host, s)
}
