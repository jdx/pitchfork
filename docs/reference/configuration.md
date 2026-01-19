# Configuration Reference

Complete reference for `pitchfork.toml` configuration files.

## Configuration Hierarchy

Pitchfork loads configuration files in order, with later files overriding earlier ones:

1. **System-level:** `/etc/pitchfork/config.toml`
2. **User-level:** `~/.config/pitchfork/config.toml`
3. **Project-level:** `pitchfork.toml` files from filesystem root to current directory

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

Number of retry attempts on failure. Default: `0`

```toml
[daemons.api]
run = "npm run server"
retry = 3
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

List of daemon names that must be started first.

```toml
[daemons.api]
run = "npm run server"
depends = ["postgres", "redis"]
```

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
