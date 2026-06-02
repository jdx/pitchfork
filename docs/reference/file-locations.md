# File Locations

Where pitchfork stores its files.

## Directory Resolution

Pitchfork resolves key directories as follows:

| Directory | Resolution Order |
|-----------|-----------------|
| **Home** | `SUDO_USER`'s home (when euid=0) → `dirs::home_dir()` → `/tmp` |
| **Config** | `PITCHFORK_CONFIG_DIR` env → `~/.config/pitchfork` |
| **State** | `PITCHFORK_STATE_DIR` env → (root + `settings.supervisor.user`) configured user's `~/.local/state/pitchfork` → (sudo) `SUDO_USER`'s `~/.local/state/pitchfork` → (non-sudo) `dirs::state_dir()/pitchfork` → `~/.local/state/pitchfork` |

> **Note:** Under root (euid=0), `settings.supervisor.user` controls the default state directory owner and location when set. Otherwise, `sudo` invocations resolve `~` from `SUDO_USER` via the system password database, and `dirs::state_dir()` is bypassed to keep paths consistent with non-sudo invocations. On macOS `dirs::state_dir()` returns `None`, so the fallback `~/.local/state` is always used.

## Configuration Files

Pitchfork supports configuration files in multiple locations. Files are merged in order, with later files overriding earlier ones.

| Location | Purpose |
| --- | --- |
| `/etc/pitchfork/config.toml` | System-wide configuration |
| `~/.config/pitchfork/config.toml` | User configuration |
| `.config/pitchfork.toml` | Project configuration |
| `.config/pitchfork.local.toml` | Project configuration |
| `pitchfork.toml` | Project configuration |
| `pitchfork.local.toml` | Local project overrides |

### Config File Precedence (lowest to highest)

1. `/etc/pitchfork/config.toml` - System-wide (lowest precedence)
2. `~/.config/pitchfork/config.toml` - User-wide
3. `.config/pitchfork.toml` - Project-level (in project's `.config/` subdirectory)
4. `.config/pitchfork.local.toml` - Project-level (in project's `.config/` subdirectory)
5. `pitchfork.toml` - Project-level (in project root)
6. `pitchfork.local.toml` - Local project overrides (highest precedence)

### Global Config: Slug Registry

The global config (`~/.config/pitchfork/config.toml`) also contains the `[slugs]` section — the single source of truth for reverse proxy slug→project mappings:

```toml
[slugs]
api = { dir = "/home/user/my-api", daemon = "server" }
docs = { dir = "/home/user/docs-site" }  # daemon defaults to "docs"
```

Manage slugs with `pitchfork proxy add` / `pitchfork proxy remove`.

Within a given project directory, files take precedence in this order:
- `.config/pitchfork.toml` has lowest precedence in that project
- `.config/pitchfork.local.toml` overrides `.config/pitchfork.toml`
- `pitchfork.toml` overrides anything in `.config/`
- `pitchfork.local.toml` overrides both (typically git-ignored)

## State Directory

**Location:** `~/.local/state/pitchfork/`

| File/Directory | Purpose |
|----------------|---------|
| `state.toml` | Persistent daemon state |
| `logs/` | Daemon log files |
| `sock/main.sock` | Unix socket for CLI-supervisor communication |

### State File

`~/.local/state/pitchfork/state.toml` tracks:
- Known daemons and their status
- Enabled/disabled state
- Last run information

### Logs

Logs are stored in a single SQLite database (`logs.db`) for efficient querying, filtering, and rotation. The database uses WAL mode for concurrent readers so the CLI, TUI, and Web UI can all read logs at the same time without blocking the supervisor's writes.

```
~/.local/state/pitchfork/logs/logs.db
```

Inside the database, each daemon is identified by its qualified ID (`namespace/name`). Log entries include a timestamp (millisecond precision) and the raw message text, so time-based filtering is fast and reliable.

For backwards compatibility, the legacy log directory structure still exists but is no longer written to by the supervisor:

```
~/.local/state/pitchfork/logs/<namespace>--<daemon-name>/
```

If you have legacy text log files from an older pitchfork version, migrate them into the SQLite store with:

```bash
pitchfork logs --migrate
```

### IPC Socket

`~/.local/state/pitchfork/sock/main.sock`

Unix domain socket used for communication between CLI commands and the supervisor daemon.

## Boot Start Files

Varies by platform:

| Platform | Location |
|----------|----------|
| macOS | `~/Library/LaunchAgents/com.pitchfork.agent.plist` |
| Linux | `~/.config/systemd/user/pitchfork.service` |
