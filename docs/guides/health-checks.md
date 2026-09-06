---
description: Detect unresponsive daemons with periodic HTTP, TCP, or command probes and recover using retries.
---
# Health checks

A process can stay alive after it stops doing useful work. Health checks probe
a daemon periodically and terminate it after consecutive failures. Add `retry`
if it should restart afterward.

[Ready checks](/guides/ready-checks) gate startup. Health checks continue for the
daemon's lifetime, including while it is still becoming ready.

## Check an HTTP endpoint

```toml
[daemons.api]
run = "node server.js"
ready_http = { url = "http://127.0.0.1:3000/health", timeout = "30s" }
health_http = { url = "http://127.0.0.1:3000/health", interval = "10s", timeout = "5s", retries = 3 }
retry = 3
```

The HTTP probe accepts any `2xx` response by default. Add `status = [200]` to
require an exact code. The shorthand is:

```toml
health_http = "http://127.0.0.1:3000/health"
```

## Use a command or TCP connection

An application-specific command can check more than an open port:

```toml
[daemons.redis]
run = "redis-server --port $PORT"
port = { expect = [6379], bump = 10 }
health_cmd = { run = "redis-cli -p $PORT ping", interval = "10s", timeout = "5s", retries = 3 }
retry = true
```

Exit code `0` is healthy. Commands receive the daemon's working directory,
environment, and resolved port variables.

For a simple listening check:

```toml
[daemons.cache]
run = "redis-server --port 6379"
health_port = { port = 6379, interval = "10s", timeout = "5s", retries = 3 }
retry = 3
```

`health_port = 6379` is the shorthand. TCP probes connect to `127.0.0.1` and only
verify that a connection is accepted. For a tunnel, use an end-to-end request
through the tunnel if you need to verify the remote service too.

## Tune the failure budget

| Field | Meaning | Default |
| --- | --- | --- |
| `interval` | Time between probes | `10s` |
| `timeout` | Maximum duration of one probe | Command: `10s`; HTTP/TCP: `5s` |
| `retries` | Consecutive failed probes before termination | `3` |

A successful probe resets the consecutive-failure counter. A restarted daemon
gets a fresh counter. If several probe kinds are configured, all are checked
and any failure makes the probe cycle unhealthy.

::: tip Allow for startup
Probing begins while the daemon is starting. Give slow services enough time by
increasing `interval` or `retries`; readiness does not pause health checks.
:::

The `retries` inside a health check counts **failed probes**. The daemon's
top-level `retry` counts **restart attempts**. Without `retry`, an unhealthy
daemon is stopped and marked errored but is not restarted.

Global defaults live under `[settings.supervisor]`: `health_check_interval`,
`health_check_retries`, `health_cmd_timeout`, `health_http_timeout`, and
`health_port_timeout`. See the [settings reference](/cli/configuration).

## Override a probe from the CLI

```sh
pitchfork start api --health-http http://127.0.0.1:3000/health
pitchfork restart database --health-cmd "pg_isready -h 127.0.0.1"
pitchfork run cache --retry 3 --health-port 6379 -- redis-server --port 6379
```

The flags accept shorthand values. Put intervals, timeouts, and failure thresholds
in `pitchfork.toml`. Health fields also support
[dependency templates](/guides/configuration-templates).

Use [logs](/guides/logs) to investigate failed probes and
[lifecycle hooks](/guides/lifecycle-hooks) to react to retries or exhausted attempts.
