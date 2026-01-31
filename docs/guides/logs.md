# Log Management

View, filter, and manage daemon logs.

## View Logs

View logs for a daemon:

```bash
pitchfork logs api
```

In interactive terminals, logs automatically use a pager (like `less`) when output exceeds the terminal height. The pager starts at the end of the logs for easy viewing of recent entries.

## Tail Logs

Follow logs in real-time:

```bash
pitchfork logs api --tail
# or use --follow, -t, -f
```

Press `Ctrl+C` to stop following.

## Multiple Daemons

View logs from multiple daemons at once:

```bash
pitchfork logs api worker database
```

Logs are interleaved with timestamps to show the correct order.

## Filter by Line Count

Limit the number of lines shown:

```bash
# Last 50 lines
pitchfork logs api -n 50

# Last 10 lines
pitchfork logs api -n 10
```

When combined with time filters, `-n` limits the output from the filtered results.

## Filter by Time

Show logs from a specific time range using `--since` (or `-s`) and `--until` (or `-u`):

### Relative Time

```bash
# Logs from last 5 minutes
pitchfork logs api --since 5min

# Logs from last 2 hours
pitchfork logs api --since 2h

# Logs from last day
pitchfork logs api --since 1d
```

### Time Only (Today's Date)

```bash
# Logs since 10:30 AM today
pitchfork logs api --since 10:30

# Logs since 14:30:00 today
pitchfork logs api --since 14:30:00
```

### Full Datetime

```bash
# Logs since a specific datetime
pitchfork logs api --since "2024-01-15 09:00:00"

# Logs until a specific datetime
pitchfork logs api --until "2024-01-15 17:00:00"

# Logs within a time range
pitchfork logs api --since "2024-01-15 09:00" --until "2024-01-15 12:00"
```

### Combining with Line Limit

```bash
# Last 20 lines from the past hour
pitchfork logs api --since 1h -n 20
```

## Raw Output

Output raw log lines without color or formatting:

```bash
pitchfork logs api --raw
```

Useful for:
- Piping to other tools: `pitchfork logs api --raw | grep ERROR`
- Saving to files: `pitchfork logs api --raw > api.log`
- Processing with scripts

## Disable Pager

Disable the automatic pager in interactive terminals:

```bash
pitchfork logs api --no-pager
```

This forces direct output to stdout, even when output would normally trigger the pager.

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
