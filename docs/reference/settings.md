# Settings Reference

This page documents all configurable settings for pitchfork. Settings can be configured via:

1. **Environment variables** (highest priority)
2. **Project-level:** `pitchfork.toml` or `pitchfork.local.toml` in `[settings]` section
3. **User-level:** `~/.config/pitchfork/config.toml` in `[settings]` section
4. **System-level:** `/etc/pitchfork/config.toml` in `[settings]` section

Settings are merged in precedence order, with later sources overriding earlier ones.

Settings for supervisor-owned services, such as `[settings.web]` and
`[settings.proxy]`, are resolved when the supervisor process starts. After that,
they do not hot-reload: changing any setting requires restarting the supervisor
with `pitchfork supervisor start --force` for the change to take effect. This
applies regardless of whether the setting is in a project-level or global config
file.

## Configuration in pitchfork.toml

Add a `[settings]` section to any `pitchfork.toml` file:

```toml
# Daemon definitions
[daemons.myapp]
run = "node server.js"

# Settings configuration
[settings.general]
autostop_delay = "5m"
log_level = "debug"

[settings.logs]
time_retention = "7d"

[settings.tui]
refresh_rate = "1s"

[settings.supervisor]
file_watch_debounce = "2s"
```

## Global Configuration

For user-wide settings, create `~/.config/pitchfork/config.toml`:

```toml
# Global daemons (e.g., database services)
[daemons.postgres]
run = "postgres -D /usr/local/var/postgres"

# Global settings
[settings.general]
log_level = "info"

[settings.web]
auto_start = true
```

## Environment Variables

Every setting has a corresponding environment variable that overrides all file configurations:

```bash
# Override via environment
export PITCHFORK_LOG=debug
export PITCHFORK_WEB_AUTO_START=true
export PITCHFORK_AUTOSTOP_DELAY=5m
```

<script setup>
import SettingsTable from '../components/SettingsTable.vue'
</script>

## All Settings

<SettingsTable />
