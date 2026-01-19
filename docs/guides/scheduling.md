# Cron Scheduling

Run daemons on a schedule using cron expressions.

## Basic Configuration

Add a `cron` section to your daemon:

```toml
[daemons.backup]
run = "./scripts/backup.sh"
cron = { schedule = "0 0 2 * * *", retrigger = "finish" }  # Run at 2 AM daily
```

## Cron Expression Format

Uses standard 6-field cron format:

```
┌──────────── second (0-59)
│ ┌────────── minute (0-59)
│ │ ┌──────── hour (0-23)
│ │ │ ┌────── day of month (1-31)
│ │ │ │ ┌──── month (1-12)
│ │ │ │ │ ┌── day of week (0-6, Sunday = 0)
│ │ │ │ │ │
* * * * * *
```

**Examples:**
- `0 0 * * * *` - Every hour
- `0 */5 * * * *` - Every 5 minutes
- `0 0 2 * * *` - Daily at 2 AM
- `0 0 0 * * 0` - Weekly on Sunday at midnight
- `0 30 9 * * 1-5` - Weekdays at 9:30 AM

## Retrigger Modes

Control what happens when the schedule triggers while the previous run is still active:

### `finish` (Default)

Only retrigger if the previous execution has finished.

```toml
[daemons.backup]
run = "./backup.sh"
cron = { schedule = "0 0 2 * * *", retrigger = "finish" }
```

**Use case:** Long-running tasks that should not overlap.

### `always`

Always retrigger. Stops the previous run if still active.

```toml
[daemons.health-check]
run = "curl -f http://localhost:8080/health"
cron = { schedule = "0 */5 * * * *", retrigger = "always" }
```

**Use case:** Health checks where you always want the latest execution.

### `success`

Only retrigger if the previous execution succeeded (exit code 0).

```toml
[daemons.process-data]
run = "./process.sh"
cron = { schedule = "0 0 * * * *", retrigger = "success" }
```

**Use case:** Chained tasks that depend on prior success.

### `fail`

Only retrigger if the previous execution failed.

```toml
[daemons.retry-task]
run = "./flaky-task.sh"
cron = { schedule = "0 */10 * * * *", retrigger = "fail" }
```

**Use case:** Automatic retry logic for failing tasks.

## Starting Cron Daemons

Start cron daemons like any other:

```bash
pitchfork start backup
pitchfork start --all
```

The supervisor triggers the daemon according to its schedule.

## Monitoring

```bash
# View all daemons including cron jobs
pitchfork list

# View logs
pitchfork logs backup
```
