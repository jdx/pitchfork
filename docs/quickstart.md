# Quick Start

Get pitchfork running in under 5 minutes.

## Install

```bash
# Using mise (recommended)
mise use -g pitchfork

# Or with cargo
cargo install pitchfork-cli

# Or download from GitHub releases
# https://github.com/jdx/pitchfork/releases
```

## Run Your First Daemon

Start a background process with a single command:

```bash
pitchfork run myserver -- python -m http.server 8000
```

This starts a Python HTTP server in the background, labeled "myserver".

## Check Status

```bash
pitchfork list
```

You'll see output like:

```
NAME       PID    STATUS
myserver   12345  running
```

## View Logs

```bash
pitchfork logs myserver
```

Or follow logs in real-time:

```bash
pitchfork logs myserver --tail
```

## Stop the Daemon

```bash
pitchfork stop myserver
```

## What's Next?

- [Installation](/installation) - All installation methods and shell completion
- [Your First Project](/first-daemon) - Set up a project with multiple daemons
- [Shell Hook](/guides/shell-hook) - Auto-start daemons when entering a directory
