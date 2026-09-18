use crate::Result;
use crate::cli::json_output::{
    JsonLanInfo, JsonProxyHost, JsonProxyProject, JsonProxyStatus, JsonProxyWorktree,
    JsonSlugEntry, print_json,
};

/// Manage the pitchfork reverse proxy
#[derive(Debug, usage_rs::Args)]
#[usage(
    verbatim_doc_comment,
    long_about = "\
Manage the pitchfork reverse proxy

The reverse proxy routes requests from stable URLs to the daemon's actual
listening port. Every daemon with a `port` gets a hostname automatically,
built from the daemon, worktree and project names plus the configured TLD:

    https://api.myproject.localhost
    https://api.fix-login.myproject.localhost

Slugs are the older mechanism and still work. They are defined in the global
config (~/.config/pitchfork/config.toml) under [slugs], each mapping to a
project directory and daemon name, and are resolved before hostnames.

Enable the proxy in your pitchfork.toml or settings:

    [settings.proxy]
    enable = true

Subcommands:

    trust     Install the proxy's TLS certificate into the system trust store
    untrust   Remove the proxy's TLS certificate from the system trust store
    add       Add a slug mapping to the global config (legacy)
    remove    Remove a slug mapping from the global config (legacy)
    status    Show hostnames and registered slugs with their current state"
)]
pub struct Proxy {
    #[usage(subcommand)]
    command: ProxyCommands,
}

#[derive(Debug, usage_rs::Subcommands)]
enum ProxyCommands {
    Trust(Trust),
    Untrust(Untrust),
    Status(ProxyStatus),
    Add(Add),
    #[usage(alias = "rm")]
    Remove(Remove),
}

impl Proxy {
    pub async fn run(&self) -> Result<()> {
        match &self.command {
            ProxyCommands::Trust(trust) => trust.run().await,
            ProxyCommands::Untrust(untrust) => untrust.run().await,
            ProxyCommands::Status(status) => status.run().await,
            ProxyCommands::Add(add) => add.run().await,
            ProxyCommands::Remove(remove) => remove.run().await,
        }
    }
}

// ─── proxy trust ─────────────────────────────────────────────────────────────

/// Install the proxy's self-signed TLS certificate into the system trust store
///
/// This command installs pitchfork's auto-generated TLS certificate into your
/// system's trust store so that browsers and tools trust HTTPS proxy URLs
/// without certificate warnings.
///
/// On macOS, this installs the certificate into the current user's login
/// keychain. No `sudo` required.
///
/// On Linux, the appropriate CA certificate directory and update command are
/// detected automatically based on the running distribution:
///   - Debian/Ubuntu: /usr/local/share/ca-certificates/ + update-ca-certificates
///   - RHEL/Fedora/CentOS: /etc/pki/ca-trust/source/anchors/ + update-ca-trust
///   - Arch Linux: /etc/ca-certificates/trust-source/anchors/ + trust extract-compat
///   - openSUSE: /etc/pki/trust/anchors/ + update-ca-certificates
///
/// This DOES require sudo on Linux.
///
/// Example:
///
/// ```text
/// pitchfork proxy trust
/// sudo pitchfork proxy trust    # Linux only
/// ```
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
struct Trust {
    /// Path to the certificate file to trust (defaults to pitchfork's auto-generated cert)
    #[usage(long)]
    cert: Option<std::path::PathBuf>,
}

impl Trust {
    async fn run(&self) -> Result<()> {
        let cert_path = self.cert.clone().unwrap_or_else(|| {
            // Default: pitchfork's auto-generated CA cert in state dir
            crate::env::PITCHFORK_STATE_DIR.join("proxy").join("ca.pem")
        });

        // Check if already trusted to avoid duplicates (especially on macOS keychain)
        if crate::proxy::trust::is_ca_trusted(&cert_path) {
            println!("CA certificate is already trusted.");
            return Ok(());
        }

        crate::proxy::trust::install_cert(&cert_path)?;
        println!(
            "CA certificate installed: {}\n\
             \n\
             Browsers and tools will now trust HTTPS proxy URLs like:\n\
             https://docs.pf.localhost:7777",
            cert_path.display()
        );
        Ok(())
    }
}

// ─── proxy untrust ───────────────────────────────────────────────────────────

