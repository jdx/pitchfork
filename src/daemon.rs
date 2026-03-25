use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::pitchfork_toml::{CpuLimit, CronRetrigger, MemoryLimit};
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
    if id.contains('/') {
        DaemonId::parse(id).is_ok()
    } else {
        DaemonId::try_new("global", id).is_ok()
    }
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
    /// Expected ports from configuration (before auto-bump resolution)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub expected_port: Vec<u16>,
    /// Resolved ports actually used after auto-bump (may differ from expected)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub resolved_port: Vec<u16>,
    #[serde(default)]
    pub auto_bump_port: bool,
    #[serde(default)]
    pub port_bump_attempts: u32,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends: Vec<DaemonId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub watch: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub watch_base_dir: Option<PathBuf>,
    #[serde(default)]
    pub mise: bool,
    /// Memory limit for the daemon process (e.g. "50MB", "1GiB")
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub memory_limit: Option<MemoryLimit>,
    /// CPU usage limit as a percentage (e.g. 80 for 80%, 200 for 2 cores)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cpu_limit: Option<CpuLimit>,
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
    pub expected_port: Vec<u16>,
    pub auto_bump_port: bool,
    pub port_bump_attempts: u32,
    pub wait_ready: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends: Vec<DaemonId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub watch: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub watch_base_dir: Option<PathBuf>,
    #[serde(default)]
    pub mise: bool,
    /// Memory limit for the daemon process (e.g. "50MB", "1GiB")
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub memory_limit: Option<MemoryLimit>,
    /// CPU usage limit as a percentage (e.g. 80 for 80%, 200 for 2 cores)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cpu_limit: Option<CpuLimit>,
}

impl Default for Daemon {
    fn default() -> Self {
        Self {
            id: DaemonId::default(),
            title: None,
            pid: None,
            shell_pid: None,
            status: DaemonStatus::default(),
            dir: None,
            cmd: None,
            autostop: false,
            cron_schedule: None,
            cron_retrigger: None,
            last_cron_triggered: None,
            last_exit_success: None,
            retry: 0,
            retry_count: 0,
            ready_delay: None,
            ready_output: None,
            ready_http: None,
            ready_port: None,
            ready_cmd: None,
            expected_port: Vec::new(),
            resolved_port: Vec::new(),
            auto_bump_port: false,
            port_bump_attempts: 10,
            depends: Vec::new(),
            env: None,
            watch: Vec::new(),
            watch_base_dir: None,
            mise: false,
            memory_limit: None,
            cpu_limit: None,
        }
    }
}

impl Daemon {
    /// Build RunOptions from persisted daemon state.
    ///
    /// Carries over all configuration fields from the daemon state.
    /// Callers can override specific fields on the returned value.
    pub fn to_run_options(&self, cmd: Vec<String>) -> RunOptions {
        RunOptions {
            id: self.id.clone(),
            cmd,
            force: false,
            shell_pid: self.shell_pid,
            dir: self.dir.clone().unwrap_or_else(|| crate::env::CWD.clone()),
            autostop: self.autostop,
            cron_schedule: self.cron_schedule.clone(),
            cron_retrigger: self.cron_retrigger,
            retry: self.retry,
            retry_count: self.retry_count,
            ready_delay: self.ready_delay,
            ready_output: self.ready_output.clone(),
            ready_http: self.ready_http.clone(),
            ready_port: self.ready_port,
            ready_cmd: self.ready_cmd.clone(),
            expected_port: self.expected_port.clone(),
            auto_bump_port: self.auto_bump_port,
            port_bump_attempts: self.port_bump_attempts,
            wait_ready: false,
            depends: self.depends.clone(),
            env: self.env.clone(),
            watch: self.watch.clone(),
            watch_base_dir: self.watch_base_dir.clone(),
            mise: self.mise,
            memory_limit: self.memory_limit,
            cpu_limit: self.cpu_limit,
        }
    }
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            id: DaemonId::default(),
            cmd: Vec::new(),
            force: false,
            shell_pid: None,
            dir: crate::env::CWD.clone(),
            autostop: false,
            cron_schedule: None,
            cron_retrigger: None,
            retry: 0,
            retry_count: 0,
            ready_delay: None,
            ready_output: None,
            ready_http: None,
            ready_port: None,
            ready_cmd: None,
            expected_port: Vec::new(),
            auto_bump_port: false,
            port_bump_attempts: 10,
            wait_ready: false,
            depends: Vec::new(),
            env: None,
            watch: Vec::new(),
            watch_base_dir: None,
            mise: false,
            memory_limit: None,
            cpu_limit: None,
        }
    }
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

        // Qualified IDs (namespace/short_id)
        assert!(is_valid_daemon_id("project/api"));
        assert!(is_valid_daemon_id("global/web"));
        assert!(is_valid_daemon_id("my-project/my-app"));
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

        // Unsupported punctuation under DaemonId rules
        assert!(!is_valid_daemon_id("app@host"));
        assert!(!is_valid_daemon_id("app:8080"));
    }
}
