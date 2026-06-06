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

**Hooks:**
- `on_retry` fires before each startup retry attempt
- `on_fail` fires when all startup retries are exhausted

**Use case:** Services that fail due to:
- Waiting for dependent services
- Temporary port conflicts
- Resource constraints during startup

### Runtime Crashes (Asynchronous Retry)

**When:** The daemon crashes after successfully starting.

**Behavior:**
- The supervisor detects the crash in the background
- Retries at each supervisor interval tick (default: 10 seconds, configurable via `settings.general.interval`)
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

**Hooks:**
- `on_recover` fires before each runtime recovery attempt
- `on_crash` fires when all runtime retries are exhausted

**Use case:** Services that experience:
- Transient network issues
- Memory leaks causing periodic crashes
- External resource failures

## Recovery Configuration

By default, runtime recovery uses the same limit as `retry`. Use the `recovery` field to set a different limit for runtime crashes independently from startup retries:

```toml
[daemons.api]
run = "npm run server"
retry = 3       # Startup: retry up to 3 times
recovery = 1    # Runtime: only 1 recovery attempt after crash
```

Like `retry`, `recovery` accepts a number, `true` (infinite), or `false`/`0` (none):

```toml
[daemons.critical]
run = "npm run worker"
retry = 3        # Startup: retry up to 3 times
recovery = true  # Runtime: always recover (infinite)
```

When `recovery` is not set, it defaults to the value of `retry`. This means a daemon with `retry = 3` will also have up to 3 runtime recovery attempts unless `recovery` is explicitly configured.

### Common Patterns

**Strict startup, lenient runtime** — allow many startup retries but limit runtime recoveries:
```toml
[daemons.api]
run = "npm run server"
retry = 5        # Startup: retry up to 5 times (service may need time)
recovery = 1     # Runtime: only 1 recovery (crashes are more concerning)
```

**Lenient startup, strict runtime** — fail fast on startup but always recover at runtime:
```toml
[daemons.worker]
run = "python worker.py"
retry = 1         # Startup: only 1 retry (config is probably wrong)
recovery = true   # Runtime: always recover (transient errors are common)
```

**No runtime recovery** — only retry at startup, never recover at runtime:
```toml
[daemons.batch]
run = "./process.sh"
retry = 3          # Startup: retry up to 3 times
recovery = false   # Runtime: never recover (let it stay crashed)
```

## Retry Count vs Recovery Count

Pitchfork tracks two separate counters:

| Counter | Phase | Environment Variable | When it increments |
|---------|-------|---------------------|-------------------|
| `retry_count` | Startup | `PITCHFORK_RETRY_COUNT` | Each startup retry attempt |
| `recovery_count` | Runtime | `PITCHFORK_RECOVERY_COUNT` | Each runtime recovery attempt |

Both counters start at 0 on first start. `recovery_count` is persisted to state and survives supervisor restarts. File-change restarts reset `recovery_count` to 0 (fresh start, not a recovery).

```toml
[daemons.api]
run = "npm run server"
retry = 3

[daemons.api.hooks]
on_retry = "echo 'Startup retry $PITCHFORK_RETRY_COUNT'"
on_recover = "echo 'Runtime recovery $PITCHFORK_RECOVERY_COUNT'"
```

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

**Service with separate startup and runtime alerts:**
```toml
[daemons.api]
run = "npm run server"
retry = 3

[daemons.api.hooks]
on_fail = "./scripts/alert-startup-failure.sh"
on_crash = "./scripts/alert-runtime-crash.sh"
on_recover = "./scripts/notify-recovering.sh"
```

## Lifecycle Hooks

You can run custom commands when daemons become ready, fail, or retry. See the [Lifecycle Hooks guide](/guides/lifecycle-hooks) for details.
