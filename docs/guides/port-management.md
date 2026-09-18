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

A certificate configured here is the one the *proxy* serves. If a daemon has to
terminate TLS itself — to present its own certificate, or to require client
certificates — see [TLS Passthrough](#tls-passthrough) instead.

## TLS Passthrough

By default the proxy terminates TLS itself: it answers the handshake with a
certificate signed by its own CA and forwards plain HTTP to your daemon. That
is what you want for an ordinary web app, and it is why `https://api.localhost`
works without the daemon knowing anything about TLS.

Some daemons need to terminate TLS themselves — because they present a specific
certificate, require client certificates (mTLS), or speak a protocol tunnelled
inside TLS that the proxy would mangle. Set `proxy_tls = "passthrough"` on the
daemon:

```toml
[daemons.api]
run = "./serve --tls-cert server.pem --tls-key server-key.pem"
port = 8443
proxy_tls = "passthrough"   # default: "terminate"
```

```bash
pitchfork proxy add api
```

The proxy then reads the hostname from the TLS ClientHello (the SNI extension)
on its 443 listener and splices the raw TCP stream to `127.0.0.1:8443` without
decrypting anything:

```bash
curl --cacert ca.pem --cert client.pem --key client-key.pem https://api.localhost/
```

What survives, precisely because the proxy never terminates:

- The daemon's **own certificate**, presented to the client unchanged.
- **Client certificates**, so mTLS works end to end. A terminating proxy cannot
  forward them, since it is the party completing the handshake.
- The **ALPN protocol** the daemon negotiates, so HTTP/2 and gRPC connections
  pass through as-is.
- The **SNI hostname** the client sent, which the daemon can read for its own
  virtual hosting.

The trade-off is that the proxy cannot see the request. There are no
`X-Forwarded-*` headers, no request logging, and no HTML error pages for a
passthrough hostname: on a raw TLS stream there is nothing to write them into.
When routing fails, the connection is closed and the reason is logged to the
supervisor log.

::: warning The daemon loses the client's address

A terminating hostname gives the daemon an `X-Forwarded-For` header with the
address the connection came from. A spliced hostname cannot: the daemon sees a
connection from `127.0.0.1` for every client, because the proxy opened it.

That matters when `proxy.lan` is on, since the proxy then binds `0.0.0.0` and
accepts connections from the network. A daemon that treats loopback as trusted
— an admin route with no auth, a debug endpoint, a permissive CORS rule — would
be extending that trust to everything that can reach the proxy host. Either
authenticate such routes properly, use a daemon that terminates TLS with its own
client-certificate check, or keep passthrough off the LAN.

:::

The hostname is read from the ClientHello, which is sent in the clear. Under
Encrypted Client Hello the inner name is not visible, so routing follows the
outer `public_name`, as it must for any proxy that routes on SNI.

A connection whose ClientHello the proxy cannot read before the handshake — it
stalls, exceeds the inspection window, or never reconciles its own length
fields — is handed to the TLS listener like any other. If the handshake then
turns out to name a passthrough hostname, the proxy fails it rather than
answering with its own certificate, and logs why. A passthrough hostname is
therefore never quietly downgraded, and connections for ordinary terminating
hostnames are unaffected.

Passthrough requires the `proxy-tls` feature (enabled in default builds) and
`proxy.https = true`, because it only applies to the TLS listener. Plain HTTP
requests to a passthrough hostname are redirected to HTTPS as usual.

### Auto-Start with Passthrough

Auto-start works the same way as for terminated hostnames, except that the
client sees no "Starting…" page — a raw TLS stream has no place to put one.
Instead the proxy holds the connection open, starts the daemon, waits for it to
become ready, and only then splices the stream through. The wait is bounded by
`proxy.auto_start_timeout` (default 30 s); if the daemon is not ready by then,
the connection is closed.

One budget covers the whole wait, including the case where another connection
is already starting the same daemon. From the client's side this looks like a
slow handshake, so both its own timeout and `proxy.auto_start_timeout` need to
be longer than the daemon takes to start:

```toml
[settings.proxy]
auto_start_timeout = "60s"
```

### Choosing a Port on a Multi-Port Daemon

A hostname maps to the daemon's **first** port by default. When a daemon
declares several ports, `proxy_tls_port` selects which one the hostname maps to:

```toml
[daemons.api]
run = "./serve --http 8080 --grpc 9443"
port = [8080, 9443]
proxy_tls = "passthrough"
proxy_tls_port = 9443       # route api.localhost to the gRPC port
```

`proxy_port` is accepted as a shorter spelling of the same setting; set one or
the other, not both, and pitchfork keeps whichever spelling the file already
uses when it rewrites it. Either way the setting applies to terminated
hostnames too, where it chooses which port the proxy forwards HTTP to.

The port is matched by its **position** in the daemon's `port` list, so the
mapping follows [auto-bump](#auto-port-bumping): if the ports above bump to
`[8081, 9444]`, `api.localhost` routes to 9444 rather than to a port nothing is
listening on. `proxy_tls_port` has to name one of the ports in `port`; config
that points it somewhere else is rejected when it is read, rather than routing
traffic to a port the daemon never bound.

`proxy_tls_port` is a property of the daemon, not of the hostname that reached
it, so registering a second slug for the same daemon gives a second hostname
that routes to the same port. Exposing two of a daemon's ports under two
hostnames is therefore not supported yet: a list of hostnames on one daemon
(such as `proxy = ["core", "core-internal"]`) mapping to ports in order would
be the way to express it. Until then, split the process into two daemons, each
with its own port and slug.

### Why Plain TCP Is Not Proxied

Passthrough routes on the hostname inside the TLS ClientHello. A plain TCP
protocol — Postgres, Redis, a raw socket server — carries no hostname before
its first application byte, so a single listener has no way to tell which
daemon a connection is for. Databases and other plain-TCP services are
therefore reached directly on their own port (`pitchfork status <daemon>` prints
it), not through the proxy. Giving each such service its own listener port would
be the same as connecting to the daemon directly.

Postgres' own `SSLRequest` handshake is not TLS either: the connection opens
with a Postgres message, so the ClientHello check does not match it and the
connection is treated as plain HTTP on the TLS port.

### Seeing Which Mode a Daemon Uses

`pitchfork status` prints the mode next to the URL, and `pitchfork list`
annotates the URL when a daemon is in passthrough mode (the default
`terminate` is left unannotated to keep the table readable):

```console
$ pitchfork status api
Name: myproject/api
PID: 51321
Status: running
Port: 8443 (active)
Proxy: https://api.localhost (passthrough)
```

Both commands also report it as `proxy_tls` in `--json` output.

### Validation

`proxy_tls = "passthrough"` requires `port`, because the proxy has to know
where to splice the stream before any application data is exchanged. A daemon
that sets passthrough without a port is rejected when the config is read:

```console
$ pitchfork list
pitchfork::config::passthrough_without_port

  × daemon 'api' in /home/user/project/pitchfork.toml sets
  │ proxy_tls = "passthrough" but has no port
  help: TLS passthrough splices the raw stream to a port on 127.0.0.1, so the
        daemon's port must be known up front; add `port = <number>` to the
        daemon, or remove proxy_tls
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
