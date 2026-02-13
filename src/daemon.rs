use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::pitchfork_toml::CronRetrigger;
use indexmap::IndexMap;
use std::fmt::Display;
use std::path::PathBuf;

/// Validates a daemon ID to ensure it's safe for use in file paths and IPC.
///
/// A valid daemon ID:
/// - Is not empty
/// - Does not contain backslashes (`\`)
/// - Does not contain parent directory references (`..`)
/// - Does not contain spaces
/// - Does not contain `--` (reserved for path encoding of `/`)
/// - Is not `.` (current directory)
/// - Contains only printable ASCII characters
/// - If qualified (contains `/`), has exactly one `/` separating namespace and short ID
///
/// Format: `[namespace/]short_id`
/// - Qualified: `project/api`, `global/web`
/// - Short: `api`, `web`
///
/// This validation prevents path traversal attacks when daemon IDs are used
/// to construct log file paths or other filesystem operations.
pub fn is_valid_daemon_id(id: &str) -> bool {
    if id.is_empty()
        || id.contains('\\')
        || id.contains("..")
        || id.contains("--")
        || id.contains(' ')
        || id == "."
        || !id.chars().all(|c| c.is_ascii() && !c.is_ascii_control())
    {
        return false;
    }

    // Check for qualified ID format (namespace/short_id)
    let slash_count = id.chars().filter(|&c| c == '/').count();
    if slash_count > 1 {
        return false;
    }
    if slash_count == 1 {
        // Qualified ID: both namespace and short_id must be non-empty
        let (ns, short) = id.split_once('/').unwrap();
        return !ns.is_empty() && !short.is_empty();
    }
    true
}

/// Validates a short daemon ID (without namespace).
///
/// This is stricter than `is_valid_daemon_id` - it does not allow `/` at all.
/// Use this when validating user-provided daemon names in configuration files.
#[allow(dead_code)]
pub fn is_valid_short_daemon_id(id: &str) -> bool {
    !id.is_empty()
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains("..")
        && !id.contains("--")
        && !id.contains(' ')
        && id != "."
        && id.chars().all(|c| c.is_ascii() && !c.is_ascii_control())
}

/// Converts a daemon ID to a filesystem-safe path component.
///
/// Replaces `/` with `--` to avoid issues with filesystem path separators.
///
/// Examples:
/// - `"api"` → `"api"`
/// - `"global/api"` → `"global--api"`
/// - `"project-a/api"` → `"project-a--api"`
pub fn daemon_id_to_path(id: &str) -> String {
    id.replace('/', "--")
}

/// Converts a filesystem path component back to a daemon ID.
///
/// Replaces `--` with `/` to restore the original daemon ID.
///
/// Examples:
/// Returns the log directory path for a daemon.
///
/// The path is computed as: `$PITCHFORK_LOGS_DIR/{safe_id}/`
/// where `safe_id` has `/` replaced with `--` for filesystem safety.
///
/// Prefer using `DaemonId::log_dir()` when you have a structured ID.
#[allow(dead_code)]
pub fn daemon_log_dir(id: &str) -> std::path::PathBuf {
    crate::env::PITCHFORK_LOGS_DIR.join(daemon_id_to_path(id))
}

