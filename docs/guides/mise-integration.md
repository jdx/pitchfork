---
description: Run daemons with mise-managed tools and environment variables, including outside an interactive shell.
---
# mise integration

[mise](https://mise.jdx.dev) supplies project tools and environment variables.
Pitchfork manages the processes that use them. Enable the integration when a
daemon needs mise's environment, especially at login or from a non-interactive
supervisor.

## Enable it for a daemon

```toml
[daemons.api]
run = "node server.js"
mise = true
```

Pitchfork runs the command as `mise x -- sh -c "node server.js"` by default.
If you configure `general.shell` (or `general.windows_shell` on Windows), that
shell is used inside `mise x --`.
Shell expansion, pipes, and compound commands retain their normal behavior.

## Make it the default

In `~/.config/pitchfork/config.toml`:

```toml
[settings.general]
mise = true
```

A daemon's `mise = true` or `mise = false` overrides the global default.
Without either setting, the integration is disabled.

## Locate mise

Pitchfork searches these well-known locations:

- `~/.local/bin/mise`
- `~/.cargo/bin/mise`
- `/usr/local/bin/mise`
- `/opt/homebrew/bin/mise`
- on Windows, `mise.exe` on `PATH`

For another location, set an absolute path:

```toml
[settings.general]
mise = true
mise_bin = "/opt/tools/mise"
```

`mise_bin` in a project's `pitchfork.toml` applies to that project's daemons,
whichever directory the supervisor was started in. A relative path is taken
from the project's directory, so a project can point at a mise it ships:

```toml
[settings.general]
mise = true
mise_bin = "tools/mise"
```

`PITCHFORK_MISE_BIN` in the environment of the `pitchfork` command that starts a
daemon takes precedence over the project's files. When the project does not set
`mise_bin`, the supervisor uses its own setting, with a relative path taken from
the directory the supervisor was started in, and otherwise searches the
locations above. A `mise_bin` in your user or system configuration file is
treated as the supervisor's setting, not the project's.

If mise cannot be found, pitchfork logs a warning and runs without it. Check
the supervisor logs if a daemon cannot find its runtime.

## Templates that use mise variables

A `run` command can use [templates](/guides/configuration-templates). Pitchfork renders
the variables it defines, such as `daemons.*`, `url`, and `name`. When pitchfork cannot
render a command, for example because it uses a mise `[vars]` entry or a filter
pitchfork does not provide, and `mise x` will wrap the daemon, the command is passed
through unrendered and `mise x` finishes it with mise's own variables and filters:

```toml
[daemons.api]
run = "exec node server.js --port {{ daemons.redis.port }} --title {{ vars.title | quote }}"
mise = true
```

Pitchfork gives `mise x` its template variables, so `{{ daemons.redis.port }}` above
still resolves. It passes them in the `PITCHFORK_TEMPLATE_CONTEXT` environment variable
as JSON, which mise reads and removes before running the command.

::: warning Requires a mise that renders deferred commands
Only a mise release that reads `PITCHFORK_TEMPLATE_CONTEXT` (jdx/mise#13894) renders a
deferred command. With an older mise the command reaches the shell with its template
tags unrendered, so update mise before relying on this.
:::

Deferral applies only when `mise x` will actually wrap the daemon: `mise = true` on the
daemon, or `general.mise = true` in the settings of the daemon's project. Otherwise an
unresolved variable is a render error, as before. If mise is enabled but its binary
cannot be found when the daemon starts, the start fails rather than running the
command with unrendered tags. Only `run` is deferred; other fields, including `env`, must use variables
pitchfork defines.

## Run a mise task

You can also make a mise task the daemon command. For example, in `pitchfork.toml`:

```toml
[daemons.api]
run = "mise run api:dev"
ready_http = { url = "http://127.0.0.1:3000/health", timeout = "30s" }
auto = ["start", "stop"]
```

And in `mise.toml`:

```toml
[tools]
node = "24"

[env]
NODE_ENV = "development"

[tasks."api:setup"]
run = "npm install"

[tasks."api:dev"]
depends = ["api:setup"]
run = "node server.js"
```

`pitchfork start api` invokes the task, and mise handles its task dependencies
and environment. Pitchfork waits for the health endpoint and then monitors the
process. This example assumes your application exposes `/health` on port 3000.

With a literal `mise run` command, the shell must be able to find `mise`;
neither `mise = true` nor `mise_bin` puts it on `PATH`. Use an absolute command
path when the supervisor's `PATH` is limited.

See [boot registration](/guides/boot-start) and [cron scheduling](/guides/scheduling)
for workflows that run outside your interactive shell.

## Attached configuration

Generators can attach files outside a project through the namespace registry.
See [external configuration files](/guides/external-configs) for registration, precedence, and project-relative paths.
