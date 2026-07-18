# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> **Architecture**: see [`ARCHITECTURE.md`](./ARCHITECTURE.md) (symlink to `docs/concepts/architecture.md`) for the full technical overview: module layout, startup sequence, background tasks, daemon states, process spawning, readiness detection, termination, monitor exit path, hooks, logging, proxy, and IPC responses.

## Build Commands

```bash
# Build and restart supervisor
# (do not manually run cargo build, as it does not contain all necessary steps)
mise run build-dev

# MUST run this before committing
mise run ci-dev

# Run tests. It's already included in `ci-dev`
mise run test

# Run a single test
cargo nextest run test_name

# Lint (check)
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings

# Lint (fix)
cargo fmt --all
cargo clippy --fix --allow-dirty --allow-staged --all-targets --all-features -- -D warnings

# Install dev build and start supervisor with debug logging
mise run install-dev

# Render CLI docs. It's already included in `ci-dev`
mise run render
```

## Architecture

Pitchfork is a daemon supervisor CLI with a **client-server architecture**:

### Core Components

1. **CLI (`src/cli/`)** - User-facing commands that communicate with the supervisor via IPC
2. **Supervisor (`src/supervisor/`)** - Background daemon that manages all child processes
3. **IPC (`src/ipc/`)** - Unix domain socket communication using MessagePack serialization

### How It Works

- CLI commands connect to the supervisor at `~/.local/state/pitchfork/sock/main.sock`
- If supervisor isn't running, CLI auto-starts it in background
- Supervisor spawns and monitors daemons, handles retries, cron scheduling, and autostop
- State persisted to `~/.local/state/pitchfork/state.toml` with file locking for concurrency

### Key Files

| File | Purpose |
|------|---------|
| `src/supervisor/` | Supervisor module: `lifecycle.rs` (start/stop daemons), `ipc_handlers.rs`, `watchers.rs` (background watchers), `retry.rs`, `autostop.rs`, `state.rs`, `hooks.rs`, `pty.rs` |
| `src/ipc/` | Client/server IPC with MessagePack over Unix sockets |
| `src/pitchfork_toml.rs` | Config file parsing and merging |
| `src/state_file.rs` | Persistent state management |
| `src/daemon.rs` | Daemon struct and state |
| `src/cli/start.rs` | Main "start daemon" command logic |

### Background Watchers (in `src/supervisor/watchers.rs`)

- **Interval watcher**: Refresh process state, autostop, retry failed daemons (interval from `general.interval` setting, default 10s)
- **Cron watcher**: Trigger scheduled tasks based on cron expressions (interval from `supervisor.cron_check_interval` setting, default 10s)
- **File watcher**: Restart daemons when their watched files change (`daemon_file_watch`)

### Config Hierarchy

Configs merge in order (later overrides earlier):
1. `/etc/pitchfork/config.toml` (system - namespace: global)
2. `~/.config/pitchfork/config.toml` (user - namespace: global)
3. Project config files from filesystem root down to the current directory; within each directory:
   `.config/pitchfork.toml` < `.config/pitchfork.local.toml` < `pitchfork.toml` < `pitchfork.local.toml`

### Data Sources

| Source | Pipeline | Outputs |
|--------|----------|---------|
| `settings.toml` | `build/generate_settings.rs` (compile-time) | `Settings` struct + merge/meta Rust code; also `docs/settings.data.ts` → `SettingsTable.vue` → `docs/reference/settings.md` |
| Rust clap + schemars | `mise run render`: `pitchfork usage` → `usage` tool; `pitchfork schema` | `docs/cli/*.md` + `docs/cli/commands.json` (CLI reference); `docs/public/schema.json` (JSON Schema for editor autocomplete) |

**Update rules:**
- Changing user settings (`src/settings.rs`) → update `settings.toml` (sole source of truth for codegen)
- Changing CLI flags/args/help text (clap) or config struct (schemars) → run `mise run render` (running `mise run ci-dev` includes itself, recommended) to regenerate `docs/cli/`, `docs/public/schema.json`, and `pitchfork.usage.kdl`

**These files are generated and should not be manually edited:**
- `docs/cli/*.md`
- `docs/cli/commands.json`
- `docs/public/schema.json`
- `pitchfork.usage.kdl`

**Partially generated** (hand-authored prose + auto-populated component):
- `docs/reference/settings.md` — only the `<SettingsTable />` section is auto-generated from `settings.toml`; the surrounding prose may be edited by hand.

## Code Patterns

- **Async/Tokio**: All I/O is async; use `tokio::select!` for concurrent operations
- **Error handling**: Use `miette::Result` for rich error messages
- **Serialization**: Heavy use of serde with TOML for config/state, MessagePack for IPC
- **File locking**: Always lock state file for concurrent access (`xx::fslock`)
- **Daemon commands**: Run via the shell verbatim; do NOT prepend `exec` — it breaks compound commands (e.g. `exec a && b` silently drops `b`). Users can add `exec` themselves in the run string for single commands
- **Idiomatical Rust**: Prefer Idiomatical Rust patterns and idioms

## Conventional Commits

All commit messages and PR titles MUST follow conventional commit format:

**Format:** `<type>(<scope>): <description>`

**Types:**
- `feat:` - New features that affect the pitchfork CLI/application
- `fix:` - Bug fixes that affect the pitchfork CLI/application (not CI, docs, or infrastructure)
- `refactor:` - Code refactoring
- `docs:` - Documentation changes
- `style:` - Code style/formatting (no logic changes)
- `perf:` - Performance improvements
- `test:` - Testing changes
- `chore:` - Maintenance tasks, releases, dependency updates, CI/infrastructure changes
- `security:` - Security-related changes

**Scopes:**
- For command-specific changes, use the command name: `start`, `stop`, `status`, `logs`, `run`, etc.
- For subsystem changes: `supervisor`, `ipc`, `config`, `state`, `daemon`, `cron`, `deps`

**Description Style:**
- Use lowercase after the colon
- Use imperative mood ("add feature" not "added feature")
- Keep it concise but descriptive

**Examples:**
- `fix(supervisor): handle graceful shutdown on SIGTERM`
- `feat(start): add --restart-policy flag`
- `feat(cron): support timezone-aware scheduling`
- `docs: update configuration examples`
- `chore: release 0.2.0`
- `chore(ci): fix linting in CI pipeline`
- `chore(deps): update dependencies`

## GitHub Interactions

When posting comments on GitHub PRs or discussions, always include a note that the comment was AI-generated (e.g., "*This comment was generated by Claude Code.*").
