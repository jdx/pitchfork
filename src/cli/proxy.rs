use crate::Result;

/// Manage the pitchfork reverse proxy
#[derive(Debug, clap::Args)]
#[clap(
    verbatim_doc_comment,
    long_about = "\
Manage the pitchfork reverse proxy

The reverse proxy routes requests from stable URLs like:
  http://api.myproject.localhost:7777

to the daemon's actual listening port (e.g. localhost:3000).

This gives daemons stable, human-friendly URLs that don't change when
ports are auto-bumped or reassigned.

Enable the proxy in your pitchfork.toml or settings:
  [settings.proxy]
  enable = true

Subcommands:
  trust    Install the proxy's TLS certificate into the system trust store"
)]
pub struct Proxy {
    #[clap(subcommand)]
    command: ProxyCommands,
}

#[derive(Debug, clap::Subcommand)]
enum ProxyCommands {
    Trust(Trust),
    Status(ProxyStatus),
}

impl Proxy {
    pub async fn run(&self) -> Result<()> {
        match &self.command {
            ProxyCommands::Trust(trust) => trust.run().await,
            ProxyCommands::Status(status) => status.run().await,
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
/// On Linux, this copies the certificate to /usr/local/share/ca-certificates/
/// and runs `update-ca-certificates`, which DOES require sudo.
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
        let cert_path = match &self.cert {
            Some(p) => p.clone(),
            None => {
                // Default: pitchfork's auto-generated CA cert in state dir
                crate::env::PITCHFORK_STATE_DIR.join("proxy").join("ca.pem")
            }
        };

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
    let home = std::env::var("HOME").map_err(|_| miette::miette!("$HOME is not set"))?;
    let keychain = format!("{home}/Library/Keychains/login.keychain-db");

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

    // Linux: requires sudo to write to /usr/local/share/ca-certificates/
    let dest = std::path::Path::new("/usr/local/share/ca-certificates/pitchfork-proxy.crt");
    let ca_certs_dir = std::path::Path::new("/usr/local/share/ca-certificates");

    // Check write access using libc::access(W_OK) which correctly reflects
    // effective UID/GID permissions, unlike Permissions::readonly() which only
    // inspects the owner-write bit and always returns false for directories.
    let has_write_access = {
        use std::ffi::CString;
        let path_cstr = CString::new(ca_certs_dir.to_string_lossy().as_bytes())
            .unwrap_or_else(|_| CString::new("/").unwrap());
        // SAFETY: path_cstr is a valid NUL-terminated C string.
        unsafe { libc::access(path_cstr.as_ptr(), libc::W_OK) == 0 }
    };

    if has_write_access {
        std::fs::copy(cert_path, dest)
            .map_err(|e| miette::miette!("Failed to copy certificate: {e}"))?;
    } else {
        miette::bail!(
            "Installing certificates on Linux requires elevated privileges.\n\
             \n\
             Run with sudo:\n\
             sudo pitchfork proxy trust\n\
             \n\
             This copies the certificate to /usr/local/share/ca-certificates/\n\
             and runs update-ca-certificates."
        );
    }

    let status = Command::new("update-ca-certificates")
        .status()
        .map_err(|e| miette::miette!("Failed to run `update-ca-certificates`: {e}"))?;

    if !status.success() {
        miette::bail!(
            "update-ca-certificates failed (exit code: {}).\n\
             \n\
             The certificate was copied to {} but the system trust store was NOT updated.\n\
             To complete the installation manually, run:\n\
             sudo update-ca-certificates",
            status.code().unwrap_or(-1),
            dest.display()
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

/// Show the current proxy configuration and status
#[derive(Debug, clap::Args)]
#[clap(verbatim_doc_comment)]
struct ProxyStatus {}

impl ProxyStatus {
    async fn run(&self) -> Result<()> {
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

        let effective_port = match u16::try_from(s.proxy.port).ok().filter(|&p| p > 0) {
            Some(p) => p,
            None => {
                println!("Proxy: enabled");
                println!(
                    "  ⚠  proxy.port {} is out of valid port range (1-65535)",
                    s.proxy.port
                );
                return Ok(());
            }
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

        if effective_port < 1024 {
            println!("⚠  Port {effective_port} is a privileged port (< 1024).");
            println!("   The supervisor must be started with sudo:");
            println!("   sudo pitchfork supervisor start");
            println!();
        }

        println!("Example URLs:");
        println!("  {scheme}://api.myproject.{tld}:{effective_port}  →  daemon 'myproject/api'");
        println!("  {scheme}://api.{tld}:{effective_port}             →  daemon 'global/api'");
        println!("  {scheme}://myapp.{tld}:{effective_port}           →  daemon with slug 'myapp'");

        Ok(())
    }
}
