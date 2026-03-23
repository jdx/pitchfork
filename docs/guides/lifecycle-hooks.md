# Lifecycle Hooks

Run custom shell commands when daemons become ready, fail, are retried, or stop.

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
```

## Hook Types

### `on_ready`

Fires when the daemon passes its readiness check (delay, output match, HTTP, port, or command).

```toml
[daemons.api.hooks]
on_ready = "curl -s -X POST https://slack.example.com/webhook -d '{\"text\": \"API is up\"}'"
```

### `on_fail`

Fires when the daemon fails and all retries are exhausted. If `retry = 0`, fires immediately on failure.

```toml
[daemons.api.hooks]
on_fail = "./scripts/alert-team.sh"
```

The `PITCHFORK_EXIT_CODE` environment variable contains the exit code from the failed process.

### `on_retry`

Fires before each retry attempt.

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

> **Note:** For daemons with `retry > 0`, `on_exit` fires **only after all retries are exhausted**, not on each individual crash attempt. Use `on_retry` if you need to react to every failure.

```toml
[daemons.infra.hooks]
on_exit = "docker compose down --volumes"
```

The `PITCHFORK_EXIT_CODE` and `PITCHFORK_EXIT_REASON` environment variables are available to distinguish the cause.

## Environment Variables

All hooks receive these environment variables:

| Variable | Description |
|----------|-------------|
| `PITCHFORK_DAEMON_ID` | The daemon's fully-qualified ID (`namespace/name`) |
| `PITCHFORK_DAEMON_NAMESPACE` | The daemon's namespace |
| `PITCHFORK_RETRY_COUNT` | Current retry attempt (0 on first run) |
| `PITCHFORK_EXIT_CODE` | Exit code of the process (`on_fail`, `on_stop`, `on_exit`). On Unix, processes terminated by a signal (e.g. SIGTERM) have no POSIX exit code; in that case this is set to `-1`. |
| `PITCHFORK_EXIT_REASON` | Why the daemon stopped. Typically `"stop"` (intentional stop by pitchfork) or `"fail"` (non-zero exit); `"exit"` indicates an unexpected clean exit (process quit on its own with code 0). Available in `on_stop` and `on_exit`. |

Any custom `env` variables from the daemon config are also passed to hooks.

## Behavior

- Hooks are **fire-and-forget** — they run in the background and never block the daemon
- Hook commands run in the daemon's working directory
- Errors in hooks are logged but do not affect the daemon
- Hooks read fresh configuration from `pitchfork.toml` each time they fire

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
