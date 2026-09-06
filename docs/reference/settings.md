---
description: Configure pitchfork defaults, inspect effective values, and find which file or environment variable set them.
---
# Settings

Settings control pitchfork itself: logging, shell execution, the supervisor,
dashboards, and the reverse proxy. Daemon-specific behavior belongs under
`[daemons.<name>]`; see the [configuration reference](/reference/configuration).

**[Browse every setting, default, and environment variable →](/cli/configuration)**

## Inspect the effective value

```sh
pitchfork settings
pitchfork settings list --group supervisor
pitchfork settings get general.autostop_delay
pitchfork settings explain general.autostop_delay
```

`get` returns the value. `explain` shows which environment variable or file won,
which is useful when a local override seems to be ignored.

## Change a setting

```sh
pitchfork settings set general.autostop_delay 30s --project
pitchfork settings set web.auto_start true --global
```

| Target | File |
| --- | --- |
| `--project` (default) | `pitchfork.toml` |
| `--local` | `pitchfork.local.toml` for personal project overrides |
| `--global` | `~/.config/pitchfork/config.toml` for user-wide defaults |

Or edit the file directly:

```toml
[settings.general]
autostop_delay = "30s"

[settings.logs]
time_retention = "7d"

[settings.supervisor]
file_watch_debounce = "1s"

[settings.web]
auto_start = true
bind_port = 3120
```

## Precedence

From lowest to highest priority:

1. Built-in defaults.
2. `/etc/pitchfork/config.toml`.
3. User config (`~/.config/pitchfork/config.toml`).
4. Project configs, from filesystem root down to the current directory.
5. Environment variables.

Within each directory, the four project files follow the
[configuration hierarchy](/reference/configuration#configuration-hierarchy).
Each file stores settings in `[settings]` sections.

```sh
PITCHFORK_AUTOSTOP_DELAY=5m pitchfork settings explain general.autostop_delay
```

This environment variable wins over the file configuration for this invocation.

## When changes take effect

Client commands resolve settings from their environment and working directory.
Supervisor-owned services, including the web UI and proxy, read settings when
the supervisor starts. Restart it to apply changes:

```sh
pitchfork supervisor start --force
```

Use the user config for supervisor settings you want to apply consistently
across projects. A setting exported in a later terminal does not change the
environment of an already running supervisor.

See [environment variables](/reference/environment-vars) for process metadata
and invocation controls, and the [generated settings reference](/cli/configuration)
for the full list of supported keys.
