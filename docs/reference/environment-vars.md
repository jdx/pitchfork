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

## `PITCHFORK_WEB_PORT`

Enables the web UI on the specified port. The web UI is disabled by default.

```bash
PITCHFORK_WEB_PORT=19876 pitchfork supervisor start --force
```

If the specified port is in use, pitchfork tries up to 10 consecutive ports.

## Example: Debug Setup

Start the supervisor with debug logging and web UI enabled:

```bash
PITCHFORK_LOG=debug PITCHFORK_WEB_PORT=19876 pitchfork supervisor start --force

# View supervisor logs
pitchfork logs pitchfork
```
