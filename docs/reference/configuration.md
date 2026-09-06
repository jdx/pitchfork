---
description: Look up every daemon option, shared environment values, configuration precedence, and registries.
---
# Configuration Reference

Use `pitchfork.toml` to define daemons and their lifecycle. For a walkthrough,
see [your first project](/first-daemon). For pitchfork-wide defaults such as
logging and dashboard ports, see [settings](/reference/settings).

## Find an option

| Task | Fields |
| --- | --- |
| Run a command | [`run`](#run-required), [`dir`](#dir), [`env`](#env), [`mise`](#mise), [`user`](#user), [`pty`](#pty) |
| Order startup | [`depends`](#depends), [`ready_delay`](#ready-delay), [`ready_output`](#ready-output), [`ready_http`](#ready-http), [`ready_port`](#ready-port), [`ready_cmd`](#ready-cmd) |
| Recover and monitor | [`retry`](#retry), [`health_cmd`](#health-cmd), [`health_http`](#health-http), [`health_port`](#health-port), [`memory_limit`](#memory-limit), [`cpu_limit`](#cpu-limit) |
| Automate the lifecycle | [`auto`](#auto), [`watch`](#watch), [`watch_mode`](#watch-mode), [`boot_start`](#boot-start), [`cron`](#cron), [`hooks`](#hooks), [`stop_signal`](#stop-signal) |
| Configure ports and logs | [`port`](#port), [`logs`](#logs) |
| Share project configuration | [Environment defaults](#shared-environment), [groups](#daemon-groups), [namespace registry](#namespace-registry), [proxy slugs](#global-config-slug-registry) |

## Configuration Hierarchy

Pitchfork loads configuration files in order, with later files overriding earlier ones:

1. **System-level:** `/etc/pitchfork/config.toml` (namespace: `global`)
2. **User-level:** `~/.config/pitchfork/config.toml` (namespace: `global`)
3. **Project-level:** `.config/pitchfork.toml`, `.config/pitchfork.local.toml`, `pitchfork.toml`, `pitchfork.local.toml` from filesystem root to current directory

Within each directory, files are processed in this order:
- `.config/pitchfork.toml` (lowest precedence in directory)
- `.config/pitchfork.local.toml` (overrides `.config/pitchfork.toml`)
- `pitchfork.toml` (overrides everything in `.config/`)
- `pitchfork.local.toml` (highest precedence in directory; add it to `.gitignore` for personal overrides)

This mirrors [mise](https://mise.jdx.dev/configuration.html) behavior, allowing you to store project config in a centralized `.config/` directory if preferred.

## JSON Schema

A JSON Schema is available for editor autocompletion and validation:

**URL:** [`https://pitchfork.jdx.dev/schema.json`](/schema.json)

### Editor Setup

**VS Code** with [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml):

```toml
#:schema https://pitchfork.jdx.dev/schema.json

[daemons.api]
run = "npm run server"
```

Use the schema URL with a TOML editor or language server that supports JSON Schema validation.

## File Format

All configuration uses TOML format:

```toml
namespace = "my-project" # optional, project-directory namespace override

[daemons.api]
run = "node server.js"
# ... other options
```

### Daemon Naming Rules

Daemon names must follow these rules:

| Rule | Valid | Invalid |
|------|-------|---------| 
| No double dashes | `my-app` | `my--app` |
| No slashes | `api` | `api/v2` |
| No spaces | `my_app` | `my app` |
| No parent references | `myapp` | `..` or `foo..bar` |
| No leading/trailing dashes | `my-app` | `-app` or `app-` |
| ASCII alphanumeric, `_`, `-`, `.` only | `myapp123` | `myäpp` or `app@v1` |

The `--` sequence is reserved for internal use (namespace encoding). See [Namespaces](/concepts/namespaces) for details.

### Namespace Derivation Rules

- Global config files (`/etc/pitchfork/config.toml`, `~/.config/pitchfork/config.toml`) use namespace `global`
- Project config files (`.config/pitchfork.toml`, `.config/pitchfork.local.toml`, `pitchfork.toml`, `pitchfork.local.toml`) use:
  - Top-level `namespace = "..."` if set in any of the four config files in that project directory
  - Otherwise, the parent directory name as namespace
- For `.config/pitchfork.toml` and `.config/pitchfork.local.toml`, the namespace is derived from the project directory (the `.config` directory's parent), not from `.config` itself
- If the derived directory name is invalid (`--`, spaces, non-ASCII, etc.), parsing fails and you should set top-level `namespace`

### Top-level `namespace` (optional)

Overrides the namespace used for all daemons in the four project config files
in that directory.

```toml
namespace = "frontend"

[daemons.api]
run = "npm run dev"
```

Notes:

- One declaration applies to `.config/pitchfork.toml`,
  `.config/pitchfork.local.toml`, `pitchfork.toml`, and `pitchfork.local.toml`
- If multiple files declare `namespace`, every value must match
- Global config files must use `global`

## Shared environment

Top-level `[env]` values supply defaults for all daemons. A daemon's own `env`
overrides matching keys. Values support [templates](/guides/configuration-templates).

```toml
[env]
APP_ENV = "development"
LOG_LEVEL = "info"

[daemons.api]
run = "node server.js"
env = { LOG_LEVEL = "debug" }
```

## Settings

Use `[settings.<group>]` for pitchfork-wide behavior, such as
`[settings.general]` or `[settings.web]`. These are separate from the daemon
fields below. See [settings](/reference/settings) for precedence and commands
to inspect or update them.

## Daemon options

### `run` (required)

The shell command to execute. Keep it in the foreground so pitchfork can supervise it. Avoid shell backgrounding (`&`) or daemonization flags.

```toml
[daemons.api]
run = "npm run server"
```

::: tip
Pitchfork wraps the `run` string in a shell (`sh -c "<run>"` by default), so the tracked PID is the shell process, not the daemon itself. To make the tracked PID match the actual daemon binary, prefix the command with `exec`:

```toml
[daemons.api]
run = "exec node server.js"
```

For compound commands, place `exec` before the final command so the shell replaces itself at the right point:

```toml
[daemons.api]
run = "cd /app && exec node server.js"
```
:::

### `dir`

Working directory for the daemon. Relative paths are resolved from the config's project directory, which is also the default working directory. Configs in `.config/` use its parent as the project directory. A value of `~` or a path beginning with `~/` is resolved from the user's home directory; other shell-style expansions are left unchanged.

```toml
# Relative path (resolved from pitchfork.toml location)
[daemons.frontend]
run = "npm run dev"
dir = "frontend"

# Absolute path
[daemons.api]
run = "npm run server"
dir = "/opt/myapp/api"
```

### `env`

Environment variables to set for the daemon process. Variables are passed as key-value string pairs.

```toml
[daemons.api]
run = "npm run server"
env = { NODE_ENV = "development", PORT = "3000" }

# Multi-line format for many variables
[daemons.worker]
run = "python worker.py"

[daemons.worker.env]
DATABASE_URL = "postgres://localhost/mydb"
REDIS_URL = "redis://localhost:6379"
LOG_LEVEL = "debug"
```

### `user`

Unix user to run the daemon process as. This overrides `[settings.supervisor] user` for this daemon. Values may be usernames or numeric UIDs.

```toml
[settings.supervisor]
user = "app"

[daemons.api]
run = "npm run server"

[daemons.postgres]
run = "postgres -D /var/lib/postgres"
user = "postgres"

[daemons.low-port-web]
run = "python -m http.server 80"
user = "root"

[daemons.worker]
run = "./worker"
user = "501"
```

**Behavior:**
- If `user` is set, the daemon runs as that user.
- Otherwise, if `[settings.supervisor] user` is set, the daemon runs as that user.
- When the supervisor is running as root and `[settings.supervisor] user` is set, the default state directory, logs, and IPC sockets are stored under that user's state directory unless `PITCHFORK_STATE_DIR` overrides it. Pitchfork also chowns those state files to the configured user so non-root clients can read and write them.
- Otherwise, if the supervisor was started as root via `sudo`, daemons run as the sudo-calling user from `SUDO_UID`/`SUDO_GID`.
- If no run user can be derived, the daemon runs as the supervisor's current user.
- Switching to another user requires the supervisor to have root privileges; otherwise startup fails.

### `retry`

Number of retry attempts on failure, or `true` for infinite retries. Default: `0`

- A number (e.g., `3`) means retry that many times
- `true` means retry indefinitely
- `false` or `0` means no retries

```toml
[daemons.api]
run = "npm run server"
retry = 3  # Retry up to 3 times

[daemons.critical]
run = "npm run worker"
retry = true  # Retry forever
```

### `auto`

Auto-start and auto-stop behavior with the [shell hook or project sessions](/guides/shell-hook). Options: `"start"`, `"stop"`. Autostop waits until the last tracked session leaves, plus `general.autostop_delay` (default `1m`).

```toml
[daemons.api]
run = "npm run server"
auto = ["start", "stop"]  # Both auto-start and auto-stop
```

### `ready_delay`

Seconds to wait before considering the daemon ready. When started via `pitchfork start` or `pitchfork run`, defaults to `3` seconds if no other ready check is configured. The default can be changed globally via `[settings.general] ready_delay` (or the `PITCHFORK_READY_DELAY` environment variable); a daemon-level `ready_delay` always takes precedence. The global setting is a duration string and must be a whole number of seconds; subsecond values (e.g. `"500ms"`) are rejected with an error rather than silently truncated.

```toml
[daemons.api]
run = "npm run server"
ready_delay = 5
```

### `ready_output`

Regex pattern to match in output for readiness. Supports
[templates](/guides/configuration-templates).

```toml
[daemons.postgres]
run = "postgres -D /var/lib/pgsql/data"
ready_output = "ready to accept connections"
```

### `ready_http`

HTTP endpoint URL to poll for readiness. By default, any 2xx response is ready.
Use the object form when specific non-2xx statuses also mean the service is up
(for example an authenticated endpoint returning 401). The URL supports
[templates](/guides/configuration-templates).

```toml
[daemons.api]
run = "npm run server"
ready_http = "http://localhost:3000/health"

[daemons.private_api]
run = "npm run server"
ready_http = { url = "http://localhost:3000/health", status = [200, 401] }
```

### `ready_port`

TCP port to check for readiness. Daemon is ready when port is listening.
Accepts a port number or a [template](/guides/configuration-templates) string
that renders to one.

```toml
[daemons.api]
run = "npm run server"
port = 3000
ready_port = 3000

[daemons.worker]
run = "npm run worker"
ready_port = "{{ daemons.api.port }}"
depends = ["api"]
```

### `ready_cmd`

Shell command to poll for readiness. Daemon is ready when command exits with code 0.
Supports [templates](/guides/configuration-templates). It receives the same configured
environment and pitchfork metadata as the daemon, including `$PORT`, `$PORT0`,
`$PORT1`, and so on after port auto-bumping.

```toml
[daemons.postgres]
run = "postgres -D /var/lib/pgsql/data"
ready_cmd = "pg_isready -h localhost"

[daemons.redis]
run = "redis-server"
ready_cmd = "redis-cli ping"

[daemons.api]
run = "./server --port $PORT"
port = { expect = [3000], bump = 10 }
ready_cmd = "curl -f http://localhost:$PORT/health"
```

### Readiness timeouts

`ready_output`, `ready_http`, `ready_port`, and `ready_cmd` also accept object
forms with an overall polling deadline:

```toml
# Alternative checks: choose the one that represents readiness for your service.
ready_output = { pattern = "Server ready", timeout = "30s" }
ready_http = { url = "http://127.0.0.1:3000/health", timeout = "30s" }
ready_port = { port = 3000, timeout = "30s" }
ready_cmd = { run = "./check-ready.sh", timeout = "30s" }
```

With multiple checks, the first success marks the daemon ready. If every check
expires, startup fails with code `124`; any unbounded check keeps startup open.
`ready_delay` is only a fallback when no other check exists.
See [ready checks](/guides/ready-checks).

### `health_cmd`

Shell command to probe periodically. Exit code `0` is healthy.

```toml
[daemons.redis]
run = "redis-server --port $PORT"
port = 6379
health_cmd = { run = "redis-cli -p $PORT ping", interval = "10s", timeout = "5s", retries = 3 }
retry = 3
```

The shorthand is `health_cmd = "redis-cli ping"`. The command receives the
daemon's working directory and environment, including resolved ports.

### `health_http`

HTTP endpoint to probe periodically. Accepts any `2xx` response unless `status`
lists exact accepted codes.

```toml
health_http = { url = "http://127.0.0.1:3000/health", status = [200], interval = "10s", timeout = "5s", retries = 3 }
```

The shorthand is `health_http = "http://127.0.0.1:3000/health"`.

### `health_port`

TCP port to probe on `127.0.0.1`. A successful connection is healthy.

```toml
health_port = { port = 6379, interval = "10s", timeout = "5s", retries = 3 }
```

The shorthand is `health_port = 6379`. Health checks default to a `10s` interval
and three consecutive failures. Default per-probe timeouts are `10s` for
commands and `5s` for HTTP/TCP. Probes start while the daemon is starting.
The health-check `retries` counts failed probes; daemon `retry` determines
whether to restart after termination. See [health checks](/guides/health-checks).

### `depends`

List of daemon IDs that must be started before this daemon. Dependencies can be:

- short IDs in the same namespace (e.g. `postgres`)
- fully qualified cross-namespace IDs (e.g. `global/postgres`)

When you start a daemon, its dependencies are automatically started first in the correct order.

```toml
[daemons.api]
run = "npm run server"
depends = ["postgres", "redis"]
```

**Behavior:**

- **Auto-start**: Running `pitchfork start api` will automatically start `postgres` and `redis` first
- **Transitive dependencies**: If `postgres` depends on `storage`, that will be started too
- **Parallel starting**: Dependencies at the same level start in parallel for faster startup
- **Skip running**: Already-running dependencies are skipped (not restarted)
- **Circular detection**: Circular dependencies are detected and reported as errors
- **Strict validation**: Invalid dependency IDs fail config parsing (they are not skipped)
- **Force flag**: Using `-f` only restarts the explicitly requested daemon, not its dependencies

**Example with chained dependencies:**

```toml
[daemons.database]
run = "postgres -D /var/lib/pgsql/data"
ready_port = 5432

[daemons.cache]
run = "redis-server"
ready_port = 6379

[daemons.api]
run = "npm run server"
depends = ["database", "cache"]

[daemons.worker]
run = "npm run worker"
depends = ["database"]
```

Running `pitchfork start api worker` starts daemons in this order:
1. `database` and `cache` (in parallel, no dependencies)
2. `api` and `worker` (in parallel, after their dependencies are ready)

### `watch`

Glob patterns for files to watch. When a matched file changes, the daemon is automatically restarted.

```toml
[daemons.api]
run = "npm run dev"
watch = ["src/**/*.ts", "package.json"]
```

**Pattern syntax:**
- `*.js` - All `.js` files in the daemon's directory
- `src/**/*.ts` - All `.ts` files in `src/` and subdirectories
- `package.json` - Specific file

**Behavior:**
- Patterns are resolved relative to the config's project directory, independently of `dir`
- Only running daemons are restarted (stopped daemons ignore changes)
- Changes are debounced for 1 second by default (`supervisor.file_watch_debounce`)

See [File Watching guide](/guides/file-watching) for more details.

### `watch_mode`

Select which file watcher backend to use for this daemon. Default: `"native"`

```toml
[daemons.api]
run = "npm run dev"
watch = ["src/**/*.ts", "package.json"]
watch_mode = "auto"
```

**Allowed values:**
- `"native"` - OS-native filesystem notifications (default)
- `"poll"` - Polling-based watcher (better compatibility on some NFS/remote mounts)
- `"auto"` - Prefer native, automatically fall back to polling if native watcher setup fails

**Related settings:**
- `settings.supervisor.watch_poll_interval` controls polling scan cadence
- `settings.supervisor.watch_interval` controls how often supervisor refreshes watch config state

### `port`

Port configuration for the daemon. Accepts three forms:

```toml
# Single port (shorthand)
[daemons.api]
run = "node server.js"
port = 3000

# Multiple ports (array)
[daemons.multi]
run = "./start.sh"
port = [8080, 8443]

```

```toml
# Full form with auto-bump
[daemons.api]
run = "node server.js"
port = { expect = [3000], bump = 10 }
```

**Fields (object form):**
- `expect` - List of TCP ports the daemon is expected to bind to
- `bump` - Auto port-bump configuration: `true` = unlimited attempts, a number = max attempts, `false`/`0` = disabled (default)

**Behavior:**
- Pitchfork checks if the port is available before starting
- The first resolved port is injected as `$PORT` and `$PORT0`; additional ports use `$PORT1`, `$PORT2`, and so on. Your command must use these values, directly or through arguments
- When `bump` is enabled and the port is occupied, all ports are incremented by the same offset to maintain relative spacing
- Resolved ports are available via `pitchfork status` and in the start output

### `expected_port` (deprecated)

Use `port` instead. TCP ports the daemon is expected to bind to.

```toml
[daemons.api]
run = "node server.js"
expected_port = [3000]  # deprecated: use port = 3000
```

### `auto_bump_port` (deprecated)

Use `port.bump` instead. When `true`, pitchfork automatically finds an available port if the expected port is already in use.

```toml
[daemons.api]
run = "node server.js"
expected_port = [3000]   # deprecated
auto_bump_port = true    # deprecated: use port = { expect = [3000], bump = true }
```

### `port_bump_attempts` (deprecated)

Use `port.bump` instead. Maximum number of port increment attempts when `auto_bump_port` is enabled. Default: `10`

```toml
[daemons.api]
run = "node server.js"
expected_port = [3000]     # deprecated
auto_bump_port = true      # deprecated
port_bump_attempts = 20    # deprecated: use port = { expect = [3000], bump = 20 }
```

### `boot_start`

Start this daemon when the supervisor launches in boot mode (`supervisor run --boot`). Default: `false`. Register the supervisor with [`pitchfork boot enable`](/guides/boot-start) for login or system startup.

```toml
[daemons.postgres]
run = "postgres -D /var/lib/pgsql/data"
boot_start = true
```

### `hooks`

Lifecycle hooks that run shell commands in response to daemon events. Hooks are fire-and-forget — they run in the background and never block the daemon.

```toml
[daemons.api]
run = "npm run server"
retry = 3

[daemons.api.hooks]
on_ready = "curl -X POST https://alerts.example.com/ready"
on_fail = "./scripts/cleanup.sh"
on_retry = "echo 'retrying...'"
```

**Fields:**
- `on_ready` - Runs when the daemon becomes ready (passes readiness check)
- `on_fail` - Runs when the daemon fails and all retries are exhausted
- `on_retry` - Runs before each retry attempt
- `on_stop` - Runs when the daemon is explicitly stopped by pitchfork
- `on_exit` - Runs on terminal exit (stop, clean exit, or exhausted failure); also fires during supervisor shutdown. With retries, it waits until attempts are exhausted
- `on_output` - Fires when the daemon produces matching output. Accepts a command string (shorthand) or an inline table `{ run, filter?, regex?, debounce? }`

Hook commands receive environment variables: `PITCHFORK_DAEMON_ID` (fully-qualified `namespace/name`), `PITCHFORK_DAEMON_NAMESPACE`, `PITCHFORK_RETRY_COUNT`, `PITCHFORK_EXIT_CODE`, and (for `on_stop`/`on_exit`) `PITCHFORK_EXIT_REASON` (`"stop"`, `"exit"`, or `"fail"`). See [Lifecycle Hooks guide](/guides/lifecycle-hooks) for details.

### `cron`

Cron scheduling configuration. Accepts a cron expression string (shorthand) or an inline table (full form).

```toml
# Shorthand (retrigger defaults to "finish")
[daemons.backup]
run = "./backup.sh"
cron = "0 0 2 * * *"

```

```toml
# Full form
[daemons.backup]
run = "./backup.sh"
cron = { schedule = "0 0 2 * * *", retrigger = "always" }
```

**Fields:**
- `schedule` - Cron expression (second, minute, hour, day, month, weekday; optional year). Evaluated in local time. Use weekday names such as `MON-FRI`
- `retrigger` - Behavior when schedule fires: `"finish"` (default), `"always"`, `"success"`, `"fail"`
- `immediate` - Also fire if a scheduled time occurred within the 10 seconds before the daemon started. Default: `false`

### `mise`

Enable [mise](https://mise.jdx.dev) integration for this daemon. When `true`, the daemon's command is wrapped with `mise x --` to activate mise-managed tools and environment variables.

```toml
[daemons.api]
run = "node server.js"
mise = true
```

This is especially useful for daemons running via `pitchfork boot` (login daemon mode) where interactive shell hooks haven't set up tool paths. When not set, falls back to the global `general.mise` setting. See [mise Integration guide](/guides/mise-integration) for details.

### `memory_limit`

Maximum physical memory (RSS) for the daemon process. Accepts human-readable byte sizes. The supervisor periodically monitors the daemon's RSS and kills it if it exceeds the limit.

```toml
[daemons.worker]
run = "python worker.py"
memory_limit = "512MB"

[daemons.api]
run = "node server.js"
memory_limit = "2GiB"
```

**Supported formats:** `"50MB"`, `"512MB"`, `"1GiB"`, `"256KiB"`, etc. Both SI (MB, GB) and binary (MiB, GiB) units are accepted.

**Behavior:**
- The supervisor checks RSS at each interval tick (configured by `general.interval`, default `10s`)
- When a daemon's RSS exceeds the limit, the process group is killed via `SIGTERM` (then `SIGKILL` if unresponsive)
- The daemon is marked as `Errored`, so if `retry` is configured, it will be restarted (consuming a retry attempt)
- Works reliably with all runtimes (JVM, Node.js, Go, Python, etc.) since it measures actual physical memory, not virtual address space
- For multi-process daemons (e.g. gunicorn workers, nginx workers), RSS is aggregated across the root process and all its descendants, consistent with the process-group kill used for enforcement
- Only affects the daemon's process group, not the pitchfork supervisor itself
- Default: no limit

### `cpu_limit`

Maximum CPU usage as a percentage for the daemon process. The supervisor periodically monitors the daemon's CPU usage and kills it if it exceeds the limit.

```toml
[daemons.worker]
run = "python compute.py"
cpu_limit = 80     # 80% of one CPU core

[daemons.batch]
run = "./run-batch.sh"
cpu_limit = 200    # Up to 2 CPU cores
```

**Supported values:** Any positive number. `100` = 100% of one CPU core. Values above 100 are valid on multi-core systems (e.g. `200` allows up to 2 full cores).

**Behavior:**
- The supervisor checks CPU usage at each interval tick (configured by `general.interval`, default `10s`)
- To avoid killing daemons during transient spikes (e.g. JIT warm-up, burst responses), the process is only killed after **3 consecutive** samples exceed the limit. A single sample below the limit resets the counter. This threshold is configurable via `settings.supervisor.cpu_violation_threshold` (default: `3`).
- When the consecutive threshold is reached, the process group is killed via `SIGTERM` (then `SIGKILL` if unresponsive)
- The daemon is marked as `Errored`, so if `retry` is configured, it will be restarted (consuming a retry attempt)
- CPU usage is measured as a percentage of one core (not system-wide)
- For multi-process daemons (e.g. gunicorn workers, nginx workers), CPU usage is aggregated across the root process and all its descendants, consistent with the process-group kill used for enforcement
- Only affects the daemon's process group, not the pitchfork supervisor itself
- Default: no limit

### `stop_signal`

Unix signal to send for graceful shutdown. Accepts a signal name string or a `{ signal, timeout }` object. Default: `SIGTERM`

```toml
# Signal name only (shorthand)
[daemons.api]
run = "node server.js"
stop_signal = "SIGINT"

# Signal with custom timeout
[daemons.postgres]
run = "postgres -D /var/lib/postgres"
stop_signal = { signal = "SIGINT", timeout = "5s" }
```

**Allowed signals:** `SIGTERM`, `SIGINT`, `SIGQUIT`, `SIGHUP`, `SIGUSR1`, `SIGUSR2`

**Fields (object form):**
- `signal` - Signal name to send (with or without `SIG` prefix)
- `timeout` - Maximum time to wait for the process to exit before sending `SIGKILL` (humantime format, e.g. `"500ms"`, `"3s"`). Overrides the global `settings.supervisor.stop_timeout` for this daemon.

**Behavior:**
- When stopping a daemon, pitchfork sends the configured signal to the entire process group
- If the process does not exit within the timeout, `SIGKILL` is sent as a last resort
- Useful for daemons that handle `SIGINT` (Ctrl+C) for graceful termination but ignore `SIGTERM`

### `pty`

Allocate a pseudo-terminal on Unix. Default: `false`.

```toml
[daemons.worker]
run = "./worker"
pty = true
```

Use this for commands that change buffering or color output when attached to a
terminal. It does not turn the daemon into an interactive terminal session.

### `logs`

Configure structured parsing and retention for one daemon:

```toml
[daemons.api]
run = "node server.js"

[daemons.api.logs]
log_format = "json"
time_retention = "7d"
line_retention = 10000
archive_hook = "gzip -c >> /path/to/api-archive.jsonl.gz"
```

| Field | Meaning |
| --- | --- |
| `log_format` | `text` (default), `json`, or `logfmt` |
| `time_retention` | Maximum age, such as `7d`; no age pruning by default |
| `line_retention` | Maximum entries per daemon; `0` disables count pruning |
| `archive_hook` | Command receiving JSONL on stdin before pruning; a failure preserves the batch |

These fields override `[settings.logs]` defaults. The daemon-level fields
`time_retention`, `line_retention`, and `archive_hook` are also accepted for
compatibility; values in `[daemons.<name>.logs]` take precedence over them.
See [logs](/guides/logs) for filtering and archive behavior.

## Daemon Groups

Named groups of daemons for batch operations. Use the `--group` flag with `start`, `stop`, or `restart`.

```toml
[groups.backend]
daemons = ["api", "worker"]

[groups.all]
daemons = ["postgres", "redis", "api", "worker"]
```

- `daemons` is a list of short names or fully qualified IDs (`"global/postgres"`)
- Malformed daemon name strings (e.g. an unparseable qualified ID) fail config parsing; references to non-existent daemons are reported when the group is used (e.g. `pitchfork start --group backend`)
- Groups merge like other config values: later definitions override earlier ones
- `pitchfork start --group backend` resolves dependencies and starts daemons in parallel as usual

## Complete example

Adapt commands, initialized database paths, and application endpoints to your project. The API in this example must read `PORT` from its environment.

```toml
# Database - starts on boot, no auto-stop
[daemons.postgres]
run = "postgres -D /var/lib/pgsql/data"
ready_output = "ready to accept connections"
boot_start = true
retry = 3

# Cache - starts with API
[daemons.redis]
run = "redis-server"
ready_output = "Ready to accept connections"

# API server - depends on database and cache, hot reloads on changes
[daemons.api]
run = "npm run server"
dir = "api"
depends = ["postgres", "redis"]
watch = ["api/src/**/*.ts", "api/package.json"]
ready_cmd = "curl -fsS http://127.0.0.1:$PORT/health"
auto = ["start", "stop"]
retry = 5
port = { expect = [3000], bump = true }
env = { NODE_ENV = "development" }
memory_limit = "2GiB"
cpu_limit = 200

[daemons.api.hooks]
on_ready = "curl -X POST https://alerts.example.com/ready"
on_fail = "./scripts/alert-failure.sh"

# Frontend dev server in a subdirectory
[daemons.frontend]
run = "npm run dev"
dir = "frontend"
env = { PORT = "5173" }

# Scheduled backup
[daemons.backup]
run = "./scripts/backup.sh"
cron = { schedule = "0 0 2 * * *", retrigger = "finish" }
```

## Global Config: Slug Registry

Slugs for the reverse proxy are defined **only** in the global config (`~/.config/pitchfork/config.toml`), not in per-project `pitchfork.toml` files. The global config is the single source of truth for slug→project mappings.

```toml
# ~/.config/pitchfork/config.toml

[slugs]
api = { dir = "/home/user/my-api", daemon = "server" }
frontend = { dir = "/home/user/my-app", daemon = "dev" }
# If daemon name matches slug, it can be omitted:
docs = { dir = "/home/user/docs-site" }  # defaults daemon = "docs"
```

Each slug entry maps to:
- `dir` — the project directory containing the `pitchfork.toml`
- `daemon` (optional) — the daemon name within that project. Defaults to the slug name if omitted.

Slug and namespace `dir` values use the same `~` and `~/...` expansion.

Use `pitchfork proxy add` to manage slugs:

```bash
pitchfork proxy add api                                    # current dir, daemon = "api"
pitchfork proxy add api --daemon server                    # current dir, daemon = "server"
pitchfork proxy add api --dir /home/user/api --daemon srv  # explicit dir and daemon
pitchfork proxy remove api                                 # remove a slug
pitchfork proxy status                                     # show all slugs and their state
```

## Namespace registry

The user config can map a namespace to a project directory:

```toml
# ~/.config/pitchfork/config.toml
[namespaces.my-app]
dir = "~/projects/my-app"

[slugs]
api = { namespace = "my-app", daemon = "server" }
```

A slug can reference a registered namespace instead of repeating `dir`.
`pitchfork proxy add` manages slug registrations. See
[namespaces](/concepts/namespaces) for daemon ID resolution and worktree isolation.
