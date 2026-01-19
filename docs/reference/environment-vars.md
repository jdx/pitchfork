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

Sets the port for the web UI.

**Default:** `19876`

```bash
PITCHFORK_WEB_PORT=8080 pitchfork supervisor start --force
```

If the specified port is in use, pitchfork tries up to 10 consecutive ports.

## `PITCHFORK_NO_WEB`

Disables the web UI entirely.

**Values:** `true`, `1`, `yes`

```bash
PITCHFORK_NO_WEB=true pitchfork supervisor start --force
```

## Example: Debug Setup

Start the supervisor with debug logging and custom port:

```bash
PITCHFORK_LOG=debug PITCHFORK_WEB_PORT=8080 pitchfork supervisor start --force

# View supervisor logs
pitchfork logs pitchfork
```
