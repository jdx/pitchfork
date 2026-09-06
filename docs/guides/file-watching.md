---
description: Restart running daemons on source changes using glob patterns, debouncing, and native or polling watchers.
---
# File watching

Add `watch` patterns to restart a running daemon when its source changes.

```toml
[daemons.api]
run = "node server.js"
watch = ["server.js", "src/**/*.js", "package.json"]
ready_http = { url = "http://127.0.0.1:3000/health", timeout = "30s" }
retry = 3
```

Start it with `pitchfork start api`, edit a matching file, and inspect the restart
with `pitchfork logs api --tail`. Stopped daemons ignore changes.

::: tip Choose one reloader
If your command already watches files (such as `vite`, `flask run --reload`, or
`node --watch`), let it handle source reloads. Use pitchfork's watcher when the
command needs a full restart, or watch additional configuration files separately.
:::

## Choose patterns

| Pattern | Matches |
| --- | --- |
| `*.js` | JavaScript files in the config's project directory |
| `src/**/*.ts` | TypeScript files in `src` and its subdirectories |
| `package.json` | One file |
| `config/*.toml` | TOML files directly inside `config` |

Patterns are case-sensitive and relative to the config's project directory,
not the daemon's `dir`. Configs in `.config/` use that directory's parent as
the project base.

```toml
[daemons.api]
run = "node server.js"
dir = "api"
watch = ["api/server.js", "api/src/**/*.js"]
```

Keep patterns narrow enough to avoid build output, log files, `node_modules`,
and `target`. A daemon that writes to its own watched files can restart repeatedly.

## Native notifications or polling

```toml
[daemons.api]
run = "node server.js"
watch = ["src/**/*.js"]
watch_mode = "auto"
```

| Mode | Behavior |
| --- | --- |
| `native` | Use operating-system notifications (default) |
| `poll` | Scan files periodically; useful on network or remote mounts |
| `auto` | Try native notifications and fall back to polling if setup fails |

## Tune restart timing

Changes are debounced for one second by default. A batch of saves produces one
restart after the changes settle.

```toml
[settings.supervisor]
file_watch_debounce = "1s"
watch_poll_interval = "500ms"
watch_interval = "10s"
```

`watch_poll_interval` controls file scans in polling mode. `watch_interval`
controls refreshes of watched daemon configuration; it is not the debounce.
Restart the supervisor after changing its settings.

## If a file change does nothing

1. Check that the daemon is running with `pitchfork status api`.
2. Check the path relative to the config's project directory, especially with `dir`.
3. On a remote mount, try `watch_mode = "poll"` and restart the daemon.
4. Inspect [supervisor logs](/troubleshooting#enable-debug-logging) for watch registration errors.

Combine watching with [ready checks](/guides/ready-checks) to verify each restart,
and [retries](/guides/auto-restart) to recover from a failed attempt.
