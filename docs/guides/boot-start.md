---
description: Start the supervisor at login or boot, select its user configuration, and choose which daemons start with it.
---

# Start at login or boot

<span id="how-it-works"></span>

Register Pitchfork with launchd on macOS or systemd on Linux so the supervisor
starts without an interactive shell. Then choose which daemons it starts with
`boot_start = true`.

<span id="typical-setup"></span>

## Start at login {#enable-boot-start}

For local development, run:

```sh
pitchfork boot enable
pitchfork boot status
```

This installs a user service that starts when you log in. The supervisor and
its daemons run as your user and read your Pitchfork configuration.
For HTTPS on the standard port, use
[local proxy setup](/guides/port-management#hostname-resolution); the supervisor
can remain unprivileged.

## Choose which daemons start {#configure-boot-daemons}

Set `boot_start = true` on daemons that should start with the supervisor. For
example, in `~/.config/pitchfork/config.toml`:

```toml
[daemons.worker]
run = "exec /opt/my-app/bin/worker"
boot_start = true

[daemons.api]
run = "exec /opt/my-app/bin/server"
boot_start = false
```

Replace the commands with your application's installed programs. The worker
starts at login; the API stays stopped until explicitly started or requested
through a configured proxy. Omitting `boot_start` is equivalent to `false`.

<span id="tool-availability"></span>

Login and boot services do not load your shell's startup files. Use absolute
paths or [mise integration](/guides/mise-integration) for tools normally supplied
by shell activation.

## Run the supervisor as root {#running-the-supervisor-as-root}

Use a system service when the supervisor needs root privileges or must start
before anyone logs in:

```sh
sudo pitchfork boot enable
pitchfork boot status
```

If a user registration already exists, disable it first with
`pitchfork boot disable`. Pitchfork refuses to add a registration at another
privilege level while the first is still registered.

When Alice runs `sudo pitchfork boot enable`, Pitchfork records her account in
the generated service command:

```text
/usr/local/bin/pitchfork supervisor run --boot --invoking-user alice
```

The executable path reflects the installed binary. The recorded account replaces
the sudo environment that launchd and systemd do not inherit. At boot:

- The supervisor runs as root and reads `/etc/pitchfork/config.toml` and
  Alice's `~/.config/pitchfork/config.toml`, including proxy settings and namespaces.
- State, logs, and the IPC socket default to Alice's
  `~/.local/state/pitchfork`, with state ownership assigned to Alice. Her CLI
  commands can reach the same supervisor.
- Daemons run as Alice by default. A daemon's `user` overrides
  `settings.supervisor.user`, which overrides the recorded account.

`PITCHFORK_STATE_DIR` still overrides the state directory. To choose a different
account for default daemon identity and state ownership, set this in Alice's
user configuration or the system configuration:

```toml
[settings.supervisor]
user = "devservices"
```

This setting does not change whose user configuration is loaded. See
[daemon user selection](/reference/configuration#user) and
[file locations](/reference/file-locations#running-with-sudo) for the details.

If the recorded account no longer exists, the supervisor fails before resolving
configuration or state paths. Re-register from the account that should own the
setup. A system service installed from a root login shell without `SUDO_USER`
records no account and keeps root's configuration and identity unless explicitly
overridden.

### Update an existing system service {#updating-an-existing-system-level-entry}

Re-run `sudo pitchfork boot enable` to update an older registration that does
not record your account, or one that points to a different executable. An
unchanged registration is left alone. Reload the service to use the updated
command; this interrupts its running daemons:

::: code-group

```sh [macOS]
sudo pitchfork boot enable
sudo launchctl bootout system /Library/LaunchDaemons/pitchfork.plist
sudo launchctl bootstrap system /Library/LaunchDaemons/pitchfork.plist
```

```sh [Linux]
sudo pitchfork boot enable
sudo systemctl restart pitchfork
```

:::

A machine restart also loads the updated registration. Automatic registration
repair after an executable moves preserves the account already recorded in the
service.

## Check registration {#check-status}

```sh
pitchfork boot status
```

Status reports the registered privilege level and, for a system service, the
recorded account or root configuration. It describes the registration; use
`pitchfork supervisor status` to check the running process.

### Registration files {#user-level-vs-system-level}

| Platform | User service | System service |
| --- | --- | --- |
| macOS | `~/Library/LaunchAgents/pitchfork.plist` | `/Library/LaunchDaemons/pitchfork.plist` |
| Linux | `~/.config/systemd/user/pitchfork.service` | `/etc/systemd/system/pitchfork.service` |

## Disable registration {#disable-boot-start}

```sh
pitchfork boot disable
```

Use `sudo pitchfork boot disable` for system registrations. Disable removes
registrations at both privilege levels where permissions allow it, including
legacy macOS registrations. Check `pitchfork boot status` afterward.

## Keep one supervisor owner {#prevent-fallback-supervisor-starts}

When the service manager should be the only owner of the supervisor, add this
to the configuration it reads:

```toml
[settings.supervisor]
auto_start = false
```

CLI commands, the TUI, and shell activation then connect to the managed
supervisor without spawning a replacement if it is unavailable or still
starting. Explicit `pitchfork supervisor start` and `pitchfork supervisor run`
remain available.

