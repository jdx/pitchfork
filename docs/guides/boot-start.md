# Start on Boot

Configure pitchfork to start automatically when your system boots.

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
| macOS | `~/Library/LaunchAgents/pitchfork.plist` | `/Library/LaunchAgents/pitchfork.plist` |
| Linux | `~/.config/systemd/user/pitchfork.service` | `/etc/systemd/system/pitchfork.service` |

## Running the Supervisor as Root

If you need the supervisor to run as root (e.g. to manage system-level processes), use `sudo pitchfork boot enable`.

However, if you still want state files, IPC sockets and daemon processes to belong to a specific user rather than root, set `supervisor.user` in your global config (`/etc/pitchfork/config.toml` or `~/.config/pitchfork/config.toml`):

```toml
[supervisor]
user = "alice"
```

With this setting, the supervisor process runs as root but spawns daemons and writes state under the specified user's home directory.

## Configure Boot Daemons

Add `boot_start = true` to daemons you want to start at boot. These should be in your global config file (`~/.config/pitchfork/config.toml`):

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
| macOS | LaunchAgents (user) | LaunchAgents (system) |
| Linux | systemd user service | systemd system service |

When boot start is enabled:
1. System login (user-level) or system startup (system-level) triggers the pitchfork supervisor
2. Supervisor starts all daemons with `boot_start = true`
3. Daemons run in the background

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
