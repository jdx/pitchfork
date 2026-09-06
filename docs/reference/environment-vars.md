---
description: Find environment overrides for pitchfork and the metadata passed to daemons, probes, and hooks.
---
# Environment variables

Environment variables have two roles: configure pitchfork, and pass runtime
information to daemon commands. For every setting's environment override, see
the [full settings reference](/cli/configuration).

## Configure pitchfork

| Variable | Purpose |
| --- | --- |
| `PITCHFORK_CONFIG_DIR` | Override the user config directory |
| `PITCHFORK_STATE_DIR` | Override state and IPC paths |
| `PITCHFORK_LOGS_DIR` | Override the log directory |
| `PITCHFORK_LOG` | Console verbosity: `error`, `warn`, `info`, `debug`, `trace` |
| `PITCHFORK_LOG_FILE_LEVEL` | File-log verbosity |
| `PITCHFORK_SUPERVISOR_AUTO_START` | Allow client commands to launch the supervisor; default `true` |
| `PITCHFORK_READY_DELAY` | Default readiness delay as a whole-second duration, such as `5s` |
| `PITCHFORK_AUTOSTOP_DELAY` | Delay after the last project session leaves; default `1m` |

Environment overrides affect the process that reads them. Exporting a variable
does not change an already running supervisor; restart it when changing its
settings. Keep the same state directory in clients and the supervisor.

```sh
PITCHFORK_LOG=debug PITCHFORK_LOG_FILE_LEVEL=debug pitchfork supervisor start --force
pitchfork logs pitchfork
```

## Enable the web UI

These similarly named variables do different jobs:

| Variable | Effect |
| --- | --- |
| `PITCHFORK_WEB_PORT=3120` | Enable the web UI for this supervisor invocation |
| `PITCHFORK_WEB_AUTO_START=true` | Enable the web UI through the settings system |
| `PITCHFORK_WEB_BIND_PORT=3120` | Choose the settings-based default port; does not enable the UI alone |
| `PITCHFORK_WEB_PATH=ps` | Serve the UI under `/ps/` for this invocation |

```sh
PITCHFORK_WEB_PORT=3120 pitchfork supervisor start --force
```

The server tries up to `web.port_attempts` consecutive ports (default `10`).
Check supervisor logs for the bound address. See [web UI setup](/guides/web-ui).

## Daemon process variables

Pitchfork supplies these to daemon commands and readiness/health command probes:

| Variable | Value |
| --- | --- |
| `PITCHFORK_DAEMON_ID` | Qualified ID, such as `my-project/api` |
| `PITCHFORK_DAEMON_NAMESPACE` | Namespace alone, such as `my-project` |
| `PITCHFORK_RETRY_COUNT` | `0` on the initial run, `1` on the first retry, and so on |
| `PORT` / `PORT0` | First resolved port, when `port` is configured |
| `PORT1`, `PORT2`, … | Additional resolved ports, indexed from zero |

Ports include any offset chosen by [port bumping](/guides/port-management).
Your service must read them or accept them as command arguments:

```toml
[daemons.web]
run = "python3 -u -m http.server $PORT --bind 127.0.0.1"
port = { expect = [8000], bump = 10 }
```

## Hook variables

[Lifecycle hooks](/guides/lifecycle-hooks) receive daemon metadata and configured
environment values, plus event-specific fields:

| Variable | Available in |
| --- | --- |
| `PITCHFORK_EXIT_CODE` | `on_fail`, `on_stop`, `on_exit`; `-1` when a Unix signal leaves no exit code |
| `PITCHFORK_EXIT_REASON` | `on_stop`, `on_exit`: `stop`, `exit`, or `fail` |
| `PITCHFORK_MATCHED_LINE` | `on_output`: the matching raw output line |
| `PITCHFORK_PORT`, `PITCHFORK_PORT0`, … | Hooks for a daemon with resolved ports |

Quote variables when using them in shell scripts:

```sh
printf '%s exited: reason=%s code=%s\n' \
  "$PITCHFORK_DAEMON_ID" "$PITCHFORK_EXIT_REASON" "$PITCHFORK_EXIT_CODE"
```

To supply your own values, use top-level `[env]` or a daemon's `env` field in
[`pitchfork.toml`](/reference/configuration#env).
