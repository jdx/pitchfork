# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
# Build
cargo build

# Run tests (uses nextest for faster parallel execution)
cargo nextest run

# Run a single test
cargo nextest run test_name

# Lint (check)
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings

# Lint (fix)
cargo fmt --all
cargo clippy --fix --allow-dirty --allow-staged --all-targets --all-features -- -D warnings

# Full CI pipeline (lint + build + test)
mise run ci

# Install dev build and start supervisor with debug logging
mise run install-dev

# Render CLI docs (requires mise and usage tool)
mise run render
```

## Architecture

Pitchfork is a daemon supervisor CLI with a **client-server architecture**:

### Core Components

1. **CLI (`src/cli/`)** - User-facing commands that communicate with the supervisor via IPC
2. **Supervisor (`src/supervisor.rs`)** - Background daemon that manages all child processes
3. **IPC (`src/ipc/`)** - Unix domain socket communication using MessagePack serialization

### How It Works

- CLI commands connect to the supervisor at `~/.local/state/pitchfork/ipc/main.sock`
- If supervisor isn't running, CLI auto-starts it in background
- Supervisor spawns and monitors daemons, handles retries, cron scheduling, and autostop
- State persisted to `~/.local/state/pitchfork/state.toml` with file locking for concurrency

### Key Files

| File | Purpose |
|------|---------|
| `src/supervisor.rs` | Main supervisor logic, IPC handlers, background watchers |
| `src/ipc/` | Client/server IPC with MessagePack over Unix sockets |
| `src/pitchfork_toml.rs` | Config file parsing and merging |
| `src/state_file.rs` | Persistent state management |
| `src/daemon.rs` | Daemon struct and state |
| `src/cli/start.rs` | Main "start daemon" command logic |

### Background Watchers (in supervisor)

- **Interval watcher (10s)**: Refresh process state, autostop, retry failed daemons
- **Cron watcher (60s)**: Trigger scheduled tasks based on cron expressions

### Config Hierarchy

Configs merge in order (later overrides earlier):
1. `/etc/pitchfork/config.toml` (system)
2. `~/.config/pitchfork/config.toml` (user)
3. `pitchfork.toml` files from filesystem root to current directory (project)

## Code Patterns

- **Async/Tokio**: All I/O is async; use `tokio::select!` for concurrent operations
- **Error handling**: Use `miette::Result` for rich error messages
- **Serialization**: Heavy use of serde with TOML for config/state, MessagePack for IPC
- **File locking**: Always lock state file for concurrent access (`xx::fslock`)
- **Daemon commands**: Prepend `exec` to eliminate shell process overhead

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
