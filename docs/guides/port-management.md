---
description: Assign and bump service ports, configure stable local URLs, and enable HTTPS or LAN access.
---
# Port Management & Reverse Proxy

Assign ports to your services, choose another port when one is busy, and give each service a stable local URL with the optional reverse proxy.

## Port Assignment

Configure the ports your daemon expects to use:

```toml
[daemons.api]
run = "node server.js"
port = 3000
```

For multiple ports:

```toml
[daemons.multi]
run = "./start.sh"
port = [8080, 8443]
```

Pitchfork checks availability, injects the resolved ports into the daemon's environment,
and reports a conflict if a port is occupied and bumping is disabled.

**Your application must use the assigned port.** A program that hardcodes a port
will not move just because pitchfork sets `PORT`. For example:

```toml
[daemons.web]
run = "python3 -u -m http.server $PORT --bind 127.0.0.1"
port = { expect = [8000], bump = 10 }
ready_cmd = "curl -fsS http://127.0.0.1:$PORT/"
```

For the Node.js examples below, `server.js` must read `process.env.PORT`.

Resolved ports are exposed via the following environment variables:

| Variable | Description |
|----------|-------------|
| `$PORT` | First resolved port (alias for `$PORT0`) |
| `$PORT0` | First resolved port |
| `$PORT1` | Second resolved port |
| `$PORTN` | Nth resolved port (0-indexed) |

When a single port is configured, both `$PORT` and `$PORT0` are set to the same value. For multiple ports, each port is available at its corresponding index:

```toml
[daemons.multi]
run = "./start.sh --http-port $PORT0 --grpc-port $PORT1"
port = [8080, 8443]
```

