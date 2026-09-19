---
description: Assign service ports and set up stable proxy URLs with local DNS, HTTPS, and LAN access.
---
# Port Management & Reverse Proxy

Assign ports to your services, choose another port when one is busy, and give each service a stable local URL with the optional reverse proxy.

Start with [port assignment](#port-assignment) and the [HTTP quick start](#quick-start).
For HTTPS URLs without a port number, follow [local proxy setup](#hostname-resolution).
Use [LAN mode](#lan-mode) to reach services from another device.

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
[local proxy setup](#hostname-resolution). See also
[custom TLDs](#custom-tld) and [LAN access](#lan-mode).

## Local Proxy Setup {#hostname-resolution}

`pitchfork proxy setup` configures hostname resolution, certificate trust, and
access through the standard HTTP or HTTPS port. It shows a plan and asks for
confirmation before applying changes; `--yes` skips the confirmation prompt.
The supervisor runs as your normal user; setup requests privileges for the
steps that need them.

After registering a project as in the [quick start](#quick-start), configure
local HTTPS on an unprivileged listener port in your user config:

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
Failed checks produce a nonzero exit status; warnings alone do not.

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

### Browser Setup with PAC {#routing-without-touching-dns}

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

In LAN mode, `--pac` is ignored: its TLD is `.local`, which mDNS already resolves, and a PAC file would route every `.local` URL through pitchfork.

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

Undo checks ownership before removing files or proxy settings and leaves unrelated
files and PAC URLs alone. If setup replaced an existing automatic proxy setting,
undo restores the saved setting while pitchfork's PAC URL is still in use.
On macOS, undo removes pitchfork's firewall rules and releases its `pf` reference.
On Linux, it removes `cap_net_bind_service` from the current pitchfork binary
only if that is the binary's sole capability.

### After a Reboot or Upgrade {#what-does-not-survive-a-reboot-or-an-upgrade}

Some system changes need to be reapplied:

| Change | When it may be lost | Recovery |
|--------|---------------------|----------|
| Linux bind capability | pitchfork upgrade or reinstall | Run `pitchfork proxy setup` |
| Linux iptables redirect | Reboot, unless the distribution restores NAT rules | Run `pitchfork proxy setup` |
| macOS `pf` enablement | Reboot; the anchor remains, but `pf` may be disabled | Run `pitchfork proxy setup` |

Run `pitchfork proxy doctor` when URLs stop working after a reboot or upgrade.

## Standard Ports (80/443)

Use `proxy.port = 8443` with HTTPS, or `proxy.port = 8088` with HTTP, then run
`pitchfork proxy setup`. Setup redirects local traffic from port 443 or 80
to the listener, so you can omit the port in URLs. The supervisor remains
unprivileged.

On Linux, you can instead keep the default `proxy.port = 443`; setup grants the
binary permission to bind privileged ports. On macOS, choose an unprivileged
port and use the redirect. PAC connects directly to the configured listener and
does not install a redirect. These redirects serve connections from the proxy
host; LAN clients must use the listener port. See
[platform requirements](#what-needs-sudo).

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

## HTTPS Support

### Local Certificate Authority {#auto-generated-certificate}

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

### Trusting HTTPS from Daemons {#daemons-trusting-the-ca}

When the proxy uses HTTPS and its certificate file exists, pitchfork passes
that path to daemons it starts. With the generated CA, services can use it to
verify HTTPS calls through the proxy:

| Variable | Value |
|----------|-------|
| `PITCHFORK_CA_FILE` | Path to the CA certificate |
| `NODE_EXTRA_CA_CERTS` | The same path, for Node.js |

For other runtimes, pass `PITCHFORK_CA_FILE` to the appropriate trust setting.
For example, Python requests reads `REQUESTS_CA_BUNDLE`:

```toml
[daemons.worker]
run = 'REQUESTS_CA_BUNDLE="$PITCHFORK_CA_FILE" python worker.py'
```

With a custom certificate, these variables point to `proxy.tls_cert`. Configure
your client's trust store for that certificate's issuer as needed.

### Auto-Trust

When the proxy starts with HTTPS enabled, pitchfork automatically attempts to
install the CA certificate into your system trust store (`proxy.auto_trust = true`
by default). Run `pitchfork proxy doctor` to check whether trust is configured.

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
Pitchfork serves that certificate as supplied for every hostname instead of
signing certificates with its local CA. The certificate must cover the names
you use, and clients must trust its issuer. Setup skips CA installation for
custom certificates; set `auto_trust = false` to also disable startup CA trust. Set both paths together: a missing key or
certificate, or a mismatched pair, prevents HTTPS startup.

A certificate wildcard covers one label: `*.localhost` does not cover
`api.myproject.localhost`. Include each project and worktree you need. For
example, using [mkcert](https://github.com/FiloSottile/mkcert):

```bash
# Install mkcert and set up local CA
mkcert -install

# Cover the primary checkout and the fix-login worktree
mkcert -cert-file cert.pem -key-file key.pem \
  "*.myproject.localhost" "*.fix-login.myproject.localhost" \
  myproject.localhost fix-login.myproject.localhost localhost 127.0.0.1
```

```toml
[settings.proxy]
enable = true
https = true
auto_trust = false
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

Keep the listener settings from [local proxy setup](#hostname-resolution). After
changing the TLD, run `pitchfork proxy setup`, restart the supervisor, and run
`pitchfork proxy doctor` to check resolution for names such as `api.myproject.test`. On macOS this creates `/etc/resolver/test`; on Linux with
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
[local proxy setup](#hostname-resolution) for system-wide resolution,
or PAC for applications that honor proxy settings. Custom TLDs need local DNS
configuration or PAC as well.

## DNS Resolver Reference {#the-loopback-resolver}

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
assumes a dual-stack host. Port redirects use the address family selected by
`proxy.host`; an IPv6 listener gets an IPv6 redirect, not redirects for both families.

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

## LAN Mode

LAN mode lets other devices on your local network (phones, tablets, other
computers) access your daemons through the proxy. Instead of using
`.localhost` (which only resolves on the host machine), LAN mode switches to
the `.local` TLD and publishes slug hostnames via mDNS.

### Quick Start

Use an unprivileged HTTPS listener in `~/.config/pitchfork/config.toml`:

```toml
[settings.proxy]
enable = true
lan = true
port = 8443
```

From the project directory, register a slug for the daemon, configure local CA
trust, and restart the supervisor:

```sh
pitchfork proxy add myapp --daemon api
pitchfork proxy setup
pitchfork supervisor start --force
```

On the other device, [trust the proxy host's CA](#https-on-lan), then open
`https://myapp.local:8443`. Include the listener port: setup's loopback redirects
do not redirect connections from other devices. LAN discovery uses registered
slugs; automatic project hostnames are not published through mDNS.

On Linux, you can use `port = 443` and run setup to grant the bind capability,
then omit the port from the URL.

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

If you configured `proxy.tls_cert` instead, have client devices trust its issuer.
Distribute the issuer's CA certificate if they do not already trust it.

For HTTP-only access on a trusted development network:

```toml
[settings.proxy]
enable = true
lan = true
https = false
port = 8088
```

## Proxy Commands

| Command | Purpose |
|---------|---------|
| [`proxy setup`](/cli/proxy/setup) | Preview, apply, or undo DNS, certificate trust, port, and PAC configuration |
| [`proxy doctor`](/cli/proxy/doctor) | Diagnose listener, resolution, trust, and standard-port access |
| [`proxy status`](/cli/proxy/status) | Show hostnames, slugs, and routing conflicts |
| [`proxy add`](/cli/proxy/add) | Register a slug, for example `pitchfork proxy add api --daemon server` |
| [`proxy remove`](/cli/proxy/remove) | Remove a registered slug |
| [`proxy trust`](/cli/proxy/trust) | Install the generated CA or a certificate supplied with `--cert` |
| [`proxy untrust`](/cli/proxy/untrust) | Remove the proxy CA from the trust store |
