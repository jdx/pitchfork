# Hooks

Hooks allow you to run custom commands at specific points in a daemon's lifecycle. This is useful for notifications, logging, cleanup tasks, or any other automation you need.

## Available Hooks

| Hook | Trigger |
|------|---------|
| `on_ready` | When the daemon passes its ready check |
| `on_fail` | When the daemon fails (exits with non-zero code) |
| `on_cron_trigger` | When cron triggers (before starting the daemon) |
| `on_retry` | Before each retry attempt |

## Basic Usage

Hooks are specified as simple command strings:

```toml
[daemons.api]
run = "npm run dev"
ready_output = "Server listening"
on_ready = "echo 'API is ready!'"
on_fail = "echo 'API crashed!'"
```

## Environment Variables

Hooks automatically receive these environment variables:

| Variable | Description |
|----------|-------------|
| `PITCHFORK_DAEMON_ID` | Fully qualified daemon ID (e.g., `project/api`) |
| `PITCHFORK_DAEMON_NAMESPACE` | Daemon namespace (e.g., `project`) |
| `PITCHFORK_DAEMON_NAME` | Daemon name (e.g., `api`) |
| `PITCHFORK_HOOK_NAME` | Name of the hook being executed (e.g., `on_ready`) |
| `PITCHFORK_EXIT_CODE` | Exit code (only for `on_fail` hook) |
| `PITCHFORK_RETRY_COUNT` | Current retry attempt (only for `on_retry` hook) |
| `PITCHFORK_MAX_RETRIES` | Maximum retry count (only for `on_retry` hook) |

## Use Cases

### Desktop Notifications

Send notifications when daemons change state:

```toml
[daemons.api]
run = "npm run dev"
ready_output = "listening on port"
on_ready = "notify-send 'API' 'Server is ready'"
on_fail = "notify-send -u critical 'API' 'Server crashed!'"
```

### Alerting

Send alerts to external services:

```toml
[daemons.worker]
run = "python worker.py"
retry = 3
on_fail = "curl -X POST -H 'Content-Type: application/json' -d '{\"text\": \"Worker failed!\"}' $SLACK_WEBHOOK_URL"
on_retry = "echo 'Retrying worker...' >> /var/log/worker-retries.log"
```

### Cron Job Notifications

Get notified about scheduled task execution:

```toml
[daemons.backup]
run = "backup.sh"
cron.schedule = "0 2 * * *"
on_cron_trigger = "echo 'Starting scheduled backup at $(date)' >> /var/log/backup.log"
on_ready = "notify-send 'Backup' 'Completed successfully'"
on_fail = "notify-send -u critical 'Backup' 'Failed!'"
```

### Cleanup Tasks

Run cleanup when a daemon fails:

```toml
[daemons.database]
run = "postgres -D /var/lib/postgres/data"
on_fail = "rm -f /var/lib/postgres/data/postmaster.pid"
```

### Logging

Create custom log entries:

```toml
[daemons.api]
run = "npm run dev"
retry = 5
on_ready = "logger -t pitchfork 'API daemon is ready'"
on_fail = "logger -t pitchfork -p user.error 'API daemon failed'"
on_retry = "logger -t pitchfork -p user.warning 'API daemon retrying'"
```

## Hook Execution

- Hooks run **asynchronously** and do not block daemon operations
- Hook failures are logged but do not affect the daemon's state
- Hooks inherit the daemon's working directory unless overridden
- Environment variables from the daemon's `run.env` are **not** automatically inherited by hooks

## Best Practices

1. **Keep hooks lightweight** - Hooks should complete quickly to avoid resource buildup
2. **Handle failures gracefully** - Hook commands should not depend on external services being available
3. **Use absolute paths** - When in doubt, use absolute paths for commands and files
4. **Log important events** - Use hooks to create an audit trail of daemon lifecycle events
5. **Test hooks independently** - Verify hook commands work before adding them to configuration
