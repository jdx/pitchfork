---
description: Install pitchfork, run a background HTTP server, inspect its logs, and stop it again.
---
# Quickstart

Run a background HTTP server, check that it is ready, and stop it when you're done.
This walkthrough needs **Python 3** and an available **port 8000**.

## 1. Install pitchfork

```sh
mise use -g pitchfork
pitchfork --version
```

If you don't use mise, install with `cargo install pitchfork-cli --locked` or
choose a binary from the [installation guide](/installation).

## 2. Start a daemon

Run this from a directory whose files you want to serve:

```sh
pitchfork run demo --port 8000 -- \
  python3 -u -m http.server 8000 --bind 127.0.0.1
```

`demo` is the name pitchfork uses to track this process. Everything after `--`
is the command to run. `--port 8000` makes pitchfork wait for a TCP connection
to succeed before returning. Python's `-u` makes log output appear promptly.

Open [http://127.0.0.1:8000](http://127.0.0.1:8000) in your browser. The server
keeps running after the command finishes, even if you close this terminal.
Pitchfork starts its background supervisor automatically.

## 3. Check status and logs

```sh
pitchfork status demo
pitchfork logs demo --tail
```

Refresh the browser to produce a request log. Press `Ctrl+C` to leave the log
viewer; the server stays running. Use `pitchfork list` to see all tracked and
available daemons, or `pitchfork tui` for the interactive dashboard.

## 4. Stop and clean up

```sh
pitchfork stop demo
pitchfork clean --daemon demo
```

`stop` terminates the process. `clean` removes its stopped entry from the daemon
list; it does not delete logs or configuration.

## Next: save a project configuration

One-off commands are useful for trying things out. A `pitchfork.toml` makes your
services repeatable and shareable. Continue with [your first project](/first-daemon)
to add dependencies, readiness checks, and project-scoped commands.

If this example fails, check `python3 --version`, choose another port in both
places in the command, or see [troubleshooting](/troubleshooting).
