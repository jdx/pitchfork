//! User-configurable settings for pitchfork.
//!
//! Settings can be configured in multiple ways (in order of precedence):
//! 1. Environment variables (highest priority)
//! 2. Project-level `pitchfork.toml` or `pitchfork.local.toml` (in `[settings]` section)
//! 3. User-level `~/.config/pitchfork/config.toml` (in `[settings]` section)
//! 4. System-level `/etc/pitchfork/config.toml` (in `[settings]` section)
//! 5. Built-in defaults (lowest priority)
//!
//! Example pitchfork.toml with settings:
//! ```toml
//! [daemons.myapp]
//! run = "node server.js"
//!
//! [settings.general]
//! autostop_delay = "5m"
//! log_level = "debug"
//!
//! [settings.web]
//! auto_start = true
//! ```
//!
//! The structs below are the single declaration of every setting:
//! `#[derive(usage_rs::Config)]` generates the usage-config registry, the
//! reader that fills the structs from a resolution, and the spec `config`
//! block that documents them (carried into `pitchfork.usage.kdl` through
//! `#[usage(config = ...)]` on the CLI root). There is no `settings.toml`
//! and no build-script generator left to keep in step with this file.
//!
//! Resolution is usage-config's single origin-tracked merge, so explicitly
//! setting a value equal to the default in a higher-precedence file still
//! overrides a lower one, and `pitchfork settings` renders provenance from
//! the same merge that produced the values.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use usage_rs::config::{
    Const, EnvLayer, FileLayer, FileScope, Layers, Resolved, Ty, Value, resolve,
};

/// The `api.*` settings: the standalone API server (JSON REST endpoints for
/// the Vue SPA). When `api.bind_port` is set, the API server binds
/// independently of the web UI.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "api")]
pub struct SettingsApi {
    /// Automatically start the standalone API server
    ///
    /// When true, pitchfork starts a standalone API server on api.bind_address +
    /// api.bind_port when the supervisor launches. The web UI does not need to
    /// be enabled for this; you can run the API alone and serve the Vue SPA from
    /// a separate static host (e.g. nginx, Vite dev server).
    ///
    /// Default is false so the API is only available through the bundled web UI.
    #[usage(env = "PITCHFORK_API_AUTO_START", default = false)]
    pub auto_start: bool,

    /// IP address the API server binds to
    ///
    /// Use "0.0.0.0" to make the API reachable from other devices on your network.
    /// Keep "127.0.0.1" (default) to restrict it to localhost.
    #[usage(env = "PITCHFORK_API_BIND_ADDRESS", default = "127.0.0.1")]
    pub bind_address: String,

    /// Port the standalone API server listens on
    ///
    /// Set to 0 (default) to disable the standalone API server.
    /// Set to any valid port number (e.g. 8081) to start the API on its own port.
    #[usage(env = "PITCHFORK_API_BIND_PORT", default = 0)]
    pub bind_port: i64,

    /// Number of consecutive ports to try if api.bind_port is in use
    #[usage(env = "PITCHFORK_API_PORT_ATTEMPTS", default = 10)]
    pub port_attempts: i64,

    /// Authentication token for API access when bound to non-loopback addresses
    ///
    /// When the API server or web UI is bound to a non-loopback address (e.g.,
    /// "0.0.0.0"), this token is required on every API request via the
    /// X-Pitchfork-Token header.
    ///
    /// If left empty and the bind address is non-loopback, a random 32-byte
    /// hex token is auto-generated on startup and printed to the supervisor log.
    /// This token is injected into the served index.html, so the bundled Vue
    /// SPA works without additional configuration.
    ///
    /// For external API consumers (e.g., curl, mobile apps), set this to a
    /// fixed value or pass it via the PITCHFORK_API_TOKEN environment variable.
    #[usage(env = "PITCHFORK_API_TOKEN", default = "")]
    pub token: String,
}

/// The `general.*` settings.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "general")]
pub struct SettingsGeneral {
    /// Delay before auto-stopping daemons when leaving a directory
    ///
    /// When using shell hooks with `auto = ["stop"]`, this controls how long pitchfork waits
    /// before stopping a daemon after you leave its directory.
    ///
    /// This delay prevents unnecessary stop/start cycles when briefly switching directories.
    ///
    /// **Examples:**
    /// - `"0s"` - Stop immediately (no delay)
    /// - `"30s"` - Wait 30 seconds
    /// - `"1m"` - Wait 1 minute (default)
    /// - `"5m"` - Wait 5 minutes
    ///
    /// Set to `"0s"` to disable the delay and stop daemons immediately.
    #[usage(env = "PITCHFORK_AUTOSTOP_DELAY", default = "1m", ty = "duration")]
    pub autostop_delay: String,

    /// Supervisor background task refresh interval
    ///
    /// Controls how often the supervisor refreshes its internal state and checks for:
    /// - Daemon health status changes
    /// - Configuration file updates
    /// - Process state synchronization
    ///
    /// Lower values provide more responsive status updates but use more resources.
    ///
    /// **Recommended values:**
    /// - `"5s"` - For development/testing
    /// - `"10s"` - Default, balanced
    /// - `"30s"` - For production with many daemons
    #[usage(
        env = "PITCHFORK_INTERVAL",
        deprecated_env("PITCHFORK_INTERVAL_SECS"),
        default = "10s",
        ty = "duration"
    )]
    pub interval: String,

    /// File log level (trace, debug, info, warn, error)
    ///
    /// Controls the verbosity of log output written to log files.
    /// Can be set independently from `log_level` to have more verbose file logs.
    ///
    /// For example, set console to `"info"` but file to `"debug"` to keep
    /// detailed logs for troubleshooting without cluttering the console.
    #[usage(env = "PITCHFORK_LOG_FILE_LEVEL", default = "info")]
    pub log_file_level: String,

    /// Console log level (trace, debug, info, warn, error)
    ///
    /// Controls the verbosity of log output to the console.
    ///
    /// **Available levels:**
    /// - `"trace"` - Most verbose, includes all internal details
    /// - `"debug"` - Detailed information for debugging
    /// - `"info"` - Normal operation messages (default)
    /// - `"warn"` - Warnings and potential issues
    /// - `"error"` - Only errors
    #[usage(env = "PITCHFORK_LOG", default = "info")]
    pub log_level: String,

    /// Wrap daemon commands with mise x -- globally
    ///
    /// When enabled, pitchfork wraps every daemon command with `mise x --` so that
    /// [mise](https://mise.jdx.dev) sets up the correct tool versions, PATH,
    /// and environment variables before the daemon runs.
    ///
    /// This is especially useful when pitchfork is started as a login item or boot
    /// daemon, where the shell profile (`.zshrc`, `.bashrc`) is not sourced and
    /// tools installed via Homebrew or mise are not on PATH.
    ///
    /// Individual daemons can override this with `mise = true` or `mise = false`
    /// in their configuration.
    #[usage(env = "PITCHFORK_MISE", default = false)]
    pub mise: bool,

    /// Explicit path to the mise binary
    ///
    /// By default, pitchfork searches well-known locations for the mise binary:
    /// - `~/.local/bin/mise`
    /// - `~/.cargo/bin/mise`
    /// - `/usr/local/bin/mise`
    /// - `/opt/homebrew/bin/mise`
    ///
    /// Set this to an absolute path if mise is installed elsewhere.
    #[usage(env = "PITCHFORK_MISE_BIN", default = "")]
    pub mise_bin: String,

    /// Shell command used to execute daemon run scripts
    ///
    /// Controls the shell used to execute daemon `run` commands.
    ///
    /// The value is split with `shell_words::split` into a program and arguments,
    /// then the daemon's `run` string is appended verbatim as the final argument
    /// (passed to the shell's command flag, e.g. `sh -c "<run>"`).
    ///
    /// This means the `run` string is interpreted directly by the shell, so
    /// variable expansion (`$VAR`), globs (`*.txt`), pipes (`|`), and command
    /// chaining (`&&`) all work as expected.
    ///
    /// **Common configurations:**
    /// - `"sh -c"` — Default, POSIX shell
    /// - `"sh -o errexit -o pipefail -c"` — Exit on error, fail on pipe failure
    /// - `"bash -c"` — Use bash instead of sh
    ///
    /// When `mise = true` is enabled for a daemon, the shell wraps inside
    /// `mise x --`, e.g. `mise x -- sh -c "<run>"`.
    #[usage(env = "PITCHFORK_SHELL", default = "sh -c")]
    pub shell: String,

    /// Show timestamps in startup log output
    ///
    /// When enabled, pitchfork prefixes each startup log line and result line
    /// with a timestamp (e.g. `19:03:15`), making it easier to see how long
    /// each daemon took to start.
    ///
    /// When disabled (default), a dim bullet (`•`) is used instead to keep
    /// the output compact and aligned with the spinner / status icons.
    #[usage(env = "PITCHFORK_STARTUP_LOG_TIMESTAMPS", default = false)]
    pub startup_log_timestamps: bool,
}

/// The `ipc.*` (inter-process communication) settings.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "ipc")]
pub struct SettingsIpc {
    /// Number of connection retry attempts
    ///
    /// How many times to retry connecting to the supervisor before giving up.
    /// Each attempt uses exponential backoff between `connect_min_delay` and `connect_max_delay`.
    #[usage(env = "PITCHFORK_IPC_CONNECT_ATTEMPTS", default = 5)]
    pub connect_attempts: i64,

