# `pitchfork config add`

- **Usage**: `pitchfork config add [--autostart] [--autostop] <ID> [ARGS]...`
- **Aliases**: `a`

Add a new daemon to ./pitchfork.toml

## Arguments

### `<ID>`

ID of the daemon to add

### `[ARGS]...`

Arguments to pass to the daemon

## Flags

### `--autostart`

Autostart the daemon when entering the directory

### `--autostop`

Autostop the daemon when leaving the directory