/// Returns the main log file path for a daemon.
///
/// The path is computed as: `$PITCHFORK_LOGS_DIR/{safe_id}/{safe_id}.log`
/// where `safe_id` has `/` replaced with `--` for filesystem safety.
///
/// Prefer using `DaemonId::log_path()` when you have a structured ID.
pub fn daemon_log_path(id: &str) -> std::path::PathBuf {
    let safe_id = daemon_id_to_path(id);
    crate::env::PITCHFORK_LOGS_DIR
        .join(&safe_id)
        .join(format!("{safe_id}.log"))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Daemon {
    pub id: DaemonId,
    pub title: Option<String>,
    pub pid: Option<u32>,
    pub shell_pid: Option<u32>,
    pub status: DaemonStatus,
    pub dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cmd: Option<Vec<String>>,
    pub autostop: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cron_schedule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cron_retrigger: Option<CronRetrigger>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_cron_triggered: Option<chrono::DateTime<chrono::Local>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_exit_success: Option<bool>,
    #[serde(default)]
    pub retry: u32,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_delay: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_http: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_cmd: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends: Vec<DaemonId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env: Option<IndexMap<String, String>>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RunOptions {
    pub id: DaemonId,
    pub cmd: Vec<String>,
    pub force: bool,
    pub shell_pid: Option<u32>,
    pub dir: PathBuf,
    pub autostop: bool,
    pub cron_schedule: Option<String>,
    pub cron_retrigger: Option<CronRetrigger>,
    pub retry: u32,
    pub retry_count: u32,
    pub ready_delay: Option<u64>,
    pub ready_output: Option<String>,
    pub ready_http: Option<String>,
    pub ready_port: Option<u16>,
    pub ready_cmd: Option<String>,
    pub wait_ready: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends: Vec<DaemonId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env: Option<IndexMap<String, String>>,
}

impl Display for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id.qualified())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_daemon_ids() {
        // Short IDs
        assert!(is_valid_daemon_id("myapp"));
        assert!(is_valid_daemon_id("my-app"));
        assert!(is_valid_daemon_id("my_app"));
        assert!(is_valid_daemon_id("my.app"));
        assert!(is_valid_daemon_id("MyApp123"));
        assert!(is_valid_daemon_id("app@host"));
        assert!(is_valid_daemon_id("app:8080")); // colons are allowed in short IDs

        // Qualified IDs (namespace/short_id)
        assert!(is_valid_daemon_id("project/api"));
        assert!(is_valid_daemon_id("global/web"));
        assert!(is_valid_daemon_id("my-project/my-app"));
    }

    #[test]
    fn test_valid_short_daemon_ids() {
        assert!(is_valid_short_daemon_id("myapp"));
        assert!(is_valid_short_daemon_id("my-app"));
        assert!(is_valid_short_daemon_id("my_app"));

        // Short IDs should NOT allow slashes
        assert!(!is_valid_short_daemon_id("project/api"));

        // Short IDs should NOT allow double dash (reserved for path encoding)
        assert!(!is_valid_short_daemon_id("my--app"));
    }

    #[test]
    fn test_invalid_daemon_ids() {
        // Empty
        assert!(!is_valid_daemon_id(""));

        // Multiple slashes (invalid qualified format)
        assert!(!is_valid_daemon_id("a/b/c"));
        assert!(!is_valid_daemon_id("../etc/passwd"));

        // Invalid qualified format (empty parts)
        assert!(!is_valid_daemon_id("/api"));
        assert!(!is_valid_daemon_id("project/"));

        // Backslashes
        assert!(!is_valid_daemon_id("foo\\bar"));

        // Parent directory reference
        assert!(!is_valid_daemon_id(".."));
        assert!(!is_valid_daemon_id("foo..bar"));

        // Double dash (reserved for path encoding)
        assert!(!is_valid_daemon_id("my--app"));
        assert!(!is_valid_daemon_id("project--api"));
        assert!(!is_valid_daemon_id("--app"));
        assert!(!is_valid_daemon_id("app--"));

        // Spaces
        assert!(!is_valid_daemon_id("my app"));
        assert!(!is_valid_daemon_id(" myapp"));
        assert!(!is_valid_daemon_id("myapp "));

        // Current directory
        assert!(!is_valid_daemon_id("."));

        // Control characters
        assert!(!is_valid_daemon_id("my\x00app"));
        assert!(!is_valid_daemon_id("my\napp"));
        assert!(!is_valid_daemon_id("my\tapp"));

        // Non-ASCII
        assert!(!is_valid_daemon_id("myäpp"));
        assert!(!is_valid_daemon_id("приложение"));
    }
}