    /// Maximum delay between connection retries
    ///
    /// The maximum delay between connection retry attempts.
    /// Exponential backoff will not exceed this value.
    #[usage(
        env = "PITCHFORK_IPC_CONNECT_MAX_DELAY",
        default = "1s",
        ty = "duration"
    )]
    pub connect_max_delay: String,

    /// Minimum delay between connection retries
    ///
    /// The initial delay between connection retry attempts.
    /// The actual delay increases exponentially up to `connect_max_delay`.
    #[usage(
        env = "PITCHFORK_IPC_CONNECT_MIN_DELAY",
        default = "100ms",
        ty = "duration"
    )]
    pub connect_min_delay: String,

    /// Maximum IPC requests per second per connection
    ///
    /// Rate limiting for IPC connections to prevent local DoS attacks.
    /// Uses a sliding window algorithm.
    ///
    /// Most users won't need to change this. Increase if you have automated
    /// tools making many rapid requests to the supervisor.
    #[usage(env = "PITCHFORK_IPC_RATE_LIMIT", default = 100)]
    pub rate_limit: i64,

    /// Rate limit sliding window duration
    ///
    /// The time window for rate limiting calculations.
    /// `rate_limit` requests are allowed within each window.
    #[usage(
        env = "PITCHFORK_IPC_RATE_LIMIT_WINDOW",
        default = "1s",
        ty = "duration"
    )]
    pub rate_limit_window: String,

    /// Default timeout for IPC requests
    ///
    /// Maximum time to wait for a response from the supervisor for most operations.
    ///
    /// Note: Daemon start operations may use a longer timeout calculated from
    /// the daemon's `ready_delay` setting plus a buffer.
    #[usage(env = "PITCHFORK_IPC_REQUEST_TIMEOUT", default = "5s", ty = "duration")]
    pub request_timeout: String,
}

/// The `logs.archive_hook.*` settings.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "logs.archive_hook")]
pub struct SettingsLogsArchiveHook {
    /// Maximum log entries per archive hook invocation
    ///
    /// Log entries selected for pruning are passed to the archive hook in batches of
    /// this size. Smaller batches use less memory and are easier to retry, while larger
    /// batches reduce hook invocation overhead.
    #[usage(env = "PITCHFORK_LOG_ARCHIVE_HOOK_BATCH_SIZE", default = 1000)]
    pub batch_size: i64,

    /// Command to run before retention deletes old log entries
    ///
    /// When set, the given command is invoked before log entries are permanently
    /// pruned by retention. The command receives the entries about to be deleted as
    /// JSON Lines (JSONL) on stdin. Each line is an object with the fields:
    ///
    /// - `id`: the SQLite row id
    /// - `daemon_id`: the qualified daemon id (e.g. `"myproject/api"`)
    /// - `timestamp`: the log timestamp in RFC 3339 format
    /// - `message`: the raw log message text
    ///
    /// The following environment variables are also available:
    ///
    /// - `PITCHFORK_DAEMON_ID`: the qualified daemon id
    /// - `PITCHFORK_ARCHIVE_REASON`: either `"age"` or `"count"`
    ///
    /// If the command exits with a non-zero status, the pruning is skipped for that
    /// batch. This prevents data loss when the archive destination is unavailable.
    ///
    /// **Examples:**
    ///
    /// ```toml
    /// [logs.archive_hook]
    /// command = "gzip -c >> /var/log/pitchfork/archive.jsonl.gz"
    /// ```
    ///
    /// ```toml
    /// [logs.archive_hook]
    /// command = "aws s3 cp - s3://my-bucket/pitchfork-logs/"
    /// ```
    #[usage(env = "PITCHFORK_LOG_ARCHIVE_HOOK_COMMAND", default = "")]
    pub command: String,
}

/// The `logs.*` settings.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "logs")]
pub struct SettingsLogs {
    #[usage(flatten)]
    pub archive_hook: SettingsLogsArchiveHook,

    /// Count-based log retention (e.g. 10000)
    ///
    /// Maximum number of log entries to keep per daemon in the SQLite log store.
    ///
    /// When set, only the most recent N log entries are retained. Older entries are
    /// automatically pruned.
    ///
    /// When set to `0` (default), no count-based pruning is performed. You can combine
    /// this with `time_retention` to enforce both a time limit and a line count limit.
    ///
    /// Logs are pruned automatically by the supervisor during its interval watcher
    /// cycle (no more than once per hour). No manual rotation command is needed.
    #[usage(env = "PITCHFORK_LOG_LINE_RETENTION", default = 0)]
    pub line_retention: i64,

    /// Default log format for daemons (json | logfmt | text)
    #[usage(env = "PITCHFORK_LOG_FORMAT", default = "text")]
    pub log_format: String,

    /// Time-based log retention duration (e.g. '7d', '30d')
    ///
    /// Maximum age of log entries to keep in the SQLite log store.
    ///
    /// When set, log entries older than this duration are automatically pruned.
    /// Examples: `"7d"` for 7 days, `"30d"` for 30 days, `"1h"` for 1 hour.
    ///
    /// When empty (default), no time-based pruning is performed. You can combine
    /// this with `line_retention` to enforce both a time limit and a line count limit.
    ///
    /// Logs are pruned automatically by the supervisor during its interval watcher
    /// cycle (no more than once per hour). No manual rotation command is needed.
    #[usage(env = "PITCHFORK_LOG_TIME_RETENTION", default = "", ty = "duration")]
    pub time_retention: String,

    /// Show timestamps in log output
    ///
    /// When enabled (default), pitchfork prefixes each log line with a timestamp
    /// (e.g. `07-10 10:30:00`).
    ///
    /// When disabled, timestamps are omitted from `pitchfork logs` output. This is
    /// useful when piping logs to tools like `lnav` that expect raw JSON or when
    /// you want to process log output without the timestamp prefix.
    ///
    /// Can be overridden per-invocation with `pitchfork logs --no-timestamp`.
    #[usage(env = "PITCHFORK_LOG_TIMESTAMP", default = true)]
    pub timestamp: bool,

    /// strftime format for log timestamps in `pitchfork logs` output
    ///
    /// Controls the date/time format used when displaying timestamps in `pitchfork logs`
    /// output. Uses chrono strftime syntax.
    ///
    /// Common format specifiers:
    /// - `%Y` — full year (2024)
    /// - `%m` — month (01-12)
    /// - `%d` — day (01-31)
    /// - `%H` — hour (00-23)
    /// - `%M` — minute (00-59)
    /// - `%S` — second (00-59)
    ///
    /// Examples:
    /// - `%m-%d %H:%M:%S` — `07-10 10:30:00` (default)
    /// - `%Y-%m-%d %H:%M:%S` — `2024-07-10 10:30:00`
    /// - `%H:%M:%S` — `10:30:00` (time only)
    /// - `%Y/%m/%d %H:%M` — `2024/07/10 10:30`
    ///
    /// Only affects the text display output, not `--json` or `--raw` modes.
    #[usage(env = "PITCHFORK_LOG_TIMESTAMP_FORMAT", default = "%m-%d %H:%M:%S")]
    pub timestamp_format: String,
}

/// The `proxy.*` settings.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "proxy")]
pub struct SettingsProxy {
    /// Automatically start daemons when accessed via proxy URL
    ///
    /// When enabled (default), visiting a proxy URL for a stopped daemon will
    /// automatically start that daemon. The browser receives a "Starting…" page
    /// that refreshes every 2 seconds until the daemon is ready, at which point
    /// the request is proxied normally.
    ///
    /// Set to `false` to disable auto-start and return a plain 502 error for
    /// stopped daemons (the previous behaviour).
    #[usage(env = "PITCHFORK_PROXY_AUTO_START", default = true)]
    pub auto_start: bool,