/// Remove the proxy's TLS certificate from the system trust store
///
/// Removes the pitchfork CA certificate that was previously installed by
/// `pitchfork proxy trust` or auto-trust.
///
/// On macOS, removes the certificate from the login keychain and system keychain.
/// On Linux, removes the certificate from the distro-specific CA directory
/// and runs the appropriate update command.
///
/// Example:
///
/// ```text
/// pitchfork proxy untrust
/// sudo pitchfork proxy untrust    # Linux only
/// ```
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
struct Untrust {
    /// Path to the certificate file (defaults to pitchfork's auto-generated cert)
    #[usage(long)]
    cert: Option<std::path::PathBuf>,
}

impl Untrust {
    async fn run(&self) -> Result<()> {
        let cert_path = self
            .cert
            .clone()
            .unwrap_or_else(|| crate::env::PITCHFORK_STATE_DIR.join("proxy").join("ca.pem"));

        crate::proxy::trust::uninstall_cert(&cert_path)?;
        println!("CA certificate removed from system trust store.");
        Ok(())
    }
}

// ─── proxy status ─────────────────────────────────────────────────────────────

/// Show hostnames and registered slugs with their current state
///
/// Displays the proxy configuration, the automatic hostnames grouped by project
/// and worktree, and any slugs from the global config with their project
/// directory, daemon name, and current status (running/stopped, port).
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
struct ProxyStatus {
    /// Output in JSON format
    #[usage(long)]
    json: bool,
}

