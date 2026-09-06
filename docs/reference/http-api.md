---
description: Use pitchfork's HTTP API to inspect daemons, control services, stream logs, and manage namespaces.
---
# HTTP API

The API controls the same supervisor as the CLI. Enable the [web UI](/guides/web-ui)
or a [standalone API server](/guides/web-ui#standalone-api-server) before using these examples.
They assume the default web address, `http://127.0.0.1:3120`.

See [authentication](/guides/web-ui#authentication) when using a non-loopback address.
The [API JSON Schema](/api-schema.json) describes the response types.

The following REST endpoints are available on the web UI port (or the dedicated API port if configured). All endpoints accept and return JSON unless otherwise noted.

## GET /api/stats

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

## GET /api/daemons

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

## GET /api/daemons/{id}

Get a single daemon by qualified ID.

```bash
curl http://127.0.0.1:3120/api/daemons/myproject/api
```

Returns a single `ApiDaemonEntry` object (same shape as `/api/daemons` items).

## POST /api/daemons/{id}/start

Start a daemon.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject/api/start
```

**Response:**

```json
{ "ok": true, "error": null }
```

## POST /api/daemons/{id}/stop

Stop a running daemon.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject/api/stop
```

## POST /api/daemons/{id}/restart

Restart a daemon.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject/api/restart
```

## POST /api/daemons/{id}/enable

Enable a daemon so it can be started.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject/api/enable
```

## POST /api/daemons/{id}/disable

Disable a daemon.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject/api/disable
```

## GET /api/logs/{id}/tail

Stream logs for a daemon via **Server-Sent Events**. Each line is a server-sent event:

```bash
curl http://127.0.0.1:3120/api/logs/myproject/api/tail
```

**Response format (SSE):**

```text
data: 2026-05-31 10:00:00 Hello from api daemon

data: 2026-05-31 10:00:02 Another log line

...
```

## GET /api/namespaces

List all registered namespaces.

```bash
curl http://127.0.0.1:3120/api/namespaces
```

## POST /api/namespaces

Register a namespace by directory.

```bash
curl -X POST http://127.0.0.1:3120/api/namespaces \
  -H "Content-Type: application/json" \
  -d '{"dir": "/home/user/new-project"}'
```

## DELETE /api/namespaces/{name}

Remove a namespace.

```bash
curl -X DELETE http://127.0.0.1:3120/api/namespaces/oldproject
```

## GET /api/proxies

List all configured proxy slugs.

```bash
curl http://127.0.0.1:3120/api/proxies
```

## GET /api/processes/{id}/tree

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



## Request failures

Inspect the HTTP status and response body when a request fails. Use qualified IDs
(`namespace/name`) in daemon URLs. A missing daemon, invalid configuration, or
failed startup should be investigated through its status and logs.
