# Log Management

View, filter, and manage daemon logs.

## View Logs

See recent logs for a daemon:

```bash
pitchfork logs api
```

By default, shows the last 100 lines.

## Tail Logs

Follow logs in real-time:

```bash
pitchfork logs api --tail
```

Press `Ctrl+C` to stop following.

## Multiple Daemons

View logs from multiple daemons at once:

```bash
pitchfork logs api worker database
```

Logs are interleaved with timestamps to show the correct order.

## Filter by Line Count

Show more or fewer lines:

```bash
# Last 50 lines
pitchfork logs api -n 50

# All logs
pitchfork logs api -n 0
```

## Filter by Time

Show logs from a specific time range:

```bash
# Logs since a specific time
pitchfork logs api --from "2024-01-15 09:00:00"

# Logs until a specific time
pitchfork logs api --to "2024-01-15 17:00:00"

# Logs within a time range
pitchfork logs api --from "2024-01-15 09:00:00" --to "2024-01-15 12:00:00"
```

## Clear Logs

Delete all logs for a daemon:

```bash
pitchfork logs api --clear
```

## Supervisor Logs

View pitchfork's own logs:

```bash
pitchfork logs pitchfork
```

## Log Location

Logs are stored in `~/.local/state/pitchfork/logs/<daemon-name>/`.

Each daemon has its own log file that persists across restarts.

## TUI and Web UI

You can also view logs in real-time through the [TUI](/guides/tui) (`pitchfork tui`) or [Web UI](/guides/web-ui) (if enabled).
