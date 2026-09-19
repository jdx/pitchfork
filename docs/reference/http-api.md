---
description: Use pitchfork's HTTP API to inspect daemons, control services, stream logs, and manage namespaces.
---
# HTTP API

The API controls the same supervisor as the CLI. Enable the [web UI](/guides/web-ui)
or a [standalone API server](/guides/web-ui#standalone-api-server) before using these examples.
They assume the default web address, `http://127.0.0.1:3120`.

See [authentication](/guides/web-ui#authentication) when using a non-loopback address.
The [API JSON Schema](/api-schema.json) describes the response types.

::: warning Encrypt remote API access
The web UI and standalone API listeners serve plain HTTP. Direct non-loopback
requests send `X-Pitchfork-Token` unencrypted, allowing anyone who can observe
that traffic to capture the token and reuse it to control the supervisor.
Keep the listener bound to loopback (`127.0.0.1` or `::1`). For remote access,
place it behind an HTTPS reverse proxy on the same host and keep the HTTP
backend on loopback. See the [proxy authentication guidance](/guides/web-ui#authentication)
for token forwarding and proxies on another host.
:::

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

Start a daemon. If it is already running, the response includes `noop: true`.

```bash
curl -X POST http://127.0.0.1:3120/api/daemons/myproject%2Fapi/start
```

**Response:**

```json
{ "ok": true }
```

## POST /api/daemons/{id}/stop

Stop a running daemon. If it is already stopped, the response includes
`noop: true`. Clients can treat `noop: true` as a successful no-op even when
`ok` is false.

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

## GET /api/projects

List registered projects with worktree counts and daemon totals. All three
`/api/projects` endpoints are read-only: fetching or polling them never starts
a daemon.

If a linked worktree and its main checkout are both registered, the worktree
appears under the main checkout's project. It has no separate project endpoint;
`/api/projects/<linked-worktree-namespace>` returns HTTP 404. If only the linked
worktree is registered, it remains a separate project.

Project and worktree responses use these fields:

| Field | Meaning |
| --- | --- |
| `daemons` | Counts by status. `available` means configured but not yet tracked by the supervisor; `transitioning` includes waiting and stopping. |
| `last_activity` | RFC 3339 start time of the most recently started daemon still running, or `null` when none are running. |
| `dir_exists` | Whether the registered directory still exists. Deleted checkouts remain listed until their registration is removed. |
| `url`, `api_url` | Paths to the web page and API resource. |

```bash
curl http://127.0.0.1:3120/api/projects
```

**Response:**

```json
[
  {
    "name": "shop",
    "dir": "/home/user/shop",
    "dir_exists": true,
    "worktree_count": 1,
    "daemons": { "total": 1, "running": 1, "stopped": 0, "completed": 0, "transitioning": 0, "failed": 0, "available": 0 },
    "last_activity": "2026-05-31T10:00:00+02:00",
    "url": "/projects/shop",
    "api_url": "/api/projects/shop"
  }
]
```

## GET /api/projects/{project}

Return the project's worktrees, including those whose daemons have never
started. The optional `stack` field contains the primary checkout's stack.
Project names match case-insensitively; unknown names return HTTP 404.

Each worktree includes its namespace, group count, daemon counts, and page and
API paths. `can_start` is false when the directory is missing or the supervisor
cannot resolve configuration for a daemon shown in its stack. See the stack
endpoint below for details. `namespace` can be `null`, with `namespace_error`
explaining why it could not be derived. `disk_usage_bytes` is currently omitted
because pitchfork does not track daemon data directories.

```bash
curl http://127.0.0.1:3120/api/projects/shop
```

**Response excerpt** (the primary checkout's `stack` is omitted here):

```json
{
  "name": "shop",
  "dir": "/home/user/shop",
  "dir_exists": true,
  "daemons": { "total": 1, "running": 1, "stopped": 0, "completed": 0, "transitioning": 0, "failed": 0, "available": 0 },
  "last_activity": "2026-05-31T10:00:00+02:00",
  "worktrees": [
    {
      "name": "main",
      "branch": "main",
      "path": "/home/user/shop",
      "namespace": "shop",
      "is_primary": true,
      "can_start": true,
      "dir_exists": true,
      "group_count": 1,
      "daemons": { "total": 1, "running": 1, "stopped": 0, "completed": 0, "transitioning": 0, "failed": 0, "available": 0 },
      "last_activity": "2026-05-31T10:00:00+02:00",
      "url": "/projects/shop/main",
      "api_url": "/api/projects/shop/main"
    }
  ]
}
```

## GET /api/projects/{project}/{worktree}

Return the groups from the worktree's project configuration, their member
daemons, and ungrouped daemons. User and system config groups are excluded.
The `default` group comes first, followed by other groups in configuration order.

Project and worktree names match case-insensitively. The worktree segment
accepts its URL name, branch name, or directory name, in that order. URL names
use numeric suffixes to distinguish collisions, such as `feature-api` and
`feature-api-2`. Prefer the returned `api_url` when constructing requests.
Unknown projects or worktrees return HTTP 404.

| Field | Meaning |
| --- | --- |
| `groups[].daemons` | Full daemon objects, including members from other registered namespaces even if they have never started. |
| `groups[].missing` | Qualified IDs declared by the group that match no known daemon. |
| `groups[].total` | Number of declared members, including missing ones. |
| `ungrouped` | Daemons in this worktree's namespace that no group includes. |
| `daemons` | Counts for this worktree's namespace; members from other namespaces are excluded. |
| `can_start` | False if the directory is missing or `unresolvable_daemons` is nonempty. This is not a guarantee that every declared group member exists; also check `missing`. |
| `unresolvable_daemons` | IDs of listed daemons whose configuration the supervisor cannot resolve, including group members from other namespaces. |
| `config_error` | Present when the worktree's configuration could not be read. |

Register an unresolvable daemon's directory before starting or restarting it.
The supervisor loads configuration from its own project and the namespace
registry. Restart stops the daemon before looking up its configuration, so an
unresolvable daemon can be stopped without being brought back up. Stop does
not require configuration.

```bash
curl http://127.0.0.1:3120/api/projects/shop/main
```

**Response** (daemon objects abbreviated):

```json
{
  "project": "shop",
  "worktree": "main",
  "branch": "main",
  "namespace": "shop",
  "dir": "/home/user/shop",
  "is_primary": true,
  "can_start": true,
  "unresolvable_daemons": [],
  "dir_exists": true,
  "groups": [
    {
      "name": "default",
      "is_default": true,
      "daemons": [{ "id": { "qualified": "shop/api" }, "status": { "type": "running" } }],
      "missing": [],
      "running": 1,
      "total": 1
    }
  ],
  "ungrouped": [],
  "daemons": {
    "total": 1,
    "running": 1,
    "stopped": 0,
    "completed": 0,
    "transitioning": 0,
    "failed": 0,
    "available": 0
  },
  "url": "/projects/shop/main"
}
```

Each entry of `daemons` inside a group is a full daemon object, the same shape
`GET /api/daemons` returns; it is abbreviated above.

To control a group, POST to the daemon control endpoints with each member's
qualified ID, URL-encoded as a single path segment. There is no group mutation
endpoint.

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
    "exe": "/usr/local/bin/node",
    "cpu_percent": 2.3,
    "memory_bytes": 67108864,
    "virtual_memory_bytes": 268435456,
    "uptime_secs": 3600,
    "thread_count": 7,
    "status": "Sleep",
    "children": [
      {
        "pid": 12346,
        "name": "node",
        "exe": "/usr/local/bin/node",
        "cpu_percent": 0.5,
        "memory_bytes": 33554432,
        "virtual_memory_bytes": 134217728,
        "uptime_secs": 3590,
        "thread_count": 7,
        "status": "Sleep",
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