    /// Maximum time to wait for an auto-started daemon to become ready
    ///
    /// When a daemon is auto-started via a proxy request, the proxy waits up to
    /// this duration for the **entire** auto-start operation to complete — including
    /// waiting for the daemon's readiness signal and detecting the bound port.
    ///
    /// If the daemon does not become ready and bind a port within this timeout,
    /// the browser receives an error page indicating the startup timed out.
    ///
    /// **Examples:**
    /// - `"15s"` - Shorter timeout for fast-starting services
    /// - `"30s"` - Default, suitable for most daemons
    /// - `"60s"` - For daemons with slow initialisation (e.g. large Java apps)
    #[usage(
        env = "PITCHFORK_PROXY_AUTO_START_TIMEOUT",
        default = "30s",
        ty = "duration"
    )]
    pub auto_start_timeout: String,

    /// Automatically install the proxy TLS certificate into the system trust store
    ///
    /// When enabled (default), pitchfork automatically installs the proxy's
    /// self-signed CA certificate into the system trust store during supervisor
    /// startup, so that browsers and tools trust HTTPS proxy URLs without
    /// certificate warnings.
    ///
    /// On macOS, this triggers a system authorization dialog (Touch ID or password).
    /// On Linux, this requires write access to the system CA directory (typically
    /// needs `sudo`).
    ///
    /// If auto-trust fails (e.g. due to permissions), it is silently skipped and
    /// a warning is logged. You can manually install the certificate with:
    ///   pitchfork proxy trust
    ///
    /// Set to `false` to disable auto-trust entirely.
    #[usage(env = "PITCHFORK_PROXY_AUTO_TRUST", default = true)]
    pub auto_trust: bool,

    /// Enable the reverse proxy server for daemons
    ///
    /// When enabled, pitchfork starts a reverse proxy that routes requests from
    /// `<slug>.<tld>:<port>` to the daemon's actual listening port.
    ///
    /// Only daemons with an explicit `slug` are routable through the proxy.
    /// No slug = not proxied.
    ///
    /// Example: `myapp.localhost:7777` -> `localhost:3000` (daemon with slug = "myapp")
    #[usage(env = "PITCHFORK_PROXY_ENABLE", default = false)]
    pub enable: bool,

    /// Bind address for the reverse proxy server
    ///
    /// IP address the reverse proxy listens on.
    ///
    /// **Security Warning:** The default `127.0.0.1` only allows local connections.
    /// Setting this to `0.0.0.0` will expose the proxy on every network interface,
    /// including externally routable ones -- anyone on the same LAN can then reach
    /// your local daemons.
    ///
    /// **Examples:**
    /// - `"127.0.0.1"` - Local only (default, recommended)
    /// - `"0.0.0.0"` - All interfaces (use with caution)
    /// - `"::1"` - IPv6 loopback
    #[usage(env = "PITCHFORK_PROXY_HOST", default = "127.0.0.1")]
    pub host: String,

    /// Enable HTTPS for the reverse proxy
    ///
    /// When enabled (default), the proxy serves HTTPS instead of HTTP.
    ///
    /// You must also configure `proxy.tls_cert` and `proxy.tls_key`, or pitchfork
    /// will auto-generate a self-signed certificate stored in the state directory.
    ///
    /// Set to `false` to use plain HTTP (e.g. for simple local development).
    #[usage(env = "PITCHFORK_PROXY_HTTPS", default = true)]
    pub https: bool,

    /// Enable LAN mode for the reverse proxy
    ///
    /// When enabled, the proxy switches to the `.local` TLD and publishes slug
    /// hostnames via mDNS so that other devices on the same network can reach
    /// your daemons (e.g. `myapp.local` from a phone or another computer).
    ///
    /// LAN mode:
    /// - Forces `proxy.tld` to `local` (mDNS requirement)
    /// - Publishes each slug as an mDNS address record (`<slug>.local → <LAN-IP>`)
    /// - Binds the proxy to `0.0.0.0` instead of `127.0.0.1` (overridable via `proxy.host`)
    /// - Auto-detects your LAN IP and re-publishes mDNS records if it changes
    ///
    /// Other devices must trust the pitchfork CA certificate to use HTTPS.
    /// Run `pitchfork proxy trust` on each device, or use `proxy.https = false`.
    #[usage(env = "PITCHFORK_PROXY_LAN", default = false)]
    pub lan: bool,

    /// Pin a specific LAN IP address instead of auto-detecting
    ///
    /// When set, skips auto-detection and uses this IP for mDNS publishing.
    /// Implies `proxy.lan = true` if a non-empty value is provided.
    #[usage(env = "PITCHFORK_PROXY_LAN_IP", default = "")]
    pub lan_ip: String,

    /// Port the reverse proxy server listens on
    ///
    /// The port pitchfork's reverse proxy binds to. Must be in the range 1-65535.
    ///
    /// Default is 443 (standard HTTPS port) since the proxy defaults to HTTPS.
    /// Users can override this to any port (e.g. 7777) to avoid requiring
    /// elevated privileges.
    ///
    /// Ports below 1024 require the supervisor to be started with elevated
    /// privileges (e.g. `sudo pitchfork supervisor start`).
    #[usage(env = "PITCHFORK_PROXY_PORT", default = 443)]
    pub port: i64,

    /// Automatically sync slug hostnames to /etc/hosts
    ///
    /// When enabled (default), pitchfork automatically adds entries to `/etc/hosts`
    /// for registered slugs (e.g. `127.0.0.1 myapp.localhost`) so that browsers
    /// can resolve them.
    ///
    /// This is needed because Safari does not auto-resolve `.localhost` subdomains,
    /// and custom TLDs (e.g. `.test`) always require DNS entries.
    ///
    /// Entries are managed in a marked block in `/etc/hosts` and cleaned up when
    /// the proxy shuts down. Writing to `/etc/hosts` may require `sudo`.
    ///
    /// Set to `false` to disable automatic hosts file management. You will need to
    /// configure DNS resolution yourself (e.g. `dnsmasq`, `/etc/resolver/` on macOS).
    #[usage(env = "PITCHFORK_PROXY_SYNC_HOSTS", default = true)]
    pub sync_hosts: bool,

    /// Top-level domain used for proxy URLs
    ///
    /// The TLD appended to daemon hostnames in proxy URLs.
    ///
    /// With the default `localhost`, daemon URLs look like:
    ///   `myapp.localhost:7777`  (for a daemon with slug = "myapp")
    ///
    /// For custom TLDs (e.g. `test`), you need wildcard DNS resolution.
    /// On macOS, you can use dnsmasq or add entries to `/etc/resolver/`.
    #[usage(env = "PITCHFORK_PROXY_TLD", default = "localhost")]
    pub tld: String,

    /// Path to TLS certificate file (PEM format) for HTTPS proxy
    ///
    /// Path to a PEM-encoded TLS certificate file used when `proxy.https = true`.
    ///
    /// If left empty and `proxy.https = true`, pitchfork will auto-generate a
    /// self-signed certificate and store it in `$PITCHFORK_STATE_DIR/proxy/cert.pem`.
    #[usage(env = "PITCHFORK_PROXY_TLS_CERT", default = "")]
    pub tls_cert: String,

    /// Path to TLS private key file (PEM format) for HTTPS proxy
    ///
    /// Path to a PEM-encoded private key file used when `proxy.https = true`.
    ///
    /// If left empty and `proxy.https = true`, pitchfork will auto-generate a
    /// self-signed key and store it in `$PITCHFORK_STATE_DIR/proxy/key.pem`.
    #[usage(env = "PITCHFORK_PROXY_TLS_KEY", default = "")]
    pub tls_key: String,

    /// Enable wildcard subdomain matching for proxy routes
    ///
    /// When enabled (default), requests for subdomains of a registered slug
    /// will fall back to the parent slug's daemon.
    ///
    /// For example, with slug "myapp":
    /// - `myapp.localhost` → exact match (always works)
    /// - `tenant.myapp.localhost` → wildcard fallback to "myapp"
    ///
    /// This is useful for multi-tenant apps where each tenant gets a unique
    /// subdomain (e.g. `acme.myapp.localhost`, `globex.myapp.localhost`) but
    /// all share the same backend server.
    ///
    /// Set to `false` to require exact hostname matches only.
    #[usage(env = "PITCHFORK_PROXY_WILDCARD", default = true)]
    pub wildcard: bool,

    /// Enable git worktree / jj workspace auto-discovery for slug routing
    ///
    /// When enabled (default), pitchfork automatically detects git worktrees or jj
    /// workspaces for registered slugs and routes subdomain prefixes to the
    /// corresponding worktree / workspace.
    ///
    /// For example, with slug "myapp" pointing to /home/user/myapp (main branch),
    /// a git worktree at /home/user/myapp-feature-a (branch feature-a) or a jj
    /// workspace at /home/user/myapp-feature-a is automatically discovered.
    /// Requests to feature-a.myapp.localhost are routed to the daemon running in
    /// that directory.
    ///
    /// Each worktree / workspace gets its own namespace (the directory name), so
    /// multiple copies can run the same daemon name without conflict.
    ///
    /// Branch / workspace names are sanitized for URL compatibility:
    /// `feature/my-endpoint` becomes `feature-my-endpoint`, accessed as
    /// `feature-my-endpoint.myapp.localhost`.
    ///
    /// Set to `false` to disable worktree/workspace discovery. Only the main
    /// project directory will be used for slug routing.
    #[usage(env = "PITCHFORK_PROXY_WORKTREE", default = true)]
    pub worktree: bool,
}

/// The `supervisor.*` settings.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "supervisor")]
pub struct SettingsSupervisor {
    /// Automatically start the supervisor when a client command needs it
    ///
    /// When enabled (default), commands such as `pitchfork start`, `pitchfork list`,
    /// the TUI, and shell activation start a background supervisor automatically when
    /// one is not already running.
    ///
    /// Disable this when the supervisor is managed by systemd, launchd, or another
    /// service manager:
    ///
    /// ```toml
    /// [settings.supervisor]
    /// auto_start = false
    /// ```
    ///
    /// With auto-start disabled, client commands wait for the configured IPC
    /// connection attempts and then fail with an actionable error instead of spawning
    /// an unmanaged supervisor. Explicit `pitchfork supervisor start` and
    /// `pitchfork supervisor run` commands are unaffected.
    #[usage(env = "PITCHFORK_SUPERVISOR_AUTO_START", default = true)]
    pub auto_start: bool,

    /// Reconcile orphaned daemon processes when supervisor starts
    ///
    /// When enabled, the supervisor scans the state file on startup for daemon
    /// processes left behind by a previous supervisor instance that was killed
    /// unexpectedly (for example, with `kill -9`) and reconciles them according
    /// to `supervisor.orphan_policy` (re-adopt by default, or terminate).
    ///
    /// Before acting, the recorded process identity (PID plus kernel start
    /// time) is verified so that a PID recycled by the OS to an unrelated process
    /// is never adopted or killed — in that case only the stale state entry is
    /// cleared. When terminating, Linux and Windows also pin that identity with a
    /// pidfd or open process handle, so a recycled PID cannot be signaled. If the
    /// identity cannot be verified or pinned, reconciliation fails closed: the
    /// live process and its running state are retained rather than risk acting on
    /// the wrong process or allowing a duplicate instance to start.
    ///
    /// Disabling this leaves orphaned processes and their state entries
    /// completely untouched. This is a legacy escape hatch; prefer
    /// `orphan_policy = "adopt"` (the default), which keeps daemons running
    /// across a supervisor crash while resuming supervision.
    #[usage(env = "PITCHFORK_CLEANUP_ORPHANS", default = true)]
    pub cleanup_orphans: bool,

    /// Enable container/PID1 mode for running inside Docker containers
    ///
    /// When enabled, pitchfork operates as a proper PID 1 process inside a container:
    /// - Installs a SIGCHLD handler to reap all orphaned/zombie child processes
    /// - Routes SIGTERM/SIGINT through the graceful shutdown sequence
    ///
    /// This is essential when running pitchfork as the entrypoint of a Docker container,
    /// where PID 1 must reap zombie processes to prevent process table exhaustion.
    ///
    /// Can also be enabled via the `--container` CLI flag on `pitchfork supervisor run`.
    #[usage(env = "PITCHFORK_CONTAINER", default = false)]
    pub container: bool,

    /// Consecutive CPU-over-limit samples before killing a daemon
    ///
    /// When a daemon has `cpu_limit` configured, the supervisor checks CPU usage at
    /// each interval tick. To avoid killing daemons during transient spikes (e.g. JIT
    /// warm-up, burst responses), the process is only killed after this many
    /// **consecutive** samples exceed the limit. A single sample below the limit
    /// resets the counter.
    ///
    /// **Examples:**
    /// - `1` - Kill immediately on first over-limit sample (no grace period)
    /// - `3` - Require 3 consecutive over-limit samples (default)
    /// - `5` - More tolerant of short bursts
    ///
    /// With the default interval of `10s`, a threshold of `3` means a daemon must
    /// exceed its CPU limit for ~30 seconds before being killed.
    #[usage(env = "PITCHFORK_CPU_VIOLATION_THRESHOLD", default = 3)]
    pub cpu_violation_threshold: i64,

