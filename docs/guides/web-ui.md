---
description: Enable the browser dashboard and configure its address, path prefix, and API access.
---
# Web UI

Use the web dashboard to inspect daemon status, start and stop services, and follow logs from your browser. It connects to the same supervisor as the CLI and TUI.

<div class="webui-screenshots">
  <img loading="lazy" decoding="async" src="/img/webui-pc.png" alt="Web UI dashboard on desktop" />
  <img loading="lazy" decoding="async" src="/img/webui-phone.png" alt="Web UI on mobile" />
</div>

<style scoped>
/* Side-by-side screenshots at equal visual height.
   flex-grow ratios match each image's aspect ratio, so when both
   scale to `height: auto` they render at exactly the same height
   while always filling the content width. */
.webui-screenshots {
  display: flex;
  gap: 1rem;
  align-items: center;
  justify-content: center;
  margin: 1.5rem 0;
}
.webui-screenshots img {
  height: auto;
  min-width: 0;
  display: block;
  border-radius: 8px;
  border: 1px solid var(--vp-c-divider);
}
.webui-screenshots img:first-child {
  flex: 1.1297 1 0%;
}
.webui-screenshots img:last-child {
  flex: 0.5184 1 0%;
}
@media (max-width: 480px) {
  .webui-screenshots {
    flex-direction: column;
  }
  .webui-screenshots img:first-child,
  .webui-screenshots img:last-child {
    flex: none;
    width: 100%;
  }
}
</style>

## Enable the Web UI

The web UI is disabled by default. There are several ways to enable it:

### One-time via CLI or environment variable

```bash
# Via CLI flag (foreground)
pitchfork supervisor run --web-port 3120

# Via environment variable (works with both run and start)
PITCHFORK_WEB_PORT=3120 pitchfork supervisor start --force
```

### Persistent via settings

The web UI is owned by the supervisor process, so its settings are read once at
supervisor startup and do not hot-reload. Changing `[settings.web]` in any
config file requires restarting the supervisor with
`pitchfork supervisor start --force` for the change to take effect.

Add this to `~/.config/pitchfork/config.toml` for a consistent user-wide setup:

```toml
[settings.web]
auto_start = true    # Start web UI automatically with supervisor
bind_port = 3120     # Default port (default: 3120)
bind_address = "127.0.0.1"  # Default: localhost only
```

Or via environment variables:

```bash
export PITCHFORK_WEB_AUTO_START=true
export PITCHFORK_WEB_BIND_PORT=3120
```

Then restart the supervisor:

```bash
pitchfork supervisor start --force
```

Open http://127.0.0.1:3120 in your browser.

If the specified port is in use, pitchfork tries up to 10 consecutive ports, including the requested port, (configurable via `web.port_attempts`).

### Path prefix

You can serve the web UI under a sub-path, useful when running behind a reverse proxy:

```bash
pitchfork supervisor run --web-port 3120 --web-path ps
# Web UI available at http://127.0.0.1:3120/ps/
```

Or via settings:

```toml
[settings.web]
auto_start = true
base_path = "ps"
```

## Standalone API Server

By default, the REST API is bundled with the web UI on the same port. You can run the API on a dedicated port separate from the web UI:

```toml
[settings.api]
auto_start = true
bind_port = 8080          # Dedicated API port
bind_address = "127.0.0.1"
port_attempts = 10
```

When `api.auto_start = true` and a valid `api.bind_port` are set, the API endpoints are available on that port without the static file serving. This is useful when you only need programmatic access and not the browser UI.

## Authentication

The API uses token-based authentication when binding to non-loopback addresses.
The web UI and standalone API serve plain HTTP: the token does not encrypt the
connection. Keep their listeners on loopback, or use an HTTPS reverse proxy on
the same host with a loopback HTTP backend for remote access. Sending `X-Pitchfork-Token`
directly over a network via HTTP lets anyone who can observe the traffic
capture and reuse the token.

- **Loopback only** (`127.0.0.1`, `::1`): No token is required by default. If you configure a token, it is enforced for local requests too.
- **Non-loopback** (e.g., `0.0.0.0`, LAN IP): If no token is configured, a random 64-character hex token is auto-generated at startup. The generated token is printed to stderr and logged.

To require a token on a loopback backend behind your HTTPS reverse proxy,
configure one explicitly:

