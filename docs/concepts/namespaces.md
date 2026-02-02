# Namespaces

How Pitchfork handles daemons with the same name across different projects.

## The Problem

When working with multiple projects, you might have daemons with the same name in different directories:

```
~/projects/
├── frontend/
│   └── pitchfork.toml    # defines "api" daemon
└── backend/
    └── pitchfork.toml    # also defines "api" daemon
```

Without namespacing, these would conflict. Pitchfork solves this by automatically qualifying daemon IDs with a namespace.

## Daemon ID Format

Pitchfork uses two forms of daemon IDs:

| Format | Example | Description |
|--------|---------|-------------|
| Short ID | `api` | Just the daemon name |
| Qualified ID | `frontend/api` | Namespace + daemon name |

Namespace derivation rules:

- `~/.config/pitchfork/config.toml` and `/etc/pitchfork/config.toml` use namespace `global`
- Project configs use top-level `namespace = "..."` when provided
- Otherwise project configs use the parent directory name of the config file
- If the derived directory namespace is invalid (e.g. contains `--`, spaces, or non-ASCII), loading fails with a clear error and you should set `namespace`

Example override:

```toml
namespace = "my-project"

[daemons.api]
run = "npm run dev"
```

## Using Short IDs

When you're in a project directory, you can use short IDs:

```bash
cd ~/projects/frontend
pitchfork start api        # Starts frontend/api
pitchfork status api       # Shows status of frontend/api
pitchfork logs api         # Shows logs for frontend/api
```

Pitchfork resolves short IDs in this order:

1. Prefer the current directory namespace
2. If not found locally, use a unique match from merged config
3. If `global/<id>` exists in merged config, use it
4. Otherwise return a not-found error
5. If multiple matches exist, return an ambiguity error and require `namespace/name`

## Using Qualified IDs

From any directory, you can use fully qualified IDs:

```bash
# From anywhere
pitchfork start frontend/api
pitchfork status backend/api
pitchfork logs frontend/api
```

This is useful when:
- Operating from outside the project directory
- Managing daemons from multiple projects at once
- Avoiding ambiguity when the same short name exists in multiple projects

Qualified IDs are parsed directly and work even when there is no local `pitchfork.toml`.

## Display Behavior

Pitchfork intelligently shows or hides namespaces in output:

**When there's no conflict** (only one daemon named `api`):
```
$ pitchfork list
api  12345  running
```

**When there's a conflict** (multiple daemons named `api`):
```
$ pitchfork list
frontend/api  12345  running
backend/api   12346  running
```

## Naming Rules

Daemon IDs have the following restrictions:

| Rule | Valid | Invalid |
|------|-------|---------|
| No double dashes | `my-app` | `my--app` |
| No slashes in short ID | `api` | `api/v2` |
| Single slash for qualified ID | `project/api` | `a/b/c` |
| No spaces | `my_app` | `my app` |
| No parent references | `myapp` | `../etc` |
| ASCII only | `myapp123` | `myäpp` |

The `--` sequence is reserved for internal path encoding (converting `namespace/daemon` to `namespace--daemon` for filesystem storage).

Because of this, project directory names containing `--` (or other invalid namespace characters) require an explicit top-level `namespace` override.

## Path Encoding

Internally, Pitchfork converts qualified IDs to filesystem-safe paths:

| Daemon ID | Log Directory | Log File |
|-----------|---------------|----------|
| `frontend/api` | `logs/frontend--api/` | `frontend--api.log` |
| `my-project/web-server` | `logs/my-project--web-server/` | `my-project--web-server.log` |
| `global/postgres` | `logs/global--postgres/` | `global--postgres.log` |

This encoding is transparent to users—you always use `/` in commands, and Pitchfork handles the conversion automatically.

## Examples

### Managing Multiple Projects

```bash
# Start services in both projects
cd ~/projects/frontend && pitchfork start api
cd ~/projects/backend && pitchfork start api

# Check status of all daemons
pitchfork list
# Output:
# frontend/api  12345  running
# backend/api   12346  running

# View logs for a specific project's daemon
pitchfork logs frontend/api

# Stop a specific daemon from anywhere
pitchfork stop backend/api
```

### Working Within a Project

```bash
cd ~/projects/frontend

# Short IDs work here
pitchfork start api
pitchfork logs api
pitchfork stop api
```

### Global Configuration

Daemons defined in `~/.config/pitchfork/config.toml` use the `global` namespace:

```bash
pitchfork start global/postgres
pitchfork logs global/redis
```
