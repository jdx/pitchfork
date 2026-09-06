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
