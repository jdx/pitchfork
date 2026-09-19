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

To register a **system-level** entry that starts at boot, before anyone logs
in (requires root):

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
| macOS | `~/Library/LaunchAgents/pitchfork.plist` | `/Library/LaunchDaemons/pitchfork.plist` |
| Linux | `~/.config/systemd/user/pitchfork.service` | `/etc/systemd/system/pitchfork.service` |

## Running the Supervisor as Root

Use `sudo pitchfork boot enable` when the supervisor needs root, for example so
the [proxy](/guides/port-management) can bind ports 80 and 443 at boot.

launchd and systemd start the system service without the `SUDO_USER`,
`SUDO_UID` and `SUDO_GID` variables that `sudo` sets. So that the service
still knows whose configuration to use, `sudo pitchfork boot enable` records
your account in the service command:

```text
/usr/local/bin/pitchfork supervisor run --boot --invoking-user alice
```

At boot, the root supervisor behaves the same as `sudo pitchfork supervisor start`
run by `alice`:

- It reads `~alice/.config/pitchfork/config.toml` (as well as
  `/etc/pitchfork/config.toml`), including its daemons, proxy settings and
  namespaces.
- State, logs and the IPC socket live under `~alice/.local/state/pitchfork`,
  owned by `alice`, so `pitchfork` commands run as `alice` reach the root
  supervisor.
- Daemons run as `alice` unless `settings.supervisor.user` or a daemon's `user`
  says otherwise.

To keep state and daemons under a different account, set
`settings.supervisor.user` in `~alice/.config/pitchfork/config.toml` or
`/etc/pitchfork/config.toml`:

```toml
[settings.supervisor]
user = "devservices"
```

If the recorded account is later deleted, the service refuses to start instead
of falling back to root. Run `sudo pitchfork boot enable` again from the account
that should own the supervisor.

A system-level entry registered from a root login shell (with no `SUDO_USER`)
records no account. It runs entirely as root, reading `/etc/pitchfork/config.toml`
and root's own configuration.

`pitchfork boot status` shows which account the system-level entry uses.

### Updating an existing system-level entry

System-level entries created before pitchfork recorded the invoking user start
with root's configuration, so they miss your `~/.config/pitchfork/config.toml`.
Run `sudo pitchfork boot enable` again to add your account to the existing
entry, then reload the service or restart the machine:

::: code-group

```bash [macOS]
sudo pitchfork boot enable
sudo launchctl bootout system /Library/LaunchDaemons/pitchfork.plist
sudo launchctl bootstrap system /Library/LaunchDaemons/pitchfork.plist
```

```bash [Linux]
sudo pitchfork boot enable
sudo systemctl restart pitchfork
```

:::

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
