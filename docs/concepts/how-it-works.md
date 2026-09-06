---
description: Understand the supervisor, daemon configuration, readiness, and project sessions.
---
# How pitchfork works

Pitchfork runs your background commands under a persistent supervisor. The CLI
sends requests to that supervisor, so your services outlive the terminal command
that started them.

```text
CLI / TUI / web UI → supervisor → your services
```

## Configuration, process, supervisor

Three things have different lifetimes:

| Thing | What it holds | How long it lasts |
| --- | --- | --- |
| `pitchfork.toml` | Commands, dependencies, and lifecycle options | Until you edit the file |
| Daemon process | Your running API, database, or worker | Until it exits or is stopped |
| Supervisor | Process tracking, logs, schedules, and watchers | Across CLI invocations |

`pitchfork start api` reads configuration and starts the service if needed.
`pitchfork run demo -- command` starts an ad hoc service without writing a config.
Both use the same supervisor and can be inspected with `status` and `logs`.

The supervisor starts automatically when a client needs it, unless
[`supervisor.auto_start`](/cli/configuration#supervisor-auto-start) is disabled.
You normally do not need to manage it yourself.

## Started is different from ready

A process can be alive while still connecting to its database or opening a port.
A [ready check](/guides/ready-checks) tells pitchfork when startup is complete.
Dependencies wait for that signal; unrelated services start in parallel.

With no explicit check, a daemon is considered ready after three seconds of
running. Once ready, a daemon can be monitored with optional
[health checks](/guides/health-checks). A failed health check can trigger the same
[retry policy](/guides/auto-restart) as a crash.

## Your services belong to projects

Each daemon has a qualified name such as `my-project/api`. The namespace normally
comes from the project directory, so separate projects can each define `api`.
Inside a project, the short name is usually enough. See
[namespaces](/concepts/namespaces) for resolution rules and worktree behavior.

The [shell hook](/guides/shell-hook) tracks project sessions. With
`auto = ["start", "stop"]`, a daemon starts on entry and becomes eligible for
stopping when the last session leaves. Without that configuration, moving
between directories does not stop a manually started service.

## Choose what to automate

Start with the commands you already run locally, then add only the behavior you
need: readiness, retries, file watching, scheduled runs, or automatic start/stop.
Pitchfork runs commands directly on the host; those commands can also invoke
Docker or another tool your project already uses.

Continue with [your first project](/first-daemon), or read the
[architecture overview](/concepts/architecture) for implementation details.