impl ProxyStatus {
    async fn run(&self) -> Result<()> {
        use crate::pitchfork_toml::PitchforkToml;
        use crate::settings::settings;
        let s = settings();

        if !s.proxy.enable {
            if self.json {
                return print_json(&JsonProxyStatus {
                    enabled: false,
                    scheme: None,
                    tld: None,
                    port: None,
                    lan: None,
                    tls_cert: None,
                    trusted: None,
                    slugs: vec![],
                    projects: vec![],
                });
            }
            println!("Proxy: disabled");
            println!();
            println!("Enable with:");
            println!("  PITCHFORK_PROXY_ENABLE=true pitchfork supervisor start");
            println!("  # or in pitchfork.toml: [settings.proxy] / enable = true");
            return Ok(());
        }

        let Some(effective_port) = u16::try_from(s.proxy.port).ok().filter(|&p| p > 0) else {
            if self.json {
                return print_json(&JsonProxyStatus {
                    enabled: true,
                    scheme: None,
                    tld: None,
                    port: None,
                    lan: None,
                    tls_cert: None,
                    trusted: None,
                    slugs: vec![],
                    projects: vec![],
                });
            }
            println!("Proxy: enabled");
            println!(
                "  proxy.port {} is out of valid port range (1-65535)",
                s.proxy.port
            );
            return Ok(());
        };
        let scheme = if s.proxy.https { "https" } else { "http" };
        let lan_enabled = s.proxy.lan || !s.proxy.lan_ip.is_empty();
        let tld = if lan_enabled { "local" } else { &s.proxy.tld };

        let lan_info = if lan_enabled {
            let lan_ip = if !s.proxy.lan_ip.is_empty() {
                s.proxy.lan_ip.clone()
            } else {
                "auto-detect".to_string()
            };
            Some(JsonLanInfo {
                enabled: true,
                ip: lan_ip,
            })
        } else {
            None
        };

        let (tls_cert, trusted) = if s.proxy.https {
            let cert = if s.proxy.tls_cert.is_empty() {
                format!(
                    "{} (auto-generated)",
                    crate::env::PITCHFORK_STATE_DIR
                        .join("proxy")
                        .join("ca.pem")
                        .display()
                )
            } else {
                s.proxy.tls_cert.clone()
            };
            let cert_path = if s.proxy.tls_cert.is_empty() {
                crate::env::PITCHFORK_STATE_DIR.join("proxy").join("ca.pem")
            } else {
                std::path::PathBuf::from(&s.proxy.tls_cert)
            };
            let trusted = crate::proxy::trust::is_ca_trusted(&cert_path);
            (Some(cert), Some(trusted))
        } else {
            (None, None)
        };

        let slugs = PitchforkToml::read_global_slugs();
        let config = PitchforkToml::all_merged_all_namespaces().ok();
        let state_file =
            crate::state_file::StateFile::read(&*crate::env::PITCHFORK_STATE_FILE).ok();
        let standard_port = if s.proxy.https { 443u16 } else { 80u16 };

        let slug_entries: Vec<JsonSlugEntry> = slugs
            .iter()
            .map(|(slug, entry)| {
                let daemon_name = entry.daemon.as_deref().unwrap_or(slug);
                // Listed rather than hidden: this command is how a user finds
                // out why a registered slug stopped resolving.
                let ambiguous = PitchforkToml::slug_is_ambiguous(slug, &slugs);
                let url = (!ambiguous).then(|| {
                    if effective_port == standard_port {
                        format!("{scheme}://{slug}.{tld}")
                    } else {
                        format!("{scheme}://{slug}.{tld}:{effective_port}")
                    }
                });
                let expected_ns = entry.resolve_dir().and_then(|dir| {
                    crate::pitchfork_toml::PitchforkToml::namespace_for_dir(&dir).ok()
                });
                let (status_str, port) = if let Some(sf) = &state_file {
                    let daemon_entry = sf.daemons.iter().find(|(id, _)| {
                        id.name() == daemon_name
                            && match &expected_ns {
                                Some(ns) => id.namespace() == ns,
                                None => true,
                            }
                    });
                    if let Some((_, daemon)) = daemon_entry {
                        let port = daemon
                            .active_port
                            .or_else(|| daemon.resolved_port.first().copied());
                        let status = if daemon.status.is_running() {
                            "running".to_string()
                        } else {
                            daemon.status.to_string()
                        };
                        (status, port)
                    } else {
                        let configured = config.as_ref().is_some_and(|pt| {
                            pt.daemons.keys().any(|id| {
                                id.name() == daemon_name
                                    && match &expected_ns {
                                        Some(ns) => id.namespace() == ns,
                                        None => true,
                                    }
                            })
                        });
                        (
                            if configured {
                                "available"
                            } else {
                                "unconfigured"
                            }
                            .to_string(),
                            None,
                        )
                    }
                } else {
                    ("unknown".to_string(), None)
                };
                let status_str = if ambiguous {
                    "collision".to_string()
                } else {
                    status_str
                };
                JsonSlugEntry {
                    slug: slug.clone(),
                    url,
                    dir: entry
                        .resolve_dir()
                        .map(|d| d.display().to_string())
                        .unwrap_or_else(|| "(unresolved)".to_string()),
                    daemon: daemon_name.to_string(),
                    status: status_str,
                    port,
                }
            })
            .collect();

        let projects = collect_projects(scheme, tld, effective_port, standard_port, &state_file);

        if self.json {
            return print_json(&JsonProxyStatus {
                enabled: true,
                scheme: Some(scheme.to_string()),
                tld: Some(tld.to_string()),
                port: Some(effective_port),
                lan: lan_info,
                tls_cert,
                trusted,
                slugs: slug_entries,
                projects,
            });
        }

        println!("Proxy: enabled");
        println!("  Scheme:  {scheme}");
        println!("  TLD:     {tld}");
        println!("  Port:    {effective_port}");
        if let Some(ref lan) = lan_info {
            println!("  LAN:     enabled (IP: {})", lan.ip);
        }
        if let Some(ref cert) = tls_cert {
            println!("  TLS cert: {cert}");
        }
        if let Some(trusted) = trusted {
            println!(
                "  Trusted: {}",
                if trusted {
                    "yes"
                } else {
                    "no (run: pitchfork proxy trust)"
                }
            );
        }
        println!();

        if slug_entries.is_empty() {
            println!("No slugs registered.");
            println!();
            println!("Add a slug with:");
            println!("  pitchfork proxy add <slug>");
            println!("  pitchfork proxy add <slug> --dir /path/to/project --daemon <name>");
        } else {
            println!("Registered slugs:");
            println!();
            for entry in &slug_entries {
                println!("  {}", entry.slug);
                match &entry.url {
                    Some(url) => println!("    URL:    {url}"),
                    None => println!(
                        "    URL:    (none — another slug differs only by case; \
                         host names are case-insensitive, so neither is routed)"
                    ),
                }
                println!("    Dir:    {}", entry.dir);
                println!("    Daemon: {}", entry.daemon);
                let port_str = entry
                    .port
                    .map(|p| format!(" (port {p})"))
                    .unwrap_or_default();
                println!("    Status: {}{port_str}", entry.status);
                println!();
            }
        }

        println!();
        if projects.is_empty() {
            println!("No project hostnames.");
            println!();
            println!("Give a daemon a `port` in pitchfork.toml and it gets a hostname.");
        } else {
            println!("Hostnames:");
            println!();
            for project in &projects {
                println!("  {} — {}", project.project, project.url);
                println!("    Dir: {}", project.dir);
                print_hosts(&project.daemons, "    ");
                for wt in &project.worktrees {
                    println!("    {} — {}", wt.worktree, wt.url);
                    println!("      Dir: {}", wt.dir);
                    print_hosts(&wt.daemons, "      ");
                }
                println!();
            }
        }

        Ok(())
    }
}

