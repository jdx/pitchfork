# Lifecycle Hooks

Lifecycle hooks are shell commands that run in response to daemon lifecycle events. Each hook can be **fire-and-forget** (default) or **blocking** — blocking hooks pause the lifecycle until the command succeeds.

## Configuration

```toml
[daemons.api]
run = "npm run server"
retry = 3
ready_http = "http://localhost:3000/health"

[daemons.api.hooks]
on_ready = "curl -X POST https://alerts.example.com/ready"
on_crash = "./scripts/cleanup.sh"
on_output = { filter = "Server started", run = "./scripts/notify-ready.sh" }
```

## Hook Types

| Hook | When it runs | Default behavior |
|------|-------------|-----------------|
| `pre_start` | Before the daemon process is spawned | Fire-and-forget |
| `on_ready` | Daemon passes its readiness check | Fire-and-forget |
| `pre_stop` | Before the daemon is stopped | Fire-and-forget |
| `on_stop` | Daemon is explicitly stopped by pitchfork | Fire-and-forget |
| `on_exit` | Any daemon termination (stop, clean exit, or crash) | Fire-and-forget |
| `on_retry` | Before each startup retry attempt | Fire-and-forget |
| `on_fail` | Startup fails and all retries are exhausted | Fire-and-forget |
| `on_recover` | Before each runtime recovery attempt | Fire-and-forget |
| `on_crash` | Runtime crash and all retries exhausted | Fire-and-forget |
| `on_output` | Daemon writes a line matching an optional pattern | Fire-and-forget |

### `pre_start`

Runs before the daemon process is spawned. Use `block = true` to ensure dependencies or preconditions are met before starting.

```toml
[daemons.api.hooks]
pre_start = "curl -sf http://deps:8080/health"
```

### `on_ready`

Runs when the daemon passes its readiness check (delay, output match, HTTP, port, or command).

```toml
[daemons.api.hooks]
on_ready = "curl -s -X POST https://slack.example.com/webhook -d '{\"text\": \"API is up\"}'"
```

### `pre_stop`

Runs before the daemon is stopped. Use `block = true` to drain connections or perform graceful shutdown steps.

```toml
[daemons.api.hooks]
pre_stop = "./scripts/drain-connections.sh"
```

### `on_stop`

Runs when the daemon is explicitly stopped by pitchfork (via `pitchfork stop`, `auto = ["stop"]` directory exit, or supervisor shutdown).

```toml
[daemons.api.hooks]
on_stop = "./scripts/notify-stopped.sh"
```

### `on_exit`

Runs on **any** daemon termination — intentional stop, clean exit, or crash. Also fires during supervisor shutdown. Use this for cleanup that should always run regardless of why the daemon stopped.

> **Note:** `on_exit` does not support `block = true` — it is always fire-and-forget. If you need blocking cleanup, use `on_stop` with `block = true` instead.

> **Note:** For daemons with `retry > 0`, `on_exit` fires **only after all retries are exhausted**, not on each individual crash attempt. Use `on_recover` if you need to react to every runtime failure.

```toml
[daemons.infra.hooks]
on_exit = "docker compose down --volumes"
```

### `on_retry`

Fires before each **startup** retry attempt. This hook runs during the initial startup retry loop when the daemon fails to start.

```toml
[daemons.api.hooks]
on_retry = "echo 'Retrying api (attempt $PITCHFORK_RETRY_COUNT)...'"
```

### `on_fail`

Fires when the daemon fails to start and all **startup** retries are exhausted. If `retry = 0`, fires immediately on startup failure.

```toml
[daemons.api.hooks]
on_fail = "./scripts/alert-team.sh"
```

### `on_recover`

Fires before each **runtime** recovery attempt. This hook runs when a previously-running daemon crashes and pitchfork is about to restart it. The `PITCHFORK_RECOVERY_COUNT` environment variable tracks how many times the daemon has been recovered.

