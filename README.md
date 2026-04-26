<div align="center">

<h1 align="center">
  <a href="https://pitchfork.en.dev">
    <img src="docs/public/img/logo.png" alt="pitchfork" width="256" height="256" />
    <br>
    pitchfork
  </a>
</h1>

<p>
  <a href="https://crates.io/crates/pitchfork-cli"><img alt="Crates.io" src="https://img.shields.io/crates/v/pitchfork-cli?style=for-the-badge&color=00d9ff"></a>
  <a href="https://github.com/jdx/pitchfork/blob/main/LICENSE"><img alt="GitHub" src="https://img.shields.io/github/license/jdx/pitchfork?style=for-the-badge&color=52e892"></a>
  <a href="https://github.com/jdx/pitchfork/actions/workflows/ci.yml"><img alt="GitHub Workflow Status" src="https://img.shields.io/github/actions/workflow/status/jdx/pitchfork/ci.yml?style=for-the-badge&color=ff9100"></a>
</p>

<p><b>Daemons with DX</b></p>

<p align="center">
  <a href="https://pitchfork.en.dev/quickstart.html">Quick Start</a> •
  <a href="https://pitchfork.en.dev">Documentation</a> •
  <a href="https://pitchfork.en.dev/cli">CLI Reference</a>
</p>

<hr />

</div>

## What is it?

Pitchfork is a CLI for managing daemons with a focus on developer experience.

- **Start services once** — Only start daemons if they have not already been started
- **Auto start/stop** — Shell hook starts daemons when entering a project directory, stops when leaving
- **Ready checks** — Delay, output regex, HTTP endpoint, TCP port, or custom command
- **Dependency management** — Topological start ordering with parallel execution
- **File watching** — Auto-restart daemons when source files change
- **Cron scheduling** — Schedule recurring tasks with configurable retrigger modes
- **Lifecycle hooks** — Run commands on ready, fail, retry, stop, and exit events
- **Resource limits** — Enforce memory and CPU limits per daemon
- **TUI & Web UI** — Interactive terminal dashboard and browser-based interface
- **MCP server** — Expose daemon management to AI assistants (Claude, Cursor, etc.)
- **Container mode** — Run as PID 1 with zombie reaping and signal forwarding

## Use Cases

- Launching development services like web APIs and databases
- Running rsync/unison to synchronize directories with a remote machine
- Managing background processes for your project
- Running pitchfork as a container entrypoint (Docker/Kubernetes)

## Quickstart

### Install pitchfork

[mise-en-place](https://mise.jdx.dev) is the recommended way to install pitchfork:

```sh-session
$ mise use -g pitchfork
```

Or install via cargo:

```sh-session
$ cargo install pitchfork-cli
```

Or download from [GitHub releases](https://github.com/jdx/pitchfork/releases).

### Launch a one-off daemon

Run a process in the background—an alternative to shell jobs (`mytask &`):

```sh-session
$ pitchfork run docs -- npm start docs-dev-server
```

### Add daemons to your project

Create a `pitchfork.toml` in your project root:

```toml
[daemons.redis]
run = "redis-server"

[daemons.api]
run = "npm run server:api"

[daemons.docs]
run = "npm run server:docs"
```

Start all daemons or multiple daemons **in parallel**:

```sh-session
$ pitchfork start --all

$ pitchfork start redis api
```

### Shell hook (auto start/stop)

Enable automatic daemon management when entering/leaving project directories:

```sh-session
echo 'eval "$(pitchfork activate bash)"' >> ~/.bashrc
echo 'eval "$(pitchfork activate zsh)"' >> ~/.zshrc
echo 'pitchfork activate fish | source' >> ~/.config/fish/config.fish
```

Configure daemons with auto start/stop:

```toml
[daemons.api]
run = "npm run server:api"
auto = ["start", "stop"]
```

### View logs

View daemon logs:

```sh-session
$ pitchfork logs api
[2021-08-01T12:00:00Z] api: starting
[2021-08-01T12:00:01Z] api: listening on
```

Logs will be saved to `~/.local/state/pitchfork/logs`.

## Example Project

Here's a complete example showing how to use pitchfork for a development environment:

```toml
# pitchfork.toml
[daemons.postgres]
run = "docker run --rm -p 5432:5432 -e POSTGRES_PASSWORD=dev postgres:16"
auto = ["start", "stop"]
ready_delay = 5

[daemons.redis]
run = "redis-server --port 6379"
auto = ["start", "stop"]
ready_delay = 2

[daemons.api]
run = "npm run dev:api"
auto = ["start", "stop"]
ready_output = "listening on"
depends = ["postgres", "redis"]

[daemons.worker]
run = "npm run dev:worker"
auto = ["start"]
depends = ["postgres", "redis"]

[daemons.sync]
run = "rsync -avz --delete remote:/data/ ./local-data/"
cron = { schedule = "0 */5 * * * *" }  # Run every 5 minutes
```

Start everything:

```sh-session
$ pitchfork start --all
```

## Full Documentation

See [pitchfork.en.dev](https://pitchfork.en.dev)

## Contributors

[![Contributors](https://contrib.rocks/image?repo=jdx/pitchfork)](https://github.com/jdx/pitchfork/graphs/contributors)
