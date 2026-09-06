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

For routes containing `{id}`, URL-encode the entire qualified daemon ID as one
path segment: `myproject/api` becomes `myproject%2Fapi`. Keep the unencoded
`namespace/name` form in JSON values. In JavaScript, use `encodeURIComponent(id)`
when constructing these URLs.

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
curl http://127.0.0.1:3120/api/daemons/myproject%2Fapi
```

Returns a single `ApiDaemonEntry` object (same shape as `/api/daemons` items).

## POST /api/daemons/{id}/start

Start a daemon.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject%2Fapi/start
```

**Response:**

```json
{ "ok": true }
```

## POST /api/daemons/{id}/stop

Stop a running daemon.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject%2Fapi/stop
```

## POST /api/daemons/{id}/restart

Restart a daemon.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject%2Fapi/restart
```

## POST /api/daemons/{id}/enable

Enable a daemon so it can be started.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject%2Fapi/enable
```

## POST /api/daemons/{id}/disable

Disable a daemon.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject%2Fapi/disable
```

## GET /api/logs/{id}/tail

Stream logs for a daemon as **newline-delimited JSON**
(`Content-Type: application/x-ndjson`). Each line is a JSON object. Use `curl -N`
to display entries as they arrive, then press `Ctrl+C` to stop following.

```bash
curl -N http://127.0.0.1:3120/api/logs/myproject%2Fapi/tail
```

**Response format (NDJSON):**

```jsonl
{"id":1,"timestamp":"2026-05-31 10:00:00","daemon_id":"myproject/api","message":"Hello from api daemon"}
{"id":2,"timestamp":"2026-05-31 10:00:02","daemon_id":"myproject/api","message":"Another log line"}
```

The stream can also emit a control object such as `{"_clear":true,"_gen":1}`
when the daemon's logs are cleared. Consumers should discard their buffered
history when they receive it.

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
curl http://127.0.0.1:3120/api/processes/myproject%2Fapi/tree
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

Inspect the HTTP status and response body when a request fails. Use URL-encoded
qualified IDs (`namespace%2Fname`) in daemon URLs. Control requests can return
HTTP 200 with `"ok": false` and an `"error"` message, so check the response body
as well. Investigate missing daemons, invalid configuration, or failed startup
through the daemon's status and logs.
