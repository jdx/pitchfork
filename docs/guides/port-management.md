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

The URL stays the same when port bumping assigns a different port. For example,
`http://api.myproject.localhost:8088` can route to `http://localhost:3001`.

### Quick Start

Start with HTTP on an unprivileged port. Add this to
`~/.config/pitchfork/config.toml` so it applies regardless of the directory
where the supervisor starts:

```toml
[settings.proxy]
enable = true
https = false
port = 8088
```

In a project directory named `myproject`, add this to `pitchfork.toml`:

```toml
[daemons.api]
run = "node server.js"
port = { expect = [3000], bump = 10 }
```

Then enable the proxy and register the project by starting the daemon:

```sh
pitchfork supervisor start --force
pitchfork start api
pitchfork proxy status
```

Open `http://api.myproject.localhost:8088` in a browser that resolves
`.localhost` names. Project daemons with a `port` get an automatic hostname unless they
set `proxy = false`. There is no need to register a slug.

The supervisor reads proxy settings at startup; restart it after editing them.
Continue below for standard ports, HTTPS, custom domains, and LAN access.

## Auto-Start

When you visit a proxy URL for a daemon that isn't running, pitchfork can automatically start it for you. Instead of a `502 Bad Gateway` error, you'll see a "Starting…" page that refreshes every 2 seconds until the daemon is ready.

This is enabled by default. No extra setup is needed beyond the normal proxy configuration.

The entire auto-start operation — including waiting for the daemon's readiness signal and detecting its bound port — is bounded by `proxy.auto_start_timeout` (default 30 s). If the daemon doesn't become ready within this window the browser receives a timeout error. Increase the timeout for daemons with slow initialisation:

```toml
[settings.proxy]
auto_start_timeout = "60s"
```

## Viewing Proxy URLs

With the proxy enabled, `pitchfork start`, `pitchfork list`, `pitchfork status`,
and the web UI show URLs for routable daemons. `pitchfork proxy status` groups
automatic hostnames by project and worktree and lists registered slugs and
conflicts.

The `list --json` and `status --json` output includes a `url` field. The existing
`proxy_url` field remains an alias for the same value. If a daemon has a valid
registered slug, that URL takes precedence over its automatic hostname.