    /// Interval for checking cron schedules
    ///
    /// How often to check if any cron-scheduled daemons should be triggered.
    ///
    /// The default of 10 seconds supports sub-minute cron schedules.
    /// Increase for lower resource usage if you don't need fine-grained scheduling.
    #[usage(
        env = "PITCHFORK_CRON_CHECK_INTERVAL",
        default = "10s",
        ty = "duration"
    )]
    pub cron_check_interval: String,

    /// File watch debounce duration
    ///
    /// When using `watch` patterns to auto-restart daemons on file changes,
    /// this controls how long to wait after the last change before triggering
    /// a restart.
    ///
    /// This prevents rapid restart cycles when many files change at once
    /// (e.g., during a build or git checkout).
    #[usage(env = "PITCHFORK_FILE_WATCH_DEBOUNCE", default = "1s", ty = "duration")]
    pub file_watch_debounce: String,

    /// Timeout for HTTP ready checks
    ///
    /// Maximum time to wait for a response when checking `ready_http` endpoints.
    ///
    /// Increase if your services take a while to respond during startup.
    #[usage(env = "PITCHFORK_HTTP_CLIENT_TIMEOUT", default = "5s", ty = "duration")]
    pub http_client_timeout: String,

    /// Daemon log buffer flush interval
    ///
    /// How often daemon log output is flushed to disk.
    /// Lower values mean logs appear faster in the UI but may impact performance.
    #[usage(
        env = "PITCHFORK_LOG_FLUSH_INTERVAL",
        default = "500ms",
        ty = "duration"
    )]
    pub log_flush_interval: String,

    /// What to do with live orphaned daemons on supervisor startup: adopt or kill
    ///
    /// When the supervisor starts and finds daemons in the state file whose
    /// processes are still alive from a previous supervisor instance that died
    /// uncleanly, this policy decides what happens (after the process identity
    /// is verified via PID plus kernel start time):
    ///
    /// - `adopt` (default): keep the process running and resume supervision.
    ///   The daemon keeps its state (status, ports, proxy routing) and is
    ///   monitored by polling. Log capture is unaffected, because a daemon's
    ///   output is read by a sibling sink process rather than by the supervisor,
    ///   so it continues uninterrupted across the crash. Exit codes of adopted
    ///   daemons cannot be observed, though;
    ///   an adopted daemon that dies unexpectedly is marked `errored` with an
    ///   unknown exit code, which makes it eligible for its configured retries.
    /// - `kill`: terminate the orphaned process group so the new supervisor
    ///   starts with a clean slate, matching pre-adoption behavior.
    ///
    /// Daemons whose recorded PID is dead, or whose PID now belongs to a
    /// different process, have their state reset under either policy. If the
    /// process identity cannot be verified, reconciliation fails closed and
    /// retains the running state without adopting or killing.
    ///
    /// The same policy applies when the interval watcher finds a running daemon
    /// that has lost its monitor at runtime.
    ///
    /// This setting has no effect when `cleanup_orphans` is disabled.
    #[usage(env = "PITCHFORK_ORPHAN_POLICY", default = "adopt")]
    pub orphan_policy: String,

    /// Maximum port increment attempts when auto_bump_port is enabled
    ///
    /// When `auto_bump_port = true` is set on a daemon, pitchfork will try incrementing
    /// all of the daemon's ports by the same offset to find a free range. This setting
    /// controls how many offsets are tried before giving up with an error.
    ///
    /// For example, with `port = [3000]` and `port_bump_attempts = 10`, pitchfork will
    /// try ports 3000, 3001, 3002, ... up to 3009 before reporting failure.
    ///
    /// This is a global default; individual daemons can override it with
    /// `port_bump_attempts` in their daemon configuration.
    #[usage(env = "PITCHFORK_PORT_BUMP_ATTEMPTS", default = 10)]
    pub port_bump_attempts: i64,

    /// Interval between ready checks (HTTP, TCP, command)
    ///
    /// How often to poll when checking if a daemon is ready using:
    /// - `ready_http` - HTTP health endpoint
    /// - `ready_port` - TCP port listening
    /// - `ready_cmd` - Shell command exit code
    ///
    /// Lower values detect readiness faster but use more resources.
    #[usage(
        env = "PITCHFORK_READY_CHECK_INTERVAL",
        default = "500ms",
        ty = "duration"
    )]
    pub ready_check_interval: String,

    /// Delay between stop and start during restart
    ///
    /// Brief pause after stopping a daemon before starting it again.
    /// Helps ensure resources (like ports) are fully released.
    #[usage(env = "PITCHFORK_RESTART_DELAY", default = "100ms", ty = "duration")]
    pub restart_delay: String,

    /// Maximum time to wait for daemon to stop gracefully
    ///
    /// When stopping a daemon, pitchfork sends SIGTERM and waits this long
    /// for the process to exit gracefully before sending SIGKILL.
    ///
    /// Increase for daemons that need time to clean up (e.g., flush data).
    #[usage(env = "PITCHFORK_STOP_TIMEOUT", default = "5s", ty = "duration")]
    pub stop_timeout: String,

    /// Default user to run daemon processes as
    ///
    /// Default Unix user for daemon processes spawned by the supervisor.
    ///
    /// When set, all daemons run as this user unless an individual daemon sets
    /// `user = "..."`. The value may be a username (for example `"postgres"`) or
    /// a numeric UID (for example `"501"`).
    ///
    /// If unset and the supervisor is running as root via `sudo`, daemons default to
    /// the sudo-calling user from `SUDO_UID`/`SUDO_GID` instead of running as root.
    #[usage(env = "PITCHFORK_USER", default = "")]
    pub user: String,

    /// File watcher config refresh interval
    ///
    /// How often the supervisor refreshes file watch configuration when using `watch` patterns.
    ///
    /// This controls how quickly newly started/stopped daemons with watch patterns are reflected
    /// in the active watcher set.
    ///
    /// For polling watcher cadence, use `supervisor.watch_poll_interval`.
    ///
    /// Lower values react faster to configuration/runtime changes but use more CPU.
    /// The default `"10s"` is appropriate for most environments.
    #[usage(
        env = "PITCHFORK_WATCH_INTERVAL",
        deprecated_env("PITCHFORK_WATCH_INTERVAL_MS"),
        default = "10s",
        ty = "duration"
    )]
    pub watch_interval: String,

    /// Polling watcher filesystem scan interval
    ///
    /// How often polling-based file watchers scan for changes.
    ///
    /// This applies when daemon `watch_mode` is `poll`, or when `watch_mode = "auto"`
    /// falls back to polling because native watchers are unavailable.
    ///
    /// Lower values detect changes faster but use more CPU and I/O.
    /// `"100ms"` is useful for highly interactive workflows;
    /// `"500ms"` is a practical default for remote/networked filesystems.
    #[usage(
        env = "PITCHFORK_WATCH_POLL_INTERVAL",
        default = "500ms",
        ty = "duration"
    )]
    pub watch_poll_interval: String,
}

/// The `tui.*` (terminal UI) settings.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "tui")]
pub struct SettingsTui {
    /// Status message display duration
    ///
    /// How long status messages (like "Daemon started") remain visible
    /// in the TUI before automatically clearing.
    #[usage(
        env = "PITCHFORK_TUI_MESSAGE_DURATION",
        default = "3s",
        ty = "duration"
    )]
    pub message_duration: String,

    /// Daemon list refresh interval
    ///
    /// How often the TUI refreshes the daemon list and status information.
    ///
    /// Lower values provide more responsive updates but may increase CPU usage,
    /// especially with many daemons.
    ///
    /// **Recommended values:**
    /// - `"1s"` - More responsive
    /// - `"2s"` - Default, balanced
    /// - `"5s"` - Lower resource usage
    #[usage(env = "PITCHFORK_TUI_REFRESH_RATE", default = "2s", ty = "duration")]
    pub refresh_rate: String,

    /// Number of stat samples to keep for graphs
    ///
    /// How many CPU/memory stat samples to keep for each daemon's graph.
    /// With the default refresh rate of 2s, 60 samples = ~2 minutes of history.
    ///
    /// Increase for longer history in graphs, at the cost of more memory.
    #[usage(env = "PITCHFORK_TUI_STAT_HISTORY", default = 60)]
    pub stat_history: i64,

    /// Event loop tick rate
    ///
    /// How often the TUI checks for keyboard input and other events.
    /// This affects input responsiveness.
    ///
    /// Most users won't need to change this. Lower values make the UI more
    /// responsive but use more CPU.
    #[usage(env = "PITCHFORK_TUI_TICK_RATE", default = "100ms", ty = "duration")]
    pub tick_rate: String,
}

/// The `web.*` (web UI) settings.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "web")]
pub struct SettingsWeb {
    /// Automatically start web UI when supervisor starts
    ///
    /// When enabled, the web UI server will automatically start alongside the supervisor.
    ///
    /// By default, this is disabled. You can also start the web UI manually with:
    /// ```text
    /// pitchfork supervisor start --web-port=3120
    /// ```
    #[usage(env = "PITCHFORK_WEB_AUTO_START", default = false)]
    pub auto_start: bool,

    /// URL path prefix for the web UI (e.g. "ps" serves at /ps/)
    ///
    /// Serves the web UI under a sub-path prefix, useful when running behind a reverse
    /// proxy that routes a path prefix to pitchfork.
    ///
    /// **Examples:**
    /// - `""` - Serve at root `/` (default)
    /// - `"ps"` - Serve at `/ps/`
    /// - `"tools/pitchfork"` - Serve at `/tools/pitchfork/`
    ///
    /// Equivalent to the `--web-path` CLI flag. The CLI flag takes priority over this setting.
    #[usage(env = "PITCHFORK_WEB_PATH", default = "")]
    pub base_path: String,

    /// Web server bind address
    ///
    /// IP address for the web UI to listen on.
    ///
    /// **Security Warning:** The default `127.0.0.1` only allows local connections.
    /// Setting this to `0.0.0.0` will expose the web UI to your network.
    ///
    /// **Examples:**
    /// - `"127.0.0.1"` - Local only (default, recommended)
    /// - `"0.0.0.0"` - All interfaces (use with caution)
    /// - `"192.168.1.100"` - Specific interface
    #[usage(env = "PITCHFORK_WEB_BIND_ADDRESS", default = "127.0.0.1")]
    pub bind_address: String,

    /// Default web server port
    ///
    /// The port number for the web UI. If this port is in use, pitchfork will
    /// try subsequent ports up to `port_attempts` times.
    #[usage(env = "PITCHFORK_WEB_BIND_PORT", default = 3120)]
    pub bind_port: i64,

