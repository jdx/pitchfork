---
description: Wait for output, HTTP, TCP, or a command before marking a daemon ready and starting its dependents.
---
# Ready checks

`pitchfork start` and `pitchfork run` wait for readiness before returning.
Dependent daemons wait for the same signal. Pick a check that shows the service
can do useful work, such as an HTTP health endpoint or a database query.

## Choose a check

| Check | Ready when… | Use it for… |
| --- | --- | --- |
| `ready_http` | An endpoint returns an accepted status | APIs and web servers |
| `ready_cmd` | A shell command exits with code `0` | Database clients or custom probes |
| `ready_port` | A TCP connection succeeds on `127.0.0.1` | Services without an application-level probe |
| `ready_output` | A regex matches stdout or stderr | Services with a reliable startup message |
| `ready_delay` | The process stays running for a fixed delay | A fallback when no other check is available |

::: tip More than one check means “any,” not “all”
The first successful output, HTTP, TCP, or command check marks the daemon ready.
Use one `ready_cmd` that combines conditions if you need all of them to pass.
`ready_delay` only applies when no other readiness check is configured.
:::

## HTTP endpoint

```toml
[daemons.api]
run = "node server.js"
ready_http = { url = "http://127.0.0.1:3000/health", timeout = "30s" }
```

The string form, `ready_http = "http://127.0.0.1:3000/health"`, accepts any
`2xx` status and has no overall deadline. To accept specific status codes:

```toml
ready_http = { url = "http://127.0.0.1:3000/health", status = [200, 401], timeout = "30s" }
```

The default polling interval is `500ms`; the default per-request timeout is
`5s`. The object's `timeout` caps the overall polling period.

```sh
pitchfork start api --http http://127.0.0.1:3000/health
```

## Shell command

Use an application client when a listening socket alone is not enough:

```toml
[daemons.redis]
run = "redis-server --port $PORT"
port = { expect = [6379], bump = 10 }
ready_cmd = { run = "redis-cli -p $PORT ping", timeout = "30s" }
```

The command runs in the daemon's working directory and receives its environment,
including the resolved `$PORT`, `$PORT0`, `$PORT1`, and pitchfork metadata.
This makes command checks useful with [port bumping](/guides/port-management).
The default polling interval is `500ms`.

```sh
pitchfork start database --cmd "pg_isready -h 127.0.0.1"
```

## TCP port

```toml
[daemons.web]
run = "python3 -u -m http.server 8000 --bind 127.0.0.1"
ready_port = { port = 8000, timeout = "15s" }
```

The shorthand is `ready_port = 8000`. The check attempts a TCP connection to
`127.0.0.1:8000` every `500ms` by default. A successful connection proves that
something is listening, not that the application is healthy.

```sh
pitchfork run web --port 8000 -- python3 -u -m http.server 8000 --bind 127.0.0.1
```

`--port` is a readiness check. Port assignment uses the separate `port` config
field or `--expected-port` CLI flag; the service must actually use the assigned port.

## Output pattern

```toml
[daemons.postgres]
run = "postgres -D /path/to/initialized/data"
ready_output = { pattern = "database system is ready to accept connections", timeout = "30s" }
```

The shorthand accepts a regex string:

```toml
ready_output = "database system is ready to accept connections"
```

Match the actual startup output. If a service buffers output when run in the
background, enable unbuffered output (for example, Python's `-u`) or use a
network check instead.

```sh
pitchfork start database --output "ready to accept connections"
```

## Delay fallback

With no other check, pitchfork waits **three seconds** by default:

```toml
[daemons.worker]
run = "./worker"
ready_delay = 5
```

The daemon field is an integer number of seconds. The global default is a
duration string, which must resolve to whole seconds:

```toml
[settings.general]
ready_delay = "5s"
```

Use `pitchfork start worker --delay 5` for a one-time override. Raising the delay
does not extend the timeout of an HTTP, TCP, output, or command check.

## Timeouts and failures

The object forms of `ready_output`, `ready_http`, `ready_port`, and `ready_cmd`
accept an overall `timeout`. Without it, a check can keep waiting while the
process remains alive.

If every configured check reaches its deadline, startup fails with exit code
`124`, the daemon is killed, and normal `ready_retry` and dependency handling
applies.
One unbounded check keeps startup open. If the process exits with a nonzero code
before readiness, startup returns that code.

## Templates and dependencies

Ready checks can use [templates](/guides/configuration-templates) to reference
ports from dependencies started earlier. `ready_port` templates must render to
a port number from `1` to `65535`. For the current daemon's dynamically assigned
port, use `$PORT` in `ready_cmd`.

## Health checks {#health-checks}

Readiness gates startup. [Health checks](/guides/health-checks) keep probing
throughout a daemon's lifetime and can trigger a restart if it stops responding.
