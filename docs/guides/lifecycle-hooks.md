# Lifecycle Hooks

Run custom shell commands when daemons become ready, fail, are retried, stop, or produce specific output.

## Configuration

Add a `[daemons.<name>.hooks]` section to your `pitchfork.toml`:

```toml
[daemons.api]
run = "npm run server"
retry = 3
ready_http = "http://localhost:3000/health"

[daemons.api.hooks]
on_ready = "curl -X POST https://alerts.example.com/ready"
on_fail = "./scripts/cleanup.sh"
on_retry = "echo 'retrying api server...'"
on_stop = "./scripts/notify-stopped.sh"
on_exit = "./scripts/cleanup.sh"
on_output = { filter = "Server started", run = "./scripts/notify-ready.sh" }
```

## Hook Types

### `on_ready`

Fires when the daemon passes its readiness check (delay, output match, HTTP, port, or command).

```toml
[daemons.api.hooks]
on_ready = "curl -s -X POST https://slack.example.com/webhook -d '{\"text\": \"API is up\"}'"
```

### `on_fail`

Fires when the daemon fails and the applicable retry budget is exhausted: `ready_retry` for a daemon that died before becoming ready, `retry` for one that had become ready. If the applicable budget is `0`, fires immediately on failure.

```toml
[daemons.api.hooks]
on_fail = "./scripts/alert-team.sh"
```

The `PITCHFORK_EXIT_CODE` environment variable contains the exit code from the failed process.

### `on_retry`

Fires before each retry attempt, both startup retries (`ready_retry`) and background restarts (`retry`).

```toml
[daemons.api.hooks]
on_retry = "echo 'Retrying api (attempt $PITCHFORK_RETRY_COUNT)...'"
```

### `on_stop`

Fires when the daemon is explicitly stopped by pitchfork (via `pitchfork stop`, `auto = ["stop"]` directory exit, or supervisor shutdown).

```toml
[daemons.api.hooks]
on_stop = "./scripts/notify-stopped.sh"
```

### `on_exit`

Fires on **any** daemon termination — intentional stop, clean exit, or crash. Also fires during supervisor shutdown. Use this for cleanup that should always run regardless of why the daemon stopped.

> **Note:** For daemons with `retry > 0` or `ready_retry > 0`, `on_exit` fires **only after the applicable retry budget is exhausted**, not on each individual crash attempt. Use `on_retry` if you need to react to every failure.

```toml
[daemons.infra.hooks]
on_exit = "docker compose down --volumes"
```

The `PITCHFORK_EXIT_CODE` and `PITCHFORK_EXIT_REASON` environment variables are available to distinguish the cause.

### `on_output`

Fires when the daemon writes a line to stdout or stderr that matches an optional pattern. Useful for reacting to log messages without relying on a readiness check.

`on_output` accepts a command string (shorthand) or an inline table (full form):

```toml
# Shorthand (run only, fires on every line)
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

```toml
[daemons.api.hooks]
# Fire once when a specific string appears
on_output = { filter = "Server started", run = "curl https://monitor.example.com/up" }

# Fire when a line matches a regex
on_output = { regex = "listening on port [0-9]+", run = "./scripts/register-port.sh" }

# Fire on every line, but no more than once per 5 seconds
on_output = { run = "./scripts/log-activity.sh", debounce = "5s" }
```

The matched line is available in the hook command as `$PITCHFORK_MATCHED_LINE`.

## Environment Variables

All hooks receive these environment variables:

| Variable | Description |
|----------|-------------|
| `PITCHFORK_DAEMON_ID` | The daemon's fully-qualified ID (`namespace/name`) |
| `PITCHFORK_DAEMON_NAMESPACE` | The daemon's namespace |
| `PITCHFORK_RETRY_COUNT` | Current retry attempt (0 on first run). Counts startup retries during a blocking start and background restarts afterwards |
| `PITCHFORK_EXIT_CODE` | Exit code of the process (`on_fail`, `on_stop`, `on_exit`). On Unix, processes terminated by a signal (e.g. SIGTERM) have no POSIX exit code; in that case this is set to `-1`. |
| `PITCHFORK_EXIT_REASON` | Why the daemon stopped. Typically `"stop"` (intentional stop by pitchfork) or `"fail"` (non-zero exit); `"exit"` indicates an unexpected clean exit (process quit on its own with code 0). Available in `on_stop` and `on_exit`. |
| `PITCHFORK_MATCHED_LINE` | The raw output line that triggered the hook (`on_output` only) |
| `PITCHFORK_PORT` | The daemon's first resolved port (same as `PITCHFORK_PORT0`); omitted when the daemon has no `port` config |
| `PITCHFORK_PORT0..N` | One variable per resolved port (`PITCHFORK_PORT0`, `PITCHFORK_PORT1`, ...), mirroring the daemon's own `PORT0..N` |

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

## Behavior

- Hooks are **fire-and-forget** — they run in the background and never block the daemon
- Hook commands run in the daemon's working directory
- Errors in hooks are logged but do not affect the daemon
- Hooks read fresh configuration from the daemon's own directory each time they
  fire, independent of the directory where the per-user supervisor started

## Examples

**Send a Slack notification on failure:**

```toml
[daemons.api]
run = "npm run server"
retry = 3

[daemons.api.hooks]
on_fail = "curl -s -X POST $SLACK_WEBHOOK -d '{\"text\": \"API failed (exit $PITCHFORK_EXIT_CODE)\"}'"
```

**Log retry attempts to a file:**

```toml
[daemons.worker]
run = "python worker.py"
retry = 5

[daemons.worker.hooks]
on_retry = "sh -c 'echo \"$(date): retry $PITCHFORK_RETRY_COUNT\" >> /var/log/worker-retries.log'"
```

**Run cleanup on failure:**

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
