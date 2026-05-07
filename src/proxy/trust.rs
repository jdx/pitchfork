//! CA certificate trust management for the reverse proxy.
//!
//! Provides functions to:
//! - Check if the pitchfork CA is trusted by the system (`is_ca_trusted`)
//! - Install the CA into the system trust store (`install_cert`)
//! - Remove the CA from the system trust store (`uninstall_cert`)
//! - Auto-trust the CA during supervisor startup (`auto_trust`)

use crate::Result;

/// File name for the installed CA certificate on Linux.
const INSTALLED_CERT_NAME: &str = "pitchfork-proxy.crt";

// ---------------------------------------------------------------------------
// is_ca_trusted
// ---------------------------------------------------------------------------

/// Check if the pitchfork CA certificate is already trusted by the system.
///
/// Always queries the OS trust store directly. This is correct even when the
/// user manually removes the cert from their keychain or CA directory — the
/// check will reflect the actual state rather than a stale cached value.
pub fn is_ca_trusted(cert_path: &std::path::Path) -> bool {
    if !cert_path.exists() {
        return false;
    }

    #[cfg(target_os = "macos")]
    {
        is_ca_trusted_macos(cert_path)
    }
    #[cfg(target_os = "linux")]
    {
        is_ca_trusted_linux(cert_path)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
fn is_ca_trusted_macos(cert_path: &std::path::Path) -> bool {
    use std::process::{Command, Stdio};
    // Use verify-cert without -L -p ssl. The SSL policy evaluates the cert as
    // a leaf certificate (checking for serverAuth EKU etc.), which a CA cert
    // typically lacks. Without a policy, verify-cert respects the explicit
    // trustRoot trust override without applying leaf-oriented constraints.
    //
    // Suppress stdout/stderr to prevent security framework diagnostic messages
    // from leaking into the terminal (e.g. during `proxy status` or supervisor
    // startup when the cert is not yet trusted).
    Command::new("security")
        .args(["verify-cert", "-c", &cert_path.to_string_lossy()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Linux distro CA trust configuration.
struct LinuxCATrustConfig {
    cert_dir: &'static str,
    /// Update command split into program + args.
    update_command: &'static [&'static str],
}

#[cfg(target_os = "linux")]
fn get_linux_ca_trust_config() -> LinuxCATrustConfig {
    let configs = [
        // Debian / Ubuntu
        LinuxCATrustConfig {
            cert_dir: "/usr/local/share/ca-certificates",
            update_command: &["update-ca-certificates"],
        },
        // RHEL / Fedora / CentOS
        LinuxCATrustConfig {
            cert_dir: "/etc/pki/ca-trust/source/anchors",
            update_command: &["update-ca-trust"],
        },
        // Arch Linux (p11-kit / ca-certificates-utils)
        LinuxCATrustConfig {
            cert_dir: "/etc/ca-certificates/trust-source/anchors",
            update_command: &["trust", "extract-compat"],
        },
        // openSUSE
        LinuxCATrustConfig {
            cert_dir: "/etc/pki/trust/anchors",
            update_command: &["update-ca-certificates"],
        },
    ];

    // Find the first config whose cert_dir exists
    for config in &configs {
        if std::path::Path::new(config.cert_dir).exists() {
            return LinuxCATrustConfig {
                cert_dir: config.cert_dir,
                update_command: config.update_command,
            };
        }
    }

    // Fallback to Debian layout
    configs.into_iter().next().unwrap()
}

#[cfg(target_os = "linux")]
fn is_ca_trusted_linux(cert_path: &std::path::Path) -> bool {
    let config = get_linux_ca_trust_config();
    let installed_path = std::path::Path::new(config.cert_dir).join(INSTALLED_CERT_NAME);
    if !installed_path.exists() {
        return false;
    }
    // Compare file contents
    let ours = std::fs::read(cert_path).unwrap_or_default();
    let installed = std::fs::read(&installed_path).unwrap_or_default();
    ours == installed
}

// ---------------------------------------------------------------------------
// install_cert (shared between auto_trust and `proxy trust` command)
// ---------------------------------------------------------------------------

/// Install the CA certificate into the system trust store.
///
/// On macOS, installs into the current user's login keychain (no sudo required;
/// the OS shows a GUI authorization prompt to confirm).
///
/// On Linux, copies to the distro-specific CA directory and runs the
/// appropriate update command (requires sudo / write access).
pub fn install_cert(cert_path: &std::path::Path) -> Result<()> {
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

    #[cfg(target_os = "macos")]
    {
        install_cert_macos(cert_path)?;
    }
    #[cfg(target_os = "linux")]
    {
        install_cert_linux(cert_path)?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        miette::bail!(
            "Automatic certificate installation is not supported on this platform.\n\
             Please manually install the certificate from:\n\
             {}",
            cert_path.display()
        );
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn install_cert_macos(cert_path: &std::path::Path) -> Result<()> {
    use std::process::Command;

    let home = &*crate::env::HOME_DIR;
    let keychain = format!("{}/Library/Keychains/login.keychain-db", home.display());

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
fn install_cert_linux(cert_path: &std::path::Path) -> Result<()> {
    use std::ffi::CString;
    use std::process::Command;

    let config = get_linux_ca_trust_config();
    let dest = std::path::Path::new(config.cert_dir).join(INSTALLED_CERT_NAME);

    // Check write access using libc::access(W_OK)
    let has_write_access = {
        let path_cstr =
            CString::new(config.cert_dir.as_bytes()).unwrap_or_else(|_| CString::new("/").unwrap());
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
             This copies the certificate to {}/\n\
             and runs `{}`.",
            config.cert_dir,
            config.update_command.join(" ")
        );
    }

    std::fs::copy(cert_path, &dest)
        .map_err(|e| miette::miette!("Failed to copy certificate to {}: {e}", dest.display()))?;

    let status = Command::new(config.update_command[0])
        .args(&config.update_command[1..])
        .status()
        .map_err(|e| miette::miette!("Failed to run `{}`: {e}", config.update_command.join(" ")))?;

    if !status.success() {
        // Clean up the copied cert so is_ca_trusted_linux won't falsely
        // report it as trusted due to file-content equality.
        let _ = std::fs::remove_file(&dest);
        miette::bail!(
            "`{}` failed (exit code: {}).\n\
             \n\
             The system trust store was NOT updated.\n\
             To install manually:\n\
             sudo cp {} {}\n\
             sudo {}",
            config.update_command.join(" "),
            status.code().unwrap_or(-1),
            cert_path.display(),
            dest.display(),
            config.update_command.join(" ")
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// uninstall_cert
// ---------------------------------------------------------------------------

/// Remove the pitchfork CA certificate from the system trust store.
///
/// Handles the case where `cert_path` no longer exists but the cert is still
/// installed in the system trust store (e.g. the user deleted `ca.pem`).
pub fn uninstall_cert(cert_path: &std::path::Path) -> Result<()> {
    // Even if cert_path is gone, the cert may still be installed in the
    // system trust store. Always attempt platform-specific cleanup.
    #[cfg(target_os = "macos")]
    {
        uninstall_cert_macos(cert_path)?;
    }
    #[cfg(target_os = "linux")]
    {
        uninstall_cert_linux(cert_path)?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        if !cert_path.exists() || !is_ca_trusted(cert_path) {
            return Ok(());
        }
        miette::bail!("Automatic certificate removal is not supported on this platform.");
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_cert_macos(cert_path: &std::path::Path) -> Result<()> {
    use std::process::Command;

    // remove-trusted-cert removes the trust setting (requires the cert file)
    if cert_path.exists() {
        let _ = Command::new("security")
            .args(["remove-trusted-cert", &cert_path.to_string_lossy()])
            .status();
    }

    // Determine the CN for delete-certificate.
    // If the cert file exists, extract the CN from it. If extraction fails
    // (e.g. openssl missing), skip delete-certificate to avoid deleting the
    // wrong entry — remove-trusted-cert already removed the trust setting,
    // so the remaining keychain entry is harmless.
    // If the cert file is gone, assume the default CN since pitchfork
    // generated it.
    let cn = if cert_path.exists() {
        match cert_common_name_macos(cert_path) {
            Some(cn) => Some(cn),
            None => {
                log::warn!(
                    "Could not determine certificate CN; skipping keychain deletion. \
                     The trust setting has been removed. To delete the certificate \
                     from the keychain manually, run:\n  \
                     security delete-certificate -c \"<CN>\" ~/Library/Keychains/login.keychain-db"
                );
                None
            }
        }
    } else {
        Some("Pitchfork Local CA".to_string())
    };

    if let Some(cn) = cn {
        // delete-certificate removes from keychain(s)
        let keychains = [
            format!(
                "{}/Library/Keychains/login.keychain-db",
                crate::env::HOME_DIR.display()
            ),
            "/Library/Keychains/System.keychain".to_string(),
        ];
        for kc in &keychains {
            // Loop to remove all matching certs (there may be duplicates)
            for _ in 0..20 {
                let status = Command::new("security")
                    .args(["delete-certificate", "-c", &cn, kc])
                    .status();
                if status.map(|s| !s.success()).unwrap_or(true) {
                    break;
                }
            }
        }
    }

    // Verify removal (only possible if cert file still exists)
    if cert_path.exists() && is_ca_trusted_macos(cert_path) {
        miette::bail!("Could not remove CA from keychain. Try: sudo pitchfork proxy untrust");
    }
    Ok(())
}

/// Extract the Common Name (CN) from a PEM certificate file using `openssl`.
#[cfg(target_os = "macos")]
fn cert_common_name_macos(cert_path: &std::path::Path) -> Option<String> {
    use std::process::Command;
    // Use -nameopt RFC2253 to get a stable, escaped format, then extract CN.
    let output = Command::new("openssl")
        .args([
            "x509",
            "-noout",
            "-subject",
            "-nameopt",
            "RFC2253",
            "-in",
            &cert_path.to_string_lossy(),
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // RFC2253 format: "CN=Pitchfork Local CA,O=Org"
    // Escaped commas in values appear as \, so split on unescaped commas only.
    let subject = String::from_utf8_lossy(&output.stdout);
    extract_cn_from_subject_rfc2253(&subject)
}

/// Extract the CN from an RFC 2253 formatted subject line.
///
/// RFC 2253 uses comma-separated RDNs with backslash-escaping.
/// Example: `subject=CN=Pitchfork Local CA,O=Org` or
/// `subject=O=Org,CN=Pitchfork Local CA`
#[cfg(target_os = "macos")]
fn extract_cn_from_subject_rfc2253(subject: &str) -> Option<String> {
    let subject = subject.trim();
    let subject = subject.strip_prefix("subject=").unwrap_or(subject);
    for rdn in split_rdn(subject) {
        let rdn = rdn.trim();
        if let Some(rest) = rdn.strip_prefix("CN=") {
            let cn = rest.trim();
            if !cn.is_empty() {
                return Some(cn.to_string());
            }
        }
    }
    None
}

/// Split a subject string on unescaped commas (RFC 2253 escaping).
///
/// A comma preceded by a backslash is part of the value, not a separator.
#[cfg(target_os = "macos")]
fn split_rdn(subject: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (i, ch) in subject.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == ',' {
            parts.push(&subject[start..i]);
            start = i + ','.len_utf8();
        }
    }
    if start < subject.len() {
        parts.push(&subject[start..]);
    }
    parts
}

#[cfg(target_os = "linux")]
fn uninstall_cert_linux(cert_path: &std::path::Path) -> Result<()> {
    use std::ffi::CString;
    use std::process::Command;

    let config = get_linux_ca_trust_config();
    let installed_path = std::path::Path::new(config.cert_dir).join(INSTALLED_CERT_NAME);

    if !installed_path.exists() {
        return Ok(());
    }

    // Check write access before attempting removal
    let has_write_access = {
        let path_cstr =
            CString::new(config.cert_dir.as_bytes()).unwrap_or_else(|_| CString::new("/").unwrap());
        // SAFETY: path_cstr is a valid NUL-terminated C string.
        unsafe { libc::access(path_cstr.as_ptr(), libc::W_OK) == 0 }
    };

    if !has_write_access {
        miette::bail!(
            "Removing certificates on Linux requires elevated privileges.\n\
             \n\
             Run with sudo:\n\
             sudo pitchfork proxy untrust\n\
             \n\
             This removes the certificate from {}/\n\
             and runs `{}`.",
            config.cert_dir,
            config.update_command.join(" ")
        );
    }

    // If source cert exists, only remove if contents match (safety check
    // against deleting a cert we didn't install). If source is gone, remove
    // unconditionally — we own the file.
    let should_remove = if cert_path.exists() {
        let ours = std::fs::read(cert_path).unwrap_or_default();
        let installed = std::fs::read(&installed_path).unwrap_or_default();
        ours == installed
    } else {
        true
    };

    if should_remove {
        std::fs::remove_file(&installed_path)
            .map_err(|e| miette::miette!("Failed to remove {}: {e}", installed_path.display()))?;

        let status = Command::new(config.update_command[0])
            .args(&config.update_command[1..])
            .status()
            .map_err(|e| {
                miette::miette!("Failed to run `{}`: {e}", config.update_command.join(" "))
            })?;
        if !status.success() {
            miette::bail!(
                "`{}` failed (exit code: {}).\n\
                 The certificate was removed from {} but the system trust store was NOT updated.\n\
                 To complete the removal manually, run:\n\
                 sudo {}",
                config.update_command.join(" "),
                status.code().unwrap_or(-1),
                config.cert_dir,
                config.update_command.join(" ")
            );
        }
    }

    // Verify removal (only possible if source cert exists for content comparison)
    if cert_path.exists() && is_ca_trusted_linux(cert_path) {
        miette::bail!(
            "CA still trusted. Remove {}/{} manually and run `{}`.",
            config.cert_dir,
            INSTALLED_CERT_NAME,
            config.update_command.join(" ")
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// auto_trust
// ---------------------------------------------------------------------------

/// Result of an auto-trust attempt.
pub enum AutoTrustResult {
    /// CA was already trusted (no action needed).
    AlreadyTrusted,
    /// CA was successfully installed into the system trust store.
    Trusted,
    /// Auto-trust was skipped or failed (non-fatal).
    NotTrusted { reason: String },
}

/// Attempt to automatically install the CA certificate into the system trust
/// store during supervisor startup.
///
/// This is a best-effort operation: if it fails due to permissions or other
/// issues, it returns `NotTrusted` instead of an error. The user can then
/// manually run `pitchfork proxy trust`.
///
/// Auto trust may fail silently due to permissions; user can run
/// `pitchfork proxy trust` manually.
pub fn auto_trust(cert_path: &std::path::Path) -> AutoTrustResult {
    if !cert_path.exists() {
        return AutoTrustResult::NotTrusted {
            reason: "CA certificate not found".to_string(),
        };
    }

    if is_ca_trusted(cert_path) {
        return AutoTrustResult::AlreadyTrusted;
    }

    match install_cert(cert_path) {
        Ok(()) => AutoTrustResult::Trusted,
        Err(e) => AutoTrustResult::NotTrusted {
            reason: e.to_string(),
        },
    }
}
