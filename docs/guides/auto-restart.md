# Auto Restart on Failure

Configure pitchfork to automatically restart daemons when they crash.

## Basic Configuration

Add the `retry` field to your daemon configuration:

```toml
[daemons.api]
run = "npm run server:api"
retry = 3  # Retry up to 3 times on failure
```

This tells pitchfork to retry up to 3 times if the daemon exits with an error. Total attempts: 4 (1 initial + 3 retries).

## Infinite Retries

Use `retry = true` for daemons that should always restart:

```toml
[daemons.critical-worker]
run = "npm run worker"
retry = true  # Retry forever until manually stopped
```

This is useful for critical services that must stay running.

## CLI Override

Override retry behavior from the command line:

```bash
# For pitchfork run
pitchfork run my-task --retry 3 -- ./my-script.sh

# For pitchfork start (overrides pitchfork.toml)
pitchfork start api --retry 5
```

## How Retry Works

Pitchfork uses two different retry mechanisms depending on when the failure occurs:

### Startup Failures (Synchronous Retry)

**When:** The daemon fails before the ready check completes.

**Behavior:**
- `pitchfork start` waits and retries synchronously
- Uses exponential backoff: 1s, 2s, 4s, 8s, ...
- Blocks until daemon becomes ready or all retries are exhausted

```bash
$ pitchfork start api
# Daemon fails immediately
# Wait 1 second... retry (attempt 1/3)
# Daemon fails again
# Wait 2 seconds... retry (attempt 2/3)
# Daemon fails again
# Wait 4 seconds... retry (attempt 3/3)
# All retries exhausted
ERROR: daemon api failed with exit code 1
```

**Use case:** Services that fail due to:
- Waiting for dependent services
- Temporary port conflicts
- Resource constraints during startup

### Runtime Crashes (Asynchronous Retry)

**When:** The daemon crashes after successfully starting.

**Behavior:**
- The supervisor detects the crash in the background
- Retries every 10 seconds
- Continues until retry count is exhausted
- Happens independently of CLI commands

```bash
$ pitchfork start api
# Daemon starts successfully
started api
$ # ... daemon runs for a while ...
# Daemon crashes unexpectedly
# Supervisor detects crash
# Wait 10 seconds... retry (attempt 1/3)
# Success! Daemon stays running
```

**Use case:** Services that experience:
- Transient network issues
- Memory leaks causing periodic crashes
- External resource failures

## Example Configurations

**Flaky service with retries:**
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
retry = 3
ready_output = "ready to accept connections"
```

## Lifecycle Hooks

You can run custom commands when daemons become ready, fail, or retry. See the [Lifecycle Hooks guide](/guides/lifecycle-hooks) for details.
