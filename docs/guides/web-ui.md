# Web UI

Pitchfork includes a built-in web interface for monitoring and managing daemons.

## Access the Web UI

Open http://127.0.0.1:19876 in your browser.

The Web UI starts automatically when the supervisor runs.

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

## Configure the Port

If port 19876 is in use, pitchfork tries ports 19877-19885 automatically.

To specify a port:

```bash
# Via environment variable
PITCHFORK_WEB_PORT=8080 pitchfork supervisor start --force

# Via command line
pitchfork supervisor run --web-port 8080
```

## Disable the Web UI

If you don't need the web interface:

```bash
# Via environment variable
PITCHFORK_NO_WEB=true pitchfork supervisor start --force

# Via command line
pitchfork supervisor run --no-web
```
