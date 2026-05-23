# Web UI

Pitchfork includes a built-in web interface for monitoring and managing daemons. The web UI is served as a single-page application (SPA) that communicates with the supervisor via a REST API.

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

The web UI is owned by the supervisor process, so its settings are read once at
supervisor startup and do not hot-reload. Changing `[settings.web]` in any
config file requires restarting the supervisor with
`pitchfork supervisor start --force` for the change to take effect.

Add to your config:

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

## Standalone API Server

By default, the REST API is bundled with the web UI on the same port. You can run the API on a dedicated port separate from the web UI:

```toml
[settings.api]
bind_port = 8080          # Dedicated API port
bind_address = "127.0.0.1"
port_attempts = 10
```

When `api.bind_port` is set, the API endpoints are available on that port without the static file serving. This is useful when you only need programmatic access and not the browser UI.

## Authentication

The API uses token-based authentication when binding to non-loopback addresses.

- **Loopback only** (`127.0.0.1`, `::1`): No authentication required. Safe for local development.
- **Non-loopback** (e.g., `0.0.0.0`, LAN IP): A random 64-character hex token is auto-generated at startup. The token is printed to stderr and logged. Include it in every request:

```bash
curl -H "X-Pitchfork-Token: <token>" http://192.168.1.100:3120/api/daemons
```

You can also set a fixed token in config:

```toml
[settings.api]
token = "my-secret-token"
```

::: warning
Never expose the API to a public network without authentication. The auto-generated token is secure (128 bits of entropy), but you should still treat it as a secret.
:::

## API Reference

The following REST endpoints are available on the web UI port (or the dedicated API port if configured). All endpoints accept and return JSON unless otherwise noted.

### GET /api/stats

Return system-level statistics.

```bash
curl http://127.0.0.1:3120/api/stats
```

**Response:**

```json
{
  "process_count": 42,
  "cpu_count": 8,
  "total_memory": 17179869184
}
```

### GET /api/daemons

List all daemons with full runtime state.

```bash
curl http://127.0.0.1:3120/api/daemons
```

**Response:**

```json
[
  {
    "id": {
      "namespace": "myproject",
      "name": "api",
      "qualified": "myproject/api",
      "safe_path": "myproject--api"
    },
    "title": "API Server",
    "pid": 12345,
    "status": { "type": "running" },
    "dir": "/home/user/myproject",
    "cpu_percent": 2.3,
    "memory_bytes": 67108864,
    "uptime_secs": 3600,
    "proxy_url": "https://api.localhost",
    "slug": "api",
    "active_port": 3000,
    "resolved_port": [3000]
  }
]
```

### GET /api/daemons/{id}

Get a single daemon by qualified ID.

```bash
curl http://127.0.0.1:3120/api/daemons/myproject/api
```

Returns a single `ApiDaemonEntry` object (same shape as `/api/daemons` items).

### POST /api/daemons/{id}/start

Start a daemon.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject/api/start
```

**Response:**

```json
{ "ok": true, "error": null }
```

### POST /api/daemons/{id}/stop

Stop a running daemon.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject/api/stop
```

### POST /api/daemons/{id}/restart

Restart a daemon.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject/api/restart
```

### POST /api/daemons/{id}/enable

Enable a daemon so it can be started.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject/api/enable
```

### POST /api/daemons/{id}/disable

Disable a daemon.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject/api/disable
```

### GET /api/logs/{id}/tail

Stream logs for a daemon via **Server-Sent Events**. Each line is a server-sent event:

```bash
curl http://127.0.0.1:3120/api/logs/myproject/api/tail
```

**Response format (SSE):**

```
data: 2026-05-31 10:00:00 Hello from api daemon

data: 2026-05-31 10:00:02 Another log line

...
```

### GET /api/namespaces

List all registered namespaces.

```bash
curl http://127.0.0.1:3120/api/namespaces
```

### POST /api/namespaces

Register a namespace by directory.

```bash
curl -X POST http://127.0.0.1:3120/api/namespaces \
  -H "Content-Type: application/json" \
  -d '{"dir": "/home/user/new-project"}'
```

### DELETE /api/namespaces/{name}

Remove a namespace.

```bash
curl -X DELETE http://127.0.0.1:3120/api/namespaces/oldproject
```

### GET /api/proxies

List all configured proxy slugs.

```bash
curl http://127.0.0.1:3120/api/proxies
```

### GET /api/processes/{id}/tree

Get the process tree for a daemon, including all child processes.

```bash
curl http://127.0.0.1:3120/api/processes/myproject/api/tree
```

**Response:**

```json
[
  {
    "pid": 12345,
    "name": "node",
    "cmdline": "node server.js",
    "children": [
      {
        "pid": 12346,
        "name": "node",
        "cmdline": "node worker.js",
        "children": []
      }
    ]
  }
]
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
