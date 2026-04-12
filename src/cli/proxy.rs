use crate::Result;

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
    Status(ProxyStatus),
    Add(Add),
    Remove(Remove),
}

impl Proxy {
    pub async fn run(&self) -> Result<()> {
        match &self.command {
            ProxyCommands::Trust(trust) => trust.run().await,
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

        if !cert_path.exists() {
            miette::bail!(
                "CA certificate not found at {}\n\
                 \n\
                 The proxy CA certificate is generated automatically when the proxy\n\
                 starts with `proxy.https = true`. Start the supervisor first:\n\
                 \n\
                 pitchfork supervisor start\n\
                 \n\
                 Or specify a custom certificate path with --cert.",
                cert_path.display()
            );
        }

        install_cert(&cert_path)?;
        println!(
            "✓ CA certificate installed: {}\n\
             \n\
             Browsers and tools will now trust HTTPS proxy URLs like:\n\
             https://docs.pf.localhost:7777",
            cert_path.display()
        );
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn install_cert(cert_path: &std::path::Path) -> Result<()> {
    use std::process::Command;

    // Resolve the login keychain path for the current user.
    let home = &*crate::env::HOME_DIR;
    let keychain = format!("{}/Library/Keychains/login.keychain-db", home.display());

    // Install into the current user's login keychain — no sudo required.
    // Must specify -k explicitly; without it macOS targets the admin domain
    // and silently succeeds without actually writing to the user keychain.
    let status = Command::new("security")
        .args([
            "add-trusted-cert",
            "-r",
            "trustRoot",
            "-k",
            &keychain,
            &cert_path.to_string_lossy(),
        ])
        .status()
        .map_err(|e| miette::miette!("Failed to run `security` command: {e}"))?;

    if !status.success() {
        miette::bail!(
            "Failed to install certificate (exit code: {}).\n\
             \n\
             Try running the command again.",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_cert(cert_path: &std::path::Path) -> Result<()> {
    use std::process::Command;

    // Detect the distro family by probing well-known CA anchor directories.
    // Each entry is (anchor_dir, dest_filename, update_command).
    // Priority order: check which directories actually exist on this system.
    let candidates: &[(&str, &str, &[&str])] = &[
        // Debian / Ubuntu
        (
            "/usr/local/share/ca-certificates",
            "pitchfork-proxy.crt",
            &["update-ca-certificates"],
        ),
        // RHEL / Fedora / CentOS / Rocky / AlmaLinux
        (
            "/etc/pki/ca-trust/source/anchors",
            "pitchfork-proxy.crt",
            &["update-ca-trust"],
        ),
        // Arch Linux (p11-kit / ca-certificates-utils)
        (
            "/etc/ca-certificates/trust-source/anchors",
            "pitchfork-proxy.crt",
            &["trust", "extract-compat"],
        ),
        // openSUSE / SLES
        (
            "/etc/pki/trust/anchors",
            "pitchfork-proxy.crt",
            &["update-ca-certificates"],
        ),
    ];

    let (anchor_dir, dest_name, update_cmd) = candidates
        .iter()
        .find(|(dir, _, _)| std::path::Path::new(dir).exists())
        .copied()
        .ok_or_else(|| {
            miette::miette!(
                "Could not detect a supported CA certificate directory on this system.\n\
                 \n\
                 Supported distributions: Debian/Ubuntu, RHEL/Fedora/CentOS, Arch Linux, openSUSE.\n\
                 \n\
                 Please install the certificate manually:\n\
                 1. Copy {} to your distro's CA anchor directory.\n\
                 2. Run the appropriate update command (e.g. update-ca-certificates).",
                cert_path.display()
            )
        })?;

    let dest = std::path::Path::new(anchor_dir).join(dest_name);

    // Check write access using libc::access(W_OK) which correctly reflects
    // effective UID/GID permissions, unlike Permissions::readonly() which only
    // inspects the owner-write bit and always returns false for directories.
    let has_write_access = {
        use std::ffi::CString;
        let path_cstr =
            CString::new(anchor_dir.as_bytes()).unwrap_or_else(|_| CString::new("/").unwrap());
        // SAFETY: path_cstr is a valid NUL-terminated C string.
        unsafe { libc::access(path_cstr.as_ptr(), libc::W_OK) == 0 }
    };

    if !has_write_access {
        miette::bail!(
            "Installing certificates on Linux requires elevated privileges.\n\
             \n\
             Run with sudo:\n\
             sudo pitchfork proxy trust\n\
             \n\
             This copies the certificate to {anchor_dir}/\n\
             and runs `{}`.",
            update_cmd.join(" ")
        );
    }

    std::fs::copy(cert_path, &dest)
        .map_err(|e| miette::miette!("Failed to copy certificate to {}: {e}", dest.display()))?;

    let status = Command::new(update_cmd[0])
        .args(&update_cmd[1..])
        .status()
        .map_err(|e| miette::miette!("Failed to run `{}`: {e}", update_cmd.join(" ")))?;

    if !status.success() {
        miette::bail!(
            "`{}` failed (exit code: {}).\n\
             \n\
             The certificate was copied to {} but the system trust store was NOT updated.\n\
             To complete the installation manually, run:\n\
             sudo {}",
            update_cmd.join(" "),
            status.code().unwrap_or(-1),
            dest.display(),
            update_cmd.join(" ")
        );
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn install_cert(_cert_path: &std::path::Path) -> Result<()> {
    miette::bail!(
        "Automatic certificate installation is not supported on this platform.\n\
         \n\
         Please manually install the certificate from:\n\
         {}",
        _cert_path.display()
    )
}

// ─── proxy status ─────────────────────────────────────────────────────────────

/// Show all registered slugs and their current state
///
/// Displays the proxy configuration and lists all slugs from the global config
/// with their project directory, daemon name, and current status (running/stopped, port).
#[derive(Debug, clap::Args)]
#[clap(verbatim_doc_comment)]
struct ProxyStatus {}

impl ProxyStatus {
    async fn run(&self) -> Result<()> {
        use crate::pitchfork_toml::PitchforkToml;
        use crate::settings::settings;
        let s = settings();

        if !s.proxy.enable {
            println!("Proxy: disabled");
            println!();
            println!("Enable with:");
            println!("  PITCHFORK_PROXY_ENABLE=true pitchfork supervisor start");
            println!("  # or in pitchfork.toml: [settings.proxy] / enable = true");
            return Ok(());
        }

        let Some(effective_port) = u16::try_from(s.proxy.port).ok().filter(|&p| p > 0) else {
            println!("Proxy: enabled");
            println!(
                "  ⚠  proxy.port {} is out of valid port range (1-65535)",
                s.proxy.port
            );
            return Ok(());
        };
        let scheme = if s.proxy.https { "https" } else { "http" };
        let tld = &s.proxy.tld;

        println!("Proxy: enabled");
        println!("  Scheme:  {scheme}");
        println!("  TLD:     {tld}");
        println!("  Port:    {effective_port}");
        if s.proxy.https {
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
            println!("  TLS cert: {cert}");
        }
        println!();

        // Show all registered slugs from global config
        let slugs = PitchforkToml::read_global_slugs();
        if slugs.is_empty() {
            println!("No slugs registered.");
            println!();
            println!("Add a slug with:");
            println!("  pitchfork proxy add <slug>");
            println!("  pitchfork proxy add <slug> --dir /path/to/project --daemon <name>");
        } else {
            println!("Registered slugs:");
            println!();

            // Read state file for daemon status
            let state_file =
                crate::state_file::StateFile::read(&*crate::env::PITCHFORK_STATE_FILE).ok();

            let standard_port = if s.proxy.https { 443u16 } else { 80u16 };

            for (slug, entry) in &slugs {
                let daemon_name = entry.daemon.as_deref().unwrap_or(slug);
                let url = if effective_port == standard_port {
                    format!("{scheme}://{slug}.{tld}")
                } else {
                    format!("{scheme}://{slug}.{tld}:{effective_port}")
                };

                // Try to find the daemon in the state file, scoped to the slug's
                // registered project directory to avoid picking the wrong daemon
                // when multiple projects have daemons with the same short name.
                let expected_ns =
                    crate::pitchfork_toml::PitchforkToml::namespace_for_dir(&entry.dir).ok();
                let status_str = if let Some(sf) = &state_file {
                    let daemon_entry = sf.daemons.iter().find(|(id, _)| {
                        id.name() == daemon_name
                            && match &expected_ns {
                                Some(ns) => id.namespace() == ns,
                                None => true,
                            }
                    });
                    if let Some((_, daemon)) = daemon_entry {
                        let port_str = daemon
                            .active_port
                            .or_else(|| daemon.resolved_port.first().copied())
                            .map(|p| format!(" (port {p})"))
                            .unwrap_or_default();
                        if daemon.status.is_running() {
                            format!("running{port_str}")
                        } else {
                            format!("{}", daemon.status)
                        }
                    } else {
                        "not started".to_string()
                    }
                } else {
                    "unknown".to_string()
                };

                println!("  {slug}");
                println!("    URL:    {url}");
                println!("    Dir:    {}", entry.dir.display());
                println!("    Daemon: {daemon_name}");
                println!("    Status: {status_str}");
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

        // Don't store daemon name if it matches the slug (it defaults to slug)
        let stored_daemon = if daemon == Some(slug.as_str()) {
            None
        } else {
            daemon
        };

        PitchforkToml::add_slug(slug, &dir, stored_daemon)?;

        let global_path = &*crate::env::PITCHFORK_GLOBAL_CONFIG_USER;
        let daemon_display = daemon.unwrap_or(slug);
        println!(
            "Added slug '{slug}' → {} (daemon: {daemon_display})",
            dir.display()
        );
        println!("  Config: {}", global_path.display());

        let s = crate::settings::settings();
        if s.proxy.enable {
            let scheme = if s.proxy.https { "https" } else { "http" };
            let tld = &s.proxy.tld;
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
        } else {
            println!("Slug '{}' was not registered.", self.slug);
        }

        Ok(())
    }
}
