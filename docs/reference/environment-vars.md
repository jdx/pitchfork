# Environment Variables

Environment variables that control pitchfork behavior.

## `PITCHFORK_LOG`

Controls log verbosity for the pitchfork supervisor.

**Values:** `error`, `warn`, `info`, `debug`, `trace`

```bash
# Enable debug logging
PITCHFORK_LOG=debug pitchfork supervisor start --force
```

::: tip
The supervisor only reads this variable at startup. Use `--force` to restart the supervisor with new log settings.
:::

## `PITCHFORK_WEB_BIND_PORT`

Sets the default port for the web UI in persistent settings. The web UI is disabled by default — use `PITCHFORK_WEB_PORT` (or `--web-port`) to enable it for a single invocation.

```bash
# Persist the default port in environment
export PITCHFORK_WEB_BIND_PORT=19876
```

If the specified port is in use, pitchfork tries up to `port_attempts` consecutive ports.

## `PITCHFORK_WEB_PORT`

Enables the web UI on the specified port for this invocation. The web UI is disabled by default.

```bash
PITCHFORK_WEB_PORT=19876 pitchfork supervisor start --force
```

If the specified port is in use, pitchfork tries up to 10 consecutive ports.

## Daemon Process Variables

These environment variables are automatically set for every daemon process and its [lifecycle hooks](/guides/lifecycle-hooks).

### `PITCHFORK_DAEMON_ID`

The daemon's fully-qualified identifier in `namespace/name` format (e.g. `my-project/api`).

```bash
# In your daemon script
echo "I am daemon: $PITCHFORK_DAEMON_ID"
```

### `PITCHFORK_DAEMON_NAMESPACE`

The daemon's namespace component alone (e.g. `my-project`). Useful when you only need the
namespace part without parsing `PITCHFORK_DAEMON_ID`.

```bash
echo "Running in namespace: $PITCHFORK_DAEMON_NAMESPACE"
```

### `PITCHFORK_RETRY_COUNT`

The current retry attempt number. `0` on the initial run, `1` on the first retry, etc.

```bash
# In your daemon script
if [ "$PITCHFORK_RETRY_COUNT" -gt 0 ]; then
  echo "This is retry attempt $PITCHFORK_RETRY_COUNT"
fi
```

### `PITCHFORK_EXIT_CODE`

The exit code from the daemon process. Available in `on_fail`, `on_stop`, and `on_exit` hooks.

```bash
# In an on_exit hook
echo "Daemon exited with code: $PITCHFORK_EXIT_CODE"
```

### `PITCHFORK_EXIT_REASON`

The reason the daemon stopped. Available in `on_stop` and `on_exit` hooks.

| Value | Meaning |
|-------|---------|
| `stop` | Explicitly stopped by pitchfork (`pitchfork stop`, `auto = ["stop"]`, or supervisor shutdown) |
| `exit` | Process exited on its own with exit code 0 |
| `fail` | Process exited with a non-zero exit code |

```bash
# In an on_exit hook
if [ "$PITCHFORK_EXIT_REASON" = "fail" ]; then
  echo "Daemon crashed with code $PITCHFORK_EXIT_CODE"
fi
```

## Example: Debug Setup

Start the supervisor with debug logging and web UI enabled:

```bash
PITCHFORK_LOG=debug PITCHFORK_WEB_PORT=19876 pitchfork supervisor start --force

# View supervisor logs
pitchfork logs pitchfork
```
