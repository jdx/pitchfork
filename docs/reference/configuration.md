# Configuration Reference

Complete reference for `pitchfork.toml` configuration files.

## Configuration Hierarchy

Pitchfork loads configuration files in order, with later files overriding earlier ones:

1. **System-level:** `/etc/pitchfork/config.toml`
2. **User-level:** `~/.config/pitchfork/config.toml`
3. **Project-level:** `pitchfork.toml` files from filesystem root to current directory

## JSON Schema

A JSON Schema is available for editor autocompletion and validation:

**URL:** [`https://pitchfork.dev/schema.json`](/schema.json)

### Editor Setup

**VS Code** with [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml):

```toml
#:schema https://pitchfork.dev/schema.json

[daemons.api]
run = "npm run server"
```

**JetBrains IDEs**: Add the schema URL in Settings → Languages & Frameworks → Schemas and DTDs → JSON Schema Mappings.

## File Format

All configuration uses TOML format:

```toml
[daemons.<daemon-name>]
run = "command to execute"
# ... other options
```

## Daemon Options

### `run` (required)

The command to execute.

```toml
[daemons.api]
run = "npm run server"
```

### `retry`

Number of retry attempts on failure, or `true` for infinite retries. Default: `0`

- A number (e.g., `3`) means retry that many times
- `true` means retry indefinitely
- `false` or `0` means no retries

```toml
[daemons.api]
run = "npm run server"
retry = 3  # Retry up to 3 times

[daemons.critical]
run = "npm run worker"
retry = true  # Retry forever
```

### `auto`

Auto-start and auto-stop behavior with shell hook. Options: `"start"`, `"stop"`

```toml
[daemons.api]
run = "npm run server"
auto = ["start", "stop"]  # Both auto-start and auto-stop
```

### `ready_delay`

Seconds to wait before considering the daemon ready. Default: `3`

```toml
[daemons.api]
run = "npm run server"
ready_delay = 5
```

### `ready_output`

Regex pattern to match in output for readiness.

```toml
[daemons.postgres]
run = "postgres -D /var/lib/pgsql/data"
ready_output = "ready to accept connections"
```

### `ready_http`

HTTP endpoint URL to poll for readiness (2xx = ready).

```toml
[daemons.api]
run = "npm run server"
ready_http = "http://localhost:3000/health"
```

### `ready_port`

TCP port to check for readiness. Daemon is ready when port is listening.

```toml
[daemons.api]
run = "npm run server"
ready_port = 3000
```

### `depends`

List of daemon names that must be started before this daemon. When you start a daemon, its dependencies are automatically started first in the correct order.

```toml
[daemons.api]
run = "npm run server"
depends = ["postgres", "redis"]
```

**Behavior:**

- **Auto-start**: Running `pitchfork start api` will automatically start `postgres` and `redis` first
- **Transitive dependencies**: If `postgres` depends on `storage`, that will be started too
- **Parallel starting**: Dependencies at the same level start in parallel for faster startup
- **Skip running**: Already-running dependencies are skipped (not restarted)
- **Circular detection**: Circular dependencies are detected and reported as errors
- **Force flag**: Using `-f` only restarts the explicitly requested daemon, not its dependencies

**Example with chained dependencies:**

```toml
[daemons.database]
run = "postgres -D /var/lib/pgsql/data"
ready_port = 5432

[daemons.cache]
run = "redis-server"
ready_port = 6379

[daemons.api]
run = "npm run server"
depends = ["database", "cache"]

[daemons.worker]
run = "npm run worker"
depends = ["database"]
```

Running `pitchfork start api worker` starts daemons in this order:
1. `database` and `cache` (in parallel, no dependencies)
2. `api` and `worker` (in parallel, after their dependencies are ready)

### `boot_start`

Start this daemon automatically on system boot. Default: `false`

```toml
[daemons.postgres]
run = "postgres -D /var/lib/pgsql/data"
boot_start = true
```

### `cron`

Cron scheduling configuration.

```toml
[daemons.backup]
run = "./backup.sh"
cron = { schedule = "0 0 2 * * *", retrigger = "finish" }
```

**Fields:**
- `schedule` - Cron expression (6 fields: second, minute, hour, day, month, weekday)
- `retrigger` - Behavior when schedule fires: `"finish"` (default), `"always"`, `"success"`, `"fail"`

## Complete Example

```toml
# Database - starts on boot, no auto-stop
[daemons.postgres]
run = "postgres -D /var/lib/pgsql/data"
ready_output = "ready to accept connections"
boot_start = true
retry = 3

# Cache - starts with API
[daemons.redis]
run = "redis-server"
ready_output = "Ready to accept connections"

# API server - depends on database and cache
[daemons.api]
run = "npm run server"
depends = ["postgres", "redis"]
ready_http = "http://localhost:3000/health"
auto = ["start", "stop"]
retry = 5

# Scheduled backup
[daemons.backup]
run = "./scripts/backup.sh"
cron = { schedule = "0 0 2 * * *", retrigger = "finish" }
```
