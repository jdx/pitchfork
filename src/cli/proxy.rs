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

    setup     Point this machine's DNS, trust store and ports at the proxy
    doctor    Check everything a proxy URL needs in order to work
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
    Setup(Setup),
    Doctor(Doctor),
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
            ProxyCommands::Setup(setup) => setup.run().await,
            ProxyCommands::Doctor(doctor) => doctor.run().await,
            ProxyCommands::Trust(trust) => trust.run().await,
            ProxyCommands::Untrust(untrust) => untrust.run().await,
            ProxyCommands::Status(status) => status.run().await,
            ProxyCommands::Add(add) => add.run().await,
            ProxyCommands::Remove(remove) => remove.run().await,
        }
    }
}

// ─── proxy setup ─────────────────────────────────────────────────────────────

/// Configure local proxy DNS, HTTPS trust, and standard ports
///
/// Configure hostname resolution, HTTPS certificate trust, and access through
/// port 443 (or 80 for HTTP). Prints a plan and asks for confirmation before
/// applying changes. Use `--dry-run` to preview the plan without applying it.
///
/// The supervisor can run as your normal user. Setup uses sudo for system DNS
/// files, Linux CA trust, and port redirects or Linux bind capabilities. On
/// macOS, choose an unprivileged `proxy.port`, such as 8443, for the redirect.
/// Linux can also grant permission to bind the default port, 443, directly.
///
/// With `--pac`, applications that honor proxy settings use pitchfork's proxy
/// auto-config file instead of system DNS. This skips DNS changes and port
/// redirects. Use an unprivileged listener port to avoid bind privileges;
/// Linux still needs sudo for CA trust if the CA is not already trusted.
/// macOS may request authorization for keychain or network settings changes.
///
/// Use `--undo` to reverse setup using its saved configuration records, even
/// after proxy settings change. Files and proxy settings are checked for
/// pitchfork ownership before removal.
///
/// Example:
///
/// ```text
/// pitchfork proxy setup --dry-run
/// pitchfork proxy setup
/// pitchfork proxy doctor
/// pitchfork proxy setup --undo
/// ```
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
struct Setup {
    /// Configure a proxy auto-config (PAC) file instead of the system resolver
    #[usage(long)]
    pac: bool,
    /// Remove recorded setup resources and restore saved proxy settings
    #[usage(long)]
    undo: bool,
    /// Apply without asking for confirmation
    #[usage(long, short = 'y')]
    yes: bool,
    /// Print the plan and exit without changing anything
    #[usage(long)]
    dry_run: bool,
}

impl Setup {
    async fn run(&self) -> Result<()> {
        use crate::proxy::setup;

        let s = crate::settings::settings();
        if !s.proxy.enable && !self.undo {
            println!("Proxy: disabled");
            println!();
            println!("Enable it first, then re-run setup:");
            println!("  pitchfork settings set proxy.enable true");
            return Ok(());
        }

        // Refuse a TLD that would steer a privileged write somewhere it does
        // not belong, or a port the proxy itself would refuse to listen on,
        // before any plan is built from them. The TLD checked is the one the
        // plan uses: LAN mode always serves `local`, whatever `proxy.tld` says.
        //
        // Undoing is not refused for them. Changing `proxy.tld` or
        // `proxy.port` to something invalid after a setup must not strand the
        // resolver files, redirects and trust-store changes that setup really
        // installed; those come from the records, which were validated when
        // written. Only the current-settings half of the plan is dropped.
        let settings_error = match setup::validate_tld(crate::proxy::effective_tld(&s))
            .and_then(|()| setup::validate_proxy_port(s.proxy.port))
        {
            Ok(()) => None,
            Err(e) if self.undo => Some(e),
            Err(e) => return Err(e),
        };

        // Held for the whole run: reading the system, reading the records,
        // building the plan, writing the record, applying it and clearing it
        // are one transaction. A second run that read the records while this
        // one was applying would plan against a state about to change.
        let _lock = setup::lock_setup()?;

        let ctx = setup::context_from_settings(&s, self.pac);
        // Undo reverses what the last setup recorded, not what the settings
        // happen to say now: `proxy.port`, `proxy.tld` or `proxy.tls_cert` may
        // have changed since, and a plan built from the new values would probe
        // for resources that were never installed while leaving the real ones
        // behind.
        let recorded = setup::load_records();
        let plan = if self.undo {
            if let Some(e) = &settings_error {
                println!("Ignoring the current proxy settings: {e}");
                println!("Undoing from the recorded setups only.");
                println!();
            }
            setup::plan_undo_from(settings_error.is_none().then_some(&ctx), &recorded)
        } else {
            // Removes what an earlier setup installed that this one replaces,
            // before installing the new configuration.
            setup::plan_with_reconcile(&ctx, &recorded)
        };

        let heading = if self.undo {
            "This will undo the following:"
        } else {
            "This will do the following:"
        };
        println!("{heading}");
        println!();
        for line in plan.describe() {
            println!("  {line}");
        }
        println!();

        if self.dry_run {
            for note in &plan.manual {
                println!("{note}");
                println!();
            }
            return Ok(());
        }

        if plan.is_empty() {
            println!("Nothing to change.");
            for note in &plan.manual {
                println!();
                println!("{note}");
            }
            return Ok(());
        }

        if !self.yes && !confirm(plan.needs_sudo())? {
            println!("Aborted. Nothing was changed.");
            return Ok(());
        }

        println!();
        if !self.undo {
            // Recorded before the steps run, so a run that fails halfway still
            // leaves something for `--undo` to reverse.
            setup::save_record(&ctx);
        }
        // `apply` writes files and shells out to sudo, all of it blocking. The
        // confirmation prompt above stays on this thread — interleaving other
        // work with a password prompt would be worse, not better — but the
        // steps themselves move off the runtime.
        let plan_for_apply = plan.clone();
        let report = tokio::task::spawn_blocking(move || setup::apply(&plan_for_apply))
            .await
            .map_err(|e| miette::miette!("`pitchfork proxy setup` panicked: {e}"))?;
        println!();
        println!(
            "{} step(s) applied, {} already in place.",
            report.applied, report.skipped
        );
        for note in &plan.manual {
            println!();
            println!("{note}");
        }
        if !report.failed.is_empty() {
            println!();
            println!("The following steps failed:");
            for (summary, err) in &report.failed {
                println!("  {summary}");
                println!("    {err}");
            }
            miette::bail!("`pitchfork proxy setup` did not complete");
        }
        if self.undo {
            // Only once everything came back cleanly; a partial undo still has
            // something left to reverse on a later run.
            setup::clear_record();
        } else {
            // The supervisor binds its port once, at startup, so a redirect
            // repointed at a new `proxy.port` reaches nothing until it restarts.
            if let Some(previous) = recorded.last().map(|r| r.proxy_port)
                && previous != ctx.proxy_port
            {
                println!();
                println!(
                    "proxy.port changed from {previous} to {}. Restart the supervisor so it \
                     listens there:",
                    ctx.proxy_port
                );
                println!("  pitchfork supervisor start --force");
            }
            println!();
            println!("Check the result with: pitchfork proxy doctor");
        }
        Ok(())
    }
}