    /// Initial number of log lines to display
    ///
    /// How many lines of logs to show initially when viewing daemon logs in the web UI.
    /// More lines means slower initial load but more history visible.
    #[usage(env = "PITCHFORK_WEB_LOG_LINES", default = 100)]
    pub log_lines: i64,

    /// Number of ports to try if default is in use
    ///
    /// If the default port is occupied, pitchfork will try this many consecutive
    /// ports before giving up.
    ///
    /// For example, with `bind_port = 3120` and `port_attempts = 10`,
    /// it will try ports 3120, 3121, 3122, ... up to 3129.
    #[usage(env = "PITCHFORK_WEB_PORT_ATTEMPTS", default = 10)]
    pub port_attempts: i64,

    /// Server-Sent Events poll interval for log streaming
    ///
    /// How often the web UI checks for new log lines when streaming logs.
    /// Lower values provide more real-time updates but use more resources.
    #[usage(
        env = "PITCHFORK_WEB_SSE_POLL_INTERVAL",
        default = "500ms",
        ty = "duration"
    )]
    pub sse_poll_interval: String,
}

/// Every setting pitchfork has.
///
/// `SETTINGS_PROPS`, `SETTINGS_REGISTRY`, `read`, and `spec_kdl` are generated
/// from these fields; the CLI's emitted spec carries the `config` block
/// through `#[usage(config = ...)]` on the root.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
pub struct Settings {
    #[usage(flatten)]
    pub api: SettingsApi,
    #[usage(flatten)]
    pub general: SettingsGeneral,
    #[usage(flatten)]
    pub ipc: SettingsIpc,
    #[usage(flatten)]
    pub logs: SettingsLogs,
    #[usage(flatten)]
    pub proxy: SettingsProxy,
    #[usage(flatten)]
    pub supervisor: SettingsSupervisor,
    #[usage(flatten)]
    pub tui: SettingsTui,
    #[usage(flatten)]
    pub web: SettingsWeb,
}

impl Default for Settings {
    /// The declared defaults, read the same way any other resolution is.
    fn default() -> Self {
        let resolved = resolve(Settings::SETTINGS_REGISTRY, Layers::new())
            .expect("no layers were given, so there is nothing to fail");
        Settings::read(&resolved).expect("every pitchfork setting has a declared default")
    }
}

/// Generate the `Duration` convenience getters the rest of the codebase uses
/// (e.g. `settings().general_autostop_delay()`), each parsing its humantime
/// string field and silently falling back to the schema default. Invalid
/// values were already warned about once at load time.
macro_rules! duration_getters {
    ($($name:ident => $group:ident . $field:ident, $default:literal;)+) => {
        impl Settings {
            $(
                #[doc = concat!("Get `", stringify!($group), ".", stringify!($field), "` as Duration")]
                #[allow(dead_code)]
                pub fn $name(&self) -> std::time::Duration {
                    Self::parse_duration(&self.$group.$field).unwrap_or_else(|| {
                        humantime::parse_duration($default)
                            .unwrap_or(std::time::Duration::from_secs(1))
                    })
                }
            )+
        }
    };
}

duration_getters! {
    general_autostop_delay => general.autostop_delay, "1m";
    general_interval => general.interval, "10s";
    ipc_connect_max_delay => ipc.connect_max_delay, "1s";
    ipc_connect_min_delay => ipc.connect_min_delay, "100ms";
    ipc_rate_limit_window => ipc.rate_limit_window, "1s";
    ipc_request_timeout => ipc.request_timeout, "5s";
    logs_time_retention => logs.time_retention, "";
    proxy_auto_start_timeout => proxy.auto_start_timeout, "30s";
    supervisor_cron_check_interval => supervisor.cron_check_interval, "10s";
    supervisor_file_watch_debounce => supervisor.file_watch_debounce, "1s";
    supervisor_http_client_timeout => supervisor.http_client_timeout, "5s";
    supervisor_log_flush_interval => supervisor.log_flush_interval, "500ms";
    supervisor_ready_check_interval => supervisor.ready_check_interval, "500ms";
    supervisor_restart_delay => supervisor.restart_delay, "100ms";
    supervisor_stop_timeout => supervisor.stop_timeout, "5s";
    supervisor_watch_interval => supervisor.watch_interval, "10s";
    supervisor_watch_poll_interval => supervisor.watch_poll_interval, "500ms";
    tui_message_duration => tui.message_duration, "3s";
    tui_refresh_rate => tui.refresh_rate, "2s";
    tui_tick_rate => tui.tick_rate, "100ms";
    web_sse_poll_interval => web.sse_poll_interval, "500ms";
}

impl Settings {
    /// Load settings from pitchfork.toml files, then overlay environment variables.
    /// Settings are loaded from all pitchfork.toml files in precedence order:
    /// 1. System-level: /etc/pitchfork/config.toml
    /// 2. User-level: ~/.config/pitchfork/config.toml
    /// 3. Project-level: pitchfork.toml files from root to current directory
    ///
    /// Environment variables override all file-based settings.
    ///
    /// The global [`settings()`] accessor resolves through
    /// [`Settings::resolve_from_dir`] so it can keep the provenance; this
    /// stays as the public one-shot entry point.
    #[allow(dead_code)]
    pub fn load() -> Self {
        Self::load_from_dir(&crate::env::CWD)
    }

    /// Load settings for a specific project directory, then overlay environment variables.
    ///
    /// This is used when operating on daemons from registered namespaces whose project
    /// settings may differ from those of the invoking process's current directory.
    pub fn load_from_dir(start_dir: &Path) -> Self {
        Self::resolve_from_dir(start_dir).0
    }

    /// Resolve settings for `start_dir`, returning both the typed struct and
    /// the origin-tracked resolution it was read from (for `pitchfork settings`).
    pub(crate) fn resolve_from_dir(start_dir: &Path) -> (Self, Resolved) {
        let env_layer = EnvLayer::from_process();
        // Highest precedence first, below the environment.
        let file_layers = Self::file_layers_from(start_dir);
        let mut layers = Layers::new().then(&env_layer);
        for layer in &file_layers {
            layers = layers.then(layer);
        }
        let mut resolved = match resolve(Settings::SETTINGS_REGISTRY, layers) {
            Ok(resolved) => resolved,
            Err(e) => {
                // Should not happen: every file was pre-validated above. Fall
                // back to env + defaults rather than failing to start.
                eprintln!("pitchfork: warning: failed to resolve settings from config files: {e}");
                resolve(Settings::SETTINGS_REGISTRY, Layers::new().then(&env_layer)).unwrap_or_else(
                    |_| {
                        resolve(Settings::SETTINGS_REGISTRY, Layers::new())
                            .expect("resolving only defaults cannot fail")
                    },
                )
            }
        };
        sanitize_durations(&mut resolved);
        for warning in usage_rs::config::explain::warnings(&resolved) {
            warn!("{warning}");
        }
        // Lossy, because the alternative is worse than the problem: `read` is all or nothing,
        // so one field the merge could not hand to its type used to cost every *other* setting
        // too — `Settings::default()` throws away the environment and every config file over a
        // single bad value. Here the offending field falls back to its own declared default and
        // nothing else is touched. Warn and keep going is pitchfork's call to make: a
        // supervisor that refuses to start because one duration is misspelled is a supervisor
        // that took the whole machine's daemons down with it.
        let (settings, errors) = Settings::read_lossy(&resolved);
        for error in &errors.0 {
            warn!("{error}");
        }
        // Every setting declares a default, so the only way this is `None` is a setting added
        // without one — a hole in the declaration rather than anything a user did.
        let settings = settings.unwrap_or_else(|| {
            warn!("a setting has no value and no default; falling back to built-in defaults");
            Settings::default()
        });
        (settings, resolved)
    }

    /// The config file layers for `start_dir`, highest precedence first.
    ///
    /// Project files are found by walking up from `start_dir`; within one
    /// directory `pitchfork.local.toml` outranks `pitchfork.toml`, which
    /// outranks the same pair under `.config/`, and any file in a nearer
    /// directory outranks every file in a farther one. Below the project
    /// files come the user-level and system-level configs.
    ///
    /// Files that exist but cannot be parsed are warned about and skipped,
    /// matching the previous loader: a broken file must not stop pitchfork
    /// from starting.
    fn file_layers_from(start_dir: &Path) -> Vec<FileLayer> {
        let mut candidates: Vec<(PathBuf, FileScope)> = xx::file::find_up_all(
            start_dir,
            &[
                "pitchfork.local.toml",
                "pitchfork.toml",
                ".config/pitchfork.local.toml",
                ".config/pitchfork.toml",
            ],
        )
        .into_iter()
        .map(|path| (path, FileScope::Project))
        .collect();
        candidates.push((
            crate::env::PITCHFORK_GLOBAL_CONFIG_USER.clone(),
            FileScope::Global,
        ));
        candidates.push((
            crate::env::PITCHFORK_GLOBAL_CONFIG_SYSTEM.clone(),
            FileScope::System,
        ));
        candidates
            .into_iter()
            .filter(|(path, _)| readable_settings_file(path))
            .map(|(path, scope)| FileLayer::at(path, scope).under("settings"))
            .collect()
    }

    /// Parse a duration string (humantime format) to Duration
    pub fn parse_duration(s: &str) -> Option<std::time::Duration> {
        humantime::parse_duration(s).ok()
    }

    /// Resolve the mise binary path.
    ///
    /// If `general.mise_bin` is explicitly set, returns that path.
    /// Otherwise, searches well-known install locations:
    /// - `~/.local/bin/mise`
    /// - `~/.cargo/bin/mise`
    /// - `/usr/local/bin/mise`
    /// - `/opt/homebrew/bin/mise`
    ///
    /// Returns `None` if mise cannot be found.
    pub fn resolve_mise_bin(&self) -> Option<std::path::PathBuf> {
        // Explicit configuration takes priority
        if !self.general.mise_bin.is_empty() {
            let p = PathBuf::from(&self.general.mise_bin);
            if p.is_file() {
                return Some(p);
            }
            warn!(
                "mise_bin is set to {:?} but the file does not exist",
                self.general.mise_bin
            );
            return None;
        }

        // Search well-known install paths
        let home = crate::env::HOME_DIR.as_path();
        let candidates = [
            home.join(".local/bin/mise"),
            home.join(".cargo/bin/mise"),
            PathBuf::from("/usr/local/bin/mise"),
            PathBuf::from("/opt/homebrew/bin/mise"),
        ];

        candidates.into_iter().find(|p| p.is_file())
    }