Lifecycle hooks receive the same values as namespaced `PITCHFORK_PORT` / `PITCHFORK_PORT0..N` (see [hook environment variables](lifecycle-hooks.md#environment-variables)).

### Auto Port Bumping

When a port is occupied, enable `bump` to automatically find the next available port:

```toml
[daemons.api]
run = "node server.js"
port = { expect = [3000], bump = 10 }  # bump up to 10 times
```

Using `bump = true` enables unlimited bump attempts:

```toml
[daemons.api]
run = "node server.js"
port = { expect = [3000], bump = true }
```

These environment variables reflect the **resolved** port, so they work correctly with auto-bumping. See [Port Assignment](#port-assignment) for the full list of available variables.

### Active Port Tracking

After a daemon starts, pitchfork detects the port the process is actually listening on. This detected port is the source of truth for the reverse proxy.


## Reverse Proxy

The reverse proxy routes requests from stable URLs to the daemon's actual port.

### Why Use the Proxy?

Without the proxy, you need to know the actual port your daemon is running on — which can change if ports are auto-bumped. With the proxy:

```
https://myapp.localhost  →  http://localhost:3001
```

The URL stays the same even if the port changes. This is especially useful for:
- Using the same URL conventions in each teammate's local setup
- AI agents that need stable endpoints
- Browser bookmarks
- Webhook configurations

### Quick start

Start with HTTP on an unprivileged port. Add this to
`~/.config/pitchfork/config.toml` so it applies regardless of the directory
where the supervisor starts:

```toml
[settings.proxy]
enable = true
https = false
port = 8088
```

From a project that defines an `api` daemon:

```sh
pitchfork proxy add api
pitchfork supervisor start --force
pitchfork start api
pitchfork proxy status
```

Open `http://api.localhost:8088` in a browser that resolves `.localhost` names.
The slug maps the URL to your project's `api` daemon. If its name is `server`
instead, use `pitchfork proxy add api --daemon server` and start `server`.

The supervisor reads proxy settings at startup; restart it after editing them.
Continue below for standard ports, HTTPS, custom domains, and LAN access.

### Slugs

Slugs are defined in the global config (`~/.config/pitchfork/config.toml`) under `[slugs]`. Each slug maps to a project directory and (optionally) a specific daemon name:

```toml
# ~/.config/pitchfork/config.toml

[slugs]
api = { dir = "/home/user/my-api", daemon = "server" }
frontend = { dir = "/home/user/my-app", daemon = "dev" }
# If daemon name matches slug, it can be omitted:
docs = { dir = "/home/user/docs-site" }  # defaults daemon = "docs"
```

### URL format

Proxy URLs use this shape:

```
https://<slug>.<tld>
```

Examples:
- `https://myapp.localhost` — standard HTTPS port 443, by default
- `https://api.localhost:7777` — custom port

### Managing slugs

```bash
# Add a slug for current directory
pitchfork proxy add myapp

# Add a slug with explicit dir and daemon
pitchfork proxy add api --dir /home/user/api --daemon server

# Remove a slug
pitchfork proxy remove api
# or: pitchfork proxy rm api

# Show all slugs and their status
pitchfork proxy status
```

## Standard Ports (80/443)

To use standard HTTP/HTTPS ports without the port number in URLs:

```
http://api.localhost   (port 80)
https://api.localhost  (port 443)
```

### Binding to Privileged Ports

If your operating system restricts binding ports below 1024, start the supervisor with `sudo`:

```bash
# HTTP on port 80
sudo PITCHFORK_PROXY_PORT=80 PITCHFORK_PROXY_HTTPS=false pitchfork supervisor start

# HTTPS on port 443 (default)
sudo pitchfork supervisor start
```

Or in `pitchfork.toml`:
```toml
[settings.proxy]
enable = true
port = 80     # requires: sudo pitchfork supervisor start
https = false
```

If binding fails, use an unprivileged port such as `8088` or `8443`, or run the supervisor with the required permissions.


## HTTPS Support

### Auto-Generated Certificate

When `proxy.https = true` (the default) and no certificate is configured, pitchfork auto-generates a self-signed certificate:

```toml
[settings.proxy]
enable = true
# https = true is the default
# port = 443 is the default
```

The certificate is stored in `$PITCHFORK_STATE_DIR/proxy/cert.pem`.

### Auto-Trust

When the proxy starts with HTTPS enabled, pitchfork automatically attempts to
install the CA certificate into your system trust store (`proxy.auto_trust = true`
by default). This means you typically don't need to run any extra commands —
browsers will trust the proxy URLs right away.

On **macOS**, auto-trust triggers a system authorization dialog (Touch ID or
password) the first time. Subsequent starts skip the prompt because the
certificate is already trusted.

On **Linux**, auto-trust requires write access to the system CA directory, which
typically means the supervisor must be started with `sudo`. If auto-trust fails
(e.g. due to permissions), it is silently skipped and a warning is logged.

To disable auto-trust:

```toml
[settings.proxy]
auto_trust = false
```

### Manual Trust

If auto-trust is disabled or failed, you can manually install the certificate:

```bash
pitchfork proxy trust
```

On **macOS**, this installs the certificate into your **user login keychain** — no `sudo` required.

On **Linux**, this requires `sudo`:
```bash
sudo pitchfork proxy trust
```

### Removing the Certificate

To remove the pitchfork CA from the system trust store:

```bash
pitchfork proxy untrust
```

On **Linux**, this requires `sudo`:
```bash
sudo pitchfork proxy untrust
```

### Custom Certificate

Provide your own certificate (e.g., from mkcert or Let's Encrypt):

```toml
[settings.proxy]
enable = true
https = true
tls_cert = "/path/to/cert.pem"
tls_key = "/path/to/key.pem"
```

Using [mkcert](https://github.com/FiloSottile/mkcert) for a locally-trusted certificate:

```bash
# Install mkcert and set up local CA
mkcert -install

# Generate certificate for your TLD
mkcert "*.localhost" localhost 127.0.0.1

# Configure pitchfork to use it
```

```toml
[settings.proxy]
enable = true
https = true
tls_cert = "/path/to/_wildcard.localhost+2.pem"
tls_key = "/path/to/_wildcard.localhost+2-key.pem"
```

## Custom TLD

Use a custom TLD instead of `localhost`:

```toml
[settings.proxy]
enable = true
tld = "test"
```

With the default `proxy.sync_hosts = true`, pitchfork keeps registered slugs
synced into `/etc/hosts`, so you usually do not need to set up `dnsmasq` or any
other wildcard DNS service just to use a custom TLD.

For example, if you register these slugs:

```bash
pitchfork proxy add api
pitchfork proxy add docs
```

pitchfork will maintain matching `/etc/hosts` entries such as:

```text
127.0.0.1 api.test
127.0.0.1 docs.test
```

This works for registered slugs only. It is not wildcard DNS for arbitrary
`*.test` names.

If pitchfork cannot write `/etc/hosts`, you still need to provide DNS
resolution yourself, for example with `dnsmasq` or platform-specific resolver
configuration.


## Wildcard Subdomain Matching

When `proxy.wildcard = true` (the default), the proxy matches not only exact
slug hostnames but also their subdomains. For example, with slug `myapp`
registered, both `myapp.localhost` and `tenant.myapp.localhost` route to the
same daemon.

However, whether the subdomain actually resolves depends on the TLD:

| TLD | Exact slug (`myapp.localhost`) | Wildcard subdomain (`tenant.myapp.localhost`) |
|-----|------|------|
| `.localhost` (default) | Browser auto-resolves; /etc/hosts optional | Browser auto-resolves; wildcard routing works |
| Custom (`.test` etc.) | /etc/hosts entry makes it resolvable | /etc/hosts cannot cover; needs dnsmasq |

With the default `.localhost` TLD, wildcard subdomains work out of the box in
Chrome and Firefox (which auto-resolve `.localhost` per RFC 2606). Safari
does not auto-resolve `.localhost` subdomains, so wildcard subdomains will not
resolve unless you configure a local DNS resolver such as `dnsmasq`.

To set up wildcard DNS resolution for a custom TLD, install `dnsmasq` and add
a wildcard entry:

```text
# /etc/dnsmasq.d/pitchfork (or equivalent)
address=/test/127.0.0.1
```

Then point your system resolver at the local dnsmasq instance. On macOS, you
can create `/etc/resolver/test`:

```text
nameserver 127.0.0.1
port 53
```


## LAN Mode

LAN mode lets other devices on your local network (phones, tablets, other
computers) access your daemons through the proxy. Instead of using
`.localhost` (which only resolves on the host machine), LAN mode switches to
the `.local` TLD and publishes slug hostnames via mDNS.

### Quick start

1. Enable LAN mode in `pitchfork.toml`:

```toml
[settings.proxy]
enable = true
lan = true
```

2. Start the supervisor:

```bash
sudo pitchfork supervisor start --force
```

3. Open the proxy URL from another device on the same network:

```
https://myapp.local
```

### How it works

When LAN mode is enabled:

- The TLD is forced to `.local` (mDNS requirement)
- The proxy binds to `0.0.0.0` instead of `127.0.0.1` (overridable via `proxy.host`)
- Each registered slug is published as an mDNS address record (`myapp.local → 192.168.1.42`)
- Your LAN IP is auto-detected; if it changes, mDNS records are re-published

### Pinning the LAN IP

By default, pitchfork auto-detects your LAN IP. To pin a specific address:

```toml
[settings.proxy]
enable = true
lan_ip = "192.168.1.42"
```

Setting `lan_ip` implies `lan = true`, so you can omit the `lan` flag.

### HTTPS on LAN

Other devices need to trust the **proxy host's** certificate to use HTTPS.
Copy `proxy/cert.pem` from that host's state directory and install it using the
client device's certificate settings. On a supported desktop with pitchfork,
use `pitchfork proxy trust --cert /path/to/copied-cert.pem` (with `sudo` on Linux).
Running `proxy trust` without `--cert` would select that device's own certificate.

For HTTP-only access on a trusted development network:

```toml
[settings.proxy]
enable = true
lan = true
https = false
port = 80
```


## Proxy Commands

```bash
# Show all registered slugs and their status
pitchfork proxy status

# Add a slug for the current directory
pitchfork proxy add myapp

# Add with explicit project dir and daemon name
pitchfork proxy add api --dir /path/to/project --daemon server

# Remove a slug
pitchfork proxy remove api

# Install TLS certificate into system trust store
pitchfork proxy trust

# Install a custom certificate
pitchfork proxy trust --cert /path/to/cert.pem

# Remove TLS certificate from system trust store
pitchfork proxy untrust
```

---

## Auto-Start

When you visit a proxy URL for a daemon that isn't running, pitchfork can automatically start it for you. Instead of a `502 Bad Gateway` error, you'll see a "Starting…" page that refreshes every 2 seconds until the daemon is ready.

This is enabled by default. No extra setup is needed beyond the normal proxy configuration.

The entire auto-start operation — including waiting for the daemon's readiness signal and detecting its bound port — is bounded by `proxy.auto_start_timeout` (default 30 s). If the daemon doesn't become ready within this window the browser receives a timeout error. Increase the timeout for daemons with slow initialisation:

```toml
[settings.proxy]
auto_start_timeout = "60s"
```

---

## Viewing proxy URLs

Illustrative output with HTTPS on the default port:

Proxy URLs are shown in CLI output when the proxy is enabled and the daemon has a registered slug:

```bash
$ pitchfork start api
Daemon 'myproject/api' started on port(s): 3000
  → Proxy: https://api.localhost

$ pitchfork list
Name   PID    Status   Proxy URL
api    12345  running  https://api.localhost

$ pitchfork status api
Name: myproject/api
PID: 12345
Status: running
Port: 3000 (active)
Proxy: https://api.localhost
```