/// Print one checkout's daemon hostnames under a heading.
fn print_hosts(hosts: &[JsonProxyHost], indent: &str) {
    if hosts.is_empty() {
        println!("{indent}(no daemon with a port)");
        return;
    }
    for host in hosts {
        let port = host
            .port
            .map(|p| format!(" (port {p})"))
            .unwrap_or_default();
        println!(
            "{indent}{} — {} [{}{port}]",
            host.daemon, host.url, host.status
        );
    }
}

/// Build the hostname listing from the proxy's own hostname registry, so the
/// output matches exactly what the proxy will route.
fn collect_projects(
    scheme: &str,
    tld: &str,
    effective_port: u16,
    standard_port: u16,
    state_file: &Option<crate::state_file::StateFile>,
) -> Vec<JsonProxyProject> {
    let url = |host: &str| {
        if effective_port == standard_port {
            format!("{scheme}://{host}.{tld}")
        } else {
            format!("{scheme}://{host}.{tld}:{effective_port}")
        }
    };
    let hosts = |registry: &crate::proxy::hostname::HostRegistry,
                 checkout: &crate::proxy::hostname::CheckoutHosts,
                 suffix: &str| {
        checkout
            .labels()
            .into_iter()
            .filter(|label| {
                // The listing shows only what the proxy will route.
                crate::proxy::hostname::hostname_fits(&format!("{label}.{suffix}"))
            })
            .map(|label| {
                let name = checkout.daemons.get(&label).cloned().unwrap_or_default();
                let daemon = state_file.as_ref().and_then(|sf| {
                    sf.daemons
                        .iter()
                        .find(|(id, _)| id.name() == name && id.namespace() == checkout.namespace)
                        .map(|(_, d)| d)
                });
                // Checkouts that share a namespace share one state record, so a
                // record from another checkout says nothing about this one.
                let other_checkout = daemon.is_some_and(|d| {
                    registry.shares_daemon_id(&checkout.namespace, &name)
                        && !d.dir.as_deref().is_some_and(|dir| {
                            crate::proxy::hostname::checkout_root_of(dir) == checkout.dir
                        })
                });
                let (status, port) = match daemon {
                    _ if other_checkout => ("other checkout".to_string(), None),
                    Some(d) if d.status.is_running() => (
                        "running".to_string(),
                        d.active_port.or_else(|| d.resolved_port.first().copied()),
                    ),
                    Some(d) => (d.status.to_string(), None),
                    None => ("available".to_string(), None),
                };
                let host = format!("{label}.{suffix}");
                JsonProxyHost {
                    daemon: name,
                    url: url(&host),
                    host,
                    status,
                    port,
                }
            })
            .collect::<Vec<_>>()
    };

    let registry = crate::proxy::hostname::HostRegistry::build();
    for err in &registry.errors {
        eprintln!("warning: {err}");
    }
    registry
        .project_labels()
        .into_iter()
        .filter_map(|label| {
            let project = registry.projects.get(&label)?;
            let worktrees = project
                .worktree_labels()
                .into_iter()
                .filter_map(|wt_label| {
                    let checkout = project.worktrees.get(&wt_label)?;
                    let suffix = format!("{wt_label}.{label}");
                    Some(JsonProxyWorktree {
                        url: url(&suffix),
                        daemons: hosts(&registry, checkout, &suffix),
                        worktree: wt_label,
                        dir: checkout.dir.display().to_string(),
                    })
                })
                .collect();
            Some(JsonProxyProject {
                url: url(&label),
                daemons: hosts(&registry, &project.primary, &label),
                dir: project.primary.dir.display().to_string(),
                project: label,
                worktrees,
            })
        })
        .collect()
}

// ─── proxy add ───────────────────────────────────────────────────────────────

/// Add a slug mapping to the global config (legacy)
///
/// Registers a slug in ~/.config/pitchfork/config.toml that maps to a project
/// directory and daemon name. The proxy resolves slugs before the automatic
/// per-daemon hostnames, such as api.myproject.localhost, which need no
/// registration.
///
/// If --dir is not specified, uses the current directory.
/// If --daemon is not specified, defaults to the slug name.
///
/// Example:
///
/// ```text
/// pitchfork proxy add api
/// pitchfork proxy add api --daemon server
/// pitchfork proxy add api --dir /home/user/my-api --daemon server
/// ```
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
struct Add {
    /// The slug name (used in proxy URLs, e.g. api → api.localhost)
    slug: String,
    /// Project directory (defaults to current directory)
    #[usage(long)]
    dir: Option<std::path::PathBuf>,
    /// Daemon name within the project (defaults to slug name)
    #[usage(long)]
    daemon: Option<String>,
    /// Namespace to associate with the slug. If not provided, derived from the project directory.
    #[usage(long)]
    namespace: Option<String>,
}