    /// Return `supervisor.port_bump_attempts` as `u32`, clamping out-of-range
    /// values to the schema default (10) and zero to 1.
    ///
    /// This is the single source of truth for the fallback so that call-sites
    /// don't each duplicate the hardcoded `10`.
    pub fn default_port_bump_attempts(&self) -> u32 {
        let v = u32::try_from(self.supervisor.port_bump_attempts).unwrap_or_else(|_| {
            warn!(
                "supervisor.port_bump_attempts value {} is out of range (0-{}), clamping to 10",
                self.supervisor.port_bump_attempts,
                u32::MAX
            );
            10
        });
        if v == 0 {
            warn!("supervisor.port_bump_attempts is 0; defaulting to 1");
            1
        } else {
            v
        }
    }
}

/// Whether `path` holds a settings file this loader can hand to a `FileLayer`.
///
/// Mirrors the previous loader's per-file error handling: a missing file is
/// the ordinary case, and a file that exists but cannot be read or parsed —
/// or whose `settings` key is not a table — is warned about and skipped
/// rather than turned into a startup failure.
fn readable_settings_file(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) => {
            eprintln!(
                "pitchfork: warning: failed to read {}: {}",
                path.display(),
                e
            );
            return false;
        }
    };
    let table: toml::Table = match content.parse() {
        Ok(table) => table,
        Err(e) => {
            eprintln!(
                "pitchfork: warning: failed to parse {}: {}",
                path.display(),
                e
            );
            return false;
        }
    };
    match table.get("settings") {
        None => true,
        Some(toml::Value::Table(_)) => true,
        Some(_) => {
            eprintln!(
                "pitchfork: warning: invalid [settings] in {}: not a table",
                path.display()
            );
            false
        }
    }
}

/// Validate every duration-typed setting in a resolution.
///
/// The registry's `duration` type stores the humantime text as-is, so an
/// invalid value would otherwise ride into the struct. This warns once and
/// rewrites the value to the schema default through [`Resolved::coerced`],
/// so `pitchfork settings get/list` and the typed getters all agree —
/// matching the previous loader, which rejected invalid durations at load
/// time. A value equal to the declared default is left alone (the
/// `logs.time_retention` default is the empty string, meaning "disabled").
fn sanitize_durations(resolved: &mut Resolved) {
    let registry = Settings::SETTINGS_REGISTRY;
    for id in registry.ids() {
        let meta = registry.get(id);
        if !matches!(meta.ty.inner(), Ty::Duration) {
            continue;
        }
        let Some(Value::String(text)) = resolved.get(id) else {
            continue;
        };
        let default_text = match meta.default {
            Some(Const::Str(s)) => s,
            _ => "",
        };
        if text == default_text || humantime::parse_duration(text).is_ok() {
            continue;
        }
        let text = text.clone();
        let origin = resolved
            .origin(id)
            .map(|o| o.describe().to_string())
            .unwrap_or_default();
        warn!(
            "invalid duration {:?} for {} (set by {}), using default",
            text, meta.key, origin
        );
        resolved.coerced(
            id,
            Value::String(default_text.to_string()),
            format!("invalid duration {text:?}; the default stands"),
        );
    }
}

/// Global settings instance, kept beside the resolution it was read from
/// (RwLock<Arc> to support runtime reload).
type SettingsState = (Arc<Settings>, Arc<Resolved>);
static SETTINGS: std::sync::RwLock<Option<SettingsState>> = std::sync::RwLock::new(None);

fn settings_state() -> SettingsState {
    // Fast path: if already initialized, clone the Arc pointers.
    {
        let lock = SETTINGS.read().unwrap();
        if let Some(state) = lock.as_ref() {
            return state.clone();
        }
    }
    // Slow path: initialize on first access
    let mut lock = SETTINGS.write().unwrap();
    if let Some(state) = lock.as_ref() {
        return state.clone();
    }
    let (settings, resolved) = Settings::resolve_from_dir(&crate::env::CWD);
    let state = (Arc::new(settings), Arc::new(resolved));
    *lock = Some(state.clone());
    state
}

/// Get the global settings instance
pub fn settings() -> Arc<Settings> {
    settings_state().0
}

/// The origin-tracked resolution behind [`settings()`], for the settings CLI.
pub(crate) fn settings_resolved() -> Arc<Resolved> {
    settings_state().1
}

/// Reload settings from config files.
///
/// Called when the supervisor receives a ReloadConfig IPC request,
/// typically after `pitchfork settings set` modifies a config file.
///
/// Old Arc references remain valid until all holders drop them,
/// so no use-after-free can occur.
pub fn reload_settings() {
    let (settings, resolved) = Settings::resolve_from_dir(&crate::env::CWD);
    let mut lock = SETTINGS.write().unwrap();
    *lock = Some((Arc::new(settings), Arc::new(resolved)));
}

// ============================================================================
// Partial (all-Option) mirror structs
//
// These exist for `pitchfork.toml` round-trips: `PitchforkToml` deserializes
// its `[settings]` table into `SettingsPartial`, merges per-file overrides,
// and serializes it back out (`pitchfork settings set`, `proxy add`, ...).
// Settings *resolution* no longer goes through these — usage-config's
// origin-tracked merge replaced the apply/merge chain.
// ============================================================================

