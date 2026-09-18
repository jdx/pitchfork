---
description: Run migrations, seeds, and other setup tasks to completion before starting dependent services.
---
# Oneshot tasks

Set `oneshot = true` for a command that must finish successfully before another
daemon starts. Use it for database migrations, fixture seeds, or message-bus
setup. A successful task exits with code `0` and reports `completed`.

## Run setup before a service

This example assumes an initialized PostgreSQL data directory at `./data` and
an application with an `npm run migrate` script and a `server.js` entry point:

```toml
[daemons.db]
run = "postgres -D ./data"
ready_cmd = { run = "pg_isready -h 127.0.0.1", timeout = "30s" }

[daemons.migrate]
run = "npm run migrate"
oneshot = true
depends = ["db"]

[daemons.api]
run = "node server.js"
depends = ["migrate"]
```

Start the application with:

```sh
pitchfork start api
```

Pitchfork starts `db` and waits for `pg_isready` to succeed. It then runs
`migrate` and waits for it to exit with code `0` before starting `api`.
A failed migration blocks the API from starting.

Use a oneshot dependency for setup that other daemons must wait for.
[Lifecycle hooks](/guides/lifecycle-hooks) run asynchronously: an `on_ready`
hook does not delay dependent daemons or block them if the hook fails.

## Completion and failure

| Outcome | Status | Effect on dependent startup |
| --- | --- | --- |
| Task exits with code `0` | `completed` | Dependents can start |
| Task exits with a nonzero code | `errored` | Dependents cannot start until a retry succeeds |
| Task is stopped with `pitchfork stop` | `stopped` | The interrupted run does not satisfy dependents |

Failures follow the configured [retry policy](/guides/auto-restart). With no
retries remaining, startup fails. Stopping a running task sends its configured
[stop signal](/guides/lifecycle-hooks#stop-signal).

Check completion with `pitchfork list` or `pitchfork status migrate`.
The `completed` status also appears in JSON output and both dashboards.
A regular daemon still reports `stopped` when it exits successfully;
`oneshot` defaults to `false`.

Do not combine `oneshot = true` with any `ready_*` or `health_*` field.
Pitchfork rejects that configuration: a oneshot task's successful exit is its
readiness signal, and there is no running service to health-check afterward.

## When tasks run again

A completed task runs again when you start or restart it, including when startup
reaches it as another daemon's dependency. Automatic startup with
`auto = ["start"]` follows the same rule. Completion is not cached across starts.

::: warning Make the command safe to repeat
Use commands that tolerate repeated runs, such as a migration tool that skips
already-applied migrations or a seed script that updates existing records.
`oneshot` means one run to completion, not one run for the lifetime of a project.
:::

If a oneshot dependency is already running, startup waits for that run to finish.
It does not launch another copy of the running task.

## Allow more time for long tasks

By default, `pitchfork start` waits up to **one hour** for a oneshot task.
Set [`supervisor.oneshot_timeout`](/cli/configuration#supervisor-oneshot-timeout)
in the project's `pitchfork.toml` for longer migrations or backfills:

```toml
[settings.supervisor]
oneshot_timeout = "6h"
```

Use `oneshot_timeout = "0"` to wait without a deadline.

The budget covers the whole wait, including any [`retry`](/guides/auto-restart)
attempts and the backoff between them. A task that retries several times spends
that time against the same clock, so raise the timeout rather than expecting
each attempt to get its own.

This timeout limits the wait, not the task's runtime. When it expires, startup
reports a timeout and does not start the dependents, but the task keeps running.
If it later exits with code `0`, it reports `completed`; a nonzero exit still
counts as a failure. Inspect the task's status and logs before starting again,
because starting a completed task runs it again.
