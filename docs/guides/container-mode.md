# Container Mode <Badge type="warning" text="Experimental" />

::: warning Experimental Feature
Container mode is an **experimental** feature. Its behavior may change in future releases, and it is not guaranteed to work in all environments. Use at your own risk.
:::

Run pitchfork as PID 1 inside Docker containers, handling zombie reaping and signal forwarding.

## The Problem

When running inside a Docker container, the entrypoint process becomes **PID 1**. Unlike a normal init system, PID 1 has special responsibilities:

- **Zombie reaping** — Orphaned child processes are re-parented to PID 1. Without explicit reaping, they accumulate as zombies in the process table indefinitely.
- **Signal forwarding** — The Linux kernel does not deliver default signal handlers to PID 1. Signals like `SIGTERM` must be explicitly caught and handled.

If you run pitchfork as a container entrypoint without container mode, orphaned processes from your daemons may pile up as zombies, and `docker stop` may not shut down gracefully.

## Enabling Container Mode

There are three ways to enable container mode:

### CLI Flag

```bash
pitchfork supervisor run --container
```

### Environment Variable

```bash
PITCHFORK_CONTAINER=true pitchfork supervisor run
```

### Settings Configuration

```toml
[settings.supervisor]
container = true
```

The CLI flag and environment variable take priority over the settings file.

## What Container Mode Does

When container mode is enabled, pitchfork:

1. **Installs a SIGCHLD handler** to reap all orphaned/zombie child processes. Only processes that are _not_ managed by the supervisor are reaped by this handler — managed daemons are reaped by their own monitoring tasks.

2. **Routes SIGTERM/SIGINT through the graceful shutdown sequence**, ensuring all daemons are stopped cleanly before the container exits.

## Example Dockerfile

```dockerfile
FROM debian:bookworm-slim

# Install pitchfork
COPY --from=ghcr.io/jdx/pitchfork:latest /usr/local/bin/pitchfork /usr/local/bin/pitchfork

# Copy your configuration
COPY pitchfork.toml /app/pitchfork.toml
WORKDIR /app

# Run pitchfork as PID 1 in container mode
ENTRYPOINT ["pitchfork", "supervisor", "run", "--container"]
```

## Example Configuration

A typical `pitchfork.toml` for container use:

```toml
[settings.supervisor]
container = true

[daemons.api]
run = "node server.js"
ready_http = "http://localhost:3000/health"
retry = true

[daemons.worker]
run = "python worker.py"
depends = ["api"]
retry = true
```
