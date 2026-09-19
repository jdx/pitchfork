---
description: Locate pitchfork configuration, runtime state, logs, sockets, certificates, and boot registration files.
---
# File locations

Pitchfork keeps configuration separate from runtime state. The paths below are
the normal Unix defaults; environment variables can override the config and
state directories.

## Configuration files

| Path | Purpose |
| --- | --- |
| `/etc/pitchfork/config.toml` | System-wide defaults and global daemons |
| `~/.config/pitchfork/config.toml` | User settings, global daemons, slug and namespace registries |
| `.config/pitchfork.toml` | Project configuration in a `.config` directory |
| `.config/pitchfork.local.toml` | Personal overrides for that project configuration |
| `pitchfork.toml` | Project configuration in the project root |
| `pitchfork.local.toml` | Personal project overrides |

Project files are discovered from filesystem root down to the current directory.
Within each directory, later rows above take precedence. See
[configuration hierarchy](/reference/configuration#configuration-hierarchy).
Add local override files to `.gitignore` yourself if they should stay private.

`PITCHFORK_CONFIG_DIR` changes the user config directory. It does not move
project config files.

## State directory

The normal location is `~/.local/state/pitchfork/`:

| Path within the state directory | Purpose |
| --- | --- |
| `state.toml` | Daemon identities, status, retry state, and project sessions |
| `logs/logs.db` | SQLite daemon log store |
| `sock/main.sock` | CLI-to-supervisor Unix socket |
| `proxy/ca.pem` | Generated proxy CA certificate |
| `proxy/ca-key.pem` | Private key for the proxy CA |
| `proxy/host-certs/` | Cached certificates and private keys for individual proxy hostnames |
| `proxy/setup.toml` | Saved proxy setup configurations used for reconciliation and undo |

`PITCHFORK_STATE_DIR` overrides this location. On Linux, the default also follows
the system's state-directory resolution (including `XDG_STATE_HOME`). On macOS,
pitchfork falls back to `~/.local/state/pitchfork`.

The supervisor generates the proxy CA during HTTPS startup. The proxy caches
per-hostname certificates when using that CA to serve HTTPS.
`proxy/setup.toml` records system configuration for
[`proxy setup --undo`](/cli/proxy/setup); it is not the proxy settings file. See
[local proxy setup](/guides/port-management#hostname-resolution).

### Logs

Daemon logs live in a shared SQLite database, keyed by qualified daemon ID
(`namespace/name`). WAL mode allows concurrent readers. Query them with
`pitchfork logs`, the TUI, or the web UI rather than looking for one text file
per daemon.

Older versions wrote directories such as `logs/my-project--api/`. Legacy text
logs are imported on first access to the log store. Those paths are retained for
migration, not the current storage layout.

See [log management](/guides/logs) for filtering, retention, and export.

### Running with sudo

For an interactive sudo launch, `SUDO_USER` selects the calling user's home for
configuration lookup. A system service installed with `sudo pitchfork boot enable`
records that account as `supervisor run --boot --invoking-user <user>`, so boot
startup uses the same user configuration without a sudo environment.

State paths default to the invoking user's home. For a root supervisor,
`settings.supervisor.user` can select a different account for state paths and
ownership; it does not change which user's configuration was loaded.
An explicit `PITCHFORK_STATE_DIR` overrides the state directory.

See [system boot services](/guides/boot-start#running-the-supervisor-as-root)
for account selection and upgrading an existing registration.

Keep clients and the supervisor pointed at the same state directory. Different
values mean different state files and IPC sockets.

## Boot registration

| Platform | User registration | System registration (root) |
| --- | --- | --- |
| macOS | `~/Library/LaunchAgents/pitchfork.plist` | `/Library/LaunchDaemons/pitchfork.plist` |
| Linux | `~/.config/systemd/user/pitchfork.service` | `/etc/systemd/system/pitchfork.service` |

Use `pitchfork boot status` to inspect registration and `pitchfork boot disable`
to remove it. See [start at login or boot](/guides/boot-start).

## Attached configuration

Generators can attach files outside a project through the namespace registry.
See [external configuration files](/guides/external-configs) for registration, precedence, and project-relative paths.
