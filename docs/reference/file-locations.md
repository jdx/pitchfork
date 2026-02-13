# File Locations

Where pitchfork stores its files.

## Configuration Files

See [Configuration Hierarchy](/reference/configuration#configuration-hierarchy) for details on configuration file locations and precedence.

| Location | Purpose |
|----------|---------|
| `/etc/pitchfork/config.toml` | System-wide configuration |
| `~/.config/pitchfork/config.toml` | User configuration |
| `pitchfork.toml` / `pitchfork.local.toml` | Project configuration (in any directory) |

## State Directory

**Location:** `~/.local/state/pitchfork/`

| File/Directory | Purpose |
|----------------|---------|
| `state.toml` | Persistent daemon state |
| `logs/` | Daemon log files |
| `ipc/main.sock` | Unix socket for CLI-supervisor communication |

### State File

`~/.local/state/pitchfork/state.toml` tracks:
- Known daemons and their status
- Enabled/disabled state
- Last run information

### Logs

Each daemon has its own log directory and file. The log path is determined by the daemon's qualified ID (namespace + name):

```
~/.local/state/pitchfork/logs/<namespace>--<daemon-name>/<namespace>--<daemon-name>.log
```

The namespace is derived from the project directory name. For example:
- Daemon `api` in project `myapp` → `logs/myapp--api/myapp--api.log`
- Daemon `api` in project `yourapp` → `logs/yourapp--api/yourapp--api.log`
- Daemon `postgres` in global config → `logs/global--postgres/global--postgres.log`

The `--` separator is used to convert the `/` in qualified daemon IDs (e.g., `myapp/api`) to a filesystem-safe format.

See [Namespaces](/concepts/namespaces) for more details on how daemon IDs work across projects.

### IPC Socket

`~/.local/state/pitchfork/ipc/main.sock`

Unix domain socket used for communication between CLI commands and the supervisor daemon.

## Boot Start Files

Varies by platform:

| Platform | Location |
|----------|----------|
| macOS | `~/Library/LaunchAgents/com.pitchfork.agent.plist` |
| Linux | `~/.config/systemd/user/pitchfork.service` |
| Windows | Registry at `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run` |
