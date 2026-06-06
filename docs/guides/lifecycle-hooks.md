# Lifecycle Hooks & Gates

**Hooks** are fire-and-forget shell commands that run in response to daemon lifecycle events. **Gates** are blocking checkpoints that must pass before the lifecycle proceeds. Use hooks for notifications and side effects; use gates for preconditions and validation.

## Configuration

```toml
[daemons.api]
run = "npm run server"
retry = 3
ready_http = "http://localhost:3000/health"

[daemons.api.hooks]
on_ready = "curl -X POST https://alerts.example.com/ready"
on_fail = "./scripts/cleanup.sh"
on_output = { filter = "Server started", run = "./scripts/notify-ready.sh" }

[daemons.api.gates]
pre_start = "curl -sf http://deps:8080/health"
post_start = { run = "curl -sf http://localhost:3000/api/version", timeout = "10s" }
pre_stop = "./scripts/drain-connections.sh"
post_stop = "./scripts/cleanup-sockets.sh"
```

## Hooks

Hooks run in the background and never block the daemon. Errors are logged but do not affect the lifecycle.

### Hook Types

| Hook | When it fires |
|------|--------------|
| `on_ready` | Daemon passes its readiness check (delay, output match, HTTP, port, or command) |
| `on_fail` | Daemon fails and all retries are exhausted |
| `on_retry` | Before each retry attempt |
| `on_stop` | Daemon is explicitly stopped by pitchfork |
| `on_exit` | Any daemon termination (stop, clean exit, or crash) |
| `on_output` | Daemon writes a line matching an optional pattern |

#### `on_ready`

```toml
[daemons.api.hooks]
on_ready = "curl -s -X POST https://slack.example.com/webhook -d '{\"text\": \"API is up\"}'"
```

#### `on_fail`

Fires when the daemon fails and all retries are exhausted. If `retry = 0`, fires immediately on failure.

```toml
[daemons.api.hooks]
on_fail = "./scripts/alert-team.sh"
```

#### `on_retry`

Fires before each retry attempt.

```toml
[daemons.api.hooks]
on_retry = "echo 'Retrying api (attempt $PITCHFORK_RETRY_COUNT)...'"
```

#### `on_stop`

Fires when the daemon is explicitly stopped by pitchfork (via `pitchfork stop`, `auto = ["stop"]` directory exit, or supervisor shutdown).

```toml
[daemons.api.hooks]
on_stop = "./scripts/notify-stopped.sh"
```

#### `on_exit`

Fires on **any** daemon termination — intentional stop, clean exit, or crash. Also fires during supervisor shutdown. Use this for cleanup that should always run regardless of why the daemon stopped.

> **Note:** For daemons with `retry > 0`, `on_exit` fires **only after all retries are exhausted**, not on each individual crash attempt. Use `on_retry` if you need to react to every failure.

```toml
[daemons.infra.hooks]
on_exit = "docker compose down --volumes"
```

#### `on_output`

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

### Hook Behavior

- Hooks are **fire-and-forget** — they run in the background and never block the daemon
- Hook commands run in the daemon's working directory
- Errors in hooks are logged but do not affect the daemon
- Hooks read fresh configuration from `pitchfork.toml` each time they fire

## Gates

Gates block the lifecycle until the gate command exits with code 0. If a gate fails (non-zero exit or timeout), the lifecycle action is aborted with an error.

### Gate Types

| Gate | When it runs | Blocks until |
|------|-------------|-------------|
| `pre_start` | Before the daemon process is spawned | Command exits 0 |
| `post_start` | After the daemon becomes ready (or 500 ms after spawn if no readiness check) | Command exits 0 |
| `pre_stop` | Before the daemon is stopped | Command exits 0 |
| `post_stop` | After the daemon has stopped | Command exits 0 |

### Shorthand vs Full Form

Each gate accepts a command string (shorthand) or an inline table (full form):

```toml
# Shorthand (command only, no timeout)
pre_start = "curl -sf http://deps:8080/health"

# Full form with timeout
pre_start = { run = "curl -sf http://deps:8080/health", timeout = "30s" }
```

| Field | Required | Description |
|-------|----------|-------------|
| `run` | Yes | Shell command to execute |
| `timeout` | No | Maximum time to wait (humantime, e.g. `"30s"`, `"5m"`). Defaults to `settings.supervisor.gate_timeout` if not set. |

### Default Gate Timeout

If no per-gate `timeout` is specified, the global default from `settings.supervisor.gate_timeout` is used (default: `"30s"`). This prevents gates from blocking indefinitely.

```toml
[settings.supervisor]
gate_timeout = "60s"
```

### Gate Behavior

- Gates **block** the lifecycle — the start/stop operation waits for the gate to pass
- If a gate command exits with a non-zero code, the lifecycle action fails with an error
- If a gate command times out, it is killed and the lifecycle action fails
- Gate commands run in the daemon's working directory
- Gates read fresh configuration from `pitchfork.toml` each time they run
- `post_start` in non-wait-ready mode runs as a fire-and-forget task after a 500 ms delay (errors are logged but do not block)

## Hooks vs Gates

| | Hooks | Gates |
|--|-------|-------|
| Blocking | No (fire-and-forget) | Yes (blocks lifecycle) |
| Failure impact | Logged, daemon unaffected | Lifecycle aborted with error |
| Timeout | No | Yes (per-gate or global default) |
| Config section | `[daemons.<name>.hooks]` | `[daemons.<name>.gates]` |

## Environment Variables

All hooks and gates receive these environment variables:

| Variable | Hooks | Gates | Description |
|----------|-------|-------|-------------|
| `PITCHFORK_DAEMON_ID` | All | All | The daemon's fully-qualified ID (`namespace/name`) |
| `PITCHFORK_DAEMON_NAMESPACE` | All | All | The daemon's namespace |
| `PITCHFORK_RETRY_COUNT` | All | All | Current retry attempt (0 on first run) |
| `PITCHFORK_EXIT_CODE` | `on_fail`, `on_stop`, `on_exit` | `post_stop` | Exit code of the process. On Unix, processes terminated by a signal (e.g. SIGTERM) have no POSIX exit code; set to `-1`. |
| `PITCHFORK_EXIT_REASON` | `on_stop`, `on_exit` | `post_stop` | Why the daemon stopped: `"stop"` (intentional), `"fail"` (non-zero exit), or `"exit"` (clean exit) |
| `PITCHFORK_MATCHED_LINE` | `on_output` | — | The raw output line that triggered the hook |

Any custom `env` variables from the daemon config are also passed to hooks and gates.

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

### Hooks

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

### Gates

**Wait for a dependency before starting:**

```toml
[daemons.api]
run = "npm run server"

[daemons.api.gates]
pre_start = "curl -sf http://localhost:5432/health"
```

**Validate the daemon is healthy after startup:**

```toml
[daemons.api]
run = "npm run server"
ready_http = "http://localhost:3000/health"

[daemons.api.gates]
post_start = { run = "curl -sf http://localhost:3000/api/version", timeout = "10s" }
```

**Drain connections before stopping:**

```toml
[daemons.api]
run = "npm run server"

[daemons.api.gates]
pre_stop = "./scripts/drain-connections.sh"
```

**Clean up after a daemon stops:**

```toml
[daemons.api]
run = "npm run server"

[daemons.api.gates]
post_stop = "./scripts/cleanup-sockets.sh"
```
