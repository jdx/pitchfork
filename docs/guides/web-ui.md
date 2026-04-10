# Web UI

Pitchfork includes a built-in web interface for monitoring and managing daemons.

## Enable the Web UI

The web UI is disabled by default. There are several ways to enable it:

### One-time via CLI or environment variable

```bash
# Via CLI flag (foreground)
pitchfork supervisor run --web-port 3120

# Via environment variable (works with both run and start)
PITCHFORK_WEB_PORT=3120 pitchfork supervisor start --force
```

### Persistent via settings

Add to your `pitchfork.toml` or `~/.config/pitchfork/config.toml`:

```toml
[settings.web]
auto_start = true    # Start web UI automatically with supervisor
bind_port = 3120     # Default port (default: 3120)
bind_address = "127.0.0.1"  # Default: localhost only
```

Or via environment variables:

```bash
export PITCHFORK_WEB_AUTO_START=true
export PITCHFORK_WEB_BIND_PORT=3120
```

Then restart the supervisor:

```bash
pitchfork supervisor start --force
```

Open http://127.0.0.1:3120 in your browser.

If the specified port is in use, pitchfork tries the next 10 ports automatically (configurable via `web.port_attempts`).

### Path prefix

You can serve the web UI under a sub-path, useful when running behind a reverse proxy:

```bash
pitchfork supervisor run --web-port 3120 --web-path ps
# Web UI available at http://127.0.0.1:3120/ps/
```

Or via settings:

```toml
[settings.web]
auto_start = true
base_path = "ps"
```

## Features

### Dashboard

Overview of all daemons showing:
- Name and status (running, stopped, failed)
- Process ID (PID)
- Error messages for failed daemons

### Daemon Management

Control daemons directly from the browser:
- **Start** — Launch a stopped daemon
- **Stop** — Gracefully stop a running daemon
- **Restart** — Stop and start a daemon
- **Enable/Disable** — Control whether a daemon can be started

### Live Logs

Real-time log streaming for each daemon via Server-Sent Events (SSE):
- Select a daemon to view its logs
- Logs update automatically in real-time
- Scroll through historical logs
- Clear logs per daemon

### Config Editing

Edit `pitchfork.toml` files with:
- TOML syntax validation
- Save changes directly from the UI
