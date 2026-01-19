# Shell Hook (Auto Start/Stop)

Automatically start daemons when you enter a project directory and stop them when you leave.

## Install the Shell Hook

Add the shell hook to your shell configuration:

::: code-group

```bash [Bash]
echo 'eval "$(pitchfork activate bash)"' >> ~/.bashrc
```

```bash [Zsh]
echo 'eval "$(pitchfork activate zsh)"' >> ~/.zshrc
```

```bash [Fish]
echo 'pitchfork activate fish | source' >> ~/.config/fish/config.fish
```

:::

Restart your shell or source your config file for changes to take effect.

## Configure Daemons

In your project's `pitchfork.toml`, add the `auto` option:

```toml
[daemons.api]
run = "npm run server:api"
auto = ["start", "stop"]  # Auto-start and auto-stop

[daemons.database]
run = "postgres -D /var/lib/pgsql/data"
auto = ["start"]  # Auto-start only, stays running when you leave

[daemons.worker]
run = "npm run worker"
auto = ["stop"]  # Manually start, auto-stops when you leave
```

## Auto Options

| Value | Behavior |
|-------|----------|
| `["start"]` | Daemon starts automatically when entering the directory |
| `["stop"]` | Daemon stops automatically when leaving the directory |
| `["start", "stop"]` | Both auto-start and auto-stop |

## How It Works

1. When you `cd` into a directory containing `pitchfork.toml`, daemons with `auto = ["start", ...]` are started
2. When you `cd` out of the directory, daemons with `auto = [..., "stop"]` are marked for stopping
3. Pitchfork waits a few seconds before actually stopping, in case you quickly return to the directory
4. If no terminal sessions are still in the directory, the daemons stop

::: tip
You can manually start daemons with `pitchfork start` and they will still auto-stop when you leave if configured with `auto = ["stop"]`.
:::

## Example Workflow

```bash
# Enter your project directory
cd ~/projects/myapp
# api daemon starts automatically

# Work on your code...

# Leave the project
cd ~
# After a delay, api daemon stops (if no other terminals are in ~/projects/myapp)
```
