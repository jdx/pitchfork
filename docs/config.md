# Configuration

Configurations are merged in a specific order, with later configurations overriding earlier ones.

## Configuration Files

Pitchfork looks for configuration files in the following locations (in order):

- **System-level**: `/etc/pitchfork/config.toml`
- **User-level**: `~/.config/pitchfork/config.toml`
- **Project-level**: `pitchfork.toml` files from the filesystem root up to the current directory

## Configuration Format

All configuration files use the TOML format with the following structure:

```toml
[daemons.<daemon-name>]
run = "command to execute"
retry = 3
auto = ["start"]
ready_delay = 1000
ready_output = "pattern to match"

[daemons.<daemon-name>.cron]
schedule = "0 * * * *"
retrigger = "finish"
```

### Daemon Configuration Options

> The following options may be outdated.

| Option | Type | Description |
|--------|------|-------------|
| `run` | string | **Required**. The command to execute |
| `retry` | integer | Number of retry attempts on failure (default: 0) |
| `auto` | array | Auto-start/stop behavior: `["start"]`, `["stop"]`, or `["start", "stop"]` |
| `ready_delay` | integer | Milliseconds to wait before considering the daemon ready |
| `ready_output` | string | Output pattern (string or regex) to match for readiness |
| `cron` | table | See [cron](cron) |
