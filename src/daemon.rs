use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::pitchfork_toml::{
    CpuLimit, CronRetrigger, Dir, MemoryLimit, PortConfig, ReadyHttp, Retry, StopConfig, WatchMode,
};
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
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
    pub cron_immediate: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_cron_triggered: Option<chrono::DateTime<chrono::Local>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_exit_success: Option<bool>,
    #[serde(default)]
    pub retry: Retry,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_delay: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_http: Option<ReadyHttp>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_cmd: Option<String>,
    /// Port configuration (expected ports and auto-bump settings)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub port: Option<PortConfig>,
    /// Resolved ports actually used after auto-bump (may differ from expected)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub resolved_port: Vec<u16>,
    /// The first port the process is actually listening on (detected at runtime via listeners crate).
    /// This is the source of truth for the reverse proxy. Cleared when the daemon stops.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub active_port: Option<u16>,
    /// Optional stable slug alias for this daemon (used in proxy URLs and CLI commands).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub slug: Option<String>,
    /// Whether to proxy this daemon (None = inherit global proxy.enable setting).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub proxy: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends: Vec<DaemonId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub watch: Vec<String>,
    #[serde(default)]
    pub watch_mode: WatchMode,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub watch_base_dir: Option<PathBuf>,
    /// Whether to use mise for this daemon (None = inherit global general.mise setting).
    ///
    /// # Schema compatibility note
    /// This field changed from `bool` to `Option<bool>` with `skip_serializing_if = "Option::is_none"`.
    /// - **Upgrade (old → new):** safe — old files contain `mise = true/false`, which deserialize
    ///   correctly as `Some(true)` / `Some(false)`.
    /// - **Downgrade (new → old):** if `mise` is `None` (inherit global), the key is omitted from
    ///   the state file. An old binary reads the missing key as `false`, ignoring `general.mise = true`.
    ///   Any daemon that relied on the global setting would silently stop using mise after a downgrade.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mise: Option<bool>,
    /// Unix user to run this daemon as.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user: Option<String>,
    /// Memory limit for the daemon process (e.g. "50MB", "1GiB")
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub memory_limit: Option<MemoryLimit>,
    /// CPU usage limit as a percentage (e.g. 80 for 80%, 200 for 2 cores)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cpu_limit: Option<CpuLimit>,
    /// Unix signal to send for graceful shutdown (default: SIGTERM)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub stop_signal: Option<StopConfig>,
    /// Allocate a pseudo-terminal for the daemon process.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pty: Option<bool>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct RunOptions {
    pub id: DaemonId,
    pub cmd: Vec<String>,
    pub force: bool,
    pub shell_pid: Option<u32>,
    pub dir: Dir,
    pub autostop: bool,
    pub cron_schedule: Option<String>,
    pub cron_retrigger: Option<CronRetrigger>,
    pub cron_immediate: Option<bool>,
    pub retry: Retry,
    pub retry_count: u32,
    pub ready_delay: Option<u64>,
    pub ready_output: Option<String>,
    pub ready_http: Option<ReadyHttp>,
    pub ready_port: Option<u16>,
    pub ready_cmd: Option<String>,
    pub port: Option<PortConfig>,
    pub wait_ready: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends: Vec<DaemonId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub watch: Vec<String>,
    #[serde(default)]
    pub watch_mode: WatchMode,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub watch_base_dir: Option<PathBuf>,
    /// Whether to use mise for this daemon (None = inherit global general.mise setting).
    ///
    /// # Schema compatibility note
    /// See `Daemon::mise` for downgrade implications when this field is `None`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mise: Option<bool>,
    /// Optional stable slug alias for this daemon.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub slug: Option<String>,
    /// Whether to proxy this daemon (None = inherit global proxy.enable setting).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub proxy: Option<bool>,
    /// Unix user to run this daemon as.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user: Option<String>,
    /// Memory limit for the daemon process (e.g. "50MB", "1GiB")
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub memory_limit: Option<MemoryLimit>,
    /// CPU usage limit as a percentage (e.g. 80 for 80%, 200 for 2 cores)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cpu_limit: Option<CpuLimit>,
    /// Unix signal to send for graceful shutdown (default: SIGTERM)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub stop_signal: Option<StopConfig>,
    /// Hook triggered when the daemon produces matching output
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_output_hook: Option<crate::pitchfork_toml::OnOutputHook>,
    /// Allocate a pseudo-terminal for the daemon process.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pty: Option<bool>,
}

impl Daemon {
    /// Build RunOptions from persisted daemon state.
    ///
    /// Carries over all configuration fields from the daemon state.
    /// Callers can override specific fields on the returned value.
    pub fn to_run_options(&self, cmd: Vec<String>) -> RunOptions {
        // Re-read on_output_hook from fresh config so restarts (retry, watch,
        // cron) always pick up the current hook configuration.
        // Use daemon.dir if available to handle daemons started via slugs
        // whose project directory is not in the supervisor's cwd ancestry.
        let on_output_hook = self
            .dir
            .as_deref()
            .and_then(|dir| crate::pitchfork_toml::PitchforkToml::all_merged_from(dir).ok())
            .or_else(|| crate::pitchfork_toml::PitchforkToml::all_merged().ok())
            .and_then(|pt| {
                pt.daemons
                    .get(&self.id)
                    .and_then(|d| d.hooks.as_ref())
                    .and_then(|h| h.on_output.clone())
            });

        RunOptions {
            id: self.id.clone(),
            cmd,
            force: false,
            shell_pid: self.shell_pid,
            dir: Dir(self.dir.clone().unwrap_or_else(|| crate::env::CWD.clone())),
            autostop: self.autostop,
            cron_schedule: self.cron_schedule.clone(),
            cron_retrigger: self.cron_retrigger,
            cron_immediate: self.cron_immediate,
            retry: self.retry,
            retry_count: self.retry_count,
            ready_delay: self.ready_delay,
            ready_output: self.ready_output.clone(),
            ready_http: self.ready_http.clone(),
            ready_port: self.ready_port,
            ready_cmd: self.ready_cmd.clone(),
            port: self.port.clone(),
            wait_ready: false,
            depends: self.depends.clone(),
            env: self.env.clone(),
            watch: self.watch.clone(),
            watch_mode: self.watch_mode,
            watch_base_dir: self.watch_base_dir.clone(),
            mise: self.mise,
            slug: self.slug.clone(),
            proxy: self.proxy,
            user: self.user.clone(),
            memory_limit: self.memory_limit,
            cpu_limit: self.cpu_limit,
            stop_signal: self.stop_signal,
            on_output_hook,
            pty: self.pty,
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
