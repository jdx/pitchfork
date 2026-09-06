---
description: A contributor-oriented overview of pitchfork's clients, supervisor, IPC, persistence, and process lifecycle.
---
# Architecture

Pitchfork has a client-server architecture. The CLI and TUI request operations
from a background supervisor; the optional web UI uses the supervisor's HTTP API.
For the user-facing model, see [how pitchfork works](/concepts/how-it-works).

```text
CLI / TUI ────── IPC ──────┐
                          ├── Supervisor ── Daemon process groups
Web UI ─────── HTTP API ───┘       │
                                 ├── State file (TOML)
                                 └── Log store (SQLite)
```

## Code map

| Module | Responsibility |
| --- | --- |
| `src/cli/` | Parse commands, load configuration, and make client requests |
| `src/ipc/` | Serialize requests and responses with MessagePack |
| `src/supervisor/lifecycle.rs` | Spawn, monitor, and terminate daemons |
| `src/supervisor/watchers.rs` | Periodic work, cron schedules, and file watching |
| `src/supervisor/hooks.rs` | Dispatch lifecycle hooks |
| `src/supervisor/log_sink.rs` | Capture output independently of the supervisor on supported paths |
| `src/pitchfork_toml.rs` | Load and merge configuration; resolve namespaces |
| `src/state_file.rs` | Persist daemon and session state with file locking |
| `src/log_store/` | Store, query, and retain logs in SQLite |
| `src/web/` | Serve the web UI and API |

On Unix, clients connect through `sock/main.sock` in the state directory.
See [file locations](/reference/file-locations) for path resolution.

## Starting a daemon

1. The client resolves daemon IDs and orders their dependencies.
2. Templates use values from dependencies that have already started.
3. The supervisor resolves the working directory, environment, and ports.
4. The configured shell receives the `run` string verbatim (`sh -c` by default).
5. The supervisor records the process and monitors readiness and exit status.

With `mise = true`, execution becomes `mise x -- sh -c "<run>"` (or the
configured shell). Pitchfork does not prepend `exec`; doing so would break
compound commands such as `a && b`. Users can place `exec` before a final
command themselves.

Output is stored in SQLite. On supported Unix configurations, a separate log
sink process keeps capturing output if the supervisor exits unexpectedly.
The supervisor can re-adopt surviving daemons according to
[`supervisor.orphan_policy`](/cli/configuration#supervisor-orphan-policy).

## Background work

| Watcher | Work | Default cadence |
| --- | --- | --- |
| Interval | Refresh process state, evaluate autostop, retry failures, enforce resource limits | `general.interval`: `10s` |
| Cron | Discover scheduled daemons and evaluate retrigger policies | `supervisor.cron_check_interval`: `10s` |
| File | Match file changes and restart running daemons | `supervisor.file_watch_debounce`: `1s` debounce |
| Health | Probe configured commands, HTTP endpoints, or TCP ports | `supervisor.health_check_interval`: `10s` |

File watching supports native notifications and polling. Health probes have
their own timeouts and failure thresholds; they are distinct from readiness.

## State and shutdown

The locked TOML state file records process identities, status, retry state,
disabled daemons, and project sessions. SQLite stores timestamped logs and
supports concurrent readers. These are runtime files, not configuration to edit
by hand.

On Unix, stopping a daemon sends its configured signal to the process group
(`SIGTERM` by default). Pitchfork waits for `stop_signal.timeout`, falling back
to `supervisor.stop_timeout` (`5s`), then escalates to `SIGKILL` if necessary.
Batch stops use reverse dependency order.

See [contributing](/contributing) for the development and verification workflow.
