---
description: Schedule recurring tasks with six-field cron expressions, local time, and explicit overlap policies.
---
# Cron scheduling

Run a command on a schedule by adding `cron` to its daemon configuration:

```toml
[daemons.backup]
run = "./scripts/backup.sh"
cron = "0 0 2 * * *"
```

This runs daily at **02:00 in the supervisor's local time zone**. Keep the
supervisor running for schedules to fire. It discovers cron daemons from known
configuration, including ones that have not been manually started.

## Expression format

Pitchfork uses six fields, starting with **seconds**. A seventh year field is
optional. This differs from the five-field format commonly used by `crontab`.

```text
second  minute  hour  day-of-month  month  day-of-week  [year]
0       30      9     *             *      MON-FRI
```

| Schedule | Expression |
| --- | --- |
| Every hour | `0 0 * * * *` |
| Every five minutes | `0 */5 * * * *` |
| Daily at 02:00 | `0 0 2 * * *` |
| Sunday at midnight | `0 0 0 * * SUN` |
| Weekdays at 09:30 | `0 30 9 * * MON-FRI` |

Use weekday names for clarity. Numeric weekdays are `1` (Sunday) through `7`
(Saturday), not `0` through `6`.

The supervisor checks schedules every `10s` by default
(`supervisor.cron_check_interval`). A due run starts on a check, so scheduling
is not a guarantee of execution at the exact second.

## Decide what happens to the previous run

```toml
[daemons.backup]
run = "./scripts/backup.sh"
cron = { schedule = "0 0 2 * * *", retrigger = "finish" }
```

| `retrigger` | When a scheduled time is reached |
| --- | --- |
| `finish` (default) | Run only if the previous execution has finished |
| `always` | Stop any active execution and start again |
| `success` | Run if the previous execution finished successfully |
| `fail` | Run if the previous execution failed |

`success` and `fail` both allow the first execution. After that, the previous
result decides whether another run is eligible. These modes do not create
overlapping copies of the same daemon.

## Startup behavior

Starting or discovering a cron daemon registers its schedule; it does not
normally run the command immediately.

```sh
pitchfork start backup
pitchfork logs backup --tail
```

`immediate = true` adds a ten-second lookback on the first schedule check:

```toml
cron = { schedule = "0 0 2 * * *", immediate = true }
```

This catches a scheduled time that just passed. It does **not** mean “run now
regardless of the schedule.” For a manual execution, use a separate one-off
command, such as `pitchfork run backup-now -- ./scripts/backup.sh`.

## Inspect schedule timing

Use `pitchfork status` to see when a scheduled daemon last ran and when it is
next due, even when no process is running:

```sh
pitchfork status backup
```

The schedule-related fields look like this (example timestamps):

```text
Cron: 0 0 2 * * *
Last run: 2026-09-21 02:00:03 (7h 41m ago)
Next run: 2026-09-22 02:00:00 (in 16h 18m)
```

Timestamps are displayed in local time. The TUI detail pane also shows
last-run and next-run timing for registered cron daemons.

### Last run and daemon status

`Last run` records the most recent scheduled execution that started a process.
Manual starts, skipped executions, and failed attempts to start a process do
not update it. It reads `never` until a scheduled start has been recorded;
runs from before upgrading to a version that records this field are not
reconstructed.

`Status` describes the daemon's current state or most recent outcome, such as
`running`, `completed`, or `failed`. It can reflect a later manual execution,
so it is not necessarily the outcome of the execution shown in `Last run`.
Use `pitchfork logs backup` to inspect the command's output.

### Next run and overdue schedules

`Next run` is the next scheduled time the supervisor will evaluate. The
supervisor must be running and the `retrigger` policy must allow execution
for a process to start.

A past due time is labeled `overdue`:

```text
Next run: 2026-09-21 02:00:00 (2h 0m overdue)
```

This can happen when the supervisor was stopped during a scheduled time, or
while a due time is waiting for the next schedule check. It does not mean the
command ran at that time. The supervisor evaluates the overdue schedule on
its next check, subject to the same execution rules.

### Read timing as JSON

```sh
pitchfork status backup --json
```

| Field | Value |
| --- | --- |
| `cron_schedule` | Cron expression |
| `cron_last_run` | Most recent recorded scheduled start, in RFC 3339 format |
| `cron_next_run` | Next scheduled time to evaluate, in RFC 3339 format; may be in the past |

CLI JSON omits fields when their values are unavailable, including
`cron_last_run` before the first recorded scheduled start. Daemon entries in
the [HTTP API](/reference/http-api) expose the same fields, with `null` for
unavailable values.

## Pause a schedule

```sh
pitchfork disable backup
pitchfork enable backup
```

Use `disable` to prevent future scheduled starts. Stopping a process alone does
not remove its schedule. Use `pitchfork list` and `pitchfork logs backup` to
inspect status and output.

## Tools outside an interactive shell

Scheduled services may run without your shell's tool setup. Enable
[mise integration](/guides/mise-integration) when they need mise-managed tools:

```toml
[daemons.backup]
run = "node scripts/backup.js"
cron = "0 0 2 * * *"
mise = true
```

For a supervisor that starts at login or boot, see [boot registration](/guides/boot-start).