impl Add {
    async fn run(&self) -> Result<()> {
        use crate::pitchfork_toml::PitchforkToml;

        // Validate slug characters
        let slug = &self.slug;
        if slug.is_empty() {
            miette::bail!("Slug must be non-empty.");
        }
        if slug.contains('.') {
            miette::bail!(
                "Slug '{slug}' contains a dot ('.'). \
                 Slugs must not contain dots because they are used as \
                 DNS subdomain labels in proxy URLs (<slug>.<tld>)."
            );
        }
        if !slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            miette::bail!(
                "Slug '{slug}' contains invalid characters. \
                 Slugs must be alphanumeric with '-' and '_' allowed."
            );
        }

        let dir = self
            .dir
            .as_ref()
            .map(crate::env::expand_tilde)
            .unwrap_or_else(|| crate::env::CWD.clone());
        let dir = dir.canonicalize().unwrap_or(dir);

        let daemon = self.daemon.as_deref();

        let resolved_ns = self
            .namespace
            .clone()
            .or_else(|| crate::pitchfork_toml::PitchforkToml::namespace_for_dir(&dir).ok());

        let Some(resolved_ns) = resolved_ns else {
            miette::bail!(
                "Cannot derive a namespace for '{}'. \
                 Make sure the directory contains a pitchfork.toml with a valid `namespace`, \
                 or provide an explicit `--namespace`.",
                dir.display()
            );
        };

        // Auto-register namespace if not already registered
        let namespaces = crate::pitchfork_toml::PitchforkToml::read_global_namespaces();
        if !namespaces.contains_key(&resolved_ns) {
            crate::pitchfork_toml::PitchforkToml::register_namespace(
                &resolved_ns,
                &dir.to_string_lossy(),
            )?;
            println!("Registered namespace '{resolved_ns}' at {}", dir.display());
        }

        // Don't store daemon name if it matches the slug (it defaults to slug)
        let stored_daemon = if daemon == Some(slug.as_str()) {
            None
        } else {
            daemon
        };

        PitchforkToml::add_slug_with_namespace(slug, Some(&resolved_ns), stored_daemon)?;

        // Notify the supervisor so it can update mDNS records.
        if let Ok(client) = crate::ipc::client::IpcClient::connect(false).await {
            let _ = client.sync_mdns().await;
        }

        let global_path = &*crate::env::PITCHFORK_GLOBAL_CONFIG_USER;
        let daemon_display = daemon.unwrap_or(slug);
        println!("Added slug '{slug}' → namespace '{resolved_ns}' (daemon: {daemon_display})");
        println!("  Config: {}", global_path.display());

        let s = crate::settings::settings();
        if s.proxy.enable {
            let scheme = if s.proxy.https { "https" } else { "http" };
            let lan_enabled = s.proxy.lan || !s.proxy.lan_ip.is_empty();
            let tld = if lan_enabled { "local" } else { &s.proxy.tld };
            let standard_port = if s.proxy.https { 443u16 } else { 80u16 };
            if let Some(effective_port) = u16::try_from(s.proxy.port).ok().filter(|&p| p > 0) {
                let url = if effective_port == standard_port {
                    format!("{scheme}://{slug}.{tld}")
                } else {
                    format!("{scheme}://{slug}.{tld}:{effective_port}")
                };
                println!("  URL:    {url}");
            }
        }

        Ok(())
    }
}

// ─── proxy remove ────────────────────────────────────────────────────────────

/// Remove a slug mapping from the global config
///
/// Example:
///
/// ```text
/// pitchfork proxy remove api
/// ```
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
struct Remove {
    /// The slug name to remove
    slug: String,
}

impl Remove {
    async fn run(&self) -> Result<()> {
        use crate::pitchfork_toml::PitchforkToml;

        if PitchforkToml::remove_slug(&self.slug)? {
            println!("Removed slug '{}'", self.slug);

            // Notify the supervisor so it can update mDNS records.
            if let Ok(client) = crate::ipc::client::IpcClient::connect(false).await {
                let _ = client.sync_mdns().await;
            }
        } else {
            println!("Slug '{}' was not registered.", self.slug);
        }

        Ok(())
    }
}
