# Log Management

View, filter, and manage daemon logs. Pitchfork stores all daemon logs in an SQLite database (`~/.local/state/pitchfork/logs/logs.db`) with full timestamp indexing, making filtering by time fast and reliable.

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

## Log Rotation

Pitchfork supports automatic log rotation via `time_retention` and `line_retention` settings. Old entries are pruned periodically by the supervisor so the database does not grow unbounded.

### Automatic Rotation

Configure in any `pitchfork.toml` under `[settings.logs]`:

```toml
[settings.logs]
# Keep only the last 7 days of logs
time_retention = "7d"

# Or keep only the most recent 10,000 entries
line_retention = 10000

# You can also combine both (entries older than 7d OR exceeding 10,000 lines are pruned)
# time_retention = "7d"
# line_retention = 10000
```

Supported formats:
- **Time-based (`time_retention`):** `"7d"`, `"30d"`, `"1h"` — delete entries older than this duration
- **Count-based (`line_retention`):** `10000`, `5000` — keep only the most recent N entries per daemon
- **Unset (default):** no automatic pruning

The supervisor evaluates this policy during its regular interval watcher cycle.

## Migrate Legacy Logs

If you were using pitchfork before the SQLite log store was introduced, legacy text log files may still exist under the logs directory. Import them into the SQLite database with:

```bash
pitchfork logs --migrate
```

This is a one-time operation. After migration, the legacy text files are no longer used.

## Supervisor Logs

View pitchfork's own logs:

```bash
pitchfork logs pitchfork
```

## TUI and Web UI

You can also view logs in real-time through the [TUI](/guides/tui) (`pitchfork tui`) or [Web UI](/guides/web-ui) (if enabled).

## Log Storage Location

Logs are stored in a single SQLite database at `~/.local/state/pitchfork/logs/logs.db`. Each daemon has its own table partition identified by its qualified ID (`namespace/name`). See [File Locations](/reference/file-locations#logs) for details on the state directory resolution.
