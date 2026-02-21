# Lifecycle Hooks

Run custom shell commands when daemons become ready, fail, or are retried.

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

## Environment Variables

All hooks receive these environment variables:

| Variable | Description |
|----------|-------------|
| `PITCHFORK_DAEMON_ID` | The daemon's name |
| `PITCHFORK_RETRY_COUNT` | Current retry attempt (0 on first run) |
| `PITCHFORK_EXIT_CODE` | Exit code of the failed process (`on_fail` only) |

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