```toml
[daemons.api.hooks]
on_recover = "echo 'Recovering api (attempt $PITCHFORK_RECOVERY_COUNT)...'"
```

### `on_crash`

Fires when a previously-running daemon crashes and all **runtime** retries are exhausted. Also fires `on_exit`. This is the runtime equivalent of `on_fail`.

```toml
[daemons.api.hooks]
on_crash = "./scripts/alert-team.sh"
```

### `on_output`

Fires when the daemon writes a line to stdout or stderr that matches an optional pattern. Accepts a command string (shorthand) or an inline table (full form):

```toml
# Shorthand (fires on every line)
on_output = "./scripts/log-activity.sh"

# Full form with filter/regex/debounce
on_output = { filter = "Server started", run = "curl https://monitor.example.com/up" }
```

| Field | Required | Description |
|-------|----------|-------------|
| `run` | Yes | Shell command to execute |
| `filter` | No | Fire only when the line **contains** this substring |
| `regex` | No | Fire only when the line **matches** this regular expression |
| `debounce` | No | Minimum time between firings (humantime, e.g. `"500ms"`, `"2s"`). Defaults to `"1000ms"` |

`filter` and `regex` are mutually exclusive. When neither is specified the hook fires on every line of output, subject to debouncing.

The matched line is available as `$PITCHFORK_MATCHED_LINE`.

## Startup vs Runtime Hooks

Hooks are split into two phases based on when they fire:

### Startup phase

These hooks fire during the initial `pitchfork start` or `pitchfork run` command:

- `on_retry` — before each startup retry attempt
- `on_fail` — when all startup retries are exhausted

Startup hooks support `block = true` because they run in the caller's context.

### Runtime phase

These hooks fire in the background (monitor task or interval watcher) after the daemon has been running:

- `on_recover` — before each runtime recovery attempt
- `on_crash` — when all runtime retries are exhausted

Runtime hooks are always fire-and-forget because blocking would stall the monitor task or interval watcher.

## Shorthand vs Full Form

Each hook (except `on_output`) accepts a command string (shorthand) or an inline table (full form):

```toml
# Shorthand (command only, fire-and-forget)
on_ready = "curl -sf http://localhost:3000/health"

# Full form with block and timeout
on_ready = { run = "curl -sf http://localhost:3000/health", block = true, timeout = "30s" }
```

| Field | Required | Description |
|-------|----------|-------------|
| `run` | Yes | Shell command to execute |
| `block` | No | Whether to block the lifecycle until the command exits with code 0. Default: `false` |
| `timeout` | No | Maximum time to wait when `block = true` (humantime, e.g. `"30s"`, `"5m"`). Defaults to `settings.supervisor.hook_block_timeout` if not set. Ignored when `block = false`. |

## Blocking vs Fire-and-Forget

By default, hooks are **fire-and-forget** — they run in the background and never block the daemon. Set `block = true` to make a hook **blocking**:

- The lifecycle pauses until the hook command exits with code 0
- If the command exits non-zero, a warning is logged but the daemon state is not changed
- If the command times out, it is killed and a warning is logged

### Block support by hook

Not all hooks support `block = true`. Hooks that run in the monitor task or interval watcher cannot block because doing so would stall those background loops.

| Hook | `block = true` | Notes |
|------|----------------|-------|
| `pre_start` | Yes | Blocks `run()`, failure returns error |
| `on_ready` | Partial | Blocks during `wait_ready` start; fire-and-forget in monitor task and non-waiting start |
| `pre_stop` | Yes | Blocks `stop()`, failure returns error |
| `on_stop` | Partial | Blocks during explicit `stop()`; fire-and-forget in monitor task |
| `on_exit` | No | Always fire-and-forget |
| `on_retry` | Partial | Blocks during startup retry loop; fire-and-forget in interval watcher |
| `on_fail` | No | Always fire-and-forget |
| `on_recover` | No | Always fire-and-forget (runtime only) |
| `on_crash` | No | Always fire-and-forget (runtime only) |
| `on_output` | No | Has its own config format, always fire-and-forget |