```toml
[settings.api]
token = "replace-with-a-long-random-token"
```

The proxy must forward `X-Pitchfork-Token` unchanged to the loopback API, which
validates it. HTTPS protects the client-to-proxy connection; the HTTP hop to
`127.0.0.1` stays on the same host. If the proxy runs on another host, use an
authenticated encrypted tunnel to the API host instead of forwarding the
token over a plain HTTP network connection. Pitchfork's API listener does not
support TLS directly.

Include the token in every request when one is configured. This example assumes
you have configured an HTTPS reverse proxy at `pitchfork.example.com`:

```bash
curl -H "X-Pitchfork-Token: <token>" https://pitchfork.example.com/api/daemons
```

::: warning
Never expose the API to a public network without authentication. The bundled web page receives the API token so it can make requests. The token is not a login barrier for the web dashboard; restrict network access to trusted clients or enforce access control at the reverse proxy.
:::

## API reference

See the [HTTP API reference](/reference/http-api) for daemon control, log streaming,
namespace management, project and stack data, and response examples. The
`/api/projects` endpoints return the same data the project and stack pages
show, so mise and other tools can consume it.

## Features

### Dashboard

Overview of all daemons showing:
- Name and status (running, stopped, failed)
- Process ID (PID)
- Error messages for failed daemons

### Daemon Management

Control daemons directly from the browser:
- **Start** — Launch a stopped daemon
- **Stop** — Gracefully stop a running daemon
- **Restart** — Stop and start a daemon
- **Enable/Disable** — Control whether a daemon can be started

### Projects and stacks

The **Projects** tab lists every project registered in the namespace registry.
A project is one registered namespace; its worktrees are the git worktrees or
jj workspaces found under the project directory.

A namespace registered on a linked worktree is shown under its main checkout's
project rather than as a project of its own, so open the checkout to reach it.

Opening a project shows each worktree with its namespace, running and stopped
daemon counts, last activity, and a link to that worktree's stack. Last activity
comes from process uptime, so a worktree with nothing running shows none even if
its daemons ran earlier. A project whose registered directory has been deleted
is listed and marked "missing" rather than silently looking healthy. Worktrees
that pitchfork knows about but has never started are listed too, with their
daemons marked available. The primary checkout's stack is shown on the same
page under "Stack · primary checkout". Per-worktree disk usage is only shown
when pitchfork tracks a data directory for the daemons, which it does not do
today, so the column stays hidden rather than reporting a guess.

The supervisor resolves daemon configs from its own project directory and from
the namespace registry. A worktree in neither is still listed with its daemons,
but marked "not startable" and the start and restart actions for exactly those
daemons are disabled, because they would fail with "Daemon config not found";
a restart would stop a running one and then fail to bring it back. Stopping
needs no config, so it stays available. Its stack page offers a **Register
worktree** button that adds the namespace; `pitchfork proxy add` registers one
too, as does adding it under `[namespaces]` in the user config.

A stack page shows the groups declared by the config loaded for that worktree,
in the order they appear, except that a group named `default` comes first and
is presented as the stack's primary action:

```toml
# shop/pitchfork.toml
[daemons.api]
run = "npm run api"

[daemons.worker]
run = "npm run worker"

[groups.default]
daemons = ["api", "worker"]
```

With that config, the stack page for the `shop` project's main worktree offers
**Start stack**, **Stop stack**, and **Restart stack**, which act on
`shop/api` and `shop/worker` through the ordinary daemon control endpoints.
Stopping walks the group in reverse order. Each group also has its own
start, stop, and restart buttons, and every member row links to that daemon's
logs. Daemons in the worktree that no group names are listed under
"ungrouped". See [daemon groups](/reference/configuration#daemon-groups) for
the config format.

### Starting is always a click

A request to a daemon's proxy hostname starts that daemon if it is not already
running. Project and stack pages behave the opposite way: loading one, or
polling the JSON endpoints behind it, never starts anything. A daemon in a
stack starts only when someone clicks Start, Restart, or Start stack.

### Live Logs

Real-time log streaming for each daemon via Server-Sent Events (SSE):
- Select a daemon to view its logs
- Logs update automatically in real-time
- Scroll through historical logs
- Clear logs per daemon

### Config Editing

Edit `pitchfork.toml` files with:
- TOML syntax validation
- Save changes directly from the UI
