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
auto = ["start", "stop"]
ready_delay = 5
ready_output = "pattern to match"
ready_http = "http://localhost:8080/health"
depends = ["other-daemon"]
boot_start = true
cron = { schedule = "0 0 * * * *", retrigger = "finish" }
```

### Daemon Configuration Options

| Option | Type | Description |
|--------|------|-------------|
| `run` | string | **Required**. The command to execute |
| `retry` | integer | Number of retry attempts on failure (default: 0) |
| `auto` | array | Auto-start/stop behavior: `["start"]`, `["stop"]`, or `["start", "stop"]` |
| `ready_delay` | integer | Seconds to wait before considering the daemon ready (default: 3) |
| `ready_output` | string | Output pattern (regex) to match for readiness |
| `ready_http` | string | HTTP endpoint URL to poll for readiness (2xx = ready) |
| `depends` | array | List of daemon names that must be started before this daemon |
| `boot_start` | boolean | Start this daemon automatically on system boot (default: false). See [Start on Boot](/boot-start) |
| `cron` | table | Cron scheduling configuration. See [Cron](/cron) |
