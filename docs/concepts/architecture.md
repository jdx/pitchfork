# Architecture

Technical overview of pitchfork's internal design.

## System Overview

```
┌────────────────────────────────────────────────────────────┐
│                         USER                                │
│           pitchfork start/stop/status/run/logs              │
└───────────────────────────┬────────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────────┐
│                          CLI                                │
│  • Reads pitchfork.toml configs                             │
│  • Sends IPC requests                                       │
└───────────────────────────┬────────────────────────────────┘
                            │ Unix Socket
                            │ ~/.local/state/pitchfork/ipc/main.sock
                            ▼
┌────────────────────────────────────────────────────────────┐
│                       SUPERVISOR                            │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐         │
│  │ IPC Server  │  │  Interval   │  │    Cron     │         │
│  │             │  │  Watcher    │  │   Watcher   │         │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘         │
│         │                │                │                 │
│         └────────────────┼────────────────┘                 │
│                          ▼                                  │
│              ┌───────────────────────┐                      │
│              │   Daemon Management   │                      │
│              │  • spawn processes    │                      │
│              │  • monitor output     │                      │
│              │  • handle retries     │                      │
│              └───────────────────────┘                      │
└────────────────────────────────────────────────────────────┘
                            │
             ┌──────────────┼──────────────┐
             ▼              ▼              ▼
        ┌─────────┐   ┌─────────┐   ┌─────────┐
        │ Daemon  │   │ Daemon  │   │ Daemon  │
        └─────────┘   └─────────┘   └─────────┘
```

## Components

| Component | Purpose |
|-----------|---------|
| CLI | User commands, config parsing, IPC client |
| Supervisor | Background daemon, process management |
| IPC | Unix socket communication (MessagePack) |
| State File | Persistent state in TOML with file locking |

## Supervisor Auto-Start

When you run a command like `pitchfork start`, the CLI:

1. Checks if the supervisor is running
2. If not, starts it in the background
3. Connects via Unix socket
4. Sends the command

The supervisor runs independently and manages all daemons.

## Background Watchers

### Interval Watcher (10 seconds)

- Refreshes process list (checks which PIDs are alive)
- Handles autostop (stops daemons when shell leaves directory)
- Retries failed daemons with remaining retry attempts

### Cron Watcher (60 seconds)

- Checks daemons with cron schedules
- Triggers according to retrigger policy

## Daemon States

| State | Meaning |
|-------|---------|
| Running | Process is alive |
| Waiting | Waiting for ready check |
| Stopped | Exited successfully (code 0) |
| Errored | Exited with error (code ≠ 0) |

## State Persistence

Daemon state is stored in `~/.local/state/pitchfork/state.toml`:

```toml
[daemons.myapp]
id = "myapp"
pid = 12345
status = "running"
dir = "/path/to/project"
autostop = true
retry = 2
retry_count = 0

[disabled]
# Set of disabled daemon IDs

[shell_dirs]
# Map of shell_pid → working_directory
```

All state file access uses file locking for concurrent safety.

## Process Spawning

When starting a daemon:

1. Check if already running
2. Prepend `exec` to command (eliminates shell wrapper)
3. Create log file
4. Spawn process with piped stdout/stderr
5. Record PID in state file
6. Start monitoring for readiness

## Readiness Detection

| Method | Trigger |
|--------|---------|
| Delay | Wait N seconds, still running = ready |
| Output | Regex matches stdout/stderr |
| HTTP | Endpoint returns 2xx |

First check to succeed marks daemon as ready.
