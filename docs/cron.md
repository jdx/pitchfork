# Cron Scheduling

Pitchfork supports cron-based scheduling for daemons, allowing you to run commands on a schedule with flexible retrigger behaviors.

## Configuration

Add a `cron` section to your daemon configuration in `pitchfork.toml`:

```toml
[daemons.my-task]
run = "./scripts/my-script.sh"
cron = { schedule = "0 2 * * *", retrigger = "finish" } # Run at 2 AM every day
```

## Retrigger Behavior

The `retrigger` field controls what happens when the scheduled time arrives:

### `finish` (Default)

Retrigger the command only if the previous execution has finished (whether it succeeded or failed). If the previous command is still running, do nothing.

**Use case:** Long-running tasks that should not overlap, like backups or data processing jobs.

```toml
[daemons.backup]
run = "./backup.sh"
cron = { schedule = "0 2 * * *", retrigger = "finish" }
```

### `always`

Always retrigger the command at the scheduled time. If the previous command is still running, stop it first, then start a new execution.

**Use case:** Health checks or monitoring tasks where you always want the latest execution.

```toml
[daemons.health-check]
run = "curl -f http://localhost:8080/health"
cron = { schedule = "*/5 * * * *", retrigger = "always" }
```

### `success`

Retrigger the command only if the previous execution finished successfully (exit code 0). If the previous command failed or is still running, do nothing.

**Use case:** Chained tasks where the next execution should only run if the previous one succeeded.

```toml
[daemons.process-data]
run = "./process.sh"
cron = { schedule = "0 * * * *", retrigger = "success" }
```

### `fail`

Retrigger the command only if the previous execution failed (non-zero exit code). If the previous command succeeded or is still running, do nothing.

**Use case:** Retry logic for tasks that may fail temporarily.

```toml
[daemons.retry-task]
run = "./flaky-task.sh"
cron = { schedule = "*/10 * * * *", retrigger = "fail" }
```

## Starting Cron Daemons

Cron daemons are started like regular daemons:

```bash
# Start a specific cron daemon
pitchfork start my-cron-task

# Start all daemons (including cron ones)
pitchfork start --all
```

Once started, the supervisor will automatically trigger the daemon according to its cron schedule and retrigger policy.

## Monitoring

You can monitor cron daemons like any other daemon:

```bash
# View status of all daemons
pitchfork status

# View logs
pitchfork logs my-cron-task
```

