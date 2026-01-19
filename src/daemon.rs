use crate::daemon_status::DaemonStatus;
use crate::pitchfork_toml::CronRetrigger;
use std::fmt::Display;
use std::path::PathBuf;

/// Validates a daemon ID to ensure it's safe for use in file paths and IPC.
///
/// A valid daemon ID:
/// - Is not empty
/// - Does not contain path separators (`/` or `\`)
/// - Does not contain parent directory references (`..`)
/// - Does not contain spaces
/// - Is not `.` (current directory)
/// - Contains only printable ASCII characters
///
/// This validation prevents path traversal attacks when daemon IDs are used
/// to construct log file paths or other filesystem operations.
pub fn is_valid_daemon_id(id: &str) -> bool {
    !id.is_empty()
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains("..")
        && !id.contains(' ')
        && id != "."
        && id.chars().all(|c| c.is_ascii() && !c.is_ascii_control())
}

/// Returns an error message explaining why a daemon ID is invalid.
pub fn validate_daemon_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("daemon ID cannot be empty".to_string());
    }
    if id.contains('/') || id.contains('\\') {
        return Err("daemon ID cannot contain path separators (/ or \\)".to_string());
    }
    if id.contains("..") {
        return Err("daemon ID cannot contain '..'".to_string());
    }
    if id.contains(' ') {
        return Err("daemon ID cannot contain spaces".to_string());
    }
    if id == "." {
        return Err("daemon ID cannot be '.'".to_string());
    }
    if !id.chars().all(|c| c.is_ascii() && !c.is_ascii_control()) {
        return Err("daemon ID must contain only printable ASCII characters".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Daemon {
    pub id: String,
    pub title: Option<String>,
    pub pid: Option<u32>,
    pub shell_pid: Option<u32>,
    pub status: DaemonStatus,
    pub dir: Option<PathBuf>,
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
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends: Vec<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RunOptions {
    pub id: String,
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
    pub wait_ready: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends: Vec<String>,
}

impl Display for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_daemon_ids() {
        assert!(is_valid_daemon_id("myapp"));
        assert!(is_valid_daemon_id("my-app"));
        assert!(is_valid_daemon_id("my_app"));
        assert!(is_valid_daemon_id("my.app"));
        assert!(is_valid_daemon_id("MyApp123"));
        assert!(is_valid_daemon_id("app@host"));
        assert!(is_valid_daemon_id("app:8080"));
    }

    #[test]
    fn test_invalid_daemon_ids() {
        // Empty
        assert!(!is_valid_daemon_id(""));

        // Path separators
        assert!(!is_valid_daemon_id("../etc/passwd"));
        assert!(!is_valid_daemon_id("foo/bar"));
        assert!(!is_valid_daemon_id("foo\\bar"));

        // Parent directory reference
        assert!(!is_valid_daemon_id(".."));
        assert!(!is_valid_daemon_id("foo..bar"));

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

    #[test]
    fn test_validate_daemon_id_error_messages() {
        assert!(validate_daemon_id("myapp").is_ok());

        assert_eq!(
            validate_daemon_id("").unwrap_err(),
            "daemon ID cannot be empty"
        );
        assert_eq!(
            validate_daemon_id("foo/bar").unwrap_err(),
            "daemon ID cannot contain path separators (/ or \\)"
        );
        assert_eq!(
            validate_daemon_id("foo\\bar").unwrap_err(),
            "daemon ID cannot contain path separators (/ or \\)"
        );
        assert_eq!(
            validate_daemon_id("..").unwrap_err(),
            "daemon ID cannot contain '..'"
        );
        assert_eq!(
            validate_daemon_id("my app").unwrap_err(),
            "daemon ID cannot contain spaces"
        );
        assert_eq!(
            validate_daemon_id(".").unwrap_err(),
            "daemon ID cannot be '.'"
        );
        assert_eq!(
            validate_daemon_id("my\x00app").unwrap_err(),
            "daemon ID must contain only printable ASCII characters"
        );
    }
}
