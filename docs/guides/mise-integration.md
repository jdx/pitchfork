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
If you configure `general.shell`, that shell is used inside `mise x --`.
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

For another location, set an absolute path:

```toml
[settings.general]
mise = true
mise_bin = "/opt/tools/mise"
```

If mise cannot be found, pitchfork logs a warning and runs without it. Check
the supervisor logs if a daemon cannot find its runtime.

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

With a literal `mise run` command, the shell must be able to find `mise`; the
`mise_bin` setting applies to pitchfork's built-in wrapper. Use `mise = true`
or an absolute command path when the supervisor's `PATH` is limited.

See [boot registration](/guides/boot-start) and [cron scheduling](/guides/scheduling)
for workflows that run outside your interactive shell.
