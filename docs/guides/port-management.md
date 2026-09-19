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
For system-wide hostname resolution and HTTPS on standard ports, continue to
[hostname resolution setup](#hostname-resolution). See also
[custom TLDs](#custom-tld) and [LAN access](#lan-mode).

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

## Hostname Resolution

`pitchfork proxy setup` configures hostname resolution, certificate trust, and
access through the standard HTTP or HTTPS port. It shows a plan and asks for
confirmation before applying changes. The supervisor can keep running as your
normal user; setup requests privileges for the steps that need them.

For local HTTPS, use an unprivileged listener port in your user config:

```toml
# ~/.config/pitchfork/config.toml
[settings.proxy]
enable = true
https = true
port = 8443
```

Apply setup, restart the supervisor to load the settings, and check the result:

```sh
pitchfork proxy setup --dry-run  # Preview changes without applying them
pitchfork proxy setup
pitchfork supervisor start --force
pitchfork proxy doctor
```

On supported systems, setup routes names under your TLD to pitchfork and
redirects port 443 to 8443. You can then open `https://api.myproject.localhost`
without a port number. Follow any manual instructions printed by setup, such as
configuring DNS on Linux without systemd-resolved.

### Checking the Setup {#checking-it}

`pitchfork proxy doctor` checks the listener, DNS responder, system hostname
resolution, certificate trust, and standard-port access. It uses a fresh hostname
for the resolution check so an existing `/etc/hosts` entry cannot mask missing
wildcard DNS.

When doctor detects pitchfork's PAC URL in the system proxy settings, it checks
that the PAC file is available instead of requiring system DNS resolution.

### Platform Requirements {#what-needs-sudo}

| Step | macOS | Linux |
|------|-------|-------|
| DNS | Writes `/etc/resolver/<tld>` with sudo | With systemd-resolved, `.localhost` needs no change; other TLDs need a drop-in and service restart with sudo |
| CA trust for HTTPS | Uses the login keychain; macOS may prompt for authorization | Installs the CA into the system trust store with sudo |
| Standard ports with an unprivileged listener | Adds a `pf` redirect and enables `pf` with sudo | Adds a loopback iptables redirect with sudo (ip6tables when `proxy.host` is IPv6) |
| Direct binding below port 1024 | Setup asks you to choose an unprivileged port | Grants the binary `cap_net_bind_service` with sudo, unless it already carries other capabilities |

Routing a custom TLD through systemd-resolved requires systemd 247 or newer.
Restarting the service briefly interrupts DNS, including on repeated setup runs.
Without systemd-resolved, setup prints dnsmasq instructions for you to apply.

On macOS, `/etc/resolver/localhost` makes subdomains such as
`api.myproject.localhost` depend on the supervisor's DNS responder. Plain
`localhost` continues to resolve through `/etc/hosts`.

### Routing Without Changing DNS {#routing-without-touching-dns}

Use a proxy auto-config (PAC) file for applications that support system or
browser proxy settings:

```sh
pitchfork proxy setup --pac
```

The PAC file sends names under `proxy.tld` through pitchfork and leaves other
requests direct. Setup configures active network services on macOS and the
user's proxy settings under GNOME. On other desktops, it prints the URL to enter
in your browser's automatic proxy settings:
`http://127.0.0.1:<proxy.port>/proxy.pac` with the default `proxy.host`.

PAC avoids system DNS changes and port redirects. Use an unprivileged listener
port, such as 8443, so the browser can connect directly without a bind capability.
Linux still needs sudo to trust the CA for HTTPS if it is not already trusted;
macOS may request authorization for keychain or network settings changes.
Applications that ignore PAC settings still need DNS and a reachable proxy port.

HTTPS requests use CONNECT tunnels restricted to names under the configured TLD,
with a cap on how many are open at once.

### Undoing Setup {#undoing-it}

```sh
pitchfork proxy setup --undo --dry-run
pitchfork proxy setup --undo
```

Undo removes pitchfork's resolver configuration, port redirects, PAC settings,
bind capability, and CA trust. Setup records configurations in
`$PITCHFORK_STATE_DIR/proxy/setup.toml`, so undo can find resources even after
settings such as `proxy.port` or `proxy.tld` change. Re-running setup reconciles
previous configurations before applying the new one.

Undo checks ownership before removing files or proxy settings. It leaves
unrelated files and PAC URLs alone, and removes `cap_net_bind_service` only when
it is the binary's sole capability. On macOS, it removes pitchfork's firewall
rules and releases the `pf` reference setup took with `pfctl -E`, so `pf` stays
enabled only if other software still holds it. The record itself is
treated as input when it is read back: its TLD is validated, and the system
paths and the binary are rebuilt, so a record naming something else cannot
point a privileged removal at it. Undo therefore revokes the capability from the
running pitchfork only.

### After a Reboot or Upgrade {#what-does-not-survive-a-reboot-or-an-upgrade}

Some system changes need to be reapplied:

| Change | When it may be lost | Recovery |
|--------|---------------------|----------|
| Linux bind capability | pitchfork upgrade or reinstall | Run `pitchfork proxy setup` |
| Linux iptables redirect | Reboot, unless the distribution restores NAT rules | Run `pitchfork proxy setup` |
| macOS `pf` enablement | Reboot; the anchor remains, but `pf` may be disabled | Run `pitchfork proxy setup` |

Run `pitchfork proxy doctor` when URLs stop working after a reboot or upgrade.

### DNS Resolver Reference {#the-loopback-resolver}

With `proxy.dns = true` (the default), the supervisor listens for UDP and TCP DNS
queries on `127.0.0.1:15353`. Change `proxy.dns_port` if that port is occupied.
The default avoids privileged port 53 and the mDNS port, 5353.

The responder answers names under `proxy.tld`, including nested names such as
`api.fix-login.myproject.localhost`, without per-host entries. DNS resolution
does not register a daemon or create a proxy route. Names outside the configured
TLD receive REFUSED; the responder does not forward queries.

The returned addresses follow the proxy listener:

| `proxy.host` | A record | AAAA record |
|--------------|----------|-------------|
| `127.0.0.1` (default) | `127.0.0.1` | None |
| `0.0.0.0` | `127.0.0.1` | None |
| `::1` | None | `::1` |
| `::` | `127.0.0.1` | `::1` |
| A specific address | That address if IPv4 | That address if IPv6 |

Unsupported record types and address families receive NODATA. The `::` case
assumes a dual-stack host. Setup's port redirects are IPv4-only.

In LAN mode, DNS answers use the LAN IPv4 address, but setup leaves system
resolution of `.local` to mDNS. See [LAN mode](#lan-mode) for slug discovery.

### Migrating from /etc/hosts {#etchosts-sync-is-deprecated}

`proxy.sync_hosts` is deprecated and scheduled for removal after one release.
It still defaults to `true` and maintains exact `/etc/hosts` entries for
registered slugs; it cannot cover automatic project hostnames or subdomains.

Run `pitchfork proxy setup`, verify resolution with `pitchfork proxy doctor`,
then disable hosts-file synchronization and restart the supervisor:

```toml
[settings.proxy]
sync_hosts = false
```

## Standard Ports (80/443)

Use `proxy.port = 8443` with HTTPS, or `proxy.port = 8088` with HTTP, then run
`pitchfork proxy setup`. Setup redirects local IPv4 traffic from port 443 or 80
to the listener, so you can omit the port in URLs. The supervisor remains
unprivileged.

On Linux, you can instead keep the default `proxy.port = 443`; setup grants the
binary permission to bind privileged ports. On macOS, choose an unprivileged
port and use the redirect. PAC connects directly to the configured listener and
does not install a redirect. See [platform requirements](#what-needs-sudo).

## HTTPS Support

### Auto-Generated Certificate

With `proxy.https = true` (the default) and no custom certificate configured,
pitchfork generates a local certificate authority (CA). Its certificate is stored
at `$PITCHFORK_STATE_DIR/proxy/ca.pem`; its private key is stored alongside it.
Trust the CA with `pitchfork proxy setup` to use HTTPS across your proxy hostnames.

### Per-Hostname Certificates

On the first TLS connection to a hostname under your TLD, pitchfork signs a
certificate for that name using its local CA. Certificates are cached in
`$PITCHFORK_STATE_DIR/proxy/host-certs/` across restarts. The cache is bounded;
evicted certificates are generated again when needed.

A certificate for the exact hostname supports nested names such as
`api.fix-login.myproject.localhost`. A wildcard such as `*.localhost` covers
only one label. Pitchfork refuses to generate certificates outside its TLD.

### Daemons trusting the CA

Daemons started by pitchfork get the CA path in their environment, so services
that call each other through the proxy over HTTPS can verify it:

| Variable | Value |
|----------|-------|
| `PITCHFORK_CA_FILE` | Path to the CA certificate |
| `NODE_EXTRA_CA_CERTS` | The same path, for Node.js |

Most TLS libraries take a CA bundle path from configuration, and several read
one from the environment. Daemon commands run through the shell, so you can
forward it to whichever name your runtime expects, such as `SSL_CERT_FILE` for
OpenSSL or `REQUESTS_CA_BUNDLE` for Python requests:

```toml
[daemons.worker]
run = "REQUESTS_CA_BUNDLE=$PITCHFORK_CA_FILE python worker.py"
```

### Auto-Trust

When the proxy starts with HTTPS enabled, pitchfork automatically attempts to
install the CA certificate into your system trust store (`proxy.auto_trust = true`
by default). This means you typically don't need to run any extra commands —
browsers will trust the proxy URLs right away.

On **macOS**, auto-trust triggers a system authorization dialog (Touch ID or
password) the first time. Subsequent starts skip the prompt because the
certificate is already trusted.

On **Linux**, auto-trust requires write access to the system CA directory, so use
`pitchfork proxy setup` to perform that step with sudo. If auto-trust fails,
pitchfork logs a warning and continues starting the proxy.

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

Set `proxy.tls_cert` and `proxy.tls_key` to a matching PEM certificate and key.
Pitchfork serves that certificate as supplied for every hostname and does not
generate or install a CA. The certificate must cover the names you use, and
clients must trust its issuer.

A certificate wildcard covers one label: `*.localhost` does not cover
`api.myproject.localhost`. Include each project and worktree you need. For
example, using [mkcert](https://github.com/FiloSottile/mkcert):

```bash
# Install mkcert and set up local CA
mkcert -install

# Cover the primary checkout and the fix-login worktree
mkcert -cert-file cert.pem -key-file key.pem \
  "*.myproject.localhost" "*.fix-login.myproject.localhost" \
  myproject.localhost localhost 127.0.0.1
```

```toml
[settings.proxy]
enable = true
https = true
tls_cert = "/path/to/cert.pem"
tls_key = "/path/to/key.pem"
```

## Custom TLD

Use a custom TLD instead of `localhost`:

```toml
[settings.proxy]
enable = true
tld = "test"
```

Restart the supervisor after changing the TLD, then run
`pitchfork proxy setup` to configure resolution for names such as
`api.myproject.test`. On macOS this creates `/etc/resolver/test`; on Linux with
systemd-resolved it installs a routing drop-in. Without systemd-resolved, follow
the dnsmasq instructions printed by setup. See
[platform requirements](#what-needs-sudo).

## Wildcard Subdomain Matching

With `proxy.wildcard = true` (the default), extra labels on the left of a
hostname route to the same daemon. For example, `api.myproject.localhost` and
`tenant.api.myproject.localhost` reach the same service. This also works for
legacy slugs: `tenant.myapp.localhost` routes to `myapp`.

The loopback DNS responder resolves these nested names without extra entries.
For HTTPS, the local CA signs a certificate for each requested hostname. If you
supply a custom certificate, it must cover those names itself.

Browser support for `.localhost` subdomains varies. Use
[hostname resolution setup](#hostname-resolution) for system-wide resolution,
or PAC for applications that honor proxy settings. Custom TLDs need local DNS
configuration or PAC as well.

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

2. Grant the privileged parts once, if you are on the default port 443:

```bash
pitchfork proxy setup
```

On Linux this grants the binary `cap_net_bind_service`. On macOS, set an
unprivileged `proxy.port` instead, since nothing there lets an unprivileged
process bind 443. With a port above 1023, no bind capability is needed. Setup
may still be needed for local CA trust.

3. Register a slug for the daemon you want to reach, from its project
   directory. LAN mode publishes slugs over mDNS, not automatic hostnames:

```sh
pitchfork proxy add myapp --daemon api
```

4. Start the supervisor, without `sudo`:

```bash
pitchfork supervisor start --force
```

5. Open the proxy URL from another device on the same network. Include the
   listener port when it is not 443; for example, with `proxy.port = 8443`:

```
https://myapp.local:8443
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

Other devices need to trust the **proxy host's** certificate authority to use
HTTPS. Copy `proxy/ca.pem` from that host's state directory and install it using
the client device's certificate settings. Trusting that one CA covers every
proxy host name, because the proxy signs a certificate per name from it. On a
supported desktop with pitchfork, use
`pitchfork proxy trust --cert /path/to/copied-ca.pem` (with `sudo` on Linux).
Running `proxy trust` without `--cert` would select that device's own CA.

If you configured `proxy.tls_cert` instead, there is no pitchfork CA: distribute
whatever issued your certificate, exactly as you would for any other service.

For HTTP-only access on a trusted development network:

```toml
[settings.proxy]
enable = true
lan = true
https = false
port = 8088
```

## Proxy Commands

```bash
# Point this machine's DNS, trust store and ports at the proxy
pitchfork proxy setup

# Same, but routing names through a PAC file instead of the system resolver
pitchfork proxy setup --pac

# Reverse everything setup did
pitchfork proxy setup --undo

# Check everything a proxy URL needs in order to work
pitchfork proxy doctor

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
