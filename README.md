<div align="center">
  <a href="https://pitchfork.jdx.dev"><img src="docs/public/img/logo.png" alt="pitchfork" width="160" height="160" /></a>
  <h1>pitchfork</h1>
  <p><strong>Your project's background services, under control.</strong></p>
  <p>Start once. Wait for readiness. Get back to work.</p>
  <p>
    <a href="https://crates.io/crates/pitchfork-cli"><img alt="Crates.io version" src="https://img.shields.io/crates/v/pitchfork-cli?color=c44b35" /></a>
    <a href="https://github.com/jdx/pitchfork/actions/workflows/ci.yml"><img alt="CI status" src="https://img.shields.io/github/actions/workflow/status/jdx/pitchfork/ci.yml" /></a>
    <a href="LICENSE"><img alt="MIT license" src="https://img.shields.io/github/license/jdx/pitchfork" /></a>
  </p>
  <p><a href="https://pitchfork.jdx.dev/quickstart">Quickstart</a> · <a href="https://pitchfork.jdx.dev/guides/">Guides</a> · <a href="https://pitchfork.jdx.dev/cli/">CLI reference</a></p>
</div>

Pitchfork manages the long-running commands your project needs: an API, a database,
a frontend server, or a background worker. Define them in `pitchfork.toml`, then
start, inspect, and stop them from any terminal. A background supervisor keeps
track of the processes and their logs after the CLI exits.

## Get started

Install with [mise](https://mise.jdx.dev):

```sh
mise use -g pitchfork
```

You can also use `cargo install pitchfork-cli --locked` or download a binary from
[GitHub releases](https://github.com/jdx/pitchfork/releases).
See [installation](https://pitchfork.jdx.dev/installation) for platform details and shell completion.

Try a daemon without a config file (requires Python 3):

```sh
pitchfork run demo --port 8000 -- \
  python3 -u -m http.server 8000 --bind 127.0.0.1
pitchfork status demo
pitchfork logs demo --tail
# Ctrl+C leaves the daemon running. Stop it when you're done:
pitchfork stop demo
```

Open [localhost:8000](http://localhost:8000) while the daemon is running.

## Put your services in version control

Create `pitchfork.toml` in your project. This example assumes Redis is installed
and your project has a `server.js` that reads `PORT` and `REDIS_URL`:

```toml
#:schema https://pitchfork.jdx.dev/schema.json

[daemons.redis]
run = "redis-server --port $PORT"
port = 6379
ready_cmd = "redis-cli -p $PORT ping"

[daemons.api]
run = "node server.js"
port = 3000
depends = ["redis"]
env = { REDIS_URL = "redis://127.0.0.1:{{ daemons.redis.port }}" }
ready_http = { url = "http://127.0.0.1:3000/health", timeout = "30s" }
retry = 3
```

```sh
pitchfork start api       # Starts Redis first, then waits for the API's health check
pitchfork start --local   # Starts all daemons in the project's merged local config
pitchfork list --project  # Shows this project's daemons
pitchfork restart api     # Applies config changes and restarts the API
pitchfork stop --local    # Stops local services in reverse dependency order
```

Starting an already running daemon leaves it running. Independent dependencies
start in parallel. `--all` includes global daemons as well as local ones.

## Make it fit your workflow

| When you need to… | Use… |
| --- | --- |
| Start services when you enter a project | [Shell hooks](https://pitchfork.jdx.dev/guides/shell-hook) with `auto = ["start", "stop"]` |
| Wait for a service to accept requests | [Ready checks](https://pitchfork.jdx.dev/guides/ready-checks) using output, HTTP, TCP, or a command |
| Recover from a crash or failed health probe | [Retries](https://pitchfork.jdx.dev/guides/auto-restart) and [health checks](https://pitchfork.jdx.dev/guides/health-checks) |
| Restart after a source edit | [File watching](https://pitchfork.jdx.dev/guides/file-watching) with glob patterns |
| Keep a stable URL across port changes | [Port assignment and reverse proxy](https://pitchfork.jdx.dev/guides/port-management) |
| Inspect services and their output | `pitchfork tui`, the [web UI](https://pitchfork.jdx.dev/guides/web-ui), and [structured logs](https://pitchfork.jdx.dev/guides/logs) |
| Run tasks on a schedule | [Cron scheduling](https://pitchfork.jdx.dev/guides/scheduling) |
| Connect an AI assistant | The built-in [MCP server](https://pitchfork.jdx.dev/guides/mcp) |

The docs also cover [namespaces and worktrees](https://pitchfork.jdx.dev/concepts/namespaces),
[mise integration](https://pitchfork.jdx.dev/guides/mise-integration),
[lifecycle hooks](https://pitchfork.jdx.dev/guides/lifecycle-hooks), and
[resource limits](https://pitchfork.jdx.dev/reference/configuration#memory-limit).

## Contribute

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup, checks, and documentation development.
Found a problem? Include your version, config, and relevant logs in a
[GitHub issue](https://github.com/jdx/pitchfork/issues).

## Sponsors

<p align="center">
  Sponsored by<br><br>
  <a href="https://entire.io">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://jdx.dev/sponsors/entire-lockup.svg">
      <img src="https://jdx.dev/sponsors/entire-lockup-on-light.svg" alt="Entire" height="36">
    </picture>
  </a>
  &nbsp;&nbsp;&nbsp;
  <a href="https://omarchy.org/patrons/">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://jdx.dev/sponsors/omacom-foundation.svg">
      <img src="https://jdx.dev/sponsors/omacom-foundation-on-light.svg" alt="Omacom Foundation" height="36">
    </picture>
  </a>
  <br><br>
  <a href="https://jdx.dev/sponsors.html">View all sponsors</a>
</p>


## Contributors

[![Contributors](https://contrib.rocks/image?repo=jdx/pitchfork)](https://github.com/jdx/pitchfork/graphs/contributors)