/// Ask for confirmation on stdin. A non-interactive stdin declines.
fn confirm(needs_sudo: bool) -> Result<bool> {
    use std::io::{BufRead, Write};

    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        println!("Not running interactively — re-run with --yes to apply.");
        return Ok(false);
    }
    if needs_sudo {
        println!("Some steps need sudo and will prompt for your password.");
    }
    print!("Continue? [y/N] ");
    std::io::stdout()
        .flush()
        .map_err(|e| miette::miette!("Failed to write prompt: {e}"))?;
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|e| miette::miette!("Failed to read confirmation: {e}"))?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

// ─── proxy doctor ────────────────────────────────────────────────────────────

/// Diagnose proxy connectivity, hostname resolution, and HTTPS trust
///
/// Prints one line per check: the proxy listener, the loopback DNS resolver,
/// whether a random name under your TLD resolves through the system resolver,
/// certificate trust, and whether the standard port reaches the proxy. When
/// system proxy settings use pitchfork's PAC URL, checks PAC availability
/// instead of requiring system DNS resolution.
///
/// Exits with a nonzero status if any check fails. Warnings alone do not fail
/// the command.
///
/// Example:
///
/// ```text
/// pitchfork proxy doctor
/// ```
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
struct Doctor {}

impl Doctor {
    async fn run(&self) -> Result<()> {
        let s = crate::settings::settings();
        let checks = crate::proxy::doctor::run(&s).await;
        for check in &checks {
            println!("{}", check.line());
        }
        let failed = checks
            .iter()
            .filter(|c| c.status == crate::proxy::doctor::Status::Fail)
            .count();
        if failed > 0 {
            println!();
            println!("{failed} check(s) failed. Run `pitchfork proxy setup` to fix them.");
            // Non-zero, so `pitchfork proxy doctor || setup-the-proxy` works
            // and a CI step gating on this command does not read a broken
            // proxy as a healthy one. Warnings do not count: they are the
            // checks that could not reach a verdict, and failing on those
            // would make the command unusable in a script.
            miette::bail!("{failed} proxy check(s) failed");
        }
        Ok(())
    }
}

// ─── proxy trust ─────────────────────────────────────────────────────────────

/// Install the proxy CA certificate into the system trust store
///
/// This command installs pitchfork's generated CA certificate into your
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
/// Requires sudo on Linux.
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
    /// Path to the certificate file to trust (defaults to pitchfork's generated CA)
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

/// Remove the proxy CA certificate from the system trust store
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
    /// Path to the certificate file (defaults to pitchfork's generated CA)
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
                    conflicts: vec![],
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
                    conflicts: vec![],
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

        let (projects, conflicts) =
            collect_projects(scheme, tld, effective_port, standard_port, &state_file);

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
                conflicts,
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

        if !conflicts.is_empty() {
            println!();
            println!("Conflicts:");
            println!();
            for conflict in &conflicts {
                println!("  {conflict}");
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
) -> (Vec<JsonProxyProject>, Vec<String>) {
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
                let name = checkout
                    .daemons
                    .get(&label)
                    .map(|d| d.name.clone())
                    .unwrap_or_default();
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
    let conflicts = registry.errors.clone();
    let projects = registry
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
        .collect();
    (projects, conflicts)
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
        // The slug becomes a single DNS label in `<slug>.<tld>`, so it has to
        // fit a label (63 bytes) as well as leave the whole host name inside
        // 253. A short TLD does not excuse an over-long label.
        //
        // Measured against the TLD actually in force, which LAN mode replaces
        // with `local`; `proxy.tld` alone would check the wrong suffix.
        let settings = crate::settings::settings();
        let tld = crate::proxy::effective_tld(&settings);
        if !crate::proxy::pac::hostname_fits(slug, tld) {
            miette::bail!(
                "Slug '{slug}' is too long: it must be at most 63 bytes as a DNS \
                 label, and '{slug}.{tld}' at most 253 bytes as a host name."
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
