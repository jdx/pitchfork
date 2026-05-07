//! /etc/hosts management for the reverse proxy.
//!
//! Automatically syncs registered slug hostnames into `/etc/hosts` so that
//! browsers can resolve them (needed for Safari, which doesn't auto-resolve
//! `.localhost` subdomains, and for custom TLDs like `.test`).
//!
//! Entries are managed inside a marked block delimited by
//! `# pitchfork-start` / `# pitchfork-end`.  The block is replaced on each
//! sync and removed entirely on proxy shutdown.

use crate::settings::settings;
use std::sync::OnceLock;

/// Marker lines for the pitchfork-managed block in /etc/hosts.
const MARKER_START: &str = "# pitchfork-start";
const MARKER_END: &str = "# pitchfork-end";
static BLANK_LINES_RE: OnceLock<regex::Regex> = OnceLock::new();

/// Path to the hosts file on the current platform.
fn hosts_path() -> std::path::PathBuf {
    if cfg!(windows) {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        std::path::PathBuf::from(system_root)
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts")
    } else {
        std::path::PathBuf::from("/etc/hosts")
    }
}

/// Sync all registered slug hostnames into /etc/hosts.
///
/// Reads the current slug table from global config, builds the expected
/// hosts block, and replaces (or appends) the pitchfork-managed block.
///
/// Best-effort: logs a warning on failure (e.g. permission denied) and
/// does not prevent proxy startup.
pub fn sync_hosts_file(bind_ip: &str, tld: &str) {
    let slugs = crate::pitchfork_toml::PitchforkToml::read_global_slugs();
    let mut entries: Vec<String> = Vec::new();
    for slug in slugs.keys() {
        entries.push(format!("{bind_ip} {slug}.{tld}"));
    }
    write_hosts_block(&entries);
}

/// Refresh `/etc/hosts` from the current settings if sync is enabled.
///
/// Used when slug registrations change while the proxy is already running.
/// In LAN mode, entries are mapped to the detected LAN IP instead of 127.0.0.1.
pub fn sync_hosts_from_settings() {
    let s = settings();
    if s.proxy.enable && s.proxy.sync_hosts {
        let lan_enabled = s.proxy.lan || !s.proxy.lan_ip.is_empty();
        let tld = if lan_enabled { "local" } else { &s.proxy.tld };
        // In LAN mode, map to the LAN IP (or configured lan_ip) so that
        // /etc/hosts entries work for mDNS-resolved hostnames on the LAN.
        let ip = if lan_enabled {
            // Try the configured lan_ip first, otherwise fall back to the proxy.host.
            if !s.proxy.lan_ip.is_empty() {
                s.proxy.lan_ip.clone()
            } else {
                // Auto-detected LAN IP is async and not available here.
                // Fall back to proxy.host (which defaults to 127.0.0.1
                // but may be overridden by the user).  The mDNS records
                // handle the actual resolution on the LAN.
                s.proxy.host.clone()
            }
        } else {
            s.proxy.host.clone()
        };
        sync_hosts_file(&ip, tld);
    }
}

/// Remove the pitchfork-managed block from /etc/hosts.
///
/// Called on proxy shutdown to clean up stale entries.
pub fn clean_hosts_file() {
    write_hosts_block(&[]);
}

/// Read /etc/hosts, replace the marked block, write back atomically.
fn write_hosts_block(entries: &[String]) {
    let path = hosts_path();

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            if !entries.is_empty() {
                log::warn!(
                    "Failed to read {} for hosts sync: {e}. \
                     Set proxy.sync_hosts = false to suppress this warning.",
                    path.display()
                );
            }
            return;
        }
    };

    let cleaned = remove_block(&content);

    let new_content = if entries.is_empty() {
        cleaned
    } else {
        let block = build_block(entries);
        format!("{}\n{block}\n", cleaned.trim_end())
    };

    // Atomic write: write to a temp file in the same directory, then rename.
    let parent = path.parent().unwrap_or(std::path::Path::new("/etc"));
    let tmp_path = parent.join(format!(".pitchfork-hosts-tmp-{}", std::process::id()));

    if let Err(e) = std::fs::write(&tmp_path, &new_content) {
        log::warn!(
            "Failed to write {} for hosts sync: {e}. \
             Writing to /etc/hosts may require sudo. \
             Set proxy.sync_hosts = false to suppress this warning.",
            tmp_path.display()
        );
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }

    if let Err(e) = std::fs::rename(&tmp_path, &path) {
        log::warn!(
            "Failed to rename {} to {}: {e}. \
             Writing to /etc/hosts may require sudo. \
             Set proxy.sync_hosts = false to suppress this warning.",
            tmp_path.display(),
            path.display()
        );
        let _ = std::fs::remove_file(&tmp_path);
    }
}

/// Build the pitchfork-managed block for the given entries.
fn build_block(entries: &[String]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let lines = entries.join("\n");
    format!("{MARKER_START}\n{lines}\n{MARKER_END}")
}

/// Remove the pitchfork-managed block from /etc/hosts content and return
/// the cleaned content with trailing newlines normalized.
fn remove_block(content: &str) -> String {
    let start_idx = match content.find(MARKER_START) {
        Some(i) => i,
        None => return content.to_string(),
    };
    let end_idx = match content[start_idx..].find(MARKER_END) {
        Some(i) => start_idx + i + MARKER_END.len(),
        None => return content.to_string(),
    };
    let before = &content[..start_idx];
    let after = &content[end_idx..];
    let result = format!("{before}{after}");
    // Normalize excessive blank lines caused by removing the block
    let re = BLANK_LINES_RE.get_or_init(|| regex::Regex::new(r"\n{3,}").unwrap());
    re.replace_all(&result, "\n\n").trim_end().to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_block() {
        let entries = vec![
            "127.0.0.1 myapp.localhost".to_string(),
            "127.0.0.1 api.myapp.localhost".to_string(),
        ];
        let block = build_block(&entries);
        assert!(block.starts_with("# pitchfork-start\n"));
        assert!(block.ends_with("\n# pitchfork-end"));
        assert!(block.contains("127.0.0.1 myapp.localhost"));
        assert!(block.contains("127.0.0.1 api.myapp.localhost"));
    }

    #[test]
    fn test_build_block_empty() {
        assert!(build_block(&[]).is_empty());
    }

    #[test]
    fn test_remove_block() {
        let content =
            "127.0.0.1 localhost\n# pitchfork-start\n127.0.0.1 myapp.localhost\n# pitchfork-end\n";
        let cleaned = remove_block(content);
        assert!(!cleaned.contains("pitchfork-start"));
        assert!(!cleaned.contains("myapp.localhost"));
        assert!(cleaned.contains("127.0.0.1 localhost"));
    }

    #[test]
    fn test_remove_block_no_markers() {
        let content = "127.0.0.1 localhost\n";
        let cleaned = remove_block(content);
        assert_eq!(cleaned, content);
    }

    #[test]
    fn test_remove_block_normalizes_blank_lines() {
        let content = "127.0.0.1 localhost\n\n\n# pitchfork-start\n127.0.0.1 myapp.localhost\n# pitchfork-end\n\n\n";
        let cleaned = remove_block(content);
        assert!(!cleaned.contains("\n\n\n"));
    }

    #[test]
    fn test_remove_block_ignores_end_marker_before_start_marker() {
        let content = "127.0.0.1 localhost\n# pitchfork-end\n# pitchfork-start\n127.0.0.1 myapp.localhost\n# pitchfork-end\n";
        let cleaned = remove_block(content);
        assert_eq!(cleaned, "127.0.0.1 localhost\n# pitchfork-end\n");
    }
}
