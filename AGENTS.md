# CLAUDE.md

## mbx build cache

Mise installs mbx 1.1 and activates its transparent Cargo shim, so
compilation-heavy mise tasks and hk checks use ordinary `cargo` commands. If
the shim fails or creates a development papercut, rerun the exact equivalent
command from `CONTRIBUTING.md` with `MBX_DISABLE=1`; this unblocks work without
weakening the check. If bypassed Cargo succeeds, surface the mismatch and recommend a
[mr-boxington Discussion](https://github.com/jdx/mr-boxington/discussions) with
the repository and commit, OS, `mbx --version`, `mbx doctor`, and both commands
and outputs. Redact secrets, absolute cache paths, remote URLs, namespaces, and
other sensitive or identifying details. Do not permanently disable the shim,
and do not post externally without user authorization.

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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
| `src/settings.rs` (`#[derive(usage_rs::Config)]`) | derive generates the settings registry + spec `config` block; `mise run render` renders it | `docs/cli/configuration.md` (settings reference), config block in `pitchfork.usage.kdl` |
| Rust clap + schemars | `mise run render`: `pitchfork usage` → `usage` tool; `pitchfork schema` | `docs/cli/*.md` + `docs/cli/commands.json` (CLI reference); `docs/public/schema.json` (JSON Schema for editor autocomplete) |

**Update rules:**
- Changing user settings → edit the `#[derive(usage_rs::Config)]` structs in `src/settings.rs` (sole source of truth), then run `mise run render`
- Changing CLI flags/args/help text (clap) or config struct (schemars) → run `mise run render` (running `mise run ci-dev` includes itself, recommended) to regenerate `docs/cli/`, `docs/public/schema.json`, and `pitchfork.usage.kdl`

**These files are generated and should not be manually edited:**
- `docs/cli/*.md`
- `docs/cli/commands.json`
- `docs/public/schema.json`
- `pitchfork.usage.kdl`

**Partially generated:**
- `docs/reference/settings.md` — hand-authored prose; the full per-setting reference is the generated `docs/cli/configuration.md` it links to.

## Code Patterns

- **Async/Tokio**: All I/O is async; use `tokio::select!` for concurrent operations
- **Error handling**: Use `miette::Result` for rich error messages
- **Serialization**: Heavy use of serde with TOML for config/state, MessagePack for IPC
- **File locking**: Always lock state file for concurrent access (`xx::fslock`)
- **Daemon commands**: Run via the shell verbatim; do NOT prepend `exec` — it breaks compound commands (e.g. `exec a && b` silently drops `b`). Users can add `exec` themselves in the run string for single commands
- **Idiomatical Rust**: Prefer Idiomatical Rust patterns and idioms

## Dependency Updates

- Use the lowest compatibility-significant specificity in `Cargo.toml` (for example, `"1"` for stable 1.x dependencies).
- When the existing manifest requirement accepts a routine dependency update, change only `Cargo.lock`.
- Keep lockfile updates focused and avoid unrelated transitive dependency churn.

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

When AI contributes GitHub content—including a pull request description, review, pull request
comment, or discussion post—append this disclosure:

`*AI-assisted — Tool: <tool>; model: <provider>/<model>; version: <version-or-unavailable>.*`

Use the exact model and version identifiers exposed by the runtime. Never infer or guess them; use
`unavailable` when either value is not exposed.
