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

## Structured Log Parsing

Pitchfork can automatically parse structured logs produced by your daemons. When a log line is written in JSON or logfmt format, pitchfork extracts fields such as `level`, `msg`, and `logger` and stores them alongside the original message. This makes it possible to filter by log level, query individual fields, and pipe output through jq expressions.

<div class="structured-logs-screenshot">
  <img src="/img/structured-logs.png" alt="Structured log output with level badges, logger names, and highlighted fields" />
</div>

<style scoped>
.structured-logs-screenshot {
  margin: 1.5rem 0;
  text-align: center;
}
.structured-logs-screenshot img {
  height: auto;
  width: 100%;
  max-width: 880px;
  min-width: 0;
  display: block;
  margin: 0 auto;
  border-radius: 8px;
  border: 1px solid var(--vp-c-divider);
}
</style>

### Configure Log Format

Log parsing can be configured per daemon or applied globally as a default.

Per-daemon configuration:

```toml
[daemons.api]
run = "node server.js"

[daemons.api.logs]
log_format = "json"  # json | logfmt | text
```

Global default in `[settings.logs]`:

```toml
[settings.logs]
log_format = "json"  # json | logfmt | text (default: text)
```

| Format | Description |
|---|---|
| `json` | Parse as single-line JSON (NDJSON) |
| `logfmt` | Parse as `key=value` space-delimited pairs |
| `text` | No parsing, store as plain text (default) |

### Filter by Level

```bash
pitchfork logs api --level error
pitchfork logs api --level warn
```

Level values are normalized automatically. For example, `fatal`, `critical`, `panic`, and `err` all match `error`, while `warning` matches `warn`.

### Filter by Field

```bash
pitchfork logs api --field request_id=abc123
pitchfork logs api --field status=500 --field method=GET
```

Field queries use SQLite `json_extract` on the stored `fields_json` column. Multiple `--field` flags are combined with AND.

### jq Filtering

```bash
pitchfork logs api --jq '.level == "error" and .status >= 500'
pitchfork logs api --jq '.request_id | startswith("req_00")'
```

Each log entry is serialized into a JSON object with `timestamp`, `daemon_id`, `message`, `level`, `msg`, `logger`, and `fields`. The jq expression is evaluated against each object; entries that return a truthy value are kept.

Pitchfork ships with [jaq](https://github.com/01mf02/jaq), a pure-Rust jq implementation, so no external jq binary is required.

### JSON Output

```bash
pitchfork logs api --json
```

This outputs a JSON array with structured fields:

```json
[
  {
    "timestamp": "2025-07-08 12:00:00",
    "daemon_id": "global/api",
    "message": "{\"level\":\"info\",\"msg\":\"started\"}",
    "level": "info",
    "msg": "started",
    "logger": "main",
    "fields": { "port": 8080 }
  }
]
```

The `level`, `msg`, `logger`, and `fields` fields are only present when the log line was successfully parsed.

### Composing Filters

`--level`, `--field`, `--grep`, and `--regex` are applied at the SQL layer to narrow the candidate set first. `--jq` then filters the remaining entries in the application layer:

```bash
# SQL layer filters level=error, then jq filters status>=500
pitchfork logs api --level error --jq '.status >= 500'

# --grep and --field can be combined
pitchfork logs api --grep "timeout" --field service=api
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

If you were using pitchfork before the SQLite log store was introduced, legacy text log files may still exist under the logs directory. They are automatically imported into the SQLite database on the first access to the log store, so no manual action is required.

## Supervisor Logs

View pitchfork's own logs:

```bash
pitchfork logs pitchfork
```

## TUI and Web UI

You can also view logs in real-time through the [TUI](/guides/tui) (`pitchfork tui`) or [Web UI](/guides/web-ui) (if enabled).

## Log Storage Location

Logs are stored in a single SQLite database at `~/.local/state/pitchfork/logs/logs.db`. Each daemon has its own table partition identified by its qualified ID (`namespace/name`). See [File Locations](/reference/file-locations#logs) for details on the state directory resolution.

## Performance

Structured logs are parsed once at ingestion time and stored as indexed columns (`level`, `msg`, `logger`, `fields_json`) in SQLite. Queries read these columns directly without re-parsing the original log line.

Benchmarked against [hl](https://github.com/pamburus/hl) (a multi-core terminal log viewer that parses JSON on every read) on 100,000 JSON log lines (~300 bytes each, 8 fields):

| Scenario | pitchfork (single-core) | hl (single-core) | hl (16-core) |
|---|---|---|---|
| Full retrieval | 135ms | 117ms | 20ms |
| Level filter (`--level error`) | 40ms | 75ms | 15ms |

On a single core, pitchfork is competitive on full retrieval and significantly faster on filtered queries thanks to SQLite's `idx_daemon_level_ts` index. hl's multi-core advantage comes from its striped reader/worker/writer pipeline, which is suited for one-off streaming inspection rather than persistent storage and repeated queries.
