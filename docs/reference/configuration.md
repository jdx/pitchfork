# Configuration Reference

Complete reference for `pitchfork.toml` configuration files.

## Configuration Hierarchy

Pitchfork loads configuration files in order, with later files overriding earlier ones:

1. **System-level:** `/etc/pitchfork/config.toml` (namespace: `global`)
2. **User-level:** `~/.config/pitchfork/config.toml` (namespace: `global`)
3. **Project-level:** `.config/pitchfork.toml`, `.config/pitchfork.local.toml`, `pitchfork.toml`, `pitchfork.local.toml` from filesystem root to current directory

Within each directory, files are processed in this order:
- `.config/pitchfork.toml` (lowest precedence in directory)
- `.config/pitchfork.local.toml` (overrides `.config/pitchfork.toml`)
- `pitchfork.toml` (overrides everything in `.config/`)
- `pitchfork.local.toml` (highest precedence in directory, not committed to version control)

This mirrors [mise](https://mise.jdx.dev/configuration.html) behavior, allowing you to store project config in a centralized `.config/` directory if preferred.

## JSON Schema

A JSON Schema is available for editor autocompletion and validation:

**URL:** [`https://pitchfork.en.dev/schema.json`](/schema.json)

### Editor Setup

**VS Code** with [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml):

```toml
#:schema https://pitchfork.en.dev/schema.json

[daemons.api]
run = "npm run server"
```

**JetBrains IDEs**: Add the schema URL in Settings → Languages & Frameworks → Schemas and DTDs → JSON Schema Mappings.

## File Format

All configuration uses TOML format:

```toml
namespace = "my-project" # optional, per-file namespace override

[daemons.<daemon-name>]
run = "command to execute"
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
  - Top-level `namespace = "..."` if set in the config file
  - Otherwise, the parent directory name as namespace
- For `.config/pitchfork.toml` and `.config/pitchfork.local.toml`, the namespace is derived from the project directory (the `.config` directory's parent), not from `.config` itself
- If the derived directory name is invalid (`--`, spaces, non-ASCII, etc.), parsing fails and you should set top-level `namespace`

### Top-level `namespace` (optional)

Overrides the namespace used for all daemons in that specific config file.

```toml
namespace = "frontend"

[daemons.api]
run = "npm run dev"
```

Notes:

- `pitchfork.local.toml` shares namespace with sibling `pitchfork.toml`
- If both declare `namespace`, the values must match
- Global config files must use `global`

## Daemon Options

### `run` (required)

The command to execute.

```toml
[daemons.api]
run = "npm run server"
```

### `dir`

Working directory for the daemon. Relative paths are resolved from the `pitchfork.toml` file location. If not set, defaults to the directory containing the `pitchfork.toml` file.

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

Auto-start and auto-stop behavior with shell hook. Options: `"start"`, `"stop"`

```toml
[daemons.api]
run = "npm run server"
auto = ["start", "stop"]  # Both auto-start and auto-stop
```

### `ready_delay`

Seconds to wait before considering the daemon ready. When started via `pitchfork start` or `pitchfork run`, defaults to `3` seconds if no other ready check is configured.

```toml
[daemons.api]
run = "npm run server"
ready_delay = 5
```

### `ready_output`

Regex pattern to match in output for readiness.

```toml
[daemons.postgres]
run = "postgres -D /var/lib/pgsql/data"
ready_output = "ready to accept connections"
```

### `ready_http`

HTTP endpoint URL to poll for readiness (2xx = ready).

```toml
[daemons.api]
run = "npm run server"
ready_http = "http://localhost:3000/health"
```

### `ready_port`

TCP port to check for readiness. Daemon is ready when port is listening.

```toml
[daemons.api]
run = "npm run server"
ready_port = 3000
```

### `ready_cmd`

Shell command to poll for readiness. Daemon is ready when command exits with code 0.

```toml
[daemons.postgres]
run = "postgres -D /var/lib/pgsql/data"
ready_cmd = "pg_isready -h localhost"

[daemons.redis]
run = "redis-server"
ready_cmd = "redis-cli ping"
```

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
- Patterns are resolved relative to the `pitchfork.toml` file
- Only running daemons are restarted (stopped daemons ignore changes)
- Changes are debounced for 1 second to avoid rapid restarts

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

### `expected_port`

TCP ports the daemon is expected to bind to. Used for port conflict detection before starting a daemon.

```toml
[daemons.api]
run = "node server.js"
expected_port = [3000]

[daemons.multi]
run = "./start.sh"
expected_port = [8080, 8443]
```

### `auto_bump_port`

When `true`, pitchfork automatically finds an available port if the expected port is already in use. All ports in `expected_port` are bumped by the same offset to maintain relative spacing. Default: `false`

```toml
[daemons.api]
run = "node server.js"
expected_port = [3000]
auto_bump_port = true
```

**Behavior:**
- Tries incrementing all ports by the same offset (1, 2, 3, ...) up to `port_bump_attempts` (default: 10)
- Resolved ports are available via `pitchfork status` and in the start output
- Useful for running multiple instances of the same service

### `port_bump_attempts`

Maximum number of port increment attempts when `auto_bump_port` is enabled. Default: `10`

```toml
[daemons.api]
run = "node server.js"
expected_port = [3000]
auto_bump_port = true
port_bump_attempts = 20
```

### `boot_start`

Start this daemon automatically on system boot. Default: `false`

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
- `on_exit` - Runs on any daemon termination (stop, clean exit, or crash); also fires during supervisor shutdown

Hook commands receive environment variables: `PITCHFORK_DAEMON_ID` (fully-qualified `namespace/name`), `PITCHFORK_DAEMON_NAMESPACE`, `PITCHFORK_RETRY_COUNT`, `PITCHFORK_EXIT_CODE`, and (for `on_stop`/`on_exit`) `PITCHFORK_EXIT_REASON` (`"stop"`, `"exit"`, or `"fail"`). See [Lifecycle Hooks guide](/guides/lifecycle-hooks) for details.

### `cron`

Cron scheduling configuration.

```toml
[daemons.backup]
run = "./backup.sh"
cron = { schedule = "0 0 2 * * *", retrigger = "finish" }
```

**Fields:**
- `schedule` - Cron expression (6 fields: second, minute, hour, day, month, weekday)
- `retrigger` - Behavior when schedule fires: `"finish"` (default), `"always"`, `"success"`, `"fail"`

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

## Complete Example

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
watch = ["src/**/*.ts", "package.json"]
ready_http = "http://localhost:3000/health"
auto = ["start", "stop"]
retry = 5
env = { NODE_ENV = "development", PORT = "3000" }
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

Use `pitchfork proxy add` to manage slugs:

```bash
pitchfork proxy add api                                    # current dir, daemon = "api"
pitchfork proxy add api --daemon server                    # current dir, daemon = "server"
pitchfork proxy add api --dir /home/user/api --daemon srv  # explicit dir and daemon
pitchfork proxy remove api                                 # remove a slug
pitchfork proxy status                                     # show all slugs and their state
```
