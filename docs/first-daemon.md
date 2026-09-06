---
description: Define project services in pitchfork.toml, start dependencies in order, and manage their lifecycle.
---
# Your first project

Put your project's background commands in `pitchfork.toml` so everyone can start
the same services. This example uses **Redis**, **Node.js**, and an existing
`server.js` that reads `PORT` and `REDIS_URL` and serves `/health`.
For an example that only needs Python, start with the [quickstart](/quickstart).

## Define the services

Create `pitchfork.toml` in the project root:

```toml
#:schema https://pitchfork.jdx.dev/schema.json

[daemons.redis]
run = "redis-server --port $PORT"
port = 6379
ready_cmd = { run = "redis-cli -p $PORT ping", timeout = "15s" }

[daemons.api]
run = "node server.js"
port = 3000
depends = ["redis"]
env = { REDIS_URL = "redis://127.0.0.1:{{ daemons.redis.port }}" }
ready_http = { url = "http://127.0.0.1:3000/health", timeout = "30s" }
retry = 3
```

Replace the API command and health endpoint with your project's equivalents.
Commands run from the config's project directory. Use [`dir`](/reference/configuration#dir)
for a service in a subdirectory.

`depends` starts Redis before the API. Redis must answer its readiness command
before the API starts; the API must pass its HTTP check before startup completes.
The environment template uses Redis's resolved port.

::: tip Keep processes in the foreground
Use the foreground command for each service. Avoid `&`, `--daemonize`, or
`docker run -d`: pitchfork needs to track the process that does the work.
:::

## Start the project

```sh
pitchfork start api
```

This starts Redis too. Run it again and already running services stay running.
Independent dependencies start in parallel.

```sh
pitchfork start --local   # All daemons in the merged local config
pitchfork list --project  # Only this project's namespace
pitchfork status api      # Details for one daemon
```

`--local` includes local configs inherited from parent directories. `--all`
also includes system and user daemons. Use explicit names or a
[group](/reference/configuration#daemon-groups) when you want a fixed set.

## Inspect and restart

```sh
pitchfork logs api redis --tail
```

Press `Ctrl+C` to stop following logs. To apply a changed command or environment:

```sh
pitchfork restart api
```

Only the requested daemon restarts; its already running dependencies stay up.
For automatic restarts after source edits, add [file watching](/guides/file-watching).

## Stop the project

```sh
pitchfork stop --local
```

Pitchfork stops dependents before their dependencies. Configured services remain
available for the next `pitchfork start`.

## Start and stop with your shell

Install the [shell hook](/guides/shell-hook), then add this field to each daemon
that should follow your project sessions:

```toml
auto = ["start", "stop"]
```

Services start when you enter the project. Once the last tracked session leaves,
they become eligible to stop after the configured delay (one minute by default).

## Keep going

- [Ready checks](/guides/ready-checks): choose the right startup signal.
- [Health checks](/guides/health-checks): detect a running service that stops responding.
- [Namespaces](/concepts/namespaces): use the same service names across projects and worktrees.
- [Configuration reference](/reference/configuration): look up every daemon option.
