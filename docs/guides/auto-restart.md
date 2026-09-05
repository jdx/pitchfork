# Auto Restart on Failure

Configure pitchfork to automatically restart daemons when they fail.

Pitchfork has two independent retry budgets, depending on when the failure occurs:

- `ready_retry` - retries while the daemon is still starting up, before it becomes ready
- `retry` - restarts handled by the supervisor in the background after a failure

## Background Restarts: `retry`

Add the `retry` field to your daemon configuration:

```toml
[daemons.api]
run = "npm run server:api"
retry = 3  # Restart up to 3 times in the background on failure
```

This tells the supervisor to restart the daemon up to 3 times in the background if it exits with an error. Total background attempts: 4 (1 initial + 3 restarts).

### Infinite Restarts

Use `retry = true` for daemons that should always be restarted:

```toml
[daemons.critical-worker]
run = "npm run worker"
retry = true  # Restart forever until manually stopped
```

This is useful for critical services that must stay running.

### How Background Restarts Work

**When:** The daemon exits with an error, whether it crashed after running fine or failed to start.

**Behavior:**
- The supervisor detects the failure on its interval tick (default: 10 seconds, configurable via `settings.general.interval`)
- Retries once per tick until the `retry` budget is exhausted
- Happens independently of CLI commands

```bash
$ pitchfork start api
# Daemon starts successfully
started api
$ # ... daemon runs for a while ...
# Daemon crashes unexpectedly
# Supervisor detects the crash
# Next tick... restart (attempt 1/3)
# Success! Daemon stays running
```

The background counter resets whenever the daemon is started explicitly again (via `pitchfork start`, a file-watch restart, or a cron trigger).

**Use case:** Services that experience:
- Transient network issues
- Memory leaks causing periodic crashes
- External resource failures

## Startup Retries: `ready_retry`

Add the `ready_retry` field to retry starting a daemon that fails before it becomes ready:

```toml
[daemons.api]
run = "npm run server:api"
ready_retry = 3  # Retry startup up to 3 times when it fails before ready
```

### How Startup Retries Work

**When:** A blocking start (e.g. `pitchfork start`, which waits for readiness) and the daemon fails before the ready check completes.

**Behavior:**
- `pitchfork start` waits and retries synchronously
- Uses exponential backoff: 1s, 2s, 4s, 8s, ... capped at 3600s
- Blocks until the daemon becomes ready or the `ready_retry` budget is exhausted
- Total attempts: `ready_retry + 1`

```bash
$ pitchfork start api
# Daemon fails immediately
# Wait 1 second... retry (attempt 1/3)
# Daemon fails again
# Wait 2 seconds... retry (attempt 2/3)
# Daemon fails again
# Wait 4 seconds... retry (attempt 3/3)
# All startup retries exhausted
ERROR: daemon api failed with exit code 1
```

After `pitchfork start` gives up, the daemon stays in the `errored` state. If `retry` is also configured, the supervisor continues restarting it in the background under that separate budget.

**Use case:** Services that fail during startup due to:
- Waiting for dependent services
- Temporary port conflicts
- Resource constraints during startup

## Using Both Together

The two budgets are independent and cover different phases:

```toml
[daemons.api]
run = "npm run server"
ready_retry = 3  # blocking `pitchfork start` retries startup failures 3 times
retry = 5        # afterwards the supervisor restarts it up to 5 times in the background
ready_http = "http://localhost:3000/health"
```

A blocking start never overlaps with background restarts: while `pitchfork start` is running (including its backoff sleeps), the supervisor's background restarts skip that daemon, and `pitchfork start` stops retrying if the daemon was started elsewhere in the meantime.

## CLI Override

Override retry behavior from the command line:

```bash
# For pitchfork run
pitchfork run my-task --retry 3 --ready-retry 2 -- ./my-script.sh

# When adding a daemon
pitchfork daemons add api --run 'npm start' --retry 3 --ready-retry 2
```

## Example Configurations

**Flaky service with background restarts:**

```toml
[daemons.api]
run = "npm run server"
retry = 5
ready_http = "http://localhost:3000/health"
```

**Database with startup retries:**

```toml
[daemons.postgres]
run = "postgres -D /var/lib/pgsql/data"
ready_retry = 3
ready_output = "ready to accept connections"
```

## Lifecycle Hooks

You can run custom commands when daemons become ready, fail, or retry. See the [Lifecycle Hooks guide](/guides/lifecycle-hooks) for details.
