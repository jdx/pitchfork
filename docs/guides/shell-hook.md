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

## IDE / Project Session Integration

IDEs and other long-running project tools can opt into the same auto-start and auto-stop behavior without relying on the shell hook. A single host process can manage multiple workspaces by calling `enter` once per directory with the same `--pid`.

### `pitchfork project enter`

Mark a directory session as active for the given host process. Pitchfork resolves the project from the directory, then starts daemons with `auto = ["start", ...]` just as the shell hook does.

```bash
pitchfork project enter --pid 12345
```

| Option | Description |
|--------|-------------|
| `--pid PID` | **Required.** PID of the host process (IDE or shell). Used as a crash-cleanup anchor. |
| `--directory DIR` | Optional. Project directory. Defaults to the current working directory. |

### `pitchfork project leave`

Mark a directory session as inactive for the given host process. Daemons with `auto = [..., "stop"]` become eligible for auto-stop, subject to the same delay and multi-session checks as the shell hook.

```bash
pitchfork project leave --pid 12345
```

### `pitchfork project list`

List currently active project sessions. Each row shows the host PID, directory, and liveness state (`alive` while the host process is running, `dead` once it has exited but before cleanup completes).

```bash
pitchfork project list
```

Pass `--json` for machine-readable output suitable for IDE integrations and scripting:

```bash
pitchfork project list --json
```

Example output:

```bash
$ pitchfork project list
PID     DIRECTORY          STATUS   TITLE
12345   ~/projects/app     alive    code
12345   ~/projects/lib     alive    code
54321   ~/projects/app     alive    fish
```

### Example

An IDE managing multiple folders in one window uses the same `--pid` with different `--directory` values:

```bash
# Open first folder
pitchfork project enter --pid 12345 --directory ~/projects/app

# Open second folder in the same IDE
pitchfork project enter --pid 12345 --directory ~/projects/lib

# Close first folder
pitchfork project leave --pid 12345 --directory ~/projects/app

# Close IDE entirely
pitchfork project leave --pid 12345 --directory ~/projects/lib
```

Multiple terminals in the same project use distinct `--pid` values, so each can leave independently:

```bash
# Terminal A (bash/zsh)
pitchfork project enter --pid $$
pitchfork project leave --pid $$

# Terminal B in the same directory
pitchfork project enter --pid $$
pitchfork project leave --pid $$
```

::: tip
The shell variable for the current PID differs by shell: `$$` in bash/zsh, `$fish_pid` in fish, `$SHELL_PID` in nushell.
:::

If the host process crashes, the supervisor notices the PID is gone and treats the session as left, triggering the same auto-stop evaluation. This automatic crash cleanup relies on the host PID being visible to the process table, so it only applies on Unix. On Windows (where Git Bash `$$` is a Cygwin-internal PID invisible to the process table), sessions are not revoked automatically and must be ended with an explicit `project leave`.
