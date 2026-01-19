# Web UI

Pitchfork includes a built-in web interface for monitoring and managing daemons.

## Enable the Web UI

The web UI is disabled by default. To enable it, specify a port:

```bash
# Via environment variable
export PITCHFORK_WEB_PORT=19876
pitchfork supervisor start --force

# Via command line
pitchfork supervisor run --web-port 19876
```

Then open http://127.0.0.1:19876 in your browser.

If the specified port is in use, pitchfork tries the next 10 ports automatically.

## Features

### Dashboard

Overview of all daemons showing:
- Name and status (running, stopped, failed)
- Process ID (PID)
- Error messages for failed daemons

### Daemon Management

Control daemons directly from the browser:
- **Start** - Launch a stopped daemon
- **Stop** - Gracefully stop a running daemon
- **Restart** - Stop and start a daemon
- **Enable/Disable** - Control whether a daemon can be started

### Live Logs

Real-time log streaming for each daemon:
- Select a daemon to view its logs
- Logs update automatically
- Scroll through historical logs

### Config Editing

Edit `pitchfork.toml` files with:
- TOML syntax validation
- Save changes directly from the UI
