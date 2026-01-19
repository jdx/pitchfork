# File Locations

Where pitchfork stores its files.

## Configuration Files

| Location | Purpose |
|----------|---------|
| `/etc/pitchfork/config.toml` | System-wide configuration |
| `~/.config/pitchfork/config.toml` | User configuration |
| `pitchfork.toml` | Project configuration (in any directory) |

Configuration files are merged in order, with later files overriding earlier ones.

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

`~/.local/state/pitchfork/logs/<daemon-name>/<daemon-name>.log`

Each daemon has its own log directory and file.

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