macro_rules! settings_partial {
    (
        $(#[$meta:meta])*
        $name:ident {
            $(@group $group_field:ident: $group_ty:ty,)*
            $($(#[$fdoc:meta])* $field:ident: $ty:ty,)*
        }
    ) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
        )]
        #[serde(default)]
        pub struct $name {
            $(
                #[serde(default, skip_serializing_if = "is_empty_partial")]
                pub $group_field: $group_ty,
            )*
            $(
                $(#[$fdoc])*
                #[serde(skip_serializing_if = "Option::is_none", default)]
                pub $field: Option<$ty>,
            )*
        }

        #[allow(dead_code)]
        impl $name {
            /// Whether any field (or nested group field) is set.
            pub fn has_any_set(&self) -> bool {
                false
                $(|| self.$group_field.has_any_set())*
                $(|| self.$field.is_some())*
            }

            pub fn is_empty(&self) -> bool {
                !self.has_any_set()
            }

            /// Merge another partial onto this one.
            /// All `Some` values in `other` override the corresponding values in `self`.
            pub fn merge_from(&mut self, other: &Self) {
                $(self.$group_field.merge_from(&other.$group_field);)*
                $(
                    if other.$field.is_some() {
                        self.$field = other.$field.clone();
                    }
                )*
            }
        }
    };
}

/// serde `skip_serializing_if` helper for nested partial groups.
fn is_empty_partial<T: HasAnySet>(v: &T) -> bool {
    !v.has_any_set()
}

/// Internal trait so one serde skip helper serves every partial group.
trait HasAnySet {
    fn has_any_set(&self) -> bool;
}

macro_rules! impl_has_any_set {
    ($($name:ident),+ $(,)?) => {
        $(impl HasAnySet for $name {
            fn has_any_set(&self) -> bool {
                $name::has_any_set(self)
            }
        })+
    };
}

settings_partial! {
    /// Partial mirror of [`SettingsApi`].
    SettingsApiPartial {
        /// Automatically start the standalone API server
        auto_start: bool,
        /// IP address the API server binds to
        bind_address: String,
        /// Port the standalone API server listens on
        bind_port: i64,
        /// Number of consecutive ports to try if api.bind_port is in use
        port_attempts: i64,
        /// Authentication token for API access when bound to non-loopback addresses
        token: String,
    }
}

settings_partial! {
    /// Partial mirror of [`SettingsGeneral`].
    SettingsGeneralPartial {
        /// Delay before auto-stopping daemons when leaving a directory
        autostop_delay: String,
        /// Supervisor background task refresh interval
        interval: String,
        /// File log level (trace, debug, info, warn, error)
        log_file_level: String,
        /// Console log level (trace, debug, info, warn, error)
        log_level: String,
        /// Wrap daemon commands with mise x -- globally
        mise: bool,
        /// Explicit path to the mise binary
        mise_bin: String,
        /// Shell command used to execute daemon run scripts
        shell: String,
        /// Show timestamps in startup log output
        startup_log_timestamps: bool,
    }
}

settings_partial! {
    /// Partial mirror of [`SettingsIpc`].
    SettingsIpcPartial {
        /// Number of connection retry attempts
        connect_attempts: i64,
        /// Maximum delay between connection retries
        connect_max_delay: String,
        /// Minimum delay between connection retries
        connect_min_delay: String,
        /// Maximum IPC requests per second per connection
        rate_limit: i64,
        /// Rate limit sliding window duration
        rate_limit_window: String,
        /// Default timeout for IPC requests
        request_timeout: String,
    }
}

settings_partial! {
    /// Partial mirror of [`SettingsLogsArchiveHook`].
    SettingsLogsArchiveHookPartial {
        /// Maximum log entries per archive hook invocation
        batch_size: i64,
        /// Command to run before retention deletes old log entries
        command: String,
    }
}

settings_partial! {
    /// Partial mirror of [`SettingsLogs`].
    SettingsLogsPartial {
        @group archive_hook: SettingsLogsArchiveHookPartial,
        /// Count-based log retention (e.g. 10000)
        line_retention: i64,
        /// Default log format for daemons (json | logfmt | text)
        log_format: String,
        /// Time-based log retention duration (e.g. '7d', '30d')
        time_retention: String,
        /// Show timestamps in log output
        timestamp: bool,
        /// strftime format for log timestamps in `pitchfork logs` output
        timestamp_format: String,
    }
}

settings_partial! {
    /// Partial mirror of [`SettingsProxy`].
    SettingsProxyPartial {
        /// Automatically start daemons when accessed via proxy URL
        auto_start: bool,
        /// Maximum time to wait for an auto-started daemon to become ready
        auto_start_timeout: String,
        /// Automatically install the proxy TLS certificate into the system trust store
        auto_trust: bool,
        /// Enable the reverse proxy server for daemons
        enable: bool,
        /// Bind address for the reverse proxy server
        host: String,
        /// Enable HTTPS for the reverse proxy
        https: bool,
        /// Enable LAN mode for the reverse proxy
        lan: bool,
        /// Pin a specific LAN IP address instead of auto-detecting
        lan_ip: String,
        /// Port the reverse proxy server listens on
        port: i64,
        /// Automatically sync slug hostnames to /etc/hosts
        sync_hosts: bool,
        /// Top-level domain used for proxy URLs
        tld: String,
        /// Path to TLS certificate file (PEM format) for HTTPS proxy
        tls_cert: String,
        /// Path to TLS private key file (PEM format) for HTTPS proxy
        tls_key: String,
        /// Enable wildcard subdomain matching for proxy routes
        wildcard: bool,
        /// Enable git worktree / jj workspace auto-discovery for slug routing
        worktree: bool,
    }
}

settings_partial! {
    /// Partial mirror of [`SettingsSupervisor`].
    SettingsSupervisorPartial {
        /// Automatically start the supervisor when a client command needs it
        auto_start: bool,
        /// Reconcile orphaned daemon processes when supervisor starts
        cleanup_orphans: bool,
        /// Enable container/PID1 mode for running inside Docker containers
        container: bool,
        /// Consecutive CPU-over-limit samples before killing a daemon
        cpu_violation_threshold: i64,
        /// Interval for checking cron schedules
        cron_check_interval: String,
        /// File watch debounce duration
        file_watch_debounce: String,
        /// Timeout for HTTP ready checks
        http_client_timeout: String,
        /// Daemon log buffer flush interval
        log_flush_interval: String,
        /// What to do with live orphaned daemons on supervisor startup: adopt or kill
        orphan_policy: String,
        /// Maximum port increment attempts when auto_bump_port is enabled
        port_bump_attempts: i64,
        /// Interval between ready checks (HTTP, TCP, command)
        ready_check_interval: String,
        /// Delay between stop and start during restart
        restart_delay: String,
        /// Maximum time to wait for daemon to stop gracefully
        stop_timeout: String,
        /// Default user to run daemon processes as
        user: String,
        /// File watcher config refresh interval
        watch_interval: String,
        /// Polling watcher filesystem scan interval
        watch_poll_interval: String,
    }
}

settings_partial! {
    /// Partial mirror of [`SettingsTui`].
    SettingsTuiPartial {
        /// Status message display duration
        message_duration: String,
        /// Daemon list refresh interval
        refresh_rate: String,
        /// Number of stat samples to keep for graphs
        stat_history: i64,
        /// Event loop tick rate
        tick_rate: String,
    }
}

settings_partial! {
    /// Partial mirror of [`SettingsWeb`].
    SettingsWebPartial {
        /// Automatically start web UI when supervisor starts
        auto_start: bool,
        /// URL path prefix for the web UI (e.g. "ps" serves at /ps/)
        base_path: String,
        /// Web server bind address
        bind_address: String,
        /// Default web server port
        bind_port: i64,
        /// Initial number of log lines to display
        log_lines: i64,
        /// Number of ports to try if default is in use
        port_attempts: i64,
        /// Server-Sent Events poll interval for log streaming
        sse_poll_interval: String,
    }
}

settings_partial! {
    /// Partial mirror of [`Settings`], for `[settings]` tables in
    /// pitchfork.toml files.
    SettingsPartial {
        @group api: SettingsApiPartial,
        @group general: SettingsGeneralPartial,
        @group ipc: SettingsIpcPartial,
        @group logs: SettingsLogsPartial,
        @group proxy: SettingsProxyPartial,
        @group supervisor: SettingsSupervisorPartial,
        @group tui: SettingsTuiPartial,
        @group web: SettingsWebPartial,
    }
}

impl_has_any_set!(
    SettingsApiPartial,
    SettingsGeneralPartial,
    SettingsIpcPartial,
    SettingsLogsArchiveHookPartial,
    SettingsLogsPartial,
    SettingsProxyPartial,
    SettingsSupervisorPartial,
    SettingsTuiPartial,
    SettingsWebPartial,
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();

        // Test general settings
        assert_eq!(settings.general.autostop_delay, "1m");
        assert_eq!(settings.general.interval, "10s");
        assert_eq!(settings.general.log_level, "info");

        // Test IPC settings
        assert_eq!(settings.ipc.connect_attempts, 5);
        assert_eq!(settings.ipc.request_timeout, "5s");
        assert_eq!(settings.ipc.rate_limit, 100);

        // Test web settings
        assert!(!settings.web.auto_start);
        assert_eq!(settings.web.bind_address, "127.0.0.1");
        assert_eq!(settings.web.bind_port, 3120);
        assert_eq!(settings.web.log_lines, 100);

        // Test TUI settings
        assert_eq!(settings.tui.refresh_rate, "2s");
        assert_eq!(settings.tui.stat_history, 60);

        // Test supervisor settings
        assert!(settings.supervisor.auto_start);
        assert_eq!(settings.supervisor.ready_check_interval, "500ms");
        assert_eq!(settings.supervisor.file_watch_debounce, "1s");
        assert_eq!(settings.supervisor.user, "");
    }

    #[test]
    fn a_bad_value_costs_only_its_own_setting() {
        // `Settings::read` is all or nothing, and the fallback for its failure used to be
        // `Settings::default()` — one field the merge could not hand to its type demoting all
        // sixty-eight settings and discarding the environment along with them.
        let env = EnvLayer::new([("PITCHFORK_LOG".to_string(), "debug".to_string())]);
        let mut resolved = resolve(Settings::SETTINGS_REGISTRY, Layers::new().then(&env)).unwrap();

        // The one door into a resolution the merge does not check: a post-merge hook, which is
        // what `sanitize_durations` is. A bool where an integer belongs is what a buggy one
        // could write.
        let registry = Settings::SETTINGS_REGISTRY;
        let rate_limit = registry.lookup("ipc.rate_limit").unwrap().id;
        resolved.coerced(rate_limit, Value::Bool(true), "a hook that got it wrong");

        assert!(
            Settings::read(&resolved).is_err(),
            "the strict read still refuses it"
        );

        let (settings, errors) = Settings::read_lossy(&resolved);
        let settings = settings.expect("every setting declares a default");
        assert_eq!(
            settings.ipc.rate_limit, 100,
            "the bad field takes its declared default"
        );
        assert_eq!(
            settings.general.log_level, "debug",
            "and PITCHFORK_LOG is not collateral damage"
        );
        assert_eq!(errors.0.len(), 1, "{errors}");
        assert_eq!(errors.0[0].key, "ipc.rate_limit");
    }

    #[test]
    fn every_setting_declares_a_default() {
        // What makes the `None` arm of the lossy read unreachable: a setting with no value and
        // no default is a hole in this file, not something a user can cause. Adding one without
        // a default should fail here rather than silently demote the whole struct at runtime.
        let missing: Vec<&str> = Settings::SETTINGS_PROPS
            .iter()
            .filter(|meta| meta.default.is_none())
            .map(|meta| meta.key)
            .collect();
        assert!(
            missing.is_empty(),
            "these settings declare no default: {missing:?}"
        );
    }

    #[test]
    fn test_registry_matches_previous_schema() {
        // The emitted config block is the settings documentation now; make
        // sure the registry still declares what settings.toml used to.
        let keys: Vec<&str> = Settings::SETTINGS_PROPS
            .iter()
            .map(|meta| meta.key)
            .collect();
        assert_eq!(keys.len(), 68, "{keys:?}");
        assert!(keys.contains(&"general.autostop_delay"));
        assert!(keys.contains(&"logs.archive_hook.command"));
        assert!(keys.contains(&"supervisor.watch_interval"));

        let registry = Settings::SETTINGS_REGISTRY;
        let interval = registry.get(registry.lookup("general.interval").unwrap().id);
        assert_eq!(interval.envs, &["PITCHFORK_INTERVAL"]);
        assert_eq!(interval.deprecated_envs, &["PITCHFORK_INTERVAL_SECS"]);
        let watch = registry.get(registry.lookup("supervisor.watch_interval").unwrap().id);
        assert_eq!(watch.deprecated_envs, &["PITCHFORK_WATCH_INTERVAL_MS"]);
        let log_level = registry.get(registry.lookup("general.log_level").unwrap().id);
        assert_eq!(log_level.envs, &["PITCHFORK_LOG"]);
    }

    #[test]
    fn test_parse_duration() {
        assert_eq!(Settings::parse_duration("1s"), Some(Duration::from_secs(1)));
        assert_eq!(
            Settings::parse_duration("500ms"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            Settings::parse_duration("1m"),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            Settings::parse_duration("2h"),
            Some(Duration::from_secs(7200))
        );
        assert_eq!(Settings::parse_duration("invalid"), None);
    }

    #[test]
    fn test_env_override() {
        // The environment is described rather than reached for, so this test
        // does not mutate process env vars (the old test needed a mutex).
        let env = EnvLayer::new([
            ("PITCHFORK_AUTOSTOP_DELAY".to_string(), "10m".to_string()),
            ("PITCHFORK_INTERVAL".to_string(), "5s".to_string()),
            (
                "PITCHFORK_IPC_CONNECT_ATTEMPTS".to_string(),
                "20".to_string(),
            ),
            (
                "PITCHFORK_SUPERVISOR_AUTO_START".to_string(),
                "false".to_string(),
            ),
            ("PITCHFORK_WEB_AUTO_START".to_string(), "true".to_string()),
        ]);
        let resolved = resolve(Settings::SETTINGS_REGISTRY, Layers::new().then(&env)).unwrap();
        let settings = Settings::read(&resolved).unwrap();

        assert_eq!(settings.general.autostop_delay, "10m");
        assert_eq!(settings.general.interval, "5s");
        assert_eq!(settings.ipc.connect_attempts, 20);
        assert!(!settings.supervisor.auto_start);
        assert!(settings.web.auto_start);

        // Fields with no corresponding env var set remain at defaults
        assert_eq!(settings.general.log_level, "info");
        assert_eq!(settings.ipc.rate_limit, 100);
    }

    #[test]
    fn test_deprecated_env_still_works_and_warns() {
        let env = EnvLayer::new([("PITCHFORK_INTERVAL_SECS".to_string(), "30s".to_string())]);
        let resolved = resolve(Settings::SETTINGS_REGISTRY, Layers::new().then(&env)).unwrap();
        let settings = Settings::read(&resolved).unwrap();
        assert_eq!(settings.general.interval, "30s");
        let warnings = usage_rs::config::explain::warnings(&resolved);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("PITCHFORK_INTERVAL_SECS is deprecated")),
            "{warnings:?}"
        );

        // The current name wins over the deprecated alias when both are set.
        let env = EnvLayer::new([
            ("PITCHFORK_INTERVAL_SECS".to_string(), "30s".to_string()),
            ("PITCHFORK_INTERVAL".to_string(), "7s".to_string()),
        ]);
        let resolved = resolve(Settings::SETTINGS_REGISTRY, Layers::new().then(&env)).unwrap();
        let settings = Settings::read(&resolved).unwrap();
        assert_eq!(settings.general.interval, "7s");
    }

    #[test]
    fn test_invalid_duration_from_env_falls_back_to_default() {
        // The previous loader rejected invalid durations at load time; the
        // sanitize pass reproduces that through Resolved::coerced.
        let env = EnvLayer::new([(
            "PITCHFORK_AUTOSTOP_DELAY".to_string(),
            "not_a_duration".to_string(),
        )]);
        let mut resolved = resolve(Settings::SETTINGS_REGISTRY, Layers::new().then(&env)).unwrap();
        sanitize_durations(&mut resolved);
        let settings = Settings::read(&resolved).unwrap();
        assert_eq!(settings.general.autostop_delay, "1m");
        assert_eq!(settings.general_autostop_delay(), Duration::from_secs(60));
    }

    #[test]
    fn test_invalid_duration_fallback() {
        let mut settings = Settings::default();

        // Set invalid duration values
        settings.general.autostop_delay = "invalid".to_string();
        settings.general.interval = "not_a_duration".to_string();

        // Convenience methods should fallback to default values
        assert_eq!(settings.general_autostop_delay(), Duration::from_secs(60)); // default "1m"
        assert_eq!(settings.general_interval(), Duration::from_secs(10)); // default "10s"
    }

    #[test]
    fn test_duration_methods_all_fields() {
        let settings = Settings::default();

        assert_eq!(settings.general_autostop_delay(), Duration::from_secs(60));
        assert_eq!(settings.general_interval(), Duration::from_secs(10));
        assert_eq!(settings.ipc_connect_min_delay(), Duration::from_millis(100));
        assert_eq!(settings.ipc_connect_max_delay(), Duration::from_secs(1));
        assert_eq!(settings.ipc_request_timeout(), Duration::from_secs(5));
        assert_eq!(settings.ipc_rate_limit_window(), Duration::from_secs(1));
        assert_eq!(settings.web_sse_poll_interval(), Duration::from_millis(500));
        assert_eq!(settings.tui_refresh_rate(), Duration::from_secs(2));
        assert_eq!(settings.tui_tick_rate(), Duration::from_millis(100));
        assert_eq!(settings.tui_message_duration(), Duration::from_secs(3));
        assert_eq!(
            settings.supervisor_ready_check_interval(),
            Duration::from_millis(500)
        );
        assert_eq!(
            settings.supervisor_file_watch_debounce(),
            Duration::from_secs(1)
        );
        assert_eq!(
            settings.supervisor_log_flush_interval(),
            Duration::from_millis(500)
        );
        assert_eq!(settings.supervisor_stop_timeout(), Duration::from_secs(5));
        assert_eq!(
            settings.supervisor_restart_delay(),
            Duration::from_millis(100)
        );
        assert_eq!(
            settings.supervisor_cron_check_interval(),
            Duration::from_secs(10)
        );
        assert_eq!(
            settings.supervisor_http_client_timeout(),
            Duration::from_secs(5)
        );
    }

    /// A directory tree, cleaned up when the test ends.
    struct Tree(std::path::PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("pitchfork_settings_{}_{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, rel: &str, text: &str) -> std::path::PathBuf {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_settings_read_from_pitchfork_toml_settings_table() {
        let tree = Tree::new("table");
        let path = tree.write(
            "pitchfork.toml",
            r#"
[daemons.myapp]
run = "node server.js"

[settings.general]
autostop_delay = "5m"
log_level = "debug"

[settings.web]
auto_start = true
bind_port = 8080

[settings.supervisor]
auto_start = false
user = "postgres"
"#,
        );
        let layer = FileLayer::at(&path, FileScope::Project).under("settings");
        let resolved = resolve(Settings::SETTINGS_REGISTRY, Layers::new().then(&layer)).unwrap();
        let settings = Settings::read(&resolved).unwrap();

        assert_eq!(settings.general.autostop_delay, "5m");
        assert_eq!(settings.general.log_level, "debug");
        assert!(settings.web.auto_start);
        assert_eq!(settings.web.bind_port, 8080);
        assert!(!settings.supervisor.auto_start);
        assert_eq!(settings.supervisor.user, "postgres");
        // Unset fields fall back to defaults, and [daemons] is not a warning.
        assert_eq!(settings.general.interval, "10s");
        assert_eq!(settings.ipc.connect_attempts, 5);
        assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
    }

    #[test]
    fn test_explicit_default_value_in_higher_file_still_overrides() {
        // "Bug 5": the old merge_from skipped values equal to the default, so
        // a higher-precedence file explicitly setting log_level = "info"
        // could not override "warn" from a lower one. usage-config's
        // origin-tracked merge does this natively; pin it.
        let tree = Tree::new("bug5");
        let lower = tree.write("lower.toml", "[settings.general]\nlog_level = \"warn\"\n");
        let higher = tree.write("higher.toml", "[settings.general]\nlog_level = \"info\"\n");
        let lower = FileLayer::at(&lower, FileScope::Global).under("settings");
        let higher = FileLayer::at(&higher, FileScope::Project).under("settings");
        let resolved = resolve(
            Settings::SETTINGS_REGISTRY,
            Layers::new().then(&higher).then(&lower),
        )
        .unwrap();
        let settings = Settings::read(&resolved).unwrap();
        assert_eq!(settings.general.log_level, "info");

        // And the provenance says the higher file set it, not the default.
        let id = Settings::SETTINGS_REGISTRY
            .lookup("general.log_level")
            .unwrap()
            .id;
        let origin = resolved.origin(id).unwrap().describe();
        assert!(origin.contains("higher.toml"), "{origin}");
    }

    #[test]
    fn test_file_precedence_local_over_base_and_nearer_dir_wins() {
        let tree = Tree::new("precedence");
        tree.write("pitchfork.toml", "[settings.general]\ninterval = \"9s\"\n");
        tree.write(
            "pitchfork.local.toml",
            "[settings.general]\ninterval = \"2s\"\n",
        );
        tree.write(
            ".config/pitchfork.toml",
            "[settings.general]\ninterval = \"7s\"\n[settings.web]\nbind_port = 9999\n",
        );
        let layers = Settings::file_layers_from(&tree.0);
        let mut chain = Layers::new();
        for layer in &layers {
            chain = chain.then(layer);
        }
        let resolved = resolve(Settings::SETTINGS_REGISTRY, chain).unwrap();
        let settings = Settings::read(&resolved).unwrap();
        // pitchfork.local.toml > pitchfork.toml > .config/pitchfork.toml
        assert_eq!(settings.general.interval, "2s");
        // A key only the lowest file sets still applies.
        assert_eq!(settings.web.bind_port, 9999);

        // A nested directory's file outranks everything above it.
        let deep = tree.0.join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        tree.write(
            "a/b/pitchfork.toml",
            "[settings.general]\ninterval = \"1s\"\n",
        );
        let layers = Settings::file_layers_from(&deep);
        let mut chain = Layers::new();
        for layer in &layers {
            chain = chain.then(layer);
        }
        let resolved = resolve(Settings::SETTINGS_REGISTRY, chain).unwrap();
        let settings = Settings::read(&resolved).unwrap();
        assert_eq!(settings.general.interval, "1s");
    }

    #[test]
    fn test_broken_file_is_skipped_rather_than_fatal() {
        let tree = Tree::new("broken");
        tree.write("pitchfork.toml", "[invalid toml [[");
        // The broken file is skipped with a warning, so the resolution still
        // happens and produces the defaults.
        let layers = Settings::file_layers_from(&tree.0);
        assert!(layers.is_empty());
    }

    #[test]
    fn test_unknown_settings_keys_do_not_fail_the_load() {
        // Forward compatibility: a config written for a newer pitchfork must
        // still load. Unknown keys are warned about rather than fatal.
        let tree = Tree::new("unknown");
        let path = tree.write(
            "pitchfork.toml",
            "[settings.general]\nlog_level = \"debug\"\nfrom_the_future = true\n",
        );
        let layer = FileLayer::at(&path, FileScope::Project).under("settings");
        let resolved = resolve(Settings::SETTINGS_REGISTRY, Layers::new().then(&layer)).unwrap();
        let settings = Settings::read(&resolved).unwrap();
        assert_eq!(settings.general.log_level, "debug");
        assert_eq!(resolved.warnings.len(), 1, "{:?}", resolved.warnings);
    }

    #[test]
    fn test_partial_merge_from() {
        let mut base = SettingsPartial::default();
        base.general.log_level = Some("warn".to_string());
        base.web.bind_address = Some("0.0.0.0".to_string());

        let mut overlay = SettingsPartial::default();
        overlay.general.log_level = Some("debug".to_string());
        overlay.tui.refresh_rate = Some("1s".to_string());

        base.merge_from(&overlay);
        assert_eq!(base.general.log_level.as_deref(), Some("debug"));
        assert_eq!(base.web.bind_address.as_deref(), Some("0.0.0.0"));
        assert_eq!(base.tui.refresh_rate.as_deref(), Some("1s"));

        // An empty partial changes nothing.
        let sealed = base.clone();
        base.merge_from(&SettingsPartial::default());
        assert_eq!(base.general.log_level, sealed.general.log_level);
        assert!(SettingsPartial::default().is_empty());
        assert!(base.has_any_set());
    }

    #[test]
    fn test_partial_serialization_skips_unset() {
        let mut partial = SettingsPartial::default();
        partial.general.interval = Some("5s".to_string());
        let toml = toml::to_string_pretty(&partial).unwrap();
        assert_eq!(toml, "[general]\ninterval = \"5s\"\n");
    }
}
