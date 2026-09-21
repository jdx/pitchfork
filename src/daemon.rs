use crate::config_types::OneshotWait;
use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::pitchfork_toml::{
    CpuLimit, CronRetrigger, Dir, HealthCmd, HealthHttp, HealthPort, MemoryLimit, PortConfig,
    ReadyCmd, ReadyHttp, ReadyOutput, ReadyPort, Retry, StopConfig, WatchMode,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct Daemon {
    pub id: DaemonId,
    pub title: Option<String>,
    pub pid: Option<u32>,
    /// High-resolution kernel start token recorded at spawn. Together with
    /// `pid` this identifies the process across a supervisor crash: a recycled
    /// PID has a different token, so orphan cleanup can tell a genuine orphan
    /// from an unrelated process.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub start_time: Option<u64>,
    /// System boot time (seconds since epoch) recorded at spawn. Lets orphan
    /// reconciliation tell a daemon that died under a crashed supervisor
    /// during this boot from one whose process died with the machine, which
    /// need different terminal states. Unlike `start_time` this is a
    /// wall-clock value comparable across processes and platforms.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub boot_time: Option<u64>,
    pub shell_pid: Option<u32>,
    pub status: DaemonStatus,
    pub dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cmd: Option<Vec<String>>,
    /// Original shell command string, persisted for retry/watch restarts.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub run: Option<String>,
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
    pub ready_output: Option<ReadyOutput>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_http: Option<ReadyHttp>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_port: Option<ReadyPort>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_cmd: Option<ReadyCmd>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub health_cmd: Option<HealthCmd>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub health_http: Option<HealthHttp>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub health_port: Option<HealthPort>,
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
    /// Archive hook command invoked before retention prunes this daemon's logs.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub archive_hook: Option<String>,
    /// Log format for this daemon.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub log_format: Option<String>,
    /// Allocate a pseudo-terminal for the daemon process.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pty: Option<bool>,
    /// True for daemons auto-registered from config by the cron watcher,
    /// not yet started. Treated as "available" by list/status/stats.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub config_registered: bool,
    /// Run-to-completion task rather than a long-running service. Readiness is
    /// a zero exit code, and the terminal state is `completed` instead of
    /// `stopped`. See `DaemonStatus::Completed`.
    ///
    /// Appended rather than grouped with `status`: IPC encodes this struct
    /// positionally, so a field inserted in the middle shifts every field
    /// after it for a peer that does not have it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub oneshot: bool,
    /// Set when the proxy started this run and it may be stopped for
    /// inactivity: how long, in milliseconds, it may go without proxy
    /// activity. `None` for a daemon started any other way, or claimed since
    /// by an explicit start.
    ///
    /// Appended last for the positional IPC encoding, like `oneshot`.
    #[serde(default)]
    pub proxy_idle_timeout_ms: Option<u64>,
    /// When the cron watcher last actually started this daemon.
    ///
    /// Distinct from `last_cron_triggered`, which advances on every scheduled
    /// tick the watcher observes -- including the anchoring tick that
    /// `immediate = false` uses to skip the first window, and ticks where the
    /// `retrigger` policy declines to run. Only this field means "it ran",
    /// which is what `last_exit_success` describes the outcome of.
    ///
    /// Appended after `proxy_idle_timeout_ms` for the positional IPC encoding.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_cron_run: Option<chrono::DateTime<chrono::Local>>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct RunOptions {
    pub id: DaemonId,
    pub cmd: Vec<String>,
    /// Original shell command string (from config `run`), passed verbatim to the shell.
    /// Falls back to joining `cmd` when None (e.g. ad-hoc `pitchfork run -- cmd args`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub run: Option<String>,
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
    pub ready_output: Option<ReadyOutput>,
    pub ready_http: Option<ReadyHttp>,
    pub ready_port: Option<ReadyPort>,
    pub ready_cmd: Option<ReadyCmd>,
    pub health_cmd: Option<HealthCmd>,
    pub health_http: Option<HealthHttp>,
    pub health_port: Option<HealthPort>,
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
    /// Archive hook command invoked before retention prunes this daemon's logs.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub archive_hook: Option<String>,
    /// Log format for this daemon: `json`, `logfmt`, `auto`, or `text`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub log_format: Option<String>,
    /// Hook triggered when the daemon produces matching output
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_output_hook: Option<crate::pitchfork_toml::OnOutputHook>,
    /// Allocate a pseudo-terminal for the daemon process.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pty: Option<bool>,
    /// Run-to-completion task rather than a long-running service.
    ///
    /// Appended rather than grouped with `autostop`: IPC encodes this struct
    /// positionally, so a field inserted in the middle shifts every field
    /// after it for a CLI or supervisor that does not have it, and a version
    /// mismatch is only warned about, not refused.
    #[serde(default)]
    pub oneshot: bool,
    /// How long to wait for a oneshot to finish, already resolved from the
    /// project's `supervisor.oneshot_timeout`.
    ///
    /// Resolved by the client and carried on the request because the
    /// supervisor is long-lived and may have started in another directory, so
    /// its own `settings()` would not see the project's value. `None` leaves
    /// the supervisor to fall back to whatever it can resolve.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub oneshot_wait: Option<OneshotWait>,
    /// This start came from entering a directory rather than from a person
    /// asking for it, so a completed `oneshot` is left alone. Decided by the
    /// supervisor because only it holds authoritative state: the state file
    /// lags it by up to the flush interval, which is exactly the window a
    /// second directory entry lands in.
    #[serde(default)]
    pub on_directory_enter: bool,
    /// The proxy is starting this daemon, and it may be stopped after this
    /// many milliseconds without proxy activity. `None` for every other start,
    /// which is what makes such a start explicit. Carried over by restarts
    /// (retry, file watch), which continue the same ownership.
    ///
    /// Appended last for the positional IPC encoding.
    #[serde(default)]
    pub proxy_idle_timeout_ms: Option<u64>,
    /// The cron watcher is starting this run, so a successful spawn is what
    /// `Daemon::last_cron_run` records.
    ///
    /// Set only by the watcher's own call. A retry, file-watch or manual
    /// restart of a scheduled daemon carries the daemon's `cron_schedule` but
    /// not this, because the schedule did not ask for it.
    ///
    /// Appended after `proxy_idle_timeout_ms` for the positional IPC
    /// encoding.
    #[serde(default)]
    pub cron_started: bool,
}

impl Daemon {
    /// The next time the cron watcher will consider this daemon due, or
    /// `None` when it has no schedule or the schedule does not parse.
    ///
    /// Anchored exactly the way `check_cron_schedules` anchors itself, so the
    /// answer is what the watcher will actually do rather than an independent
    /// reading of the schedule. That includes the case where the supervisor
    /// was down across a window: the anchor is still the old tick, so the
    /// result is a time in the past -- the overdue run the watcher takes on
    /// its next check.
    pub fn next_cron_run(
        &self,
        now: chrono::DateTime<chrono::Local>,
    ) -> Option<chrono::DateTime<chrono::Local>> {
        use std::str::FromStr;
        let schedule = cron::Schedule::from_str(self.cron_schedule.as_ref()?).ok()?;
        let anchor = match self.last_cron_triggered {
            Some(t) => t,
            // Mirrors the watcher's first-sighting branch: `immediate` looks
            // back ten seconds, the default anchors to now.
            None if self.cron_immediate.unwrap_or(false) => now - chrono::Duration::seconds(10),
            None => now,
        };
        schedule.after(&anchor).next()
    }

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
            .or_else(|| crate::pitchfork_toml::PitchforkToml::all_merged_all_namespaces().ok())
            .and_then(|pt| {
                pt.daemons
                    .get(&self.id)
                    .and_then(|d| d.hooks.as_ref())
                    .and_then(|h| h.on_output.clone())
            });

        RunOptions {
            id: self.id.clone(),
            cmd,
            run: self.run.clone(),
            force: false,
            shell_pid: self.shell_pid,
            dir: Dir(self.dir.clone().unwrap_or_else(|| crate::env::CWD.clone())),
            autostop: self.autostop,
            oneshot: self.oneshot,
            // Re-resolved by the client on the paths that have a project to
            // resolve it from; a supervisor-internal restart keeps None and
            // falls back.
            oneshot_wait: None,
            on_directory_enter: false,
            // A restart continues whatever ownership the run it replaces had.
            proxy_idle_timeout_ms: self.proxy_idle_timeout_ms,
            // A restart of a scheduled daemon is not the schedule starting a
            // run; only the cron watcher's own call sets this.
            cron_started: false,
            cron_schedule: self.cron_schedule.clone(),
            cron_retrigger: self.cron_retrigger,
            cron_immediate: self.cron_immediate,
            retry: self.retry,
            retry_count: self.retry_count,
            ready_delay: self.ready_delay,
            ready_output: self.ready_output.clone(),
            ready_http: self.ready_http.clone(),
            ready_port: self.ready_port.clone(),
            ready_cmd: self.ready_cmd.clone(),
            health_cmd: self.health_cmd.clone(),
            health_http: self.health_http.clone(),
            health_port: self.health_port.clone(),
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
            archive_hook: self.archive_hook.clone(),
            log_format: self.log_format.clone(),
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
    use chrono::TimeZone;

    fn at(h: u32, m: u32, sec: u32) -> chrono::DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(2026, 9, 21, h, m, sec)
            .unwrap()
    }

    /// Daily at 03:00, the schedule from the report that asked for this.
    fn daily_3am(last_triggered: Option<chrono::DateTime<chrono::Local>>) -> Daemon {
        Daemon {
            cron_schedule: Some("0 0 3 * * *".to_string()),
            last_cron_triggered: last_triggered,
            ..Daemon::default()
        }
    }

    #[test]
    fn next_cron_run_is_none_without_a_schedule() {
        assert!(Daemon::default().next_cron_run(at(7, 0, 0)).is_none());
    }

    /// The watcher warns and skips an expression it cannot parse; there is no
    /// next run to report for one.
    #[test]
    fn next_cron_run_is_none_for_an_invalid_schedule() {
        let d = Daemon {
            cron_schedule: Some("not a cron expression".to_string()),
            ..Daemon::default()
        };
        assert!(d.next_cron_run(at(7, 0, 0)).is_none());
    }

    #[test]
    fn next_cron_run_follows_the_last_tick() {
        let d = daily_3am(Some(at(3, 0, 4)));
        assert_eq!(
            d.next_cron_run(at(7, 0, 0)),
            Some(
                chrono::Local
                    .with_ymd_and_hms(2026, 9, 22, 3, 0, 0)
                    .unwrap()
            )
        );
    }

    /// A supervisor that was down across 03:00 has a stale anchor, so the next
    /// run is in the past: the window it still owes, which is what the watcher
    /// will take on its next check.
    #[test]
    fn next_cron_run_reports_a_missed_window_as_past() {
        let d = daily_3am(Some(
            chrono::Local
                .with_ymd_and_hms(2026, 9, 20, 3, 0, 0)
                .unwrap(),
        ));
        let next = d.next_cron_run(at(7, 0, 0)).unwrap();
        assert_eq!(next, at(3, 0, 0));
        assert!(next < at(7, 0, 0));
    }

    /// Never triggered, `immediate = false`: the watcher will anchor to now
    /// and skip the current window, so the answer is the next one.
    #[test]
    fn next_cron_run_skips_the_current_window_without_immediate() {
        let d = daily_3am(None);
        assert_eq!(
            d.next_cron_run(at(2, 59, 0)),
            Some(at(3, 0, 0)),
            "a window still ahead of now is reported as-is"
        );
        assert_eq!(
            d.next_cron_run(at(3, 0, 30)),
            Some(
                chrono::Local
                    .with_ymd_and_hms(2026, 9, 22, 3, 0, 0)
                    .unwrap()
            ),
            "a window that just passed is not claimed: immediate=false skips it"
        );
    }

    /// `immediate = true` keeps the watcher's ten-second look-back, so a
    /// window that just passed is still due.
    #[test]
    fn next_cron_run_honors_the_immediate_lookback() {
        let d = Daemon {
            cron_immediate: Some(true),
            ..daily_3am(None)
        };
        assert_eq!(d.next_cron_run(at(3, 0, 5)), Some(at(3, 0, 0)));
    }

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