### When to use `block = true`

| Hook | Use case |
|------|----------|
| `pre_start` | Wait for a dependency to be available before starting |
| `on_ready` | Validate the daemon is healthy after startup |
| `pre_stop` | Drain connections or perform graceful shutdown steps before stopping |
| `on_stop` | Ensure cleanup completes before proceeding |
| `on_retry` | Block until pre-retry setup completes |

### Known Limitations

- **`on_exit`, `on_fail`, `on_recover`, and `on_crash` do not support `block = true`**. They are always fire-and-forget.
- **`on_ready` supports `block = true` only during startup with `wait_ready = true`**. When the daemon starts without `wait_ready`, `on_ready` fires from the monitor task as fire-and-forget (even if `block = true` is set).
- **`on_stop` only supports `block = true` during explicit stop**. When it fires from the monitor task, `block = true` is silently ignored and the hook runs as fire-and-forget.
- **`on_retry` only supports `block = true` during the startup retry loop**. When it fires from the interval watcher, `block = true` is silently ignored.

### Default Block Timeout

If no per-hook `timeout` is specified with `block = true`, the global default from `settings.supervisor.hook_block_timeout` is used (default: `"30s"`). This prevents blocking hooks from running indefinitely.

```toml
[settings.supervisor]
hook_block_timeout = "60s"
```

## Hook Behavior

- Hook commands run in the daemon's working directory
- Errors in fire-and-forget hooks are logged but do not affect the daemon
- Blocking hook failures log a warning but do not change daemon state
- Hooks read fresh configuration from `pitchfork.toml` each time they fire

## Environment Variables

All hooks receive these environment variables:

| Variable | All hooks | `on_fail`, `on_crash`, `on_exit` | `on_stop` | `on_output` |
|----------|-----------|----------------------------------|-----------|-------------|
| `PITCHFORK_DAEMON_ID` | Yes | Yes | Yes | Yes |
| `PITCHFORK_DAEMON_NAMESPACE` | Yes | Yes | Yes | Yes |
| `PITCHFORK_RETRY_COUNT` | Yes | Yes | Yes | Yes |
| `PITCHFORK_RECOVERY_COUNT` | Yes | Yes | Yes | Yes |
| `PITCHFORK_EXIT_CODE` | — | Yes | Yes | — |
| `PITCHFORK_EXIT_REASON` | — | — | Yes | Yes (`on_exit` too) |
| `PITCHFORK_MATCHED_LINE` | — | — | — | Yes |

- `PITCHFORK_RETRY_COUNT`: Number of startup retry attempts (0 on first start)
- `PITCHFORK_RECOVERY_COUNT`: Number of runtime recovery attempts (0 on first start)
- `PITCHFORK_EXIT_CODE`: Exit code of the process. On Unix, processes terminated by a signal (e.g. SIGTERM) have no POSIX exit code; set to `-1`.
- `PITCHFORK_EXIT_REASON`: Why the daemon stopped: `"stop"` (intentional), `"fail"` (non-zero exit), or `"exit"` (clean exit)

Any custom `env` variables from the daemon config are also passed to hooks.

## Stop Signal

By default, pitchfork sends `SIGTERM` to gracefully stop daemons. Some daemons (e.g. Node.js, Docker-based services) may handle `SIGINT` (Ctrl+C) for graceful shutdown instead. Use `stop_signal` to configure this:

```toml
# Signal name only (shorthand)
[daemons.api]
run = "node server.js"
stop_signal = "SIGINT"

# Signal with custom timeout
[daemons.api]
run = "node server.js"
stop_signal = { signal = "SIGINT", timeout = "5s" }
```

**Allowed signals:** `SIGTERM`, `SIGINT`, `SIGQUIT`, `SIGHUP`, `SIGUSR1`, `SIGUSR2`

