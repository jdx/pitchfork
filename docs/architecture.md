# Architecture

Pitchfork is a process supervisor with two main components: a **CLI** for user commands and a **Supervisor** daemon that manages processes. They communicate via Unix domain sockets.

## System Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         USER                                     │
│            pitchfork start/stop/status/run/logs                  │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                          CLI                                     │
│  • Reads pitchfork.toml configs                                  │
│  • Constructs RunOptions                                         │
│  • Sends IPC requests                                            │
└────────────────────────────┬────────────────────────────────────┘
                             │ Unix Domain Socket
                             │ ~/.local/state/pitchfork/ipc/main.sock
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                       SUPERVISOR                                 │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │   IPC Server    │  │ Interval Watch  │  │   Cron Watch    │  │
│  │  (handles CLI)  │  │    (10 sec)     │  │    (60 sec)     │  │
│  └────────┬────────┘  └────────┬────────┘  └────────┬────────┘  │
│           │                    │                    │           │
│           └────────────────────┼────────────────────┘           │
│                                ▼                                 │
│                    ┌───────────────────────┐                    │
│                    │   Daemon Management   │                    │
│                    │  • spawn processes    │                    │
│                    │  • monitor output     │                    │
│                    │  • handle retries     │                    │
│                    └───────────┬───────────┘                    │
└────────────────────────────────┼────────────────────────────────┘
                                 │
              ┌──────────────────┼──────────────────┐
              ▼                  ▼                  ▼
         ┌─────────┐       ┌─────────┐       ┌─────────┐
         │ Daemon  │       │ Daemon  │       │ Daemon  │
         │  redis  │       │   api   │       │  worker │
         └─────────┘       └─────────┘       └─────────┘

Storage:
  ~/.local/state/pitchfork/
    ├── state.toml          # Daemon state (PIDs, status, config)
    ├── ipc/main.sock       # Unix socket for CLI↔Supervisor
    └── logs/<daemon>/      # Per-daemon log files
```

## Core Components

| Component | Location | Purpose |
|-----------|----------|---------|
| CLI | `src/cli/` | User-facing commands, config parsing, IPC client |
| Supervisor | `src/supervisor.rs` | Background daemon, process management, watchers |
| IPC | `src/ipc/` | Unix socket communication with MessagePack serialization |
| State File | `src/state_file.rs` | Persistent state in TOML with file locking |
| Config | `src/pitchfork_toml.rs` | Reads and merges `pitchfork.toml` files |

## IPC Protocol

Communication uses Unix domain sockets with MessagePack-serialized messages delimited by null bytes.

**Request Types:**

| Request | Purpose |
|---------|---------|
| `Run(RunOptions)` | Start a daemon |
| `Stop { id }` | Stop a daemon |
| `GetActiveDaemons` | List running daemons |
| `Enable/Disable { id }` | Toggle daemon enabled state |
| `UpdateShellDir` | Track shell working directory (for autostop) |

**Response Types:**

| Response | Meaning |
|----------|---------|
| `DaemonReady` | Daemon started and passed readiness check |
| `DaemonFailedWithCode(i32)` | Daemon exited with error code |
| `DaemonAlreadyRunning` | Daemon was already running |
| `ActiveDaemons(Vec)` | List of running daemons |

## Request Flow: `pitchfork start myapp`

1. **CLI reads config** — Merges all `pitchfork.toml` files from current directory up to root
2. **CLI connects to supervisor** — If not running, auto-starts it (exponential backoff: 100ms→1s, max 5 attempts)
3. **CLI sends `Run(RunOptions)`** — Contains daemon ID, command, working dir, retry config, readiness settings
4. **Supervisor spawns process** — Prepends `exec` to command, redirects stdout/stderr to log file
5. **Supervisor monitors readiness** — Waits for `ready_delay` seconds OR `ready_output` pattern match
6. **Supervisor responds** — `DaemonReady` or `DaemonFailedWithCode`
7. **CLI prints result and exits**

## Daemon Lifecycle

### Spawning

When starting a daemon, the supervisor:

1. Checks if already running (returns `DaemonAlreadyRunning` unless `--force`)
2. Prepends `exec` to the command (eliminates shell wrapper process)
3. Creates log file at `~/.local/state/pitchfork/logs/<id>/<id>.log`
4. Spawns via `tokio::process::Command` with piped stdout/stderr
5. Records PID in state file immediately
6. Starts monitoring task

### Readiness Detection

| Method | Config | Behavior |
|--------|--------|----------|
| Delay | `ready_delay = 5` | Wait N seconds; if still running, it's ready |
| Output | `ready_output = "listening"` | Match regex pattern in stdout/stderr |
| Default | (none) | Wait 3 seconds |

### Retry on Failure

If `retry > 0` and daemon fails, supervisor retries with exponential backoff:
- Attempt 1: immediate
- Attempt 2: wait 1s
- Attempt 3: wait 2s
- Attempt 4: wait 4s
- ...and so on

### Daemon States

| State | Meaning |
|-------|---------|
| `Running` | Process is alive (has PID) |
| `Stopped` | Exited successfully (code 0) |
| `Errored` | Exited with error (code ≠ 0) |
| `Waiting` | Waiting for ready signal |

## State Persistence

State is stored in `~/.local/state/pitchfork/state.toml`:

```toml
[daemons.myapp]
id = "myapp"
pid = 12345
status = "running"
dir = "/path/to/project"
autostop = true
retry = 2
retry_count = 0
last_exit_success = true

[disabled]
# Set of disabled daemon IDs

[shell_dirs]
# Map of shell_pid → working_directory (for autostop)
```

All reads/writes use file locking (`fslock`) for concurrent safety.

## Background Watchers

### Interval Watcher (every 10 seconds)

- **Refresh process list** — Uses `sysinfo` to check which PIDs are alive
- **Autostop** — If a shell exits and no other shells remain in that directory, stop daemons marked `autostop: true`
- **Retry failed daemons** — Restart `Errored` daemons that have remaining retry attempts

### Cron Watcher (every 60 seconds)

Checks daemons with `cron_schedule` config:

| Retrigger Mode | Behavior |
|----------------|----------|
| `finish` (default) | Start only if not currently running |
| `always` | Force restart, stop existing first |
| `success` | Start only if previous run succeeded |
| `fail` | Start only if previous run failed |

## Key Implementation Details

- **`exec` prefix** — All daemon commands are prefixed with `exec` to replace the shell process, giving us direct PID control
- **Oneshot channels** — Used for readiness notification between monitoring task and spawn function
- **`tokio::select!`** — Monitors stdout, stderr, process exit, and delay timer concurrently
- **File locking** — Essential for concurrent state file access from multiple CLI invocations
