---
description: Diagnose startup, readiness, port, shell-session, and supervisor problems without deleting state first.
---
# Troubleshooting

Start with the daemon's status and recent output. They usually show whether a
failure belongs to the command, its environment, or pitchfork's startup checks.

```sh
pitchfork --version
pitchfork status api
pitchfork logs api -n 100 --no-pager
pitchfork supervisor status
```

Replace `api` with your daemon's name. Use a qualified ID, such as
`my-project/api`, when working outside its project.

## Daemon won't start

Run the configured command manually from the daemon's working directory.
Check `dir`, required files, runtime versions, and environment variables.
Keep the process in the foreground so pitchfork can track it.

```sh
pitchfork daemons --json
pitchfork logs api --tail
```

If it works in your shell but fails at login or on a schedule, the supervisor
may not have your shell's `PATH`. Use [mise integration](/guides/mise-integration)
or absolute executable paths. A disabled daemon needs `pitchfork enable api`
before it can start.

## Ready check times out

An alive process is not necessarily ready. Test the configured check directly:

```sh
curl -i http://127.0.0.1:3000/health
```

Check the URL, expected status, listening address, and actual port. With port
bumping, a hardcoded health URL can target the wrong process; use `$PORT` in
`ready_cmd` when the daemon's port is dynamic.

```toml
[daemons.api]
run = "node server.js"
port = { expect = [3000], bump = 10 }
ready_cmd = { run = "curl -fsS http://127.0.0.1:$PORT/health", timeout = "60s" }
```

This assumes `server.js` reads `PORT`. Raise the check's `timeout` for a slow
startup. Raising `ready_delay` will not affect an explicit HTTP, TCP, output,
or command check. If all checks expire, startup exits with code `124`.
See [ready checks](/guides/ready-checks).

## Port already in use

Inspect the listener before changing or stopping anything:

```sh
lsof -nP -iTCP:3000 -sTCP:LISTEN
```

Choose another port or configure [port bumping](/guides/port-management).
For the web dashboard, choose a different starting port:

```sh
PITCHFORK_WEB_PORT=8888 pitchfork supervisor start --force
```

The dashboard can select a later port if that one is occupied. Read the
supervisor log for its bound address.

## Autostop not working

Check that the [shell hook](/guides/shell-hook) is loaded and the daemon has
`auto = ["stop"]` or `auto = ["start", "stop"]`.

```sh
pitchfork project list
pitchfork settings explain general.autostop_delay
```

Another terminal or IDE session in the project keeps it active. The default
delay is **one minute**, followed by the next supervisor evaluation. Leaving
and quickly returning cancels the pending stop.

If an integration created a session, it must call `project leave` with the same
PID and directory. On Windows, crashed host sessions also need explicit cleanup.

## A setting appears to be ignored

```sh
pitchfork settings explain web.auto_start
```

Environment variables override files. Project files can override user settings.
Supervisor-owned settings are read at supervisor startup; restart it after
editing those settings. See [settings precedence](/reference/settings#precedence).

## Supervisor won't start

First check that clients use the same `PITCHFORK_STATE_DIR` as the supervisor.
If `supervisor.auto_start = false`, a service manager must start it, or you can
start it explicitly with `pitchfork supervisor start`.

For a stuck supervisor, try the normal stop/start sequence:

```sh
pitchfork supervisor stop
pitchfork supervisor start
```

Stopping the supervisor affects its managed services. Inspect its logs before
escalating to manual process or socket cleanup. Removing the IPC socket while
a supervisor is alive can disconnect clients from it.

## Daemon won't stop

On Unix, pitchfork sends the configured signal (`SIGTERM` by default), waits
up to the configured stop timeout (`5s` by default), and escalates to `SIGKILL`
if needed. The signal applies to the process group.

For a service that expects `SIGINT`, or needs longer to flush data:

```toml
[daemons.api]
run = "node server.js"
stop_signal = { signal = "SIGINT", timeout = "10s" }
```

Apply configuration changes with a restart. If shutdown remains slow, inspect
the service's signal handling and [supervisor debug logs](#enable-debug-logging).

## Stale entries or damaged state

For stopped entries you no longer need, use:

```sh
pitchfork clean --daemon api
pitchfork clean --prune
```

These clean registrations, not config files. They do not stop running daemons.
Do not delete `state.toml` as a first troubleshooting step: it contains the
identities pitchfork uses to recognize managed processes. For a parse error,
stop the supervisor normally and preserve a copy of the state file before any
manual repair. See [file locations](/reference/file-locations).

## Enable debug logging

Both console and file verbosity can be set explicitly:

```sh
PITCHFORK_LOG=debug PITCHFORK_LOG_FILE_LEVEL=debug pitchfork supervisor start --force
pitchfork logs pitchfork
```

If the CLI cannot connect, inspect the supervisor text log directly at the
default path:

```sh
tail -n 100 ~/.local/state/pitchfork/logs/pitchfork/pitchfork.log
```

Adjust the path if you override the state or log directory. Use `trace` instead
of `debug` for more detail. A later normal supervisor start restores defaults
unless verbosity is also set in a config file.

## Getting help

Search [existing issues](https://github.com/jdx/pitchfork/issues), then report:

- Pitchfork version and operating system.
- The command you ran and the first substantive error.
- A minimal `pitchfork.toml` that reproduces the problem.
- Relevant daemon and supervisor logs, with credentials removed.