**Fields (object form):**
- `signal` - Signal name to send (with or without `SIG` prefix)
- `timeout` - Maximum time to wait for the process to exit before sending `SIGKILL` (overrides the global `settings.supervisor.stop_timeout`)

**Behavior:**
- Pitchfork sends the configured signal to the entire process group
- If the process does not exit within the timeout, `SIGKILL` is sent as a last resort
- The default signal is `SIGTERM`, and the default timeout comes from `settings.supervisor.stop_timeout`

## Examples

### Fire-and-forget notifications

**Send a Slack notification on crash:**

```toml
[daemons.api]
run = "npm run server"
retry = 3

[daemons.api.hooks]
on_crash = "curl -s -X POST $SLACK_WEBHOOK -d '{\"text\": \"API crashed (exit $PITCHFORK_EXIT_CODE)\"}'"
```

**Log startup retry attempts to a file:**

```toml
[daemons.worker]
run = "python worker.py"
retry = 5

[daemons.worker.hooks]
on_retry = "sh -c 'echo \"$(date): startup retry $PITCHFORK_RETRY_COUNT\" >> /var/log/worker-retries.log'"
```

**Log runtime recovery attempts:**

```toml
[daemons.worker]
run = "python worker.py"
retry = 5

[daemons.worker.hooks]
on_recover = "sh -c 'echo \"$(date): recovery $PITCHFORK_RECOVERY_COUNT\" >> /var/log/worker-recoveries.log'"
```

**Run cleanup on startup failure:**

```toml
[daemons.processor]
run = "./process-queue.sh"
retry = 2

[daemons.processor.hooks]
on_fail = "./scripts/release-locks.sh"
on_ready = "./scripts/acquire-locks.sh"
```

**Tear down infrastructure on any exit:**

```toml
[daemons.infra]
run = "docker compose up"

[daemons.infra.hooks]
on_exit = "docker compose down --volumes --remove-orphans"
```

**Distinguish stop reason in a shared cleanup script:**

```toml
[daemons.api]
run = "npm run server"

[daemons.api.hooks]
on_exit = "sh -c 'echo \"Daemon exited: reason=$PITCHFORK_EXIT_REASON code=$PITCHFORK_EXIT_CODE\" >> /var/log/api-exits.log'"
```

### Blocking hooks

**Wait for a dependency before starting:**

```toml
[daemons.api]
run = "npm run server"

[daemons.api.hooks]
pre_start = { run = "curl -sf http://localhost:5432/health", block = true }
```

**Validate the daemon is healthy after startup:**

```toml
[daemons.api]
run = "npm run server"
ready_http = "http://localhost:3000/health"

[daemons.api.hooks]
on_ready = { run = "curl -sf http://localhost:3000/api/version", block = true, timeout = "10s" }
```

**Drain connections before stopping:**

```toml
[daemons.api]
run = "npm run server"

[daemons.api.hooks]
pre_stop = { run = "./scripts/drain-connections.sh", block = true }
```

**Clean up after a daemon stops:**

```toml
[daemons.api]
run = "npm run server"

[daemons.api.hooks]
on_stop = { run = "./scripts/cleanup-sockets.sh", block = true }
```

### Output hooks

**React to a specific log message:**

```toml
[daemons.api]
run = "npm run server"

[daemons.api.hooks]
on_output = { filter = "Database connected", run = "curl https://monitor.example.com/db-ready" }
```

**Parse a port from startup output and register it:**

```toml
[daemons.api]
run = "node server.js"

[daemons.api.hooks]
on_output = { regex = "listening on port [0-9]+", run = "sh -c 'echo \"$PITCHFORK_MATCHED_LINE\" | grep -o \"[0-9]*$\" | xargs register-port'" }
```

**Rate-limited activity logging:**

```toml
[daemons.worker]
run = "python worker.py"

[daemons.worker.hooks]
on_output = { run = "sh -c 'echo \"$(date): active\" >> /var/log/worker-activity.log'", debounce = "10s" }
```
