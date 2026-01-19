# Your First Project

This tutorial walks through setting up a project with multiple daemons managed by pitchfork.

## Create a Configuration File

In your project root, create `pitchfork.toml`:

```toml
[daemons.api]
run = "npm run server:api"

[daemons.docs]
run = "npm run server:docs"

[daemons.redis]
run = "redis-server"
```

This defines three daemons: an API server, a docs server, and Redis.

## Start Your Daemons

Start all daemons at once:

```bash
pitchfork start --all
```

Or start specific ones:

```bash
pitchfork start api redis
```

## Check Status

View all running daemons:

```bash
pitchfork list
```

Output:

```
NAME   PID    STATUS
api    12345  running
docs   12346  running
redis  12347  running
```

Get detailed status for one daemon:

```bash
pitchfork status api
```

## View Logs

See logs for a specific daemon:

```bash
pitchfork logs api
```

Follow logs in real-time:

```bash
pitchfork logs api --tail
```

View logs for multiple daemons:

```bash
pitchfork logs api docs
```

## Stop Daemons

Stop a specific daemon:

```bash
pitchfork stop api
```

Stop all daemons:

```bash
pitchfork stop api docs redis
```

## Restart on Changes

If a daemon is already running, `pitchfork start` does nothing. Use `--force` to restart:

```bash
pitchfork start api --force
```

## Add Ready Checks

Make pitchfork wait until your daemon is actually ready:

```toml
[daemons.api]
run = "npm run server:api"
ready_http = "http://localhost:3000/health"

[daemons.redis]
run = "redis-server"
ready_output = "Ready to accept connections"
```

See [Ready Checks](/guides/ready-checks) for all options.

## Enable Auto-Restart

Have pitchfork automatically restart daemons that crash:

```toml
[daemons.api]
run = "npm run server:api"
retry = 3  # Restart up to 3 times on failure
```

See [Auto Restart](/guides/auto-restart) for details.

## What's Next?

- [Shell Hook](/guides/shell-hook) - Auto-start when entering project directories
- [Ready Checks](/guides/ready-checks) - Configure readiness detection
- [Configuration Reference](/reference/configuration) - All configuration options