Daemons receive their URL in `PITCHFORK_URL`. Configuration templates can use
<code v-pre>{{ url }}</code> for the current daemon or
<code v-pre>{{ daemons.api.url }}</code> for a dependency. See
[configuration templates](configuration-templates.md#proxy-url).

## Hostnames

Automatic hostnames identify a daemon by its project and, for linked git
worktrees, its checkout:

| Checkout | Hostname | Example |
|----------|----------|---------|
| Primary | `<daemon>.<project>.<tld>` | `api.myproject.localhost` |
| Linked worktree | `<daemon>.<worktree>.<project>.<tld>` | `api.fix-login.myproject.localhost` |

The TLD defaults to `localhost`. The URL's scheme and port come from the proxy
settings; the examples in the quick start use HTTP on port 8088.

### Where Labels Come From

| Label | Source |
|-------|--------|
| Project | The primary checkout's explicit namespace, including a registered namespace, or its directory name |
| Worktree | The linked worktree's `worktree_label`, or its directory name |
| Daemon | The daemon's `proxy` string, or its name |

Labels use lowercase ASCII letters, digits, and hyphens. Other characters
become hyphens, repeated and leading/trailing hyphens are removed, and labels
are limited to 63 characters. For example, `Fix Login` becomes `fix-login`.

To keep a worktree's hostname independent of its directory name, add this to
that checkout's `pitchfork.local.toml`:

```toml
worktree_label = "fix-login"
```

You can also set it in an external config registered for that checkout with
`pitchfork config add`. Keep the label in a checkout-specific file: committing
the same label for multiple worktrees creates a conflict.

### Project Discovery

The proxy uses projects registered in `[namespaces]` or `[slugs]`, and projects
with a daemon in pitchfork's state file. Start a daemon once to make its project
known, or register its configuration without starting it:

```sh
pitchfork config add /home/user/myproject/pitchfork.toml --dir /home/user/myproject --namespace myproject
```

Changing the current directory alone does not register a project.

Daemons defined in global configuration have no project hostname; use a
[slug](#slugs-legacy) to route them. A linked worktree of a bare git repository
is treated as a separate project, using `<daemon>.<project>.<tld>`.

### Rename or Disable a Hostname

Use `proxy` to override the daemon label or disable its automatic hostname:

```toml
[daemons.admin]
run = "node admin.js"
port = 3001
proxy = false

[daemons.web-frontend]
run = "npm run dev"
port = 5173
proxy = "web"          # web.myproject.localhost
```

`proxy = true` is the default. Automatic hostnames require a configured `port`.
Existing slug mappings are independent of this setting and still take precedence.

When the proxy is enabled, routed daemons receive `HOST=127.0.0.1`. Applications
that honor `HOST` therefore bind to loopback. This now applies to automatic
hostnames as well as slugs. A daemon with `proxy = false` and no slug does not
receive this variable; LAN mode also leaves it unset.

### Worktree Namespaces

Hostnames distinguish checkouts, but daemon identity still depends on the
namespace and daemon name. Directory-derived namespaces keep worktrees
separate by default. An explicit top-level `namespace` is inherited by every
worktree that uses the same config, so only one copy of a same-named daemon can
run at a time.

Give each checkout a distinct namespace to run those copies together. If a
daemon is already running in another checkout with the same identity, the
proxy returns an error identifying the conflicting checkouts. See
[namespaces](/concepts/namespaces#git-worktrees).

### Reserved Addresses and Conflicts

`<project>.<tld>` opens the project page, and `<worktree>.<project>.<tld>`
opens that worktree's stack in the [web UI](/guides/web-ui#projects-and-stacks).
Both redirect to the web UI's address. Opening these pages does not start any
daemons.

If the web UI is off, these addresses list the checkout's daemons and explain
how to enable it. If the checkout has no registered project page, they explain
how to register its directory.

A worktree label takes precedence over a daemon label. If a worktree is named
`api`, `api.myproject.localhost` displays the worktree page. Rename the worktree
label or set a different `proxy` label for the primary checkout's `api` daemon
to give that daemon an unambiguous hostname.

If two projects, two worktrees within a project, or two daemons within a
checkout produce the same label, neither conflicting entry is routed. For
example, `foo_bar` and `foo-bar` both become `foo-bar`. Run
`pitchfork proxy status` to see **Conflicts**, then change the relevant
`namespace`, `worktree_label`, or daemon `proxy` label. Hostnames that exceed
the 253-byte DNS limit, including the TLD, are also rejected.

### Slugs (Legacy)

Slugs predate hostnames and still work. They are defined in the global config
(`~/.config/pitchfork/config.toml`) under `[slugs]`, and each maps to a project
directory and (optionally) a specific daemon name:

```toml
# ~/.config/pitchfork/config.toml

[slugs]
api = { dir = "/home/user/my-api", daemon = "server" }
frontend = { dir = "/home/user/my-app", daemon = "dev" }
# If daemon name matches slug, it can be omitted:
docs = { dir = "/home/user/docs-site" }  # defaults daemon = "docs"
```

Slugs are resolved before automatic hostnames, so an existing slug wins a
matching route. Use them for existing integrations, global daemons, or LAN
mDNS discovery. Project daemons can use automatic hostnames without a slug.

## Standard Ports (80/443)

To use standard HTTP/HTTPS ports without the port number in URLs:

```
http://api.myproject.localhost   (port 80)
https://api.myproject.localhost  (port 443)
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

A certificate wildcard covers one label: `*.localhost` does not cover
`api.myproject.localhost`. Include each project and worktree you need. For
example, using [mkcert](https://github.com/FiloSottile/mkcert):

```bash
# Install mkcert and set up local CA
mkcert -install

# Generate certificate for your TLD
mkcert -cert-file cert.pem -key-file key.pem \
  "*.myproject.localhost" "*.fix-login.myproject.localhost" \
  myproject.localhost localhost 127.0.0.1

# Configure pitchfork to use it
```

```toml
[settings.proxy]
enable = true
https = true
tls_cert = "/path/to/cert.pem"
tls_key = "/path/to/key.pem"
```

These settings configure the certificate served by the proxy. To let a daemon
present its own certificate or require client certificates, use
[TLS passthrough](#tls-passthrough).

## TLS Passthrough

By default, pitchfork terminates TLS with its own certificate and forwards
plain HTTP to the daemon. Set `proxy_tls = "passthrough"` when the daemon needs
to handle TLS itself, for example to use its own certificate, require mutual
TLS (mTLS), or serve gRPC over TLS.

### Configure a TLS Daemon

Enable the proxy and declare the daemon's TLS port in `pitchfork.toml`:

```toml
[settings.proxy]
enable = true
https = true

[daemons.api]
run = "./serve --port 8443 --tls-cert server.pem --tls-key server-key.pem"
port = 8443
proxy_tls = "passthrough"
```

Replace `./serve` and its flags with your server's command. The server must
listen for TLS on the declared port. Passthrough requires a nonzero `port`
and a build with the `proxy-tls` feature, which is enabled by default.

[Register the project](#project-discovery) or start the daemon once, then
connect using its [automatic hostname](#hostnames). For a project named
`myproject`:

```bash
pitchfork start api
curl --cacert ca.pem https://api.myproject.localhost/
```

The daemon's certificate must cover `api.myproject.localhost`, and the client
must trust its issuing CA (`ca.pem` in this example). `pitchfork proxy trust`
trusts the proxy's CA; it does not install trust for the daemon's certificate.
For mTLS, configure the server to require and verify client certificates, then
supply a client certificate and key:

```bash
curl --cacert ca.pem --cert client.pem --key client-key.pem \
  https://api.myproject.localhost/
```

The proxy reads the SNI hostname from the TLS ClientHello and forwards the
connection to the daemon on `127.0.0.1`, without decrypting it. The daemon's
certificate, client certificates, SNI, and ALPN negotiation pass through
unchanged, including for HTTP/2 and gRPC. Existing [slugs](#slugs-legacy) also
support passthrough.

::: warning All clients appear to connect from loopback

The daemon sees every proxied client as `127.0.0.1`. Pitchfork cannot add
`X-Forwarded-For` or other HTTP headers to an encrypted stream.

With `proxy.lan = true`, network clients also appear to come from loopback.
Do not rely on the client's IP address to protect admin or debug endpoints.
Require authentication, such as mTLS verified by the daemon, or keep
passthrough off the LAN.

:::

### Auto-Start with Passthrough

When auto-start is enabled, a request to a stopped daemon starts it as usual.
The proxy holds the connection until the daemon is ready, so the client sees
a delayed TLS handshake instead of the HTML "Starting…" page.

`proxy.auto_start_timeout` bounds the startup wait, including time spent
waiting for another request to start the same daemon. The default is 30 seconds.
For a slower daemon, increase the budget and the client's handshake timeout:

```toml
[settings.proxy]
auto_start_timeout = "60s"
```

If startup fails or times out, the proxy closes the connection and logs the
reason. It cannot return an HTML error page or log HTTP requests inside the
encrypted stream.

### Worktrees

Automatic worktree hostnames, such as `api.fix-login.myproject.localhost`,
use `proxy_tls` and `proxy_tls_port` from that checkout's configuration. Include
the worktree hostname in the daemon's certificate and give each checkout a
[distinct namespace](#worktree-namespaces) to run them together.

For legacy slug worktree routes, an unavailable worktree configuration falls
back to its last known TLS route. If no worktree route is known, it inherits
the slug's TLS mode but uses the worktree daemon's first port.

### Connection Requirements and Errors

- Clients must send the destination hostname as SNI. Changing only the HTTP
  `Host` header cannot select a passthrough route. A connection without SNI
  fails the TLS handshake.
- With Encrypted Client Hello, routing uses the visible outer name; the proxy
  cannot route on the encrypted inner name.
- If the proxy cannot inspect a ClientHello before the handshake, it refuses
  to terminate TLS for a passthrough hostname with its own certificate.
- Plain HTTP sent to the HTTPS listener is redirected to HTTPS. With
  `proxy.https = false`, requests for a passthrough hostname receive a 502
  error explaining that HTTPS is required.

### Why Plain TCP Is Not Proxied

Passthrough requires a TLS ClientHello with an SNI hostname at the start of
the connection. Plain TCP services such as Redis and PostgreSQL connections
that begin with `SSLRequest` do not meet this requirement. Connect to those
services directly using the port shown by `pitchfork status <daemon>`.

### Seeing Which Mode a Daemon Uses

`pitchfork status` and `pitchfork list` append `(passthrough)` to the proxy URL
for a passthrough daemon. Terminating daemons have no mode annotation. Both
commands include `proxy_tls` in `--json` output when a proxy URL is available.

## Choosing a Port on a Multi-Port Daemon

Use `proxy_tls_port` to choose which declared port receives proxy traffic.
Despite its name, this setting applies to both TLS modes: it selects the TLS
listener for passthrough or the HTTP listener for termination.

```toml
[daemons.api]
run = "./serve --http 8080 --grpc 9443"
port = [8080, 9443]
proxy_tls = "passthrough"
proxy_tls_port = 9443
```

Here, the daemon's hostname routes to its gRPC listener on port 9443.
`proxy_port` is an alternative spelling; use one spelling per daemon.
Pitchfork preserves that spelling when it rewrites the configuration.

The selected port must be a nonzero value declared in `port`. Pitchfork
rejects invalid selections when it reads the configuration. Selection follows
the port's position in the list after [auto-bump](#auto-port-bumping): if
`[8080, 9443]` becomes `[8081, 9444]`, the hostname routes to port 9444.

Without an explicit selection, passthrough uses the first declared port.
Termination uses the detected active port, falling back to the first port.
All hostnames for a daemon share the same selection; mapping different
hostnames to different ports of one daemon is not supported.

## Custom TLD

Use a custom TLD instead of `localhost`:

```toml
[settings.proxy]
enable = true
tld = "test"
```

Automatic hostnames such as `api.myproject.test` need local DNS resolution.
Pitchfork does not add them to `/etc/hosts`. Configure a local resolver such as
`dnsmasq` for the custom TLD; see [wildcard subdomains](#wildcard-subdomain-matching)
below for an example.

For registered slugs only, `proxy.sync_hosts = true` (the default) maintains
exact entries in `/etc/hosts`. A slug named `api` gets an entry for `api.test`,
but not for its subdomains. If pitchfork cannot write the file, provide those
entries or DNS resolution yourself.

## Wildcard Subdomain Matching

When `proxy.wildcard = true` (the default), the proxy matches not only exact
hostnames but also their subdomains. Extra labels on the left route to the same
daemon, so `api.myproject.localhost` and `tenant.api.myproject.localhost` reach
the same place. The same holds for a legacy slug: `myapp.localhost` and
`tenant.myapp.localhost` both route to `myapp`.

However, whether the subdomain actually resolves depends on the TLD:

| TLD | Automatic hostnames and subdomains | Registered slugs |
|-----|------------------------------------|------------------|
| `.localhost` (default) | Resolve in browsers with `.localhost` support | Resolve in browsers with `.localhost` support |
| Custom (`.test`, etc.) | Need local DNS configuration | Exact names can use `/etc/hosts`; subdomains need DNS |

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

### Quick Start

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

3. Register a slug for the daemon you want to reach (from its project directory):

```sh
pitchfork proxy add myapp --daemon api
```

4. Open the proxy URL from another device on the same network:

```
https://myapp.local
```

### How it works

When LAN mode is enabled:

- The TLD is forced to `.local` (mDNS requirement)
- The proxy binds to `0.0.0.0` instead of `127.0.0.1` (overridable via `proxy.host`)
- Automatic project hostnames are not published through mDNS; use a slug for discovery
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
# Show hostnames, slugs, and routing conflicts
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
