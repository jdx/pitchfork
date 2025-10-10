# Retry on Failure

Pitchfork provides automatic retry functionality to handle transient failures in your daemons. When a daemon exits with a non-zero exit code, Pitchfork can automatically restart it, making your services more resilient to temporary issues.

## Basic Configuration

Configure retry behavior using the `retry` field in `pitchfork.toml`:

```toml
[daemons.api]
run = "npm run server:api"
retry = 3  # Retry up to 3 times on failure
```

This tells Pitchfork to retry the daemon up to 3 times if it exits with an error. The total number of execution attempts will be 4 (1 initial attempt + 3 retries).

### Command-line Override

You can also specify retry behavior for one-off daemons or override the configured value:

```bash
# For pitchfork run
pitchfork run my-task --retry 3 -- ./my-flaky-script.sh

# For pitchfork start (overrides pitchfork.toml)
pitchfork start api --retry 5
```

## How Retry Works

Pitchfork uses two different retry mechanisms depending on when and how the failure occurs:

### 1. Synchronous Retry (Startup Failures)

**When:** The daemon fails during startup, before the ready check completes and `pitchfork start/run` returns.

**Behavior:**
- The `pitchfork start` command waits and retries synchronously
- Uses exponential backoff between attempts (1s, 2s, 4s, 8s, ...)
- The command blocks until either:
  - The daemon becomes [ready](./ready-checks.md) (success)
  - All retry attempts are exhausted (failure)

**Example:**
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

**Use case:** Ideal for services that may fail due to:
- Waiting for dependent services to be ready
- Port conflicts that resolve quickly
- Temporary resource constraints during system startup

### 2. Asynchronous Retry (Runtime Crashes)

**When:** The daemon crashes after it has been successfully running for a while.

**Behavior:**
- The supervisor detects the crash in the background
- Automatically retries the daemon every 10 seconds
- Continues until the retry count is exhausted
- Happens independently of any CLI commands

**Example:**
```bash
$ pitchfork start api
# Daemon starts successfully and ready check passes
started api
$ # ... daemon runs for a while ...
# Daemon crashes unexpectedly
# Supervisor detects crash
# Wait 10 seconds... retry (attempt 1/3)
# Daemon crashes again
# Wait 10 seconds... retry (attempt 2/3)
# Success! Daemon stays running
```

**Use case:** Ideal for services that may:
- Experience transient network issues
- Have memory leaks that cause periodic crashes
- Depend on external resources that temporarily fail


