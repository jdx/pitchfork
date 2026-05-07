# Port Management & Reverse Proxy

Pitchfork provides smart port management and an optional reverse proxy that gives your daemons stable, human-friendly URLs.

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

Pitchfork checks if the port is available before starting, injects `PORT=3000` into the daemon's environment, and fails with a clear error if the port is already in use.

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

The daemon receives the actual allocated port via `$PORT`.

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
- Sharing URLs with teammates
- AI agents that need stable endpoints
- Browser bookmarks
- Webhook configurations

### Quick start

1. Enable the proxy in `pitchfork.toml`:

```toml
[settings.proxy]
enable = true
```

2. Start the supervisor:

```bash
sudo pitchfork supervisor start --force   # port 80 or 443 requires sudo
```

3. Add a slug in the **global** config:

```bash
pitchfork proxy add api
# or with explicit dir and daemon name:
pitchfork proxy add api --dir /path/to/project --daemon server
```

This registers the slug in `~/.config/pitchfork/config.toml`:

```toml
[slugs]
api = { dir = "/path/to/project", daemon = "server" }
```

4. Start the daemon:

```bash
pitchfork start api
```

5. Open the proxy URL:

```bash
open https://api.localhost
```

The CA certificate is automatically installed into your system trust store
on first start. If auto-trust failed (e.g. on Linux without `sudo`),
you can manually install it:

```bash
pitchfork proxy trust         # On Linux, run with `sudo`
```

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

Ports below 1024 require elevated privileges on Unix systems. You must start the supervisor with `sudo`:

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

::: warning Requires sudo
Binding to ports below 1024 (including 80 and 443) requires the supervisor to be started with `sudo`. The proxy will fail to start if it cannot bind to the configured port.
:::


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

## Viewing Proxy URLs

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
