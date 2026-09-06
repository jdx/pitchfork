---
description: Register the supervisor at login or boot and choose which daemons start automatically.
---
# Start at login or boot

Register the supervisor with launchd on macOS or systemd on Linux, then choose which daemons it starts. User registration starts at login; system registration runs at boot.

## Enable Boot Start

```bash
pitchfork boot enable
```

This registers pitchfork to start automatically when you log in.

To register a **system-level** entry (starts for all users, requires root):

```bash
sudo pitchfork boot enable
```

## Disable Boot Start

```bash
pitchfork boot disable
```

## Check Status

```bash
pitchfork boot status
```

## User-level vs System-level

The registration mode is determined automatically based on whether the command runs as root:

| | User-level | System-level (`sudo`) |
|---|---|---|
| macOS | `~/Library/LaunchDaemons/pitchfork.plist` | `/Library/LaunchDaemons/pitchfork.plist` |
| Linux | `~/.config/systemd/user/pitchfork.service` | `/etc/systemd/system/pitchfork.service` |

## Running the Supervisor as Root

If you need the supervisor to run as root (e.g. to manage system-level processes), use `sudo pitchfork boot enable`.

However, if you still want state files, IPC sockets and daemon processes to belong to a specific user rather than root, set `settings.supervisor.user` in your global config (`/etc/pitchfork/config.toml` or `~/.config/pitchfork/config.toml`):

```toml
[settings.supervisor]
user = "alice"
```

With this setting, the supervisor process runs as root but spawns daemons and writes state under the specified user's home directory.

## Configure Boot Daemons

Add `boot_start = true` to daemons you want to start at boot. For a straightforward setup, define them in your user config file (`~/.config/pitchfork/config.toml`):

```toml
[daemons.postgres]
run = "postgres -D /usr/local/var/postgres"
boot_start = true

[daemons.redis]
run = "redis-server"
boot_start = true

[daemons.my-app]
run = "npm start"
boot_start = false  # Won't start at boot
```

## How It Works

| Platform | User-level method | System-level method |
|----------|-------------------|---------------------|
| macOS | LaunchAgent | LaunchDaemon |
| Linux | systemd user service | systemd system service |

When boot start is enabled:
1. System login (user-level) or system startup (system-level) triggers the pitchfork supervisor
2. Supervisor starts all daemons with `boot_start = true`
3. Daemons run in the background

### Prevent fallback supervisor starts

When systemd, launchd, or another service manager is the only intended owner of
the supervisor, disable client-side auto-start in your global configuration:

```toml
[settings.supervisor]
auto_start = false
```

Commands such as `pitchfork list`, the TUI, and shell activation will then
connect to the managed supervisor without spawning an unmanaged replacement if
the service is unavailable or still starting. Explicit
`pitchfork supervisor start` and `pitchfork supervisor run` commands remain
available.

## Tool availability

Login and boot services do not load your interactive shell setup. Use absolute
paths or [mise integration](/guides/mise-integration) when a command relies on
tools that are normally added to `PATH` by shell hooks.

Use the same registration mode when disabling: `pitchfork boot disable` removes
the user entry; `sudo pitchfork boot disable` removes the system entry.

## Typical Setup

1. Enable boot start:
   ```bash
   pitchfork boot enable
   ```

2. Add daemons to global config (`~/.config/pitchfork/config.toml`):
   ```toml
   [daemons.postgres]
   run = "postgres -D /usr/local/var/postgres"
   boot_start = true
   ready_output = "ready to accept connections"
   ```

3. Verify it's working:
   ```bash
   pitchfork boot status
   pitchfork list
   ```
