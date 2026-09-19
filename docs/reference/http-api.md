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

List registered projects with their worktree counts and daemon totals. These
project endpoints are read-only: fetching or polling them never starts a daemon.

A linked worktree belongs to its main checkout's project when both are
registered. It is not listed as a separate project, and requests using its
namespace as `{project}` return HTTP 404. If only the linked worktree is
registered, its project contains that worktree alone.

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
    "daemons": { "total": 2, "running": 1, "stopped": 1, "completed": 0, "transitioning": 0, "failed": 0, "available": 0 },
    "last_activity": "2026-05-31T10:00:00+02:00",
    "url": "/projects/shop",
    "api_url": "/api/projects/shop"
  }
]
```

Fields shared by project and worktree summaries:

| Field | Meaning |
| --- | --- |
| `daemons` | Counts by state. `available` means configured but not yet tracked by the supervisor; `transitioning` includes waiting and stopping. Project totals cover its worktree namespaces. |
| `last_activity` | Start time of the most recently started daemon still running, in RFC 3339 format. `null` when none are running; this is not a history of past activity. |
| `dir_exists` | Whether the directory exists. Deleted checkouts remain listed until their namespace registrations are removed. |
| `url`, `api_url` | Paths to the corresponding web page and API resource. |

## GET /api/projects/{project}

Return a project's worktrees, including those whose daemons have never run.
The optional `stack` field contains the primary checkout's stack in the format returned
by `GET /api/projects/{project}/{worktree}`.
Project names match case-insensitively; unknown projects return HTTP 404.

```bash
curl http://127.0.0.1:3120/api/projects/shop
```

**Response (the nested `stack` object is abbreviated):**

```json
{
  "name": "shop",
  "dir": "/home/user/shop",
  "dir_exists": true,
  "daemons": { "total": 2, "running": 1, "stopped": 1, "completed": 0, "transitioning": 0, "failed": 0, "available": 0 },
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
      "daemons": { "total": 2, "running": 1, "stopped": 1, "completed": 0, "transitioning": 0, "failed": 0, "available": 0 },
      "last_activity": "2026-05-31T10:00:00+02:00",
      "url": "/projects/shop/main",
      "api_url": "/api/projects/shop/main"
    }
  ],
  "stack": { "project": "shop", "worktree": "main" }
}
```

`can_start` is false if the worktree directory is missing or the supervisor
cannot resolve configuration for a daemon shown in its stack. See the stack
fields below for details. `disk_usage_bytes` is currently omitted because
pitchfork does not track daemon data directories.

## GET /api/projects/{project}/{worktree}

Return a worktree's groups and their daemon state. Groups come from the
worktree's project configuration chain, excluding user and system configs.
The `default` group appears first; other groups retain configuration order.

Both path segments match case-insensitively. The worktree segment accepts its
URL name, branch name, or directory name, in that order of precedence. Colliding
URL names receive numeric suffixes, such as `feature-api-2`. Use the `api_url`
from the worktree summary to avoid ambiguous names. Unknown projects or
worktrees return HTTP 404.

```bash
curl http://127.0.0.1:3120/api/projects/shop/main
```

**Response (daemon objects are abbreviated):**

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
      "daemons": [
        { "id": { "qualified": "shop/api" }, "status": { "type": "running" } },
        { "id": { "qualified": "shop/worker" }, "status": { "type": "stopped" } }
      ],
      "missing": [],
      "running": 1,
      "total": 2
    }
  ],
  "ungrouped": [],
  "daemons": { "total": 2, "running": 1, "stopped": 1, "completed": 0, "transitioning": 0, "failed": 0, "available": 0 },
  "url": "/projects/shop/main"
}
```

| Field | Meaning |
| --- | --- |
| `groups[].daemons` | Known group members, each with the same full object shape as an item from `GET /api/daemons`. |
| `groups[].missing` | Qualified IDs declared in the group that match no known daemon. |
| `groups[].total` | Number of declared members, including missing members. |
| `ungrouped` | Daemons in this worktree's namespace that no group lists, as full daemon objects. |
| `daemons` | Counts for this worktree's namespace. Members borrowed from other namespaces contribute to their group's counts only. |
| `can_start` | Whether the directory exists and `unresolvable_daemons` is empty. This does not guarantee that every declared member exists or that a start will succeed. |
| `unresolvable_daemons` | Known daemons shown in the stack whose configuration the supervisor cannot resolve, including members from other namespaces. |
| `namespace`, `namespace_error` | The worktree's namespace, or `null` with an explanation in `namespace_error` if one cannot be derived. |
| `config_error` | Present when the worktree's configuration could not be read; its groups are unavailable. |

The supervisor resolves configurations from its own project and the namespace
registry. Register or restore the relevant directory before starting or
restarting an unresolvable daemon. Restart stops a daemon before looking up
its configuration, so it can leave the daemon stopped if that lookup fails.
Stop does not require configuration.

To control a group, call the daemon start, stop, or restart endpoint for each
member's qualified ID, URL-encoded as a single path segment. The web UI stops
members in reverse declaration order. There is no group mutation endpoint.
A control response with `noop: true` means the daemon was already in the
requested state, even though `ok` is false. Group actions can treat that member
as already handled.

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
