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
                        let slug = PitchforkToml::find_slug_for_daemon_in_registry(
                            &entry.id,
                            &global_slugs,
                        );
                        build_proxy_url(slug.as_deref(), &s)
                            .map(|url| (url, proxy_tls_mode(slug.as_deref(), &global_slugs)))
                    } else {
                        None
                    };
                    let (proxy_url, proxy_tls) = match proxy_url {
                        Some((url, mode)) => (Some(url), Some(mode.to_string())),
                        None => (None, None),
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
                        proxy_tls,
                        proxy_url,
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
                let slug =
                    PitchforkToml::find_slug_for_daemon_in_registry(&entry.id, &global_slugs);
                build_proxy_url(slug.as_deref(), &s)
                    .filter(|_| {
                        entry.daemon.active_port.is_some() || !entry.daemon.resolved_port.is_empty()
                    })
                    .map(|url| (url, proxy_tls_mode(slug.as_deref(), &global_slugs)))
            } else {
                None
            };

            let mut extra_parts: Vec<String> = Vec::new();
            if entry.is_disabled {
                extra_parts.push("disabled".to_string());
            }
            if let Some((url, mode)) = &proxy_url {
                // Only the non-default mode is called out: annotating every
                // terminating daemon would add a column's worth of noise to
                // the common case.
                if mode.is_passthrough() {
                    extra_parts.push(format!("{url} ({mode})"));
                } else {
                    extra_parts.push(url.clone());
                }
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

/// The TLS mode the proxy uses for a daemon's hostname.
///
/// Resolved exactly as the router resolves it: from the config in the slug's
/// own registered directory, reached through the same slug entry the URL was
/// built from.
///
/// Reading the daemon's recorded state instead would report the mode it
/// started with, and reading config rooted at the current directory would miss
/// a slug whose project is registered by directory rather than as a namespace.
/// Either way the displayed mode could disagree with the live route. A slug
/// that resolves to no directory, or a directory whose config no longer
/// describes the daemon, reports `terminate`, the mode it would be routed
/// with.
pub fn proxy_tls_mode(
    slug: Option<&str>,
    global_slugs: &indexmap::IndexMap<String, crate::pitchfork_toml::SlugEntry>,
) -> crate::pitchfork_toml::ProxyTlsMode {
    let Some(entry) = slug.and_then(|slug| global_slugs.get(slug)) else {
        return crate::pitchfork_toml::ProxyTlsMode::default();
    };
    let Some(dir) = entry.resolve_dir() else {
        return crate::pitchfork_toml::ProxyTlsMode::default();
    };
    let daemon_name = entry
        .daemon
        .as_deref()
        .unwrap_or_else(|| slug.expect("slug is Some when its registry entry was found"));
    crate::proxy::server::read_proxy_tls_route(
        &dir,
        entry.resolve_namespace().as_deref(),
        daemon_name,
    )
    .map(|route| route.mode)
    .unwrap_or_default()
}

/// Build the proxy URL for a daemon based on its slug and proxy settings.
///
/// Only daemons with a `slug` are routable through the proxy — no slug means
/// not proxied.  This matches the routing logic in `resolve_target_port`.
///
/// Returns `None` if:
/// - The daemon has no slug (not proxied)
/// - `proxy.port` is invalid (out of range or zero)
pub fn build_proxy_url(slug: Option<&str>, s: &crate::settings::Settings) -> Option<String> {
    // No slug = not proxied.
    let slug = slug?;

    let scheme = if s.proxy.https { "https" } else { "http" };
    let tld = &s.proxy.tld;
    let standard_port = if s.proxy.https { 443u16 } else { 80u16 };

    // Return None for an invalid port so callers don't display a broken URL.
    let effective_port = u16::try_from(s.proxy.port).ok().filter(|&p| p > 0)?;

    let host = format!("{slug}.{tld}");

    // Omit port for standard ports (80 for http, 443 for https)
    Some(if effective_port == standard_port {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{effective_port}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitchfork_toml::{ProxyTlsMode, SlugEntry};

    /// A slug whose project is registered by directory reports the mode from
    /// that directory's config, which is the one the router reads. Config
    /// rooted at the working directory would not describe this daemon at all.
    #[test]
    fn test_proxy_tls_mode_reads_the_slug_directory() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("dirslug-project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("pitchfork.toml"),
            "[daemons.secure]\nrun = \"serve\"\nport = 8443\nproxy_tls = \"passthrough\"\n",
        )
        .unwrap();

        let mut slugs = indexmap::IndexMap::new();
        slugs.insert(
            "dirslug".to_string(),
            SlugEntry {
                dir: Some(project.clone()),
                namespace: None,
                daemon: Some("secure".to_string()),
            },
        );

        assert_eq!(
            proxy_tls_mode(Some("dirslug"), &slugs),
            ProxyTlsMode::Passthrough
        );
    }

    /// A slug whose daemon name defaults to the slug itself resolves the same
    /// way, and anything that cannot be resolved reports the mode it would be
    /// routed with.
    #[test]
    fn test_proxy_tls_mode_defaults_to_terminate() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("named-like-slug");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("pitchfork.toml"),
            "[daemons.dirslug]\nrun = \"serve\"\nport = 8443\nproxy_tls = \"passthrough\"\n",
        )
        .unwrap();

        let mut slugs = indexmap::IndexMap::new();
        slugs.insert(
            "dirslug".to_string(),
            SlugEntry {
                dir: Some(project.clone()),
                namespace: None,
                daemon: None,
            },
        );
        assert_eq!(
            proxy_tls_mode(Some("dirslug"), &slugs),
            ProxyTlsMode::Passthrough
        );

        // No slug, an unregistered slug, and a slug pointing at a directory
        // with no matching daemon all fall back to terminate.
        assert_eq!(proxy_tls_mode(None, &slugs), ProxyTlsMode::Terminate);
        assert_eq!(
            proxy_tls_mode(Some("missing"), &slugs),
            ProxyTlsMode::Terminate
        );

        let empty = dir.path().join("empty-project");
        std::fs::create_dir_all(&empty).unwrap();
        let mut other = indexmap::IndexMap::new();
        other.insert(
            "dirslug".to_string(),
            SlugEntry {
                dir: Some(empty),
                namespace: None,
                daemon: None,
            },
        );
        assert_eq!(
            proxy_tls_mode(Some("dirslug"), &other),
            ProxyTlsMode::Terminate
        );
    }
}
