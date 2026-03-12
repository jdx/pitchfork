# Port Management & Reverse Proxy

Pitchfork provides smart port management and an optional reverse proxy that gives your daemons stable, human-friendly URLs.

## Port Assignment

### Expected Ports

Configure the ports your daemon expects to use:

```toml
[daemons.api]
run = "node server.js"
expected_port = [3000]
```

Pitchfork will:
1. Check if port 3000 is available before starting
2. Inject `PORT=3000` into the daemon's environment
3. Fail with a clear error if the port is already in use

### Auto Port Bumping

When a port is occupied, pitchfork can automatically find the next available port:

```toml
[daemons.api]
run = "node server.js"
expected_port = [3000]
auto_bump_port = true
```

With `auto_bump_port = true`, pitchfork tries 3000, 3001, 3002, ... until it finds a free port. The daemon receives the actual port via `$PORT`.

Control how many attempts are made:

```toml
[daemons.api]
run = "node server.js"
expected_port = [3000]
auto_bump_port = true
port_bump_attempts = 10   # try up to 10 ports (default: 3)
```

Or via environment variable:
```bash
PITCHFORK_PORT_BUMP_ATTEMPTS=10 pitchfork start api
```

### Active Port Tracking

After a daemon starts, pitchfork detects which port the process is actually listening on. This detected port is the source of truth for the reverse proxy — it's what gets routed when you access the proxy URL.

---

## Reverse Proxy

The reverse proxy routes requests from stable URLs to the daemon's actual port.

### Why Use the Proxy?

Without the proxy, you need to know the actual port your daemon is running on — which can change if ports are auto-bumped. With the proxy:

```
http://api.myproject.localhost:7777  →  http://localhost:3001
```

The URL stays the same even if the port changes. This is especially useful for:
- Sharing URLs with teammates
- AI agents that need stable endpoints
- Browser bookmarks
- Webhook configurations

### Enabling the Proxy

In your `pitchfork.toml`:

```toml
[settings.proxy]
enable = true
```

Or via environment variable:
```bash
PITCHFORK_PROXY_ENABLE=true pitchfork supervisor start
```

### URL Format

```
http://<id>.<namespace>.<tld>:<port>
```

Examples:
- `http://api.myproject.localhost:7777` — daemon `myproject/api`
- `http://api.localhost:7777` — daemon `global/api` (namespace omitted)
- `http://myapp.localhost:7777` — daemon with `slug = "myapp"`

### Configuration

```toml
[settings.proxy]
enable = true          # Enable the proxy (default: false)
tld = "localhost"      # Top-level domain (default: localhost)
port = 7777            # Proxy port (default: 7777)
https = false          # Enable HTTPS (default: false)
tls_cert = ""          # Path to TLS cert (auto-generated if empty)
tls_key = ""           # Path to TLS key (auto-generated if empty)
```

### Daemon Slugs

You can assign a daemon a short `slug` alias. In proxy URLs, the slug replaces the `<id>.<namespace>` portion:

```
http://myapp.localhost:7777   # daemon with slug = "myapp"
```

See the [Namespace guide](/concepts/namespaces) for full details on slugs.

### Per-Daemon Proxy Control

Override the global proxy setting for individual daemons:

```toml
[settings.proxy]
enable = true   # proxy enabled globally

[daemons.api]
run = "node server.js"
# proxy = true  # inherits global setting (default)

[daemons.internal-worker]
run = "python worker.py"
proxy = false   # opt out — this daemon won't be proxied
```

---

## Standard Ports (80/443)

To use standard HTTP/HTTPS ports without the port number in URLs:

```
http://api.myproject.localhost   (port 80)
https://api.myproject.localhost  (port 443)
```

### Binding to Privileged Ports

Ports below 1024 require elevated privileges on Unix systems. You must start the supervisor with `sudo`:

```bash
# HTTP on port 80
sudo PITCHFORK_PROXY_WEB_PORT=80 pitchfork supervisor start

# HTTPS on port 443
sudo PITCHFORK_PROXY_WEB_PORT=443 PITCHFORK_PROXY_HTTPS=true pitchfork supervisor start
```

Or in `pitchfork.toml`:
```toml
[settings.proxy]
enable = true
web_port = 80   # requires: sudo pitchfork supervisor start
```

::: warning Requires sudo
Binding to ports below 1024 (including 80 and 443) requires the supervisor to be started with `sudo`. The proxy will fail to start if it cannot bind to the configured port.
:::

---

## HTTPS Support

### Auto-Generated Certificate

When `proxy.https = true` and no certificate is configured, pitchfork auto-generates a self-signed certificate:

```toml
[settings.proxy]
enable = true
https = true
web_port = 443   # optional: use standard HTTPS port
```

The certificate is stored in `$PITCHFORK_STATE_DIR/proxy/cert.pem`.

### Trusting the Certificate

Install the auto-generated certificate into your system trust store:

```bash
pitchfork proxy trust
```

On **macOS**, this installs the certificate into your **user login keychain** — no `sudo` required.

On **Linux**, this requires `sudo`:
```bash
sudo pitchfork proxy trust
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

---

## Custom TLD

Use a custom TLD instead of `localhost`:

```toml
[settings.proxy]
enable = true
tld = "test"
```

This requires wildcard DNS resolution for `*.test`. On macOS with dnsmasq:

```bash
# Install dnsmasq
brew install dnsmasq

# Add wildcard DNS entry
echo "address=/.test/127.0.0.1" >> /usr/local/etc/dnsmasq.conf

# Configure macOS resolver
sudo mkdir -p /etc/resolver
echo "nameserver 127.0.0.1" | sudo tee /etc/resolver/test

# Start dnsmasq
sudo brew services start dnsmasq
```

---

## Proxy Commands

```bash
# Show proxy configuration and status
pitchfork proxy status

# Install TLS certificate into system trust store
pitchfork proxy trust

# Install a custom certificate
pitchfork proxy trust --cert /path/to/cert.pem
```

---

## Viewing Proxy URLs

Proxy URLs are shown in CLI output when the proxy is enabled:

```bash
$ pitchfork start api
Daemon 'myproject/api' started on port(s): 3000
  → Proxy: http://api.myproject.localhost:7777

$ pitchfork list
Name   PID    Status   Proxy URL
api    12345  running  http://api.myproject.localhost:7777

$ pitchfork status api
Name: myproject/api
PID: 12345
Status: running
Port: 3000 (active)
Proxy: http://api.myproject.localhost:7777
```
