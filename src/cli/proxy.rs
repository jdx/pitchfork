use crate::Result;
use crate::cli::json_output::{JsonLanInfo, JsonProxyStatus, JsonSlugEntry, print_json};

/// Manage the pitchfork reverse proxy
#[derive(Debug, clap::Args)]
#[clap(
    verbatim_doc_comment,
    long_about = "\
Manage the pitchfork reverse proxy

The reverse proxy routes requests from stable slug-based URLs like:
  https://myapp.localhost

to the daemon's actual listening port (e.g. localhost:3000).

Slugs are defined in the global config (~/.config/pitchfork/config.toml)
under [slugs]. Each slug maps to a project directory and daemon name.

Enable the proxy in your pitchfork.toml or settings:
  [settings.proxy]
  enable = true

Subcommands:
  trust     Install the proxy's TLS certificate into the system trust store
  untrust   Remove the proxy's TLS certificate from the system trust store
  add       Add a slug mapping to the global config
  remove    Remove a slug mapping from the global config
  status    Show all registered slugs and their current state"
)]
pub struct Proxy {
    #[clap(subcommand)]
    command: ProxyCommands,
}

#[derive(Debug, clap::Subcommand)]
enum ProxyCommands {
    Trust(Trust),
    Untrust(Untrust),
    Status(ProxyStatus),
    Add(Add),
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
///   pitchfork proxy trust
///   sudo pitchfork proxy trust    # Linux only
#[derive(Debug, clap::Args)]
#[clap(verbatim_doc_comment)]
struct Trust {
    /// Path to the certificate file to trust (defaults to pitchfork's auto-generated cert)
    #[clap(long)]
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
///   pitchfork proxy untrust
///   sudo pitchfork proxy untrust    # Linux only
#[derive(Debug, clap::Args)]
#[clap(verbatim_doc_comment)]
struct Untrust {
    /// Path to the certificate file (defaults to pitchfork's auto-generated cert)
    #[clap(long)]
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

/// Show all registered slugs and their current state
///
/// Displays the proxy configuration and lists all slugs from the global config
/// with their project directory, daemon name, and current status (running/stopped, port).
#[derive(Debug, clap::Args)]
#[clap(verbatim_doc_comment)]
struct ProxyStatus {
    /// Output in JSON format
    #[clap(long)]
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
        let state_file =
            crate::state_file::StateFile::read(&*crate::env::PITCHFORK_STATE_FILE).ok();
        let standard_port = if s.proxy.https { 443u16 } else { 80u16 };

        let slug_entries: Vec<JsonSlugEntry> = slugs
            .iter()
            .map(|(slug, entry)| {
                let daemon_name = entry.daemon.as_deref().unwrap_or(slug);
                let url = if effective_port == standard_port {
                    format!("{scheme}://{slug}.{tld}")
                } else {
                    format!("{scheme}://{slug}.{tld}:{effective_port}")
                };
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
                        ("not started".to_string(), None)
                    }
                } else {
                    ("unknown".to_string(), None)
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
                println!("    URL:    {}", entry.url);
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

        Ok(())
    }
}

// ─── proxy add ───────────────────────────────────────────────────────────────

/// Add a slug mapping to the global config
///
/// Registers a slug in ~/.config/pitchfork/config.toml that maps to a project
/// directory and daemon name. The proxy uses this to route requests.
///
/// If --dir is not specified, uses the current directory.
/// If --daemon is not specified, defaults to the slug name.
///
/// Example:
///   pitchfork proxy add api
///   pitchfork proxy add api --daemon server
///   pitchfork proxy add api --dir /home/user/my-api --daemon server
#[derive(Debug, clap::Args)]
#[clap(verbatim_doc_comment)]
struct Add {
    /// The slug name (used in proxy URLs, e.g. api → api.localhost)
    slug: String,
    /// Project directory (defaults to current directory)
    #[clap(long)]
    dir: Option<std::path::PathBuf>,
    /// Daemon name within the project (defaults to slug name)
    #[clap(long)]
    daemon: Option<String>,
    /// Namespace to associate with the slug. If not provided, derived from the project directory.
    #[clap(long)]
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

        let dir = self.dir.clone().unwrap_or_else(|| crate::env::CWD.clone());
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
///   pitchfork proxy remove api
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "rm", verbatim_doc_comment)]
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
