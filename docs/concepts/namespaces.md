---
description: Resolve daemon names across projects, global configuration, and Git worktrees.
---
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
- Otherwise project configs use the project directory name (`.config/` files use its parent)
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

For a daemon known to the supervisor or registered configuration, use its qualified ID from another directory:

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

Qualified IDs are parsed directly even without a local `pitchfork.toml`. A name alone does not discover an unknown project directory; start from that project first or register its namespace.

## Git Worktrees

Pitchfork supports Git worktrees out of the box — no extra configuration required.

Namespaces are derived from the directory a project config lives in, and every worktree is a separate directory. Worktrees with distinct directory names therefore get distinct namespaces. Worktrees with the same final directory name need explicit namespace overrides to avoid collisions.

```bash
cd ~/myapp-feature            # a linked worktree
pitchfork start api           # starts myapp-feature/api

cd ~/myapp                    # the main checkout
pitchfork status api          # myapp/api, unaffected by the worktree
```

Daemons started in a worktree run with the worktree directory as their working directory, and supervisor background tasks (cron scheduling, boot_start, file watching) automatically discover daemons defined in every worktree of a repository.

This automatic discovery (and worktree-aware proxy slug routing) is controlled by the `general.worktree` setting, which is enabled by default. Set it to `false` to disable all worktree/workspace discovery; only the main project directory is then used.

Automatic worktree isolation applies when namespaces are derived from project directories — so worktree directories need distinct names (the default when you create worktrees per branch). If a config sets an explicit top-level `namespace`, it overrides the directory-derived namespace, so give each worktree a distinct explicit namespace to keep same-named daemons from colliding.

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

## Scoping Lists to Namespaces

`pitchfork list` and `pitchfork tui` can be scoped to one or more namespaces:

```bash
# Only show daemons in the 'frontend' namespace
pitchfork list --namespace frontend

# Multiple namespaces (OR logic)
pitchfork list --namespace frontend --namespace backend

# Only the current project's namespace, resolved from the current directory
# the same way short daemon IDs are
pitchfork tui --project
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
| No leading/trailing dashes | `my-app` | `-app` or `app-` |
| ASCII alphanumeric, `_`, `-`, `.` only | `myapp123` | `myäpp` or `app@v1` |

The `--` sequence is reserved for internal path encoding (converting `namespace/daemon` to `namespace--daemon` for filesystem storage).

Because of this, project directory names containing `--` (or other invalid namespace characters) require an explicit top-level `namespace` override.

## Path encoding

Some internal paths encode `frontend/api` as `frontend--api`. This is why `--`
is reserved in names. Current logs use a shared SQLite store keyed by qualified
ID, not one text file per daemon. See [file locations](/reference/file-locations#logs).

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
