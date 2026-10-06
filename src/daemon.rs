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
    /// Template context handed to `mise x` for a `run` command left unrendered, kept
    /// so a restart rebuilt from this record can pass it again.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deferred_template_context: Option<String>,
    /// The mise binary the daemon's project set with `general.mise_bin`, kept
    /// so a retry or a cron run wraps the command with the same mise. `None`
    /// leaves it to the supervisor's own settings and search.
    ///
    /// # Schema compatibility note
    /// Omitted when `None`, and read as `None` when missing, so state files
    /// from older binaries load unchanged. An older binary reading a newer
    /// file ignores the key and falls back to its own `mise_bin` lookup.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mise_bin: Option<PathBuf>,
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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub oneshot: bool,
    /// Set when the proxy started this run and it may be stopped for
    /// inactivity: how long, in milliseconds, it may go without proxy
    /// activity. `None` for a daemon started any other way, or claimed since
    /// by an explicit start.
    #[serde(default)]
    pub proxy_idle_timeout_ms: Option<u64>,
    /// When the cron watcher last actually started this daemon.
    ///
    /// Distinct from `last_cron_triggered`, which advances on every scheduled
    /// tick the watcher observes -- including the anchoring tick that
    /// `immediate = false` uses to skip the first window, and ticks where the
    /// `retrigger` policy declines to run. Only this field means "it ran",
    /// which is what `last_exit_success` describes the outcome of.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_cron_run: Option<chrono::DateTime<chrono::Local>>,
    /// Start `cmd` directly, without a shell. See `RunOptions::no_shell`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_shell: bool,
    /// Registered from config by the cron watcher and only ever started by
    /// its schedule since, so each scheduled run is built from the current
    /// config, templates rendered, rather than from what was stored at
    /// registration. Cleared once a client starts the daemon: a run it asked
    /// for carries its own options, which the schedule then keeps.
    ///
    /// Appended after `no_shell` for the positional IPC encoding.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub scheduled_from_config: bool,
    /// The ready port as the start gave it. `ready_port` holds the port the
    /// run actually checked, moved along with a port bump, so it cannot tell
    /// a ready port that was bumped from one given as the bumped number. A
    /// restart of an ad-hoc daemon starts from this one and bumps it afresh.
    ///
    /// Appended after `scheduled_from_config` for the positional IPC encoding.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub configured_ready_port: Option<ReadyPort>,
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
    /// Template context for a `run` command left unrendered for `mise x`, which
    /// is started with it in `PITCHFORK_TEMPLATE_CONTEXT`. Kept apart from `env`
    /// so a variable of that name the user configured stays theirs.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deferred_template_context: Option<String>,
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
    /// The mise binary to wrap the command with, from the daemon's project
    /// settings (`general.mise_bin`). Resolved by the client, because the
    /// supervisor is long-lived and may have been started from a different
    /// directory. `None` falls back to the supervisor's `resolve_mise_bin`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mise_bin: Option<PathBuf>,
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
    /// This start describes the daemon in full, so the configuration fields
    /// it leaves unset are cleared from the saved record instead of kept. Set
    /// by every start except a restart of an ad-hoc daemon, which carries only
    /// part of its record.
    ///
    /// False by default so that a request from an older client, which does
    /// not send it, keeps merging into the record as it always did.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replaces_saved_record: bool,
    /// Run-to-completion task rather than a long-running service.
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
    #[serde(default)]
    pub proxy_idle_timeout_ms: Option<u64>,
    /// The cron watcher is starting this run, so a successful spawn is what
    /// `Daemon::last_cron_run` records.
    ///
    /// Set only by the watcher's own call. A retry, file-watch or manual
    /// restart of a scheduled daemon carries the daemon's `cron_schedule` but
    /// not this, because the schedule did not ask for it.
    #[serde(default)]
    pub cron_started: bool,
    /// Start `cmd` directly, without a shell: the config's `run` was an
    /// array. `run` is `None` then, since there is no command line.
    #[serde(default)]
    pub no_shell: bool,
    /// This start's ready checks are the daemon's only ones: those it leaves
    /// unset are cleared from the record instead of kept. Set by an ad-hoc
    /// restart given readiness flags, which replace how it was waited for.
    ///
    /// Appended after `no_shell` for the positional IPC encoding.
    #[serde(default)]
    pub replaces_ready_checks: bool,
    /// Set by the supervisor on a start a client asked for over IPC; never
    /// sent. See `Daemon::scheduled_from_config`.
    #[serde(skip)]
    pub requested_by_client: bool,
}

impl Daemon {
    /// Whether this record was made by `pitchfork run` rather than from
    /// config. Every start from config records `watch_base_dir`, the
    /// directory of the project that defined it; a config daemon recorded
    /// before that directory was stored still has the cron schedule it was
    /// started with, which `pitchfork run` never gives a daemon.
    pub fn is_adhoc(&self) -> bool {
        self.watch_base_dir.is_none() && self.cron_schedule.is_none()
    }

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
    pub async fn to_run_options(&self, cmd: Vec<String>) -> RunOptions {
        // Re-read on_output_hook from fresh config so restarts (retry, watch,
        // cron) always pick up the current hook configuration.
        // Read it from the daemon's project directory, which need not be in
        // the supervisor's cwd ancestry (e.g. daemons started via slugs), nor
        // contain the daemon's working directory when `dir` points elsewhere.
        // Reading config walks the filesystem, so it runs on a blocking worker.
        let id = self.id.clone();
        let project_dir = self.watch_base_dir.clone().or_else(|| self.dir.clone());
        let on_output_hook = tokio::task::spawn_blocking(move || {
            project_dir
                .as_deref()
                .and_then(|dir| crate::pitchfork_toml::PitchforkToml::all_merged_from(dir).ok())
                .or_else(|| {
                    crate::pitchfork_toml::PitchforkToml::all_merged_all_namespaces_blocking().ok()
                })
                .and_then(|pt| {
                    pt.daemons
                        .get(&id)
                        .and_then(|d| d.hooks.as_ref())
                        .and_then(|h| h.on_output.clone())
                })
        })
        .await
        .ok()
        .flatten();

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
            no_shell: self.no_shell,
            // Built from the whole record, whose ready checks it carries.
            replaces_ready_checks: false,
            // Set by the IPC handler for a client's own request.
            requested_by_client: false,
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
            deferred_template_context: self.deferred_template_context.clone(),
            mise_bin: self.mise_bin.clone(),
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
            // Built from the whole record, so it describes the daemon in full.
            replaces_saved_record: true,
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

    #[tokio::test]
    async fn a_restart_from_the_saved_record_keeps_the_deferred_template_context() {
        let daemon = Daemon {
            deferred_template_context: Some("{\"name\":\"api\"}".to_string()),
            ..Daemon::default()
        };
        let opts = daemon.to_run_options(vec!["echo".to_string()]).await;
        assert_eq!(
            opts.deferred_template_context.as_deref(),
            Some("{\"name\":\"api\"}")
        );
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
