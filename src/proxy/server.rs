//! Reverse proxy server implementation.
//!
//! Listens on a configured port and routes requests to daemon processes based
//! on the `Host` header subdomain pattern.
//!
//! When `proxy.https = true`, a local CA is auto-generated (via `rcgen`) and
//! each incoming TLS connection is served with a per-domain certificate signed
//! by that CA (SNI-based dynamic certificate issuance).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use hyper::header::{COOKIE, HOST};

/// Response header used to identify a pitchfork proxy (for health checks and debugging).
const PITCHFORK_HEADER: &str = "x-pitchfork";

/// Request header tracking how many times a request has passed through the proxy.
/// Used to detect forwarding loops.
const PROXY_HOPS_HEADER: &str = "x-pitchfork-hops";

/// Maximum number of proxy hops before rejecting as a loop.
const MAX_PROXY_HOPS: u64 = 5;

/// HTTP/1.1 hop-by-hop headers that are forbidden in HTTP/2 responses.
/// These must be stripped when proxying an HTTP/1.1 backend response back to an HTTP/2 client.
const HOP_BY_HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "upgrade",
];

use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::net::{TcpListener, TcpStream};

use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::ProxyTlsMode;
use crate::settings::settings;
use crate::supervisor::SUPERVISOR;

// ─── Slug resolution cache ──────────────────────────────────────────────────
//
// `read_global_slugs()` reads ~/.config/pitchfork/config.toml from disk on every
// call, and `namespace_for_dir()` traverses the filesystem upward to find the
// nearest pitchfork.toml.  Both are called from `resolve_target_port()` which
// sits in the hot path of every proxied HTTP request.
//
// This cache stores the resolved slug → (namespace, daemon_name) mapping
// in memory with a short TTL so that the proxy does zero disk I/O for the vast
// majority of requests while still picking up config changes within seconds.

/// How long to cache the slug resolution table before re-reading from disk.
const SLUG_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(2);

/// How a hostname's TLS is handled, and which daemon port it maps to.
///
/// Read from the daemon's `pitchfork.toml` (`proxy_tls`, `proxy_tls_port`) when
/// the slug table is refreshed, so the routing decision needs no disk I/O and
/// is available whether or not the daemon is currently running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProxyTlsRoute {
    /// `terminate` (default) or `passthrough`.
    pub mode: ProxyTlsMode,
    /// The daemon port this hostname maps to. `None` means the first port,
    /// which is what a single-port daemon always wants.
    pub port: Option<u16>,
}

/// Cached slug entry: pre-resolved namespace + daemon name for a slug.
#[derive(Clone, Debug)]
pub struct CachedSlugEntry {
    /// The slug key as registered in config (needed for display in auto-start pages).
    pub slug: String,
    /// Expected namespace derived from `entry.resolve_dir()` (None if derivation failed).
    pub namespace: Option<String>,
    /// Daemon short name (defaults to slug name when not explicitly set).
    pub daemon_name: String,
    /// Project directory for this slug (needed for auto-start).
    pub dir: std::path::PathBuf,
    /// Worktrees (git) / workspaces (jj) discovered under this slug's project directory.
    pub worktrees: Vec<crate::proxy::worktree::WorktreeEntry>,
    /// Sanitized worktree prefixes (ASCII-lowercased) that were discovered but
    /// are ambiguous, kept so a request naming one is refused rather than
    /// falling through to the parent slug as an unknown wildcard prefix.
    pub rejected_worktree_prefixes: std::collections::HashSet<String>,
    /// TLS route for the slug's main checkout.
    pub tls: ProxyTlsRoute,
    /// TLS routes per worktree, keyed by ASCII-lowercased sanitized branch.
    /// A worktree can configure its own `proxy_tls`, since each checkout has
    /// its own `pitchfork.toml`.
    pub worktree_tls: std::collections::HashMap<String, ProxyTlsRoute>,
}

/// In-memory cache for the global slug registry + derived namespaces.
struct SlugCache {
    entries: Arc<std::collections::HashMap<String, CachedSlugEntry>>,
    expires_at: std::time::Instant,
    /// When the build that produced `entries` started reading config, used to
    /// keep an older build from overwriting a newer one.
    built_from: Option<std::time::Instant>,
}

/// The cached slug table.
///
/// A `std::sync::RwLock` rather than an async one on purpose: `rustls` calls
/// its certificate resolver from a synchronous trait method, and that resolver
/// has to know whether a hostname is passthrough before it issues a certificate
/// for it. One lock means routing and the resolver read the same table by
/// construction, so they cannot disagree about a hostname's mode. Every
/// critical section is a clone of an `Arc` or a comparison of two `Instant`s,
/// with the disk I/O deliberately outside, and nothing awaits while holding it.
static SLUG_CACHE: once_cell::sync::Lazy<std::sync::RwLock<SlugCache>> =
    once_cell::sync::Lazy::new(|| {
        std::sync::RwLock::new(SlugCache {
            entries: Arc::new(std::collections::HashMap::new()),
            expires_at: std::time::Instant::now(), // expired → will be populated on first access
            built_from: None,
        })
    });

/// Read the cached slug table without awaiting.
///
/// Empty until the table has been populated once, which every TLS connection
/// does before reaching the certificate resolver. An empty table resolves every
/// hostname to `terminate`, which is the behavior of a proxy with no slugs.
fn slug_snapshot() -> Arc<std::collections::HashMap<String, CachedSlugEntry>> {
    let guard = SLUG_CACHE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(&guard.entries)
}

/// Whether a table built starting at `build_start` is newer than what the cache
/// holds, and so should replace it.
///
/// Two refreshes can overlap, and the one that finishes last is not
/// necessarily the one that read the newer config. Publishing by build start
/// keeps a slow older build from reinstating config that has since changed,
/// which for a hostname that just became `passthrough` would mean the
/// certificate resolver issuing for it.
fn should_publish_slugs(
    stored_built_from: Option<std::time::Instant>,
    build_start: std::time::Instant,
) -> bool {
    stored_built_from.is_none_or(|stored| build_start >= stored)
}

/// Drop every worktree whose sanitized branch is ambiguous under
/// case-insensitive host matching, keeping the unambiguous ones.
///
/// Both sides of a collision are dropped rather than one being picked: the
/// alternative routes a request to a worktree the user did not name, which is
/// worse than not routing it at all.
fn reject_case_colliding_worktrees(
    wts: Vec<crate::proxy::worktree::WorktreeEntry>,
) -> (
    Vec<crate::proxy::worktree::WorktreeEntry>,
    std::collections::HashSet<String>,
) {
    let collisions =
        crate::proxy::ascii_case_collisions(wts.iter().map(|w| w.sanitized_branch.as_str()));
    if collisions.is_empty() {
        return (wts, collisions);
    }

    let (dropped, kept): (Vec<_>, Vec<_>) = wts
        .into_iter()
        .partition(|w| collisions.contains(&w.sanitized_branch.to_ascii_lowercase()));

    let mut folded: Vec<&String> = collisions.iter().collect();
    folded.sort();
    for key in folded {
        let mut branches: Vec<&str> = dropped
            .iter()
            .filter(|w| w.sanitized_branch.eq_ignore_ascii_case(key))
            .map(|w| w.branch.as_str())
            .collect();
        branches.sort();
        log::warn!(
            "Worktree slug collision: branches [{}] all route to '{key}' under \
             case-insensitive host matching. None of them will be routed; \
             rename a branch to disambiguate.",
            branches.join(", "),
        );
    }

    (kept, collisions)
}

/// Read a daemon's TLS route (`proxy_tls`, `proxy_tls_port`) from the config
/// that applies in `dir`.
///
/// Falls back to the default route (terminate, first port) whenever the config
/// cannot be read or names no such daemon: an unreadable config must not turn
/// into a passthrough splice to an unknown port.
///
/// Any other way of resolving a hostname to a daemon can reuse this to get the
/// same TLS decision, since the mode belongs to the daemon rather than to the
/// hostname that reached it.
///
/// The config read goes through the merged-config cache, so a refresh that
/// finds nothing changed costs a `stat` per config file rather than a reparse.
pub(crate) fn read_proxy_tls_route(
    dir: &std::path::Path,
    namespace: Option<&str>,
    daemon_name: &str,
) -> ProxyTlsRoute {
    let Some(namespace) = namespace else {
        return ProxyTlsRoute::default();
    };
    let Ok(id) = DaemonId::try_new(namespace, daemon_name) else {
        return ProxyTlsRoute::default();
    };
    let pt = match crate::pitchfork_toml::PitchforkToml::all_merged_from(dir) {
        Ok(pt) => pt,
        Err(e) => {
            log::debug!(
                "Proxy TLS route: could not read config in {}: {e}",
                dir.display()
            );
            return ProxyTlsRoute::default();
        }
    };
    match pt.daemons.get(&id) {
        Some(cfg) => ProxyTlsRoute {
            mode: cfg.proxy_tls.unwrap_or_default(),
            port: cfg.effective_proxy_tls_port(),
        },
        None => ProxyTlsRoute::default(),
    }
}

/// Build the slug lookup table from disk (expensive — involves file I/O + subprocesses).
/// Called outside the cache lock via `spawn_blocking` to avoid blocking the Tokio runtime.
///
/// Keys are ASCII-lowercased, and slugs that collide once folded are left out
/// entirely — see [`reject_case_colliding_worktrees`] for why ambiguity is
/// rejected rather than resolved.
fn build_slug_entries() -> std::collections::HashMap<String, CachedSlugEntry> {
    let global_slugs = crate::pitchfork_toml::PitchforkToml::read_global_slugs();
    let collisions = crate::proxy::ascii_case_collisions(global_slugs.keys().map(String::as_str));
    let mut folded: Vec<&String> = collisions.iter().collect();
    folded.sort();
    for key in folded {
        let mut spellings: Vec<&str> = global_slugs
            .keys()
            .filter(|s| s.eq_ignore_ascii_case(key))
            .map(String::as_str)
            .collect();
        spellings.sort();
        log::warn!(
            "Slug collision: [{}] differ only by case and host names are case-insensitive. \
             None of them will be routed; remove or rename all but one.",
            spellings.join(", "),
        );
    }

    let mut entries: std::collections::HashMap<String, CachedSlugEntry> =
        std::collections::HashMap::with_capacity(global_slugs.len());
    let worktree_enabled = crate::settings::settings().general.worktree;
    for (slug, entry) in &global_slugs {
        let key = slug.to_ascii_lowercase();
        if collisions.contains(&key) {
            continue;
        }
        let ns = entry.resolve_namespace();
        let daemon_name = entry.daemon.as_deref().unwrap_or(slug).to_string();
        let (worktrees, rejected_worktree_prefixes) = if worktree_enabled {
            let wts = match entry.resolve_dir() {
                Some(dir) => crate::proxy::worktree::discover_worktrees(&dir),
                None => vec![],
            };
            let wts = wts
                .into_iter()
                .map(|mut wt| {
                    wt.namespace =
                        crate::pitchfork_toml::PitchforkToml::namespace_for_dir(&wt.path).ok();
                    wt
                })
                .collect();
            reject_case_colliding_worktrees(wts)
        } else {
            (vec![], std::collections::HashSet::new())
        };
        let dir = entry.resolve_dir().unwrap_or_default();
        let tls = read_proxy_tls_route(&dir, ns.as_deref(), &daemon_name);
        let worktree_tls = worktrees
            .iter()
            .map(|wt| {
                (
                    wt.sanitized_branch.to_ascii_lowercase(),
                    read_proxy_tls_route(&wt.path, wt.namespace.as_deref(), &daemon_name),
                )
            })
            .collect();
        entries.insert(
            key,
            CachedSlugEntry {
                slug: slug.clone(),
                namespace: ns,
                daemon_name,
                dir,
                worktrees,
                rejected_worktree_prefixes,
                tls,
                worktree_tls,
            },
        );
    }
    entries
}

/// Return the cached slug table, refreshing from disk if expired.
///
/// The disk I/O happens *outside* the lock so a refresh does not block
/// concurrent requests. Two callers may therefore refresh at once; the table
/// published is the one whose build read config last, and both callers return
/// their own build, which is at most one TTL stale either way.
pub async fn get_cached_slugs() -> Arc<std::collections::HashMap<String, CachedSlugEntry>> {
    // Fast path: cache still valid — just clone the Arc.
    {
        let cache = SLUG_CACHE
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if std::time::Instant::now() < cache.expires_at {
            return Arc::clone(&cache.entries);
        }
    } // lock released before disk I/O

    // Slow path: refresh from disk on a blocking thread (involves subprocess calls).
    let build_start = std::time::Instant::now();
    let new_entries = Arc::new(
        tokio::task::spawn_blocking(build_slug_entries)
            .await
            .unwrap_or_else(|e| {
                log::warn!("Failed to refresh slug cache: {e}");
                std::collections::HashMap::new()
            }),
    );

    // Publish, unless a build that read newer config got there first.
    {
        let mut cache = SLUG_CACHE
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if should_publish_slugs(cache.built_from, build_start) {
            cache.entries = Arc::clone(&new_entries);
            cache.expires_at = std::time::Instant::now() + SLUG_CACHE_TTL;
            cache.built_from = Some(build_start);
        }
    }

    new_entries
}

/// Try to match a subdomain against a slug table, with optional wildcard fallback.
///
/// When `wildcard` is true and no exact match is found, progressively strips
/// subdomain prefixes from the left until a match is found or no dots remain.
/// For example, with slug "myapp" registered, `tenant.myapp` matches "myapp".
///
/// `entries` must be keyed by the ASCII-lowercased slug, as
/// [`build_slug_entries`] produces: host names are case-insensitive (RFC 4343),
/// so the subdomain is lowercased before every lookup.
fn wildcard_slug_lookup<'a>(
    subdomain: &str,
    entries: &'a std::collections::HashMap<String, CachedSlugEntry>,
    wildcard: bool,
) -> Option<&'a CachedSlugEntry> {
    let subdomain = subdomain.to_ascii_lowercase();

    entries.get(&subdomain).or_else(|| {
        if !wildcard {
            return None;
        }
        // "a.b.myapp" has dots at 1,3 → "b.myapp", "myapp"
        subdomain
            .match_indices('.')
            .map(|(i, _)| &subdomain[i + 1..])
            .find_map(|candidate| entries.get(candidate))
    })
}

/// What a wildcard subdomain prefix resolves to within a slug's worktrees.
#[derive(Debug)]
enum PrefixMatch<'a> {
    /// The prefix names exactly one discovered worktree.
    Worktree(&'a crate::proxy::worktree::WorktreeEntry),
    /// The prefix names no worktree — an ordinary wildcard subdomain, served
    /// by the slug's main checkout.
    Unknown,
    /// The prefix names worktrees that were rejected as ambiguous.  Serving the
    /// main checkout here would answer successfully with the wrong content, so
    /// the request is refused instead.
    Ambiguous,
}

/// Resolve a wildcard subdomain prefix against a slug's cached worktrees.
fn match_worktree_prefix<'a>(cached: &'a CachedSlugEntry, prefix: &str) -> PrefixMatch<'a> {
    if let Some(wt) = cached
        .worktrees
        .iter()
        .find(|w| w.sanitized_branch.eq_ignore_ascii_case(prefix))
    {
        return PrefixMatch::Worktree(wt);
    }
    if cached
        .rejected_worktree_prefixes
        .contains(&prefix.to_ascii_lowercase())
    {
        return PrefixMatch::Ambiguous;
    }
    PrefixMatch::Unknown
}

/// Strip a trailing `.{suffix}` from `s`, ignoring ASCII case.
///
/// Returns the remaining prefix, or `None` when `s` does not end that way.
fn strip_dot_suffix_ignore_case(s: &str, suffix: &str) -> Option<String> {
    let needle_len = suffix.len() + 1;
    if s.len() <= needle_len {
        return None;
    }
    let split = s.len() - needle_len;
    if !s.is_char_boundary(split) {
        return None;
    }
    let (head, tail) = s.split_at(split);
    if tail.starts_with('.') && tail[1..].eq_ignore_ascii_case(suffix) {
        Some(head.to_string())
    } else {
        None
    }
}

/// Look up a slug in the cached table.
///
/// With wildcard enabled (default), falls back to progressively shorter
/// subdomain suffixes when an exact match is not found.  For example,
/// `tenant.myapp` will match slug `myapp` if no slug named `tenant.myapp`
/// exists.
async fn cached_slug_lookup(subdomain: &str) -> Option<CachedSlugEntry> {
    let entries = get_cached_slugs().await;
    wildcard_slug_lookup(subdomain, &entries, settings().proxy.wildcard).cloned()
}

// ─── Auto-start deduplication ───────────────────────────────────────────────
//
// When auto_start is enabled, concurrent proxy requests for the same stopped
// daemon must not trigger multiple start operations.  This set tracks daemon
// IDs that are currently being auto-started.

static AUTO_START_IN_PROGRESS: once_cell::sync::Lazy<
    tokio::sync::Mutex<std::collections::HashSet<DaemonId>>,
> = once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(std::collections::HashSet::new()));

/// Result of resolving a proxy target for a given host.
enum ResolveResult {
    /// Daemon is running and ready — forward to this port.
    /// Covers both already-running daemons and freshly auto-started ones.
    Ready(u16),
    /// Daemon is currently starting (auto-start in progress or just triggered).
    Starting { slug: String },
    /// No matching slug or daemon found.
    NotFound,
    /// Routing refused with a descriptive reason.
    Error(String),
}

/// Shared proxy state passed to each request handler.
/// Callback type invoked on proxy errors (e.g. for logging/alerting).
type OnErrorFn = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
struct ProxyState {
    /// HTTP client used to forward requests to daemon backends.
    client: Arc<Client<HttpConnector, Body>>,
    /// The configured TLD (e.g. "localhost").
    tld: String,
    /// Whether the proxy is serving HTTPS.
    is_tls: bool,
    /// Optional error callback invoked on proxy errors (e.g. for logging/alerting).
    on_error: Option<OnErrorFn>,
}

/// Start the reverse proxy server.
///
/// Binds to the configured port and serves until the process exits.
/// When `proxy.https = true`, TLS is terminated here using a self-signed
/// certificate (auto-generated if not present).
///
/// This function is intended to be spawned as a background task.
pub async fn serve(
    bind_tx: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
    cancel: tokio_util::sync::CancellationToken,
) -> crate::Result<()> {
    let s = settings();
    let lan_enabled = s.proxy.lan || !s.proxy.lan_ip.is_empty();

    let effective_tld = if lan_enabled {
        "local".to_string()
    } else {
        s.proxy.tld.clone()
    };

    let Some(effective_port) = u16::try_from(s.proxy.port).ok().filter(|&p| p > 0) else {
        let msg = format!(
            "proxy.port {} is out of valid port range (1-65535), proxy server cannot start",
            s.proxy.port
        );
        let _ = bind_tx.send(Err(msg.clone()));
        miette::bail!("{msg}");
    };

    let mut connector = HttpConnector::new();
    // Limit how long the proxy waits to establish a TCP connection to a backend.
    // Without this, a daemon that accepts the SYN but never completes the handshake
    // would stall the proxy indefinitely.
    connector.set_connect_timeout(Some(std::time::Duration::from_secs(10)));

    let client = Client::builder(TokioExecutor::new())
        // Reclaim idle keep-alive connections after 30 s so that file descriptors
        // are not held open forever when a backend goes quiet.
        .pool_idle_timeout(std::time::Duration::from_secs(30))
        .build(connector);

    let state = ProxyState {
        client: Arc::new(client),
        tld: effective_tld.clone(),
        is_tls: s.proxy.https,
        on_error: None,
    };

    let app = Router::new().fallback(proxy_handler).with_state(state);

    // Resolve bind address from settings.
    // In LAN mode, default to 0.0.0.0 so the proxy is reachable from other
    // devices on the network.  Users can still override with proxy.host.
    let bind_ip: std::net::IpAddr = if lan_enabled && s.proxy.host == "127.0.0.1" {
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
    } else {
        match s.proxy.host.parse() {
            Ok(ip) => ip,
            Err(_) => {
                log::warn!(
                    "proxy.host {:?} is not a valid IP address — falling back to 127.0.0.1. \
                     The proxy will only be reachable on the loopback interface.",
                    s.proxy.host
                );
                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
            }
        }
    };
    let addr = SocketAddr::from((bind_ip, effective_port));

    if s.proxy.https {
        serve_https_with_http_fallback(
            app,
            addr,
            &s,
            effective_port,
            effective_tld,
            bind_tx,
            cancel,
        )
        .await
    } else {
        serve_http(app, addr, effective_port, bind_tx, cancel).await
    }
}

/// Serve plain HTTP.
async fn serve_http(
    app: Router,
    addr: SocketAddr,
    effective_port: u16,
    bind_tx: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
    cancel: tokio_util::sync::CancellationToken,
) -> crate::Result<()> {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => {
            if settings().proxy.sync_hosts {
                crate::proxy::hosts::sync_hosts_from_settings();
            }
            let _ = bind_tx.send(Ok(()));
            l
        }
        Err(e) => {
            let msg = bind_error_message(effective_port, &e);
            let _ = bind_tx.send(Err(msg.clone()));
            return Err(miette::miette!("{msg}"));
        }
    };

    log::info!("Proxy server listening on http://{addr}");
    if effective_port < 1024 {
        log::info!(
            "Note: port {effective_port} is a privileged port. \
             The supervisor must be started with sudo to bind to this port."
        );
    }
    let shutdown_signal = cancel.clone().cancelled_owned();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal)
    .await
    .map_err(|e| miette::miette!("Proxy server error: {e}"))?;
    Ok(())
}

/// Serve HTTPS with automatic HTTP detection on the same port.
///
/// Peeks at the first byte of each incoming TCP connection:
/// - `0x16` (TLS ClientHello) → hand off to the TLS acceptor (HTTP/2 + HTTP/1.1 via ALPN)
/// - anything else → 302 redirect to HTTPS
#[cfg(feature = "proxy-tls")]
async fn serve_https_with_http_fallback(
    app: Router,
    addr: SocketAddr,
    s: &crate::settings::Settings,
    effective_port: u16,
    effective_tld: String,
    bind_tx: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
    cancel: tokio_util::sync::CancellationToken,
) -> crate::Result<()> {
    use rustls::ServerConfig;
    use tokio_rustls::TlsAcceptor;

    let (ca_cert_path, ca_key_path) = resolve_tls_paths(s);

    // Generate CA if not present
    if !ca_cert_path.exists() || !ca_key_path.exists() {
        generate_ca(&ca_cert_path, &ca_key_path)?;
        log::info!(
            "Generated local CA certificate at {}",
            ca_cert_path.display()
        );
        log::info!("To trust the CA in your browser, run: pitchfork proxy trust");
    }

    // Install ring as the default CryptoProvider if none has been set yet.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Build the SNI resolver (loads CA, caches per-domain certs)
    let resolver = SniCertResolver::new(&ca_cert_path, &ca_key_path, effective_tld.clone())?;

    let mut tls_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver));
    // Advertise HTTP/2 and HTTP/1.1 via ALPN so browsers negotiate HTTP/2
    // for multiplexed requests (eliminates the 6-connection-per-host limit).
    tls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    let acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = match TcpListener::bind(addr).await {
        Ok(l) => {
            if settings().proxy.sync_hosts {
                crate::proxy::hosts::sync_hosts_from_settings();
            }
            let _ = bind_tx.send(Ok(()));
            l
        }
        Err(e) => {
            let msg = bind_error_message(effective_port, &e);
            let _ = bind_tx.send(Err(msg.clone()));
            return Err(miette::miette!("{msg}"));
        }
    };

    log::info!("Proxy server listening on https://{addr} (HTTP also accepted)");
    if effective_port < 1024 {
        log::info!(
            "Note: port {effective_port} is a privileged port. \
             The supervisor must be started with sudo to bind to this port."
        );
    }

    // Build a lightweight redirect app for plain-HTTP requests.
    let redirect_app = Router::new().fallback(redirect_to_https_handler);

    // Accept connections and sniff the first byte to decide TLS vs plain HTTP.
    let mut conn_tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
    loop {
        // Reap finished connection tasks during normal operation so the JoinSet
        // does not retain one entry per historical connection.
        while conn_tasks.try_join_next().is_some() {}

        tokio::select! {
            accept_result = listener.accept() => {
                let (stream, _peer_addr) = match accept_result {
                    Ok(conn) => conn,
                    Err(e) => {
                        log::warn!("Accept error (will retry): {e}");
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                };

                let acceptor = acceptor.clone();
                let app = app.clone();
                let redirect_app = redirect_app.clone();
                let tld = effective_tld.clone();

                conn_tasks.spawn(async move {
                    // Peek at the first byte without consuming it.
                    // TLS ClientHello always starts with 0x16 (content type "handshake").
                    let mut peek_buf = [0u8; 1];
                    match stream.peek(&mut peek_buf).await {
                        Ok(0) | Err(_) => return,
                        _ => {}
                    }

                    if peek_buf[0] == 0x16 {
                        // A TLS connection whose SNI names a `proxy_tls = "passthrough"`
                        // daemon is spliced through untouched, so the daemon's own
                        // certificate, ALPN and client-certificate request reach the
                        // client. Everything else is terminated here as before.
                        match peek_sni_host(&stream, SNI_PEEK_TIMEOUT).await {
                            SniProbe::Host(host) => {
                                if resolve_tls_mode(&host, &tld).await.is_passthrough() {
                                    serve_passthrough(stream, &host, &tld).await;
                                    return;
                                }
                            }
                            SniProbe::NoHost => {}
                            // The hostname could not be read here, so this
                            // connection is handed to the TLS acceptor like any
                            // other: rustls defragments the handshake itself
                            // and hands the real host name to the certificate
                            // resolver, which refuses to issue for a
                            // passthrough hostname rather than answering with
                            // the proxy's certificate. Unrelated terminating
                            // hostnames are served normally.
                            SniProbe::Undetermined => {
                                log::debug!(
                                    "Could not read the ClientHello of a TLS connection; \
                                     handing it to the TLS acceptor, which refuses to terminate \
                                     a passthrough hostname."
                                );
                                // Make sure the resolver's synchronous snapshot
                                // has been populated before it has to decide.
                                let _ = get_cached_slugs().await;
                            }
                        }

                        // TLS handshake → HTTP/2 or HTTP/1.1 (negotiated via ALPN)
                        match acceptor.accept(stream).await {
                            Ok(tls_stream) => {
                                let io = hyper_util::rt::TokioIo::new(tls_stream);
                                let svc = hyper_util::service::TowerToHyperService::new(app);
                                if let Err(e) = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                                    .serve_connection_with_upgrades(io, svc)
                                    .await
                                {
                                    // HTTP/2 RST_STREAM errors from cancelled browser requests
                                    // (navigation, HMR) are normal — log at debug to avoid noise.
                                    log::debug!("Connection error: {e}");
                                }
                            }
                            Err(e) => {
                                log::debug!("TLS handshake error: {e}");
                            }
                        }
                    } else {
                        // Plain HTTP on the TLS port → 302 redirect to HTTPS
                        let io = hyper_util::rt::TokioIo::new(stream);
                        let svc = hyper_util::service::TowerToHyperService::new(redirect_app);
                        let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                            .serve_connection_with_upgrades(io, svc)
                            .await;
                    }
                });

                while conn_tasks.try_join_next().is_some() {}
            }
            _ = cancel.cancelled() => {
                log::info!("Proxy server shutting down (cancel signal received)");
                break;
            }
        }
    }

    // Drain in-flight connections with a timeout.
    let drain_timeout = std::time::Duration::from_secs(10);
    let _ = tokio::time::timeout(drain_timeout, async {
        while conn_tasks.join_next().await.is_some() {}
    })
    .await;

    Ok(())
}

/// How long to wait for a complete ClientHello before giving up on reading SNI.
///
/// A client sends its ClientHello immediately after the TCP handshake, so this
/// only ever expires on a stalled or malicious connection.
#[cfg(feature = "proxy-tls")]
const SNI_PEEK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How much of the connection's opening bytes to examine while looking for SNI.
///
/// A ClientHello with a realistic extension set is well under 2 KiB; the cap
/// bounds the work done for a connection that never sends a parseable one.
#[cfg(feature = "proxy-tls")]
const SNI_PEEK_MAX_BYTES: usize = 16 * 1024;

/// What peeking at a connection's opening bytes established about its hostname.
///
/// The third case is the one that matters: "no hostname" and "hostname unknown"
/// must not be treated alike, because a connection whose hostname could not be
/// read may well belong to a passthrough daemon.
#[cfg(feature = "proxy-tls")]
#[derive(Debug, PartialEq, Eq)]
enum SniProbe {
    /// The ClientHello named this host.
    Host(String),
    /// A complete ClientHello carried no SNI, or the connection is not a TLS
    /// handshake at all. Either way it cannot name a passthrough daemon.
    NoHost,
    /// No verdict: the hello stalled, exceeded the inspection window, or never
    /// reconciled its own length fields. Passthrough cannot be ruled out.
    Undetermined,
}

/// Read the SNI hostname from a connection's ClientHello *without consuming
/// it*, so the same bytes are still available to whichever path handles the
/// connection.
///
/// `timeout` bounds the wait for a hello that arrives in pieces. Note that a
/// client which sends part of a hello and then closes cannot be detected here:
/// `peek` keeps returning the buffered bytes rather than reporting end of file,
/// so such a connection is held until the timeout and then reported as
/// [`SniProbe::Undetermined`].
#[cfg(feature = "proxy-tls")]
async fn peek_sni_host(stream: &TcpStream, timeout: std::time::Duration) -> SniProbe {
    use crate::proxy::sni::{SniPeek, parse_sni};

    let deadline = tokio::time::Instant::now() + timeout;
    let mut buf = vec![0u8; 2048];

    loop {
        let n = match stream.peek(&mut buf).await {
            // End of file with nothing buffered: the client hung up before
            // saying anything, so there is nothing to route and nothing to
            // downgrade.
            Ok(0) => return SniProbe::NoHost,
            Ok(n) => n,
            Err(e) => {
                log::debug!("Failed to peek at a TLS connection: {e}");
                return SniProbe::Undetermined;
            }
        };
        match parse_sni(&buf[..n]) {
            SniPeek::Found(host) => return SniProbe::Host(host),
            SniPeek::Absent | SniPeek::NotTls => return SniProbe::NoHost,
            SniPeek::Incomplete => {}
        }

        // The hello may simply be longer than the window we looked at.
        if n == buf.len() && buf.len() < SNI_PEEK_MAX_BYTES {
            buf.resize((buf.len() * 2).min(SNI_PEEK_MAX_BYTES), 0);
            continue;
        }
        if n >= SNI_PEEK_MAX_BYTES {
            log::debug!("Giving up on SNI after {n} bytes without a complete ClientHello");
            return SniProbe::Undetermined;
        }
        if tokio::time::Instant::now() >= deadline {
            log::debug!("Timed out waiting for a complete ClientHello ({n} bytes read)");
            return SniProbe::Undetermined;
        }
        // Peeked data stays in the socket buffer, so `readable()` would return
        // immediately and spin. Sleep briefly instead and re-peek.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Splice a TLS connection straight through to its daemon.
///
/// Neither direction is inspected or rewritten, so the daemon terminates TLS
/// with its own certificate, negotiates its own ALPN (HTTP/2 and gRPC included)
/// and can require client certificates — none of which survive termination at
/// the proxy.
///
/// A stopped daemon is auto-started first and the connection is held until it
/// is ready, bounded by `proxy.auto_start_timeout`: a raw TLS stream has no
/// equivalent of the HTML "Starting…" page. When routing fails there is
/// likewise nothing to reply with, so the connection is closed and the reason
/// is logged.
#[cfg(feature = "proxy-tls")]
async fn serve_passthrough(mut stream: TcpStream, host: &str, tld: &str) {
    let port = match resolve_passthrough_port(host, tld).await {
        Ok(port) => port,
        Err(msg) => {
            log::warn!("TLS passthrough for '{host}' failed: {msg}");
            return;
        }
    };

    let addr = SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    let mut backend = match connect_backend(addr).await {
        Ok(b) => b,
        Err(e) => {
            log::warn!("TLS passthrough for '{host}': failed to connect to {addr}: {e}");
            return;
        }
    };

    log::debug!("TLS passthrough: splicing '{host}' to {addr}");
    if let Err(e) = tokio::io::copy_bidirectional(&mut stream, &mut backend).await {
        // A client or daemon closing one half mid-stream is ordinary.
        log::debug!("TLS passthrough for '{host}' ended: {e}");
    }
}

/// How long to keep retrying a refused connection to a daemon that has just
/// been reported ready.
///
/// A daemon can be recorded as running and holding a port a moment before its
/// listener actually accepts. This window covers that gap without making a
/// genuinely dead port hang the client for the full auto-start budget.
#[cfg(feature = "proxy-tls")]
const PASSTHROUGH_CONNECT_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Connect to a daemon's port, retrying a refused connection briefly.
#[cfg(feature = "proxy-tls")]
async fn connect_backend(addr: SocketAddr) -> std::io::Result<TcpStream> {
    let deadline = tokio::time::Instant::now() + PASSTHROUGH_CONNECT_GRACE;
    loop {
        match TcpStream::connect(addr).await {
            Ok(stream) => return Ok(stream),
            Err(e) => {
                let retryable = e.kind() == std::io::ErrorKind::ConnectionRefused;
                if !retryable || tokio::time::Instant::now() >= deadline {
                    return Err(e);
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// Resolve the daemon port a passthrough hostname splices to, starting the
/// daemon if it is not running.
///
/// Waiting replaces the "Starting…" page that the HTTP path shows, and the
/// whole resolution — waiting behind another connection's start *and* running
/// one of its own — is bounded by a single `proxy.auto_start_timeout` budget,
/// which is what the documentation promises.
#[cfg(feature = "proxy-tls")]
async fn resolve_passthrough_port(host: &str, tld: &str) -> std::result::Result<u16, String> {
    let budget = settings().proxy_auto_start_timeout();
    match tokio::time::timeout(budget, resolve_passthrough_port_inner(host, tld)).await {
        Ok(result) => result,
        Err(_elapsed) => Err(format!(
            "no daemon was ready for '{host}' within proxy.auto_start_timeout ({budget:?})"
        )),
    }
}

/// Inner loop of [`resolve_passthrough_port`], wrapped by the caller so that
/// waiting and starting share one deadline.
#[cfg(feature = "proxy-tls")]
async fn resolve_passthrough_port_inner(host: &str, tld: &str) -> std::result::Result<u16, String> {
    loop {
        match resolve_target(host, tld).await {
            ResolveResult::Ready(port) => return Ok(port),
            // Another connection is already auto-starting this daemon; wait
            // for it rather than starting a second copy. The caller's timeout
            // ends this wait.
            ResolveResult::Starting { slug: _ } => {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            ResolveResult::NotFound => {
                return Err(
                    "no running daemon with a port matched this hostname, and it could not \
                     be auto-started"
                        .to_string(),
                );
            }
            ResolveResult::Error(msg) => return Err(msg),
        }
    }
}

/// Fallback when proxy-tls feature is not enabled.
#[cfg(not(feature = "proxy-tls"))]
async fn serve_https_with_http_fallback(
    _app: Router,
    _addr: SocketAddr,
    _s: &crate::settings::Settings,
    _effective_port: u16,
    _effective_tld: String,
    bind_tx: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
    _cancel: tokio_util::sync::CancellationToken,
) -> crate::Result<()> {
    let msg = "HTTPS proxy support requires the `proxy-tls` feature.\n\
         Rebuild pitchfork with: cargo build --features proxy-tls"
        .to_string();
    let _ = bind_tx.send(Err(msg.clone()));
    miette::bail!("{msg}")
}

/// Resolve the CA certificate and key paths from settings.
///
/// If `tls_cert` / `tls_key` are empty, falls back to the auto-generated
/// CA paths in `$PITCHFORK_STATE_DIR/proxy/`.
#[cfg(feature = "proxy-tls")]
fn resolve_tls_paths(s: &crate::settings::Settings) -> (std::path::PathBuf, std::path::PathBuf) {
    let proxy_dir = crate::env::PITCHFORK_STATE_DIR.join("proxy");
    let resolve = |configured: &str, default: &str| {
        if configured.is_empty() {
            proxy_dir.join(default)
        } else {
            std::path::PathBuf::from(configured)
        }
    };
    (
        resolve(&s.proxy.tls_cert, "ca.pem"),
        resolve(&s.proxy.tls_key, "ca-key.pem"),
    )
}

/// Generate a local root CA certificate and private key using `rcgen`.
///
/// The CA is used to sign per-domain certificates on demand (SNI).
/// Files are written in PEM format to `cert_path` and `key_path`.
#[cfg(feature = "proxy-tls")]
pub fn generate_ca(cert_path: &std::path::Path, key_path: &std::path::Path) -> crate::Result<()> {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyUsagePurpose,
    };

    // Create parent directory if needed
    if let Some(parent) = cert_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| miette::miette!("Failed to create proxy cert directory: {e}"))?;
    }

    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "Pitchfork Local CA");
    dn.push(DnType::OrganizationName, "Pitchfork");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

    let key_pair = rcgen::KeyPair::generate()
        .map_err(|e| miette::miette!("Failed to generate CA key pair: {e}"))?;
    let ca_cert = params
        .self_signed(&key_pair)
        .map_err(|e| miette::miette!("Failed to self-sign CA certificate: {e}"))?;

    // Write the CA certificate (public — 0644 is fine)
    std::fs::write(cert_path, ca_cert.pem()).map_err(|e| {
        miette::miette!(
            "Failed to write CA certificate to {}: {e}",
            cert_path.display()
        )
    })?;

    // Write the CA private key with restrictive permissions (0600).
    // Using OpenOptions + mode() so the file is never world-readable,
    // even briefly before a chmod call.
    {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(key_path)
                .and_then(|mut f| f.write_all(key_pair.serialize_pem().as_bytes()))
                .map_err(|e| {
                    miette::miette!("Failed to write CA key to {}: {e}", key_path.display())
                })?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(key_path, key_pair.serialize_pem()).map_err(|e| {
                miette::miette!("Failed to write CA key to {}: {e}", key_path.display())
            })?;
            log::debug!(
                "CA private key written to {} (file permissions are not restricted \
                 on non-Unix platforms — consider restricting access manually)",
                key_path.display()
            );
        }
    }

    Ok(())
}

/// SNI-based certificate resolver.
///
/// Holds the local CA and a two-level cache of per-domain certificates:
/// - L1: in-memory `HashMap` (fastest, process-lifetime)
/// - L2: on-disk `host-certs/<safe_name>.pem` (survives restarts)
///
/// A `pending` set prevents concurrent requests for the same domain from
/// triggering multiple simultaneous cert-generation operations.
///
/// On each new TLS connection, `resolve()` is called with the SNI hostname;
/// if no cached cert exists for that domain, one is signed by the CA on the fly.
///
/// # Locking strategy
/// Both `cache` and `pending` use `std::sync::Mutex` paired with a
/// `std::sync::Condvar`.  The critical sections are intentionally short
/// (hash-map lookups / inserts), so the blocking time is negligible.
/// `get_or_create` is only called from the synchronous `ResolvesServerCert`
/// trait method (not from an async context), so blocking a thread here is
/// acceptable.
#[cfg(feature = "proxy-tls")]
struct SniCertResolver {
    /// The CA issuer (key + parsed cert params, used to sign leaf certs).
    issuer: rcgen::Issuer<'static, rcgen::KeyPair>,
    /// The TLD hostnames are resolved against, so a passthrough hostname can
    /// be recognized before a certificate is issued for it.
    tld: String,
    /// Directory where per-domain PEM files are cached on disk.
    host_certs_dir: std::path::PathBuf,
    /// L1 cache: domain → certified key (in-memory).
    cache: std::sync::Mutex<std::collections::HashMap<String, Arc<rustls::sign::CertifiedKey>>>,
    /// Pending set: domains currently being generated (dedup concurrent requests).
    /// Using a `Condvar` so waiting threads are parked instead of spin-sleeping,
    /// which avoids blocking tokio worker threads.
    pending: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Condvar paired with `pending` — notified when a domain is removed from the set.
    pending_cv: std::sync::Condvar,
}

#[cfg(feature = "proxy-tls")]
impl std::fmt::Debug for SniCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SniCertResolver").finish_non_exhaustive()
    }
}

#[cfg(feature = "proxy-tls")]
impl SniCertResolver {
    /// Load the CA from disk and prepare the resolver.
    fn new(
        ca_cert_path: &std::path::Path,
        ca_key_path: &std::path::Path,
        tld: String,
    ) -> crate::Result<Self> {
        let ca_key_pem = std::fs::read_to_string(ca_key_path)
            .map_err(|e| miette::miette!("Failed to read CA key {}: {e}", ca_key_path.display()))?;
        let ca_cert_pem = std::fs::read_to_string(ca_cert_path).map_err(|e| {
            miette::miette!("Failed to read CA cert {}: {e}", ca_cert_path.display())
        })?;

        // Verify the PEM is readable (sanity check)
        if !ca_cert_pem.contains("BEGIN CERTIFICATE") {
            miette::bail!("CA cert file does not contain a valid PEM certificate");
        }

        let ca_key = rcgen::KeyPair::from_pem(&ca_key_pem)
            .map_err(|e| miette::miette!("Failed to parse CA key: {e}"))?;

        // Parse the CA cert + key into an Issuer for signing leaf certs.
        let issuer = rcgen::Issuer::from_ca_cert_pem(&ca_cert_pem, ca_key)
            .map_err(|e| miette::miette!("Failed to parse CA cert: {e}"))?;

        // Ensure the host-certs directory exists
        let host_certs_dir = ca_cert_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("host-certs");
        std::fs::create_dir_all(&host_certs_dir)
            .map_err(|e| miette::miette!("Failed to create host-certs dir: {e}"))?;

        Ok(Self {
            issuer,
            tld,
            host_certs_dir,
            cache: std::sync::Mutex::new(std::collections::HashMap::new()),
            pending: std::sync::Mutex::new(std::collections::HashSet::new()),
            pending_cv: std::sync::Condvar::new(),
        })
    }

    /// Get or create a `CertifiedKey` for the given domain.
    ///
    /// Resolution order:
    /// 1. L1 in-memory cache
    /// 2. L2 on-disk cache (`host-certs/<safe_name>.pem`)
    /// 3. Generate fresh cert, persist to disk, populate both caches
    ///
    /// Concurrent requests for the same domain are deduplicated: the second
    /// thread waits on a `Condvar` until the first thread finishes, then reads
    /// from the cache.  This avoids both duplicate cert generation and the
    /// spin-sleep anti-pattern that would block tokio worker threads.
    ///
    /// # Locking discipline
    /// `cache` and `pending` are **never held simultaneously**.  The protocol is:
    /// 1. Check `cache` (lock, read, unlock).
    /// 2. Acquire `pending`; wait if domain is in-progress; re-check `cache`
    ///    after waking (unlock `cache` before re-acquiring `pending` is not
    ///    needed because we release `cache` before entering the `pending` block).
    /// 3. Insert domain into `pending`; release `pending` lock.
    /// 4. Generate cert (no locks held).
    /// 5. Insert into `cache` (lock, write, unlock).
    /// 6. Remove from `pending` and notify (lock, write, unlock).
    fn get_or_create(&self, domain: &str) -> Option<Arc<rustls::sign::CertifiedKey>> {
        // L1: memory cache (fast path — no pending lock needed)
        {
            let cache = self.cache.lock().ok()?;
            if let Some(ck) = cache.get(domain) {
                return Some(Arc::clone(ck));
            }
        } // cache lock released here

        // Dedup: acquire the pending lock, wait if another thread is generating
        // this domain, then re-check the cache (without holding pending) before
        // deciding to generate.
        //
        // We deliberately release the pending lock before re-checking the cache
        // to avoid holding both locks simultaneously.  The re-check is safe
        // because: if the generating thread inserted into the cache and then
        // removed from pending, we will see the cert in the cache.  If we miss
        // the window (extremely unlikely), we will generate a duplicate cert,
        // which is harmless — the last writer wins in the cache.
        loop {
            {
                let mut pending = self.pending.lock().ok()?;
                if pending.contains(domain) {
                    // Another thread is generating; wait until it finishes.
                    pending = self.pending_cv.wait(pending).ok()?;
                    // pending lock re-acquired; loop to re-check cache below.
                    drop(pending);
                } else {
                    // No one else is generating; claim the slot and proceed.
                    pending.insert(domain.to_string());
                    break;
                }
            } // pending lock released

            // Re-check cache after being woken (the generating thread may have
            // already populated it).  Cache lock is acquired independently of
            // pending lock here — no nesting.
            {
                let cache = self.cache.lock().ok()?;
                if let Some(ck) = cache.get(domain) {
                    return Some(Arc::clone(ck));
                }
            } // cache lock released
        } // pending lock released at break

        let result = self.get_or_create_inner(domain);

        // Always clear the pending flag and wake waiting threads.
        // notify_all() is called *inside* the lock scope so that the domain is
        // guaranteed to be removed before any waiting thread is woken up.
        // If the lock is poisoned we recover it (the data is still valid) so
        // that the domain is always removed and waiters are always notified.
        {
            let mut pending = match self.pending.lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            pending.remove(domain);
            self.pending_cv.notify_all();
        }

        result
    }

    /// Inner implementation: check disk cache, then generate.
    fn get_or_create_inner(&self, domain: &str) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let safe_name = domain.replace('.', "_").replace('*', "wildcard");
        let disk_path = self.host_certs_dir.join(format!("{safe_name}.pem"));

        // L2: disk cache — try to load existing cert+key PEM
        if disk_path.exists() {
            if let Ok(ck) = self.load_from_disk(&disk_path) {
                let ck = Arc::new(ck);
                if let Ok(mut cache) = self.cache.lock() {
                    cache.insert(domain.to_string(), Arc::clone(&ck));
                }
                return Some(ck);
            }
            // Disk cache corrupt/expired — fall through to regenerate
            let _ = std::fs::remove_file(&disk_path);
        }

        // L3: generate fresh cert
        let ck = self.sign_for_domain(domain).ok()?;

        let ck = Arc::new(ck);
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(domain.to_string(), Arc::clone(&ck));
        }
        Some(ck)
    }

    /// Load a `CertifiedKey` from a combined cert+key PEM file on disk.
    ///
    /// Returns an error if the certificate has already expired, so the caller
    /// can fall through to regeneration rather than serving a stale cert.
    fn load_from_disk(&self, path: &std::path::Path) -> crate::Result<rustls::sign::CertifiedKey> {
        use rustls::pki_types::CertificateDer;
        use rustls_pemfile::{certs, private_key};

        let pem = std::fs::read_to_string(path)
            .map_err(|e| miette::miette!("Failed to read disk cert {}: {e}", path.display()))?;

        let cert_ders: Vec<CertificateDer<'static>> = certs(&mut pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| miette::miette!("Failed to parse certs from {}: {e}", path.display()))?;

        if cert_ders.is_empty() {
            miette::bail!("No certificates found in {}", path.display());
        }

        // Check that the first certificate has not expired using x509-parser.
        {
            let (_, cert) = x509_parser::parse_x509_certificate(&cert_ders[0]).map_err(|e| {
                miette::miette!("Failed to parse certificate from {}: {e}", path.display())
            })?;
            use chrono::Utc;
            let now_ts = Utc::now().timestamp();
            let not_after_ts = cert.validity().not_after.timestamp();
            if not_after_ts < now_ts {
                miette::bail!(
                    "Cached certificate at {} has expired — will regenerate",
                    path.display()
                );
            }
        }

        let key_der = private_key(&mut pem.as_bytes())
            .map_err(|e| miette::miette!("Failed to parse key from {}: {e}", path.display()))?
            .ok_or_else(|| miette::miette!("No private key found in {}", path.display()))?;

        let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
            .map_err(|e| miette::miette!("Failed to create signing key from disk: {e}"))?;

        Ok(rustls::sign::CertifiedKey::new(cert_ders, signing_key))
    }

    /// Sign a leaf certificate for `domain` using the CA.
    ///
    /// SANs include:
    /// - `DNS:<domain>` (exact match)
    /// - `DNS:*.<parent>` (sibling wildcard, e.g. `*.pf.localhost` for `docs.pf.localhost`)
    ///
    /// Returns both the `CertifiedKey` and the combined PEM for disk caching.
    fn sign_for_domain(&self, domain: &str) -> crate::Result<rustls::sign::CertifiedKey> {
        use rcgen::date_time_ymd;
        use rcgen::{CertificateParams, DistinguishedName, DnType, SanType};
        use rustls::pki_types::CertificateDer;
        use rustls_pemfile::private_key;

        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, domain);
        params.distinguished_name = dn;

        // Set validity dynamically: from yesterday to 10 years from now.
        {
            use chrono::{Datelike, Duration, Utc};
            let yesterday = Utc::now() - Duration::days(1);
            // 397 days: stays within Chrome/Safari's 398-day maximum validity limit
            // for TLS certificates (including locally-trusted CA leaf certs).
            let expiry = Utc::now() + Duration::days(397);
            params.not_before = date_time_ymd(
                yesterday.year(),
                yesterday.month() as u8,
                yesterday.day() as u8,
            );
            params.not_after =
                date_time_ymd(expiry.year(), expiry.month() as u8, expiry.day() as u8);
        }

        // Build SANs: exact domain + sibling wildcard (e.g. *.pf.localhost)
        let mut sans =
            vec![SanType::DnsName(domain.to_string().try_into().map_err(
                |e| miette::miette!("Invalid domain name '{domain}': {e}"),
            )?)];
        // Add wildcard SAN for the parent domain (one level up)
        if let Some(dot_pos) = domain.find('.') {
            let parent = &domain[dot_pos + 1..];
            // Only add wildcard if parent has at least one dot (not a bare TLD)
            if parent.contains('.') {
                let wildcard = format!("*.{parent}");
                if let Ok(wc) = wildcard.try_into() {
                    sans.push(SanType::DnsName(wc));
                }
            }
        }
        params.subject_alt_names = sans;

        let leaf_key = rcgen::KeyPair::generate()
            .map_err(|e| miette::miette!("Failed to generate leaf key: {e}"))?;
        let leaf_cert = params
            .signed_by(&leaf_key, &self.issuer)
            .map_err(|e| miette::miette!("Failed to sign leaf cert for '{domain}': {e}"))?;

        // Convert to rustls types
        let cert_der = CertificateDer::from(leaf_cert.der().to_vec());
        let key_pem = leaf_key.serialize_pem();
        let key_der = private_key(&mut key_pem.as_bytes())
            .map_err(|e| miette::miette!("Failed to parse leaf key PEM: {e}"))?
            .ok_or_else(|| miette::miette!("No private key found in generated PEM"))?;

        let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
            .map_err(|e| miette::miette!("Failed to create signing key: {e}"))?;

        // Persist cert + key to disk cache as combined PEM.
        // Use 0600 so the private key is not world-readable.
        let safe_name = domain.replace('.', "_").replace('*', "wildcard");
        let disk_path = self.host_certs_dir.join(format!("{safe_name}.pem"));
        let combined_pem = format!("{}{}", leaf_cert.pem(), key_pem);
        {
            #[cfg(unix)]
            {
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                if let Err(e) = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&disk_path)
                    .and_then(|mut f| f.write_all(combined_pem.as_bytes()))
                {
                    log::warn!(
                        "Failed to persist cert for '{domain}' to {}: {e}",
                        disk_path.display()
                    );
                }
            }
            #[cfg(not(unix))]
            {
                if let Err(e) = std::fs::write(&disk_path, combined_pem) {
                    log::warn!(
                        "Failed to persist cert for '{domain}' to {}: {e}",
                        disk_path.display()
                    );
                } else {
                    log::debug!(
                        "Leaf cert for '{domain}' written to {} (file permissions are not \
                         restricted on non-Unix platforms — consider restricting access manually)",
                        disk_path.display()
                    );
                }
            }
        }

        Ok(rustls::sign::CertifiedKey::new(vec![cert_der], signing_key))
    }
}

#[cfg(feature = "proxy-tls")]
impl rustls::server::ResolvesServerCert for SniCertResolver {
    fn resolve(
        &self,
        client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let domain = client_hello.server_name()?;

        // This is the last point where a passthrough hostname can still be
        // caught, and the first where the host name is authoritative: rustls
        // has reassembled the handshake itself. Issuing here would answer with
        // the proxy's certificate in place of the daemon's and drop the
        // client-certificate request, so the handshake is failed instead.
        // Reached only when the pre-handshake peek could not read the hello.
        if resolve_tls_mode_in(domain, &self.tld, &slug_snapshot()).is_passthrough() {
            log::warn!(
                "Refusing to terminate TLS for '{domain}', which is configured for \
                 proxy_tls = \"passthrough\": its ClientHello could not be inspected before the \
                 handshake, so the stream could not be spliced to the daemon."
            );
            return None;
        }

        self.get_or_create(domain)
    }
}

/// Get the effective host from a request.
///
/// HTTP/2 uses the `:authority` pseudo-header, which hyper exposes via
/// `req.uri().authority()` rather than in the `HeaderMap`.
/// HTTP/1.1 uses the `Host` header.
fn get_request_host(req: &Request) -> Option<String> {
    // HTTP/2: :authority is available via the request URI, not the HeaderMap.
    let authority = req
        .uri()
        .authority()
        .map(|a| a.as_str().to_string())
        .filter(|s| !s.is_empty());

    authority.or_else(|| {
        req.headers()
            .get(HOST)
            .and_then(|h| h.to_str().ok())
            .map(str::to_string)
    })
}

/// Rejoin a `cookie` header that arrived split across several fields.
///
/// An HTTP/2 client may send each cookie as its own header field (RFC 9113
/// §8.2.3). An HTTP/1.1 backend joins repeated fields with `", "`, which
/// corrupts every cookie value, so they must be joined with `"; "` first.
fn join_cookie_fields(headers: &mut HeaderMap) {
    let fields: Vec<&[u8]> = headers
        .get_all(COOKIE)
        .iter()
        .map(HeaderValue::as_bytes)
        .collect();
    if fields.len() < 2 {
        return;
    }

    let joined = HeaderValue::from_bytes(&fields.join(b"; ".as_slice()))
        .expect("valid header values joined with \"; \" form a valid header value");
    headers.insert(COOKIE, joined);
}

/// Inject `X-Forwarded-*` headers into a proxied request.
///
/// Because the proxy is a **first-hop** dev tool (not a mid-tier forwarder),
/// all four headers are **unconditionally overwritten** with values derived
/// from the actual incoming connection.  Any values supplied by the connecting
/// client are discarded.
///
/// Trusting client-supplied `x-forwarded-for` / `x-forwarded-proto` would
/// allow a local process to spoof a remote IP or trick a backend's
/// HTTPS-detection logic (CSRF checks, secure-cookie flags, redirect rules).
fn inject_forwarded_headers(req: &mut Request, is_tls: bool, host_header: &str) {
    let remote_addr = req
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());

    let proto = if is_tls { "https" } else { "http" };
    let default_port = if is_tls { "443" } else { "80" };

    // Always set fresh values — we are the edge, never a mid-tier forwarder.
    // Discard any x-forwarded-* headers supplied by the connecting client.
    let forwarded_for = remote_addr.clone();
    let forwarded_proto = proto.to_string();
    let forwarded_host = host_header.to_string();
    let forwarded_port = host_header
        .rsplit_once(':')
        .map(|(_, port)| port.to_string())
        .unwrap_or_else(|| default_port.to_string());

    // Strip any client-supplied x-forwarded-* and RFC 7239 Forwarded headers
    // before inserting ours, so that no trace of the original values reaches
    // the backend.  The RFC 7239 `Forwarded` header is stripped alongside the
    // legacy `x-forwarded-*` set because backends that read it (Django, Rails,
    // Spring) would otherwise see client-injected spoofed IPs or protocols.
    for name in [
        "x-forwarded-for",
        "x-forwarded-proto",
        "x-forwarded-host",
        "x-forwarded-port",
        "forwarded",
    ] {
        if let Ok(header_name) = axum::http::HeaderName::from_bytes(name.as_bytes()) {
            req.headers_mut().remove(&header_name);
        }
    }

    let headers = [
        ("x-forwarded-for", forwarded_for),
        ("x-forwarded-proto", forwarded_proto),
        ("x-forwarded-host", forwarded_host),
        ("x-forwarded-port", forwarded_port),
    ];

    for (name, value) in headers {
        if let Ok(v) = HeaderValue::from_str(&value) {
            let header_name = axum::http::HeaderName::from_static(name);
            req.headers_mut().insert(header_name, v);
        }
    }
}

/// Main proxy request handler.
///
/// Parses the `Host` header, resolves the target daemon, and forwards the request.
/// WebSocket / HTTP upgrade requests are forwarded transparently via hyper's upgrade mechanism.
async fn proxy_handler(State(state): State<ProxyState>, mut req: Request) -> Response {
    // Extract the host (supports both HTTP/2 :authority and HTTP/1.1 Host)
    let Some(raw_host) = get_request_host(&req) else {
        return error_response(StatusCode::BAD_REQUEST, "Missing Host header");
    };
    // Strip port from host for routing.
    // IPv6 addresses in Host headers are bracketed per RFC 2732: `[::1]:port`.
    // Splitting naïvely on ':' would break on the colons inside the address.
    let host = if raw_host.starts_with('[') {
        // IPv6: "[::1]:port" or "[::1]"
        raw_host
            .split("]:")
            .next()
            .unwrap_or(&raw_host)
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string()
    } else {
        // IPv4 / hostname: "host:port" or "host"
        raw_host.split(':').next().unwrap_or(&raw_host).to_string()
    };

    // Loop detection: check hop count.
    //
    // Security: strip (zero out) the hop counter on the very first hop to
    // prevent external clients from forging a high value and triggering a
    // 508 Loop Detected response (denial-of-service).  A request is
    // considered "first hop" when it does not carry the `x-pitchfork-hops`
    // request header that pitchfork injects when forwarding — i.e. it did
    // not come from another pitchfork proxy instance.
    // Note: `x-pitchfork` is a *response* header added by pitchfork and is
    // never present on incoming requests, so it cannot be used here.
    let is_from_pitchfork = req.headers().contains_key(PROXY_HOPS_HEADER);
    let hops: u64 = if is_from_pitchfork {
        req.headers()
            .get(PROXY_HOPS_HEADER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    } else {
        // External request: ignore any forged hop counter.
        0
    };
    if hops >= MAX_PROXY_HOPS {
        return error_response(
            StatusCode::LOOP_DETECTED,
            &format!(
                "Loop detected for '{host}': request has passed through the proxy {hops} times.\n\
                 This usually means a backend is proxying back through pitchfork without rewriting \n\
                 the Host header. If you use Vite/webpack proxy, set changeOrigin: true."
            ),
        );
    }

    // Intercept "pitchfork.<tld>" — route to the built-in web UI
    let target_port = if let Some(subdomain) = strip_tld(&host, &state.tld) {
        if subdomain == "pitchfork" {
            crate::web::port()
        } else {
            None
        }
    } else {
        None
    };

    let target_port = if let Some(port) = target_port {
        port
    } else {
        // A passthrough hostname must never be forwarded as plain HTTP: the
        // daemon expects a TLS handshake on that port, so the request would
        // fail deep inside the daemon with nothing to point at the cause.
        if resolve_tls_mode(&host, &state.tld).await.is_passthrough() {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &passthrough_unroutable_message(&host, state.is_tls),
            );
        }
        match resolve_target(&host, &state.tld).await {
            ResolveResult::Ready(port) => port,
            ResolveResult::Starting { slug } => {
                return starting_html_response(&slug, &raw_host);
            }
            ResolveResult::NotFound => {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    &format!(
                        "No daemon found for host '{host}'.\n\
                         Make sure the daemon has a slug, is running, and has a port configured.\n\
                         Expected format: <slug>.{tld}",
                        tld = state.tld
                    ),
                );
            }
            ResolveResult::Error(msg) => {
                return error_response(StatusCode::BAD_GATEWAY, &msg);
            }
        }
    };
    // Build the forwarding URI
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let forward_uri = match Uri::builder()
        .scheme("http")
        .authority(format!("localhost:{target_port}"))
        .path_and_query(path_and_query)
        .build()
    {
        Ok(uri) => uri,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to build forward URI: {e}"),
            );
        }
    };

    // Update the request URI and Host header
    *req.uri_mut() = forward_uri;
    req.headers_mut().insert(
        HOST,
        HeaderValue::from_str(&format!("localhost:{target_port}"))
            .unwrap_or_else(|_| HeaderValue::from_static("localhost")),
    );

    // Inject X-Forwarded-* headers
    inject_forwarded_headers(&mut req, state.is_tls, &raw_host);

    // Increment hop counter
    if let Ok(v) = HeaderValue::from_str(&(hops + 1).to_string()) {
        req.headers_mut()
            .insert(axum::http::HeaderName::from_static(PROXY_HOPS_HEADER), v);
    }

    // Explicitly strip HTTP/2 pseudo-headers (":authority", ":method", etc.)
    // before forwarding to an HTTP/1.1 backend. Although hyper typically does
    // not store pseudo-headers in the HeaderMap, some middleware layers or
    // future hyper versions might; stripping them here is a defensive measure.
    let pseudo_headers: Vec<_> = req
        .headers()
        .keys()
        .filter(|k| k.as_str().starts_with(':'))
        .cloned()
        .collect();
    for key in pseudo_headers {
        req.headers_mut().remove(&key);
    }

    join_cookie_fields(req.headers_mut());

    // Downgrade the forwarded request to HTTP/1.1. TLS connections negotiate
    // HTTP/2 inbound via ALPN, but the upstream forward client speaks HTTP/1 to
    // the daemon. Without this, the still-h2-tagged request is rejected by the
    // client with `UserUnsupportedVersion`, surfacing as a 502 to the browser.
    *req.version_mut() = axum::http::Version::HTTP_11;

    // Extract the client-side OnUpgrade handle *before* consuming req
    let client_upgrade = hyper::upgrade::on(&mut req);

    // Forward the request with a per-request timeout so that a backend that
    // accepts the TCP connection but then stalls (deadlock, blocking I/O, etc.)
    // cannot hold the proxy connection open forever and exhaust file descriptors.
    //
    // 120 s is intentionally generous for a local dev proxy — it covers slow
    // test suites, large file uploads, and SSE streams while still bounding
    // the worst-case resource leak.
    let result = match tokio::time::timeout(
        std::time::Duration::from_secs(120),
        state.client.request(req),
    )
    .await
    {
        Ok(r) => r,
        Err(_elapsed) => {
            let msg = format!(
                "Request to daemon on port {target_port} timed out after 120 s.\n\
                 The daemon accepted the connection but did not respond in time."
            );
            log::warn!("{msg}");
            if let Some(ref on_error) = state.on_error {
                on_error(&msg);
            }
            return error_response(StatusCode::GATEWAY_TIMEOUT, &msg);
        }
    };
    match result {
        Ok(mut resp) => {
            // Extract backend upgrade handle *before* consuming resp
            let backend_upgrade = hyper::upgrade::on(&mut resp);
            let (mut parts, body) = resp.into_parts();

            // Add pitchfork identification header
            parts.headers.insert(
                axum::http::HeaderName::from_static(PITCHFORK_HEADER),
                HeaderValue::from_static("1"),
            );

            // Strip the internal hop-counter so it is never leaked to external clients.
            parts.headers.remove(PROXY_HOPS_HEADER);

            // Strip hop-by-hop headers when serving HTTPS (HTTP/2 forbids them).
            // Skip 101 Switching Protocols — that response is always HTTP/1.1 and
            // the client needs the `Upgrade` header to complete the WS handshake
            // (RFC 6455 §4.1 requires `Upgrade: websocket` in the 101 response).
            if state.is_tls && parts.status != StatusCode::SWITCHING_PROTOCOLS {
                for h in HOP_BY_HOP_HEADERS {
                    if let Ok(name) = axum::http::HeaderName::from_bytes(h.as_bytes()) {
                        parts.headers.remove(&name);
                    }
                }
            }

            // If the backend returned 101 Switching Protocols, pipe the upgraded streams.
            if parts.status == StatusCode::SWITCHING_PROTOCOLS {
                // Note: loop detection for WebSocket upgrades is already handled at the
                // top of proxy_handler (hops >= MAX_PROXY_HOPS check) before the request
                // is forwarded.  A 101 response here means the backend accepted the
                // upgrade, so the hop count was already within limits.
                tokio::spawn(async move {
                    if let (Ok(client_upgraded), Ok(backend_upgraded)) =
                        (client_upgrade.await, backend_upgrade.await)
                    {
                        let mut client_io = hyper_util::rt::TokioIo::new(client_upgraded);
                        let mut backend_io = hyper_util::rt::TokioIo::new(backend_upgraded);
                        // No application-level timeout here: tokio::time::timeout would be a
                        // hard wall-clock deadline for the entire tunnel, not an idle timeout.
                        // Long-lived connections (Vite/webpack HMR, SSE-over-WS) would be
                        // silently terminated after the deadline even if data is actively
                        // flowing.  The OS TCP keepalive is sufficient to reap truly dead
                        // connections; a proper idle timeout would require a custom
                        // AsyncRead/AsyncWrite wrapper that resets the timer on each I/O op.
                        let _ =
                            tokio::io::copy_bidirectional(&mut client_io, &mut backend_io).await;
                    }
                });
                return Response::from_parts(parts, Body::empty());
            }

            // Backend refused the upgrade (returned a non-101 response) — forward it as-is.
            // This can happen when the backend rejects a WebSocket handshake with e.g. 400.
            Response::from_parts(parts, Body::new(body))
        }
        Err(e) => {
            let msg = format!(
                "Failed to connect to daemon on port {target_port}: {e}\n\
                 The daemon may have stopped or is not yet ready."
            );
            if let Some(ref on_error) = state.on_error {
                on_error(&msg);
            } else {
                log::warn!("{msg}");
            }
            error_response(StatusCode::BAD_GATEWAY, &msg)
        }
    }
}

/// Explain why a request for a `proxy_tls = "passthrough"` hostname arrived on
/// the HTTP path, where it cannot be served.
///
/// Passthrough routes on the hostname in the TLS ClientHello, so there are two
/// ways to end up here: the proxy is not serving TLS at all, or the TLS
/// connection named one host and the request inside it named another.
///
/// A connection with no SNI at all never reaches this point: the certificate
/// resolver has no name to issue for, so the handshake fails outright.
fn passthrough_unroutable_message(host: &str, is_tls: bool) -> String {
    if is_tls {
        format!(
            "'{host}' uses proxy_tls = \"passthrough\", which routes on the host name \
             in the TLS ClientHello.\n\
             This connection's TLS handshake named a different host, so it was \
             terminated here and the request inside it cannot be spliced to the \
             daemon.\n\
             Make the connection itself name '{host}' rather than overriding the Host \
             header of a connection opened to something else."
        )
    } else {
        format!(
            "'{host}' uses proxy_tls = \"passthrough\", but the proxy is serving \
             plain HTTP.\n\
             Passthrough splices a TLS stream to the daemon, so it requires \
             settings.proxy.https = true.\n\
             Enable HTTPS on the proxy, or set proxy_tls = \"terminate\" on the daemon."
        )
    }
}

/// Resolve the target for a given hostname.
///
/// Slug-based routing using the global config's `[slugs]` section:
/// 1. Strip TLD to get subdomain (the slug)
/// 2. Look up slug in global config → find project dir + daemon name
/// 3. Check state file for a running daemon with that name → get its port
/// 4. If `proxy.auto_start` is enabled and the daemon is not running,
///    trigger an automatic start and wait for it to become ready.
///
/// # Returns
/// - `ResolveResult::Ready(port)`       — daemon running (or just auto-started), forward to this port
/// - `ResolveResult::Starting { slug }` — daemon start in progress (show waiting page)
/// - `ResolveResult::NotFound`          — no daemon matched
/// - `ResolveResult::Error(msg)`        — routing refused with a descriptive reason
///
/// # Locking
/// The state file lock is held only for the duration of the snapshot copy,
/// then released immediately to avoid serialising all proxy requests.
async fn resolve_target(host: &str, tld: &str) -> ResolveResult {
    let ctx = match resolve_route_context(host, tld).await {
        Ok(ctx) => ctx,
        Err(result) => return result,
    };

    let daemons = {
        let state_file = SUPERVISOR.state_file.lock().await;
        state_file.daemons.clone()
    };

    let daemon_name = &ctx.cached.daemon_name;
    let running_matches: Vec<(&DaemonId, &crate::daemon::Daemon)> = daemons
        .iter()
        .filter(|(id, d)| {
            id.name() == daemon_name
                && d.status.is_running()
                && match &ctx.expected_namespace {
                    Some(ns) => id.namespace() == ns,
                    None => true,
                }
        })
        .collect();

    match running_matches.as_slice() {
        [] => {
            try_auto_start(
                &ctx.cached.slug,
                &ctx.cached,
                ctx.worktree_dir.as_deref(),
                ctx.expected_namespace.as_deref(),
                &ctx.route,
            )
            .await
        }
        // With more than one namespace running a daemon of this name and no
        // namespace to narrow by, the first match is used — as before.
        [(_, d), ..] => match select_daemon_port(&ctx.route, d) {
            Some(port) => ResolveResult::Ready(port),
            None => ResolveResult::NotFound,
        },
    }
}

/// What a hostname resolves to before the daemon's running state is consulted:
/// the slug it matched, the namespace and worktree it names, and how its TLS
/// is handled.
struct RouteContext {
    cached: CachedSlugEntry,
    expected_namespace: Option<String>,
    worktree_dir: Option<std::path::PathBuf>,
    route: ProxyTlsRoute,
}

/// Resolve a hostname to its slug, worktree and TLS route.
///
/// This is the part of routing that does not depend on whether the daemon is
/// running, which is what lets the 443 listener decide between terminating TLS
/// and splicing the raw stream before anything is started.
///
/// Returns `Err(ResolveResult)` when the host does not route at all, carrying
/// the response the caller should produce.
async fn resolve_route_context(host: &str, tld: &str) -> Result<RouteContext, ResolveResult> {
    let Some(subdomain) = strip_tld(host, tld) else {
        return Err(ResolveResult::NotFound);
    };

    let Some(cached) = cached_slug_lookup(&subdomain).await else {
        return Err(ResolveResult::NotFound);
    };

    // ─── Worktree prefix extraction ──────────────────────────────────────
    // When a wildcard subdomain like "feature-a.myapp" matched slug "myapp",
    // the prefix "feature-a" may correspond to a git worktree or jj workspace.
    let (expected_namespace, worktree_dir, route) = if !subdomain.eq_ignore_ascii_case(&cached.slug)
    {
        let prefix = strip_dot_suffix_ignore_case(&subdomain, &cached.slug);
        match prefix {
            Some(ref p) => match match_worktree_prefix(&cached, p) {
                PrefixMatch::Worktree(wt) => {
                    let ns = wt.namespace.clone().or_else(|| {
                        log::warn!(
                            "Worktree '{}' has no cached namespace; \
                             falling back to parent slug namespace.",
                            wt.path.display()
                        );
                        cached.namespace.clone()
                    });
                    let route = cached
                        .worktree_tls
                        .get(&wt.sanitized_branch.to_ascii_lowercase())
                        .copied()
                        .unwrap_or(cached.tls);
                    (ns, Some(wt.path.clone()), route)
                }
                PrefixMatch::Ambiguous => {
                    return Err(ResolveResult::Error(format!(
                        "'{host}' is ambiguous: more than one branch or workspace of '{slug}' \
                         sanitizes to the prefix '{p}', and host names are case-insensitive.\n\
                         Rename one of them so the prefixes differ by more than case, then \
                         reload.\n\
                         The supervisor log lists the colliding branches.",
                        slug = cached.slug,
                    )));
                }
                PrefixMatch::Unknown => (cached.namespace.clone(), None, cached.tls),
            },
            None => (cached.namespace.clone(), None, cached.tls),
        }
    } else {
        (cached.namespace.clone(), None, cached.tls)
    };

    Ok(RouteContext {
        cached,
        expected_namespace,
        worktree_dir,
        route,
    })
}

/// How the proxy should handle TLS for `host`.
///
/// Answered from the slug cache alone, so it is cheap enough to call on every
/// TLS connection. Anything that does not resolve to a daemon — an unknown
/// host, the built-in web UI, a bare IP — keeps today's behavior and
/// terminates.
pub(crate) async fn resolve_tls_mode(host: &str, tld: &str) -> ProxyTlsMode {
    let entries = get_cached_slugs().await;
    resolve_tls_mode_in(host, tld, &entries)
}

/// [`resolve_tls_mode`] against a given slug table, without awaiting.
///
/// Used by the certificate resolver, which runs in a synchronous trait method
/// and reads [`slug_snapshot`].
fn resolve_tls_mode_in(
    host: &str,
    tld: &str,
    entries: &std::collections::HashMap<String, CachedSlugEntry>,
) -> ProxyTlsMode {
    let Some(subdomain) = strip_tld(host, tld) else {
        return ProxyTlsMode::Terminate;
    };
    let Some(cached) = wildcard_slug_lookup(&subdomain, entries, settings().proxy.wildcard) else {
        return ProxyTlsMode::Terminate;
    };

    // A wildcard match may name a worktree, which carries its own setting.
    if !subdomain.eq_ignore_ascii_case(&cached.slug)
        && let Some(prefix) = strip_dot_suffix_ignore_case(&subdomain, &cached.slug)
        && let PrefixMatch::Worktree(wt) = match_worktree_prefix(cached, &prefix)
    {
        return cached
            .worktree_tls
            .get(&wt.sanitized_branch.to_ascii_lowercase())
            .map(|route| route.mode)
            .unwrap_or(cached.tls.mode);
    }

    cached.tls.mode
}

/// Pick which of a running daemon's ports a hostname forwards to.
///
/// Without `proxy_tls_port`, a terminating hostname uses the port the process
/// was detected listening on, falling back to the first resolved port — the
/// historical behavior, and the right answer for a single-port daemon. A
/// passthrough hostname reverses that order, because the first detected
/// listener of a multi-port daemon need not be the one speaking TLS.
///
/// With `proxy_tls_port` set, the configured port is matched by *position* in
/// the daemon's `port` list, so the mapping survives auto-bump: a daemon
/// configured for `[8443, 9443]` whose ports bumped to `[8444, 9444]` still
/// routes `proxy_tls_port = 9443` to 9444. Config validation rejects a
/// `proxy_tls_port` the daemon does not declare, so the only way the position
/// lookup can miss is a state record that predates the current config; the
/// port is then used as written only while the daemon has no resolved ports to
/// contradict it.
fn select_daemon_port(route: &ProxyTlsRoute, daemon: &crate::daemon::Daemon) -> Option<u16> {
    let Some(want) = route.port else {
        // A passthrough hostname has to reach the daemon's *TLS* listener.
        // `active_port` is whichever port was detected first, which on a
        // multi-port daemon can be a secondary plain-HTTP or status listener
        // that came up earlier, so the declared first port wins there — as
        // documented. A terminating hostname keeps preferring the detected
        // port, which is what routes daemons that declare no ports at all.
        return if route.mode.is_passthrough() {
            daemon.resolved_port.first().copied().or(daemon.active_port)
        } else {
            daemon
                .active_port
                .or_else(|| daemon.resolved_port.first().copied())
        };
    };

    let configured = daemon
        .port
        .as_ref()
        .map(|p| p.expect.as_slice())
        .unwrap_or(&[]);
    if let Some(idx) = configured.iter().position(|&p| p == want)
        && let Some(&resolved) = daemon.resolved_port.get(idx)
    {
        return Some(resolved);
    }
    if daemon.resolved_port.contains(&want) {
        return Some(want);
    }
    if daemon.resolved_port.is_empty() {
        // Nothing recorded to place the port against — the config is the only
        // information there is, so use it.
        return Some(want);
    }
    log::warn!(
        "Daemon {} has proxy_tls_port {want}, which is not among its resolved ports {:?}; \
         refusing to route rather than forwarding to a port it never bound. \
         Restart the daemon if its ports changed.",
        daemon.id,
        daemon.resolved_port,
    );
    None
}

/// RAII guard that removes a `DaemonId` from `AUTO_START_IN_PROGRESS` on drop.
///
/// This ensures the in-progress flag is cleared even if the auto-start future
/// panics (e.g. an unexpected `unwrap` inside a dependency).  Without this,
/// the daemon ID would stay in the set permanently and every subsequent proxy
/// request would return "Starting …" forever.
struct AutoStartGuard {
    daemon_id: DaemonId,
}

impl Drop for AutoStartGuard {
    fn drop(&mut self) {
        let daemon_id = self.daemon_id.clone();
        // Spawn a cleanup task because `Drop` is synchronous and the mutex is
        // async.  If the runtime is shutting down this may not execute, but in
        // that case the entire set is being dropped anyway.
        tokio::spawn(async move {
            AUTO_START_IN_PROGRESS.lock().await.remove(&daemon_id);
        });
    }
}

/// Attempt to auto-start a daemon for the given slug.
///
/// If `proxy.auto_start` is disabled, returns `NotFound`.
/// Uses a dedup set to prevent concurrent starts for the same daemon.
/// Calls `SUPERVISOR.run()` with `wait_ready = true` so the daemon goes
/// through the same readiness lifecycle as `pf start`, then polls for the
/// active port.
///
/// The entire operation — including `SUPERVISOR.run()` and the port-polling
/// loop — is bounded by `proxy_auto_start_timeout`.
async fn try_auto_start(
    slug: &str,
    cached: &CachedSlugEntry,
    worktree_dir: Option<&std::path::Path>,
    expected_namespace: Option<&str>,
    route: &ProxyTlsRoute,
) -> ResolveResult {
    let s = settings();
    if !s.proxy.auto_start {
        return ResolveResult::NotFound;
    }

    let ns = expected_namespace
        .map(|s| s.to_string())
        .or_else(|| cached.namespace.clone())
        .unwrap_or_else(|| "global".to_string());
    let daemon_id = match DaemonId::try_new(&ns, &cached.daemon_name) {
        Ok(id) => id,
        Err(_) => return ResolveResult::NotFound,
    };

    {
        let mut in_progress = AUTO_START_IN_PROGRESS.lock().await;
        if !in_progress.insert(daemon_id.clone()) {
            return ResolveResult::Starting {
                slug: slug.to_string(),
            };
        }
    }

    let _guard = AutoStartGuard {
        daemon_id: daemon_id.clone(),
    };

    let timeout = s.proxy_auto_start_timeout();

    match tokio::time::timeout(
        timeout,
        try_auto_start_inner(slug, cached, &daemon_id, worktree_dir, route),
    )
    .await
    {
        Ok(result) => result,
        Err(_elapsed) => {
            log::warn!("Auto-start: total timeout ({timeout:?}) exceeded for daemon {daemon_id}");
            ResolveResult::Error(format!(
                "Auto-start for '{daemon_id}' timed out after {timeout:?}.\n\
                 The daemon did not become ready and bind a port within the configured \
                 proxy_auto_start_timeout.\n\
                 Increase the timeout or check the daemon's logs for slow startup."
            ))
        }
    }
}

/// Inner implementation of [`try_auto_start`] extracted so that the caller can
/// wrap it with `tokio::time::timeout` and unconditionally clean up
/// `AUTO_START_IN_PROGRESS` regardless of the outcome.
async fn try_auto_start_inner(
    slug: &str,
    cached: &CachedSlugEntry,
    daemon_id: &DaemonId,
    worktree_dir: Option<&std::path::Path>,
    route: &ProxyTlsRoute,
) -> ResolveResult {
    let config_dir = worktree_dir.unwrap_or(&cached.dir);

    let pt = match crate::pitchfork_toml::PitchforkToml::all_merged_from(config_dir) {
        Ok(pt) => pt,
        Err(e) => {
            log::warn!(
                "Auto-start: failed to load config from {}: {e}",
                config_dir.display()
            );
            return ResolveResult::NotFound;
        }
    };

    let mut daemon_config = match pt.daemons.get(daemon_id) {
        Some(cfg) => cfg.clone(),
        None => {
            log::debug!(
                "Auto-start: daemon {daemon_id} not found in config at {}",
                config_dir.display()
            );
            return ResolveResult::NotFound;
        }
    };

    // Render Tera templates and merge top-level env (per-daemon wins).
    if let Err(e) = crate::ipc::batch::render_daemon_config(daemon_id, &mut daemon_config, &pt) {
        log::warn!("Auto-start: failed to render templates for {daemon_id}: {e}");
        return ResolveResult::Error(format!("Failed to render templates: {e}"));
    }

    let opts = crate::ipc::batch::StartOptions {
        quiet: true,
        ..crate::ipc::batch::StartOptions::default()
    };
    let mut run_opts =
        match crate::ipc::batch::build_run_options(daemon_id, &daemon_config, Some(&opts)).await {
            Ok(o) => o,
            Err(e) => {
                log::warn!("Auto-start: failed to build run options for {daemon_id}: {e}");
                return ResolveResult::Error(format!("Failed to build run options: {e}"));
            }
        };

    // Only set the working directory when the daemon config didn't specify one.
    // If the config has an explicit `dir`, respect it even in a worktree context.
    if run_opts.dir.0.as_os_str().is_empty() {
        run_opts.dir = crate::config_types::Dir(config_dir.to_path_buf());
    }

    log::info!("Auto-start: starting daemon {daemon_id} for slug '{slug}'");

    let run_result = SUPERVISOR.run(run_opts).await;

    if let Err(e) = run_result {
        log::warn!("Auto-start: failed to start daemon {daemon_id}: {e}");
        return ResolveResult::Error(format!("Failed to start daemon: {e}"));
    }

    let poll_interval = std::time::Duration::from_millis(250);

    loop {
        let daemons = {
            let sf = SUPERVISOR.state_file.lock().await;
            sf.daemons.clone()
        };

        if let Some(d) = daemons.get(daemon_id) {
            if d.status.is_running() {
                // Selected through the same route as an already-running daemon,
                // so a hostname with `proxy_tls_port` lands on its configured
                // port on the request that started the daemon, not only on
                // later ones.
                if let Some(port) = select_daemon_port(route, d) {
                    log::info!("Auto-start: daemon {daemon_id} is ready on port {port}");
                    return ResolveResult::Ready(port);
                }
            } else {
                log::warn!(
                    "Auto-start: daemon {daemon_id} is no longer running (status: {})",
                    d.status
                );
                return ResolveResult::Error(format!(
                    "Daemon '{daemon_id}' started but exited unexpectedly.\n\
                     Check its logs for errors."
                ));
            }
        } else {
            log::warn!("Auto-start: daemon {daemon_id} not found in state file after start");
            return ResolveResult::Error(format!(
                "Daemon '{daemon_id}' started but disappeared from the state file.\n\
                 Check its logs for errors."
            ));
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Strip the TLD suffix from a hostname, returning the subdomain part.
///
/// Examples:
/// - `api.myproject.localhost` with tld `localhost` → `api.myproject`
/// - `api.localhost` with tld `localhost` → `api`
/// - `localhost` with tld `localhost` → `None` (no subdomain)
fn strip_tld(host: &str, tld: &str) -> Option<String> {
    host.strip_suffix(&format!(".{tld}"))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Build a human-friendly error message for port binding failures.
fn bind_error_message(port: u16, err: &std::io::Error) -> String {
    if port < 1024 {
        format!(
            "Failed to bind proxy server to port {port}: {err}\n\
             Hint: ports below 1024 require elevated privileges. \
             Try: sudo pitchfork supervisor start"
        )
    } else {
        format!(
            "Failed to bind proxy server to port {port}: {err}\n\
             Hint: another process may already be using this port."
        )
    }
}

/// Build an HTML "Starting…" response that auto-refreshes every 2 seconds.
///
/// Displayed when a proxy request triggers an auto-start for a stopped daemon.
/// Once the daemon is ready, the next refresh will proxy normally to the backend.
fn starting_html_response(slug: &str, raw_host: &str) -> Response {
    let escaped_slug = slug
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;");
    let escaped_host = raw_host
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;");

    let html = format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta http-equiv="refresh" content="2">
    <title>Starting {escaped_slug}… — pitchfork</title>
    <style>
        * {{ margin: 0; padding: 0; box-sizing: border-box; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            background: #0f1117;
            color: #e1e4e8;
            display: flex;
            align-items: center;
            justify-content: center;
            min-height: 100vh;
        }}
        .container {{
            text-align: center;
            max-width: 480px;
            padding: 2rem;
        }}
        .spinner {{
            width: 48px;
            height: 48px;
            border: 4px solid rgba(255, 255, 255, 0.1);
            border-top-color: #58a6ff;
            border-radius: 50%;
            animation: spin 0.8s linear infinite;
            margin: 0 auto 1.5rem;
        }}
        @keyframes spin {{
            to {{ transform: rotate(360deg); }}
        }}
        h1 {{
            font-size: 1.5rem;
            font-weight: 600;
            margin-bottom: 0.5rem;
        }}
        .slug {{
            color: #58a6ff;
            font-family: "SFMono-Regular", Consolas, "Liberation Mono", Menlo, monospace;
        }}
        .host {{
            color: #8b949e;
            font-size: 0.875rem;
            margin-top: 0.25rem;
        }}
        .hint {{
            color: #8b949e;
            font-size: 0.8rem;
            margin-top: 1.5rem;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="spinner"></div>
        <h1>Starting <span class="slug">{escaped_slug}</span>…</h1>
        <p class="host">{escaped_host}</p>
        <p class="hint">This page will refresh automatically when the daemon is ready.</p>
    </div>
</body>
</html>"##
    );

    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header("content-type", "text/html; charset=utf-8")
        .header("retry-after", "2")
        .body(Body::from(html))
        .unwrap_or_else(|_| (StatusCode::SERVICE_UNAVAILABLE, "Starting…").into_response())
}

/// Handler that redirects plain-HTTP requests to HTTPS.
///
/// Used when the proxy is configured for HTTPS but receives a plain-HTTP
/// request on the same port (after the first-byte peek determines it is
/// not a TLS ClientHello).  Returns a 302 redirect to the HTTPS equivalent.
///
/// WebSocket upgrade attempts over plain HTTP are rejected with 400
/// because WS-over-plain-HTTP to a TLS port is inherently broken.
async fn redirect_to_https_handler(req: Request) -> Response {
    // Reject WebSocket upgrades over plain HTTP
    if req.headers().contains_key("upgrade") {
        log::warn!("Dropping plain-HTTP WebSocket upgrade attempt — use wss:// instead of ws://");
        return (
            StatusCode::BAD_REQUEST,
            "WebSocket over plain HTTP is not supported on the HTTPS port. Use wss:// instead.",
        )
            .into_response();
    }

    let raw_host = get_request_host(&req);
    let Some(raw_host) = raw_host else {
        return (StatusCode::BAD_REQUEST, "Missing Host header").into_response();
    };

    // Strip any incoming port from Host and use the configured HTTPS port.
    let hostname = if raw_host.starts_with('[') {
        // IPv6: "[::1]:port" or "[::1]"
        raw_host
            .split_once("]:")
            .map(|(host, _)| host)
            .unwrap_or(&raw_host)
            .trim_start_matches('[')
            .trim_end_matches(']')
    } else {
        // IPv4/hostname: "host:port" or "host"
        let mut parts = raw_host.rsplitn(2, ':');
        let last = parts.next().unwrap_or(&raw_host);
        parts.next().unwrap_or(last)
    };

    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let https_port = match u16::try_from(settings().proxy.port).ok().filter(|&p| p > 0) {
        Some(443) | None => String::new(),
        Some(port) => format!(":{port}"),
    };

    let host_for_url = if raw_host.starts_with('[') {
        format!("[{hostname}]")
    } else {
        hostname.to_string()
    };

    let location = format!("https://{host_for_url}{https_port}{path}");
    (
        StatusCode::FOUND,
        [(axum::http::header::LOCATION, location)],
    )
        .into_response()
}

/// Build a plain-text error response.
fn error_response(status: StatusCode, message: &str) -> Response {
    (status, message.to_string()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_tld() {
        assert_eq!(
            strip_tld("api.myproject.localhost", "localhost"),
            Some("api.myproject".to_string())
        );
        assert_eq!(
            strip_tld("api.localhost", "localhost"),
            Some("api".to_string())
        );
        assert_eq!(strip_tld("localhost", "localhost"), None);
        assert_eq!(
            strip_tld("api.myproject.test", "test"),
            Some("api.myproject".to_string())
        );
        assert_eq!(strip_tld("other.com", "localhost"), None);
    }

    fn make_entry(name: &str) -> CachedSlugEntry {
        CachedSlugEntry {
            slug: name.to_string(),
            namespace: None,
            daemon_name: name.to_string(),
            dir: std::path::PathBuf::from(format!("/tmp/{name}")),
            worktrees: vec![],
            rejected_worktree_prefixes: std::collections::HashSet::new(),
            tls: ProxyTlsRoute::default(),
            worktree_tls: std::collections::HashMap::new(),
        }
    }

    /// A stopped daemon record with the given configured and resolved ports.
    fn make_daemon(
        configured: &[u16],
        resolved: &[u16],
        active: Option<u16>,
    ) -> crate::daemon::Daemon {
        crate::daemon::Daemon {
            id: DaemonId::try_new("proj", "api").unwrap(),
            port: crate::config_types::PortConfig::from_parts(
                configured.to_vec(),
                crate::config_types::PortBump(0),
            ),
            resolved_port: resolved.to_vec(),
            active_port: active,
            ..crate::daemon::Daemon::default()
        }
    }

    /// Without `proxy_tls_port`, the detected listening port wins, exactly as
    /// before this setting existed.
    #[test]
    fn test_select_daemon_port_prefers_active_port() {
        let route = ProxyTlsRoute::default();
        let d = make_daemon(&[8443, 9443], &[8443, 9443], Some(8443));
        assert_eq!(select_daemon_port(&route, &d), Some(8443));
    }

    /// A passthrough hostname without `proxy_tls_port` takes the daemon's
    /// declared first port rather than whichever listener was detected first:
    /// on a multi-port daemon the detected one can be a secondary plain-HTTP
    /// listener that came up before the TLS one.
    #[test]
    fn test_select_daemon_port_passthrough_prefers_declared_first_port() {
        let route = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: None,
        };
        let d = make_daemon(&[8443, 9080], &[8443, 9080], Some(9080));
        assert_eq!(select_daemon_port(&route, &d), Some(8443));

        // With nothing resolved, the detected port is still better than
        // refusing to route.
        let detected_only = make_daemon(&[], &[], Some(9080));
        assert_eq!(select_daemon_port(&route, &detected_only), Some(9080));
    }

    /// With no detected port yet, the first resolved port is used.
    #[test]
    fn test_select_daemon_port_falls_back_to_first_resolved() {
        let route = ProxyTlsRoute::default();
        let d = make_daemon(&[8443, 9443], &[8443, 9443], None);
        assert_eq!(select_daemon_port(&route, &d), Some(8443));

        // A daemon with no ports at all does not route.
        let none = make_daemon(&[], &[], None);
        assert_eq!(select_daemon_port(&route, &none), None);
    }

    /// `proxy_tls_port` picks a later port of a multi-port daemon, overriding
    /// the detected first port.
    #[test]
    fn test_select_daemon_port_honors_configured_port() {
        let route = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: Some(9443),
        };
        let d = make_daemon(&[8443, 9443], &[8443, 9443], Some(8443));
        assert_eq!(select_daemon_port(&route, &d), Some(9443));
    }

    /// Auto-bump shifts the ports a daemon actually binds. The hostname still
    /// maps to the same *position* in its port list, so it follows the bump
    /// instead of pointing at a port nothing is listening on.
    #[test]
    fn test_select_daemon_port_follows_auto_bump() {
        let route = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: Some(9443),
        };
        let d = make_daemon(&[8443, 9443], &[8444, 9444], Some(8444));
        assert_eq!(select_daemon_port(&route, &d), Some(9444));
    }

    /// A daemon whose recorded ports contradict `proxy_tls_port` — a state
    /// record left by an older config — is not routed to a port it never
    /// bound. Config validation stops this combination from being written in
    /// the first place.
    #[test]
    fn test_select_daemon_port_refuses_a_port_the_daemon_never_bound() {
        let route = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: Some(9443),
        };
        let stale = make_daemon(&[8443], &[8443], Some(8443));
        assert_eq!(select_daemon_port(&route, &stale), None);

        // With nothing recorded to contradict it, the configured port is all
        // the information there is, so it is used.
        let bare = make_daemon(&[], &[], None);
        assert_eq!(select_daemon_port(&route, &bare), Some(9443));
    }

    /// `proxy_tls_port` selects the forwarded port in terminate mode too, not
    /// only for passthrough hostnames.
    #[test]
    fn test_select_daemon_port_honors_configured_port_when_terminating() {
        let route = ProxyTlsRoute {
            mode: ProxyTlsMode::Terminate,
            port: Some(9080),
        };
        let d = make_daemon(&[8080, 9080], &[8080, 9080], Some(8080));
        assert_eq!(select_daemon_port(&route, &d), Some(9080));

        // Following auto-bump by position, as in passthrough mode.
        let bumped = make_daemon(&[8080, 9080], &[8081, 9081], Some(8081));
        assert_eq!(select_daemon_port(&route, &bumped), Some(9081));
    }

    /// The route of a wildcard worktree host comes from that worktree's own
    /// config, and falls back to the slug's when the worktree has none.
    #[test]
    fn test_worktree_route_lookup() {
        let mut entry = make_entry("myapp");
        entry.tls = ProxyTlsRoute {
            mode: ProxyTlsMode::Terminate,
            port: None,
        };
        entry.worktrees = vec![
            make_worktree("feature/b", "feature-b"),
            make_worktree("feature/c", "feature-c"),
        ];
        entry.worktree_tls.insert(
            "feature-b".to_string(),
            ProxyTlsRoute {
                mode: ProxyTlsMode::Passthrough,
                port: Some(9443),
            },
        );

        let lookup = |prefix: &str| match match_worktree_prefix(&entry, prefix) {
            PrefixMatch::Worktree(wt) => entry
                .worktree_tls
                .get(&wt.sanitized_branch.to_ascii_lowercase())
                .copied()
                .unwrap_or(entry.tls),
            _ => entry.tls,
        };

        assert_eq!(lookup("feature-b").mode, ProxyTlsMode::Passthrough);
        assert_eq!(lookup("feature-b").port, Some(9443));
        // A worktree without its own setting inherits the slug's mode.
        assert_eq!(lookup("feature-c").mode, ProxyTlsMode::Terminate);
        // So does an ordinary wildcard subdomain.
        assert_eq!(lookup("tenant").mode, ProxyTlsMode::Terminate);
    }

    #[test]
    fn test_wildcard_slug_lookup_exact_match() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("myapp".to_string(), make_entry("myapp"));
        // Exact match takes priority.
        let result = wildcard_slug_lookup("myapp", &entries, true);
        assert!(result.is_some());
        assert_eq!(result.unwrap().daemon_name, "myapp");
    }

    #[test]
    fn test_wildcard_slug_lookup_subdomain_fallback() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("myapp".to_string(), make_entry("myapp"));
        // "tenant.myapp" falls back to "myapp".
        let result = wildcard_slug_lookup("tenant.myapp", &entries, true);
        assert!(result.is_some());
        assert_eq!(result.unwrap().daemon_name, "myapp");
    }

    #[test]
    fn test_wildcard_slug_lookup_nested_fallback() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("myapp".to_string(), make_entry("myapp"));
        // "a.b.myapp" falls back to "myapp" through "b.myapp" → "myapp".
        let result = wildcard_slug_lookup("a.b.myapp", &entries, true);
        assert!(result.is_some());
        assert_eq!(result.unwrap().daemon_name, "myapp");
    }

    #[test]
    fn test_wildcard_slug_lookup_no_match() {
        let entries = std::collections::HashMap::new();
        // Empty entries → no match.
        let result = wildcard_slug_lookup("tenant.myapp", &entries, true);
        assert!(result.is_none());
    }

    #[test]
    fn test_wildcard_slug_lookup_disabled() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("myapp".to_string(), make_entry("myapp"));
        // With wildcard disabled, "tenant.myapp" does NOT match "myapp".
        let result = wildcard_slug_lookup("tenant.myapp", &entries, false);
        assert!(result.is_none());
        // But exact match still works.
        let result = wildcard_slug_lookup("myapp", &entries, false);
        assert!(result.is_some());
    }

    #[test]
    fn test_wildcard_slug_lookup_exact_beats_wildcard() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("myapp".to_string(), make_entry("myapp"));
        let mut tenant_entry = make_entry("tenant-daemon");
        tenant_entry.slug = "tenant.myapp".to_string();
        entries.insert("tenant.myapp".to_string(), tenant_entry);
        // "tenant.myapp" should match the exact slug, not fall back to "myapp".
        let result = wildcard_slug_lookup("tenant.myapp", &entries, true);
        assert!(result.is_some());
        assert_eq!(result.unwrap().daemon_name, "tenant-daemon");
    }

    #[test]
    fn test_wildcard_slug_lookup_ignores_case() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("myapp".to_string(), make_entry("myapp"));
        // Browsers lowercase the Host header, so every spelling must resolve.
        for host in ["MyApp", "MYAPP", "myapp"] {
            let result = wildcard_slug_lookup(host, &entries, true);
            assert!(result.is_some(), "exact lookup failed for {host}");
            assert_eq!(result.unwrap().daemon_name, "myapp");
        }
        // ...including through the wildcard fallback.
        for host in ["Tenant.MyApp", "tenant.MYAPP", "A.B.MyApp"] {
            let result = wildcard_slug_lookup(host, &entries, true);
            assert!(result.is_some(), "wildcard lookup failed for {host}");
            assert_eq!(result.unwrap().daemon_name, "myapp");
        }
    }

    #[test]
    fn test_wildcard_slug_lookup_case_insensitive_registration() {
        let mut entries = std::collections::HashMap::new();
        let mut entry = make_entry("upper");
        entry.slug = "MyApp".to_string();
        // build_slug_entries lowercases the key while keeping the configured
        // spelling in `slug`, so a capitalized registration stays reachable.
        entries.insert("myapp".to_string(), entry);
        for host in ["myapp", "MyApp", "tenant.MYAPP"] {
            let result = wildcard_slug_lookup(host, &entries, true);
            assert!(result.is_some(), "lookup failed for {host}");
            assert_eq!(result.unwrap().daemon_name, "upper");
        }
    }

    fn make_worktree(branch: &str, sanitized: &str) -> crate::proxy::worktree::WorktreeEntry {
        crate::proxy::worktree::WorktreeEntry {
            path: std::path::PathBuf::from(format!("/tmp/{sanitized}")),
            branch: branch.to_string(),
            sanitized_branch: sanitized.to_string(),
            namespace: Some(sanitized.to_string()),
        }
    }

    #[test]
    fn test_reject_case_colliding_worktrees_drops_both_sides() {
        let wts = vec![
            make_worktree("Feature-A", "Feature-A"),
            make_worktree("feature-a", "feature-a"),
            make_worktree("main", "main"),
        ];
        let (kept, rejected) = reject_case_colliding_worktrees(wts);
        // Neither spelling routes; picking one would send the request to a
        // worktree the user did not name.
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].sanitized_branch, "main");
        // The prefix is remembered so it is refused rather than treated as an
        // unknown wildcard prefix.
        assert!(rejected.contains("feature-a"));
    }

    #[test]
    fn test_reject_case_colliding_worktrees_keeps_unambiguous() {
        let wts = vec![
            make_worktree("main", "main"),
            make_worktree("feature/a", "feature-a"),
        ];
        let (kept, rejected) = reject_case_colliding_worktrees(wts);
        assert_eq!(kept.len(), 2);
        assert!(rejected.is_empty());
    }

    #[test]
    fn test_reject_case_colliding_worktrees_drops_sanitize_duplicates() {
        // Distinct branches can sanitize to the same string without any case
        // difference; that is ambiguous for the same reason.
        let wts = vec![
            make_worktree("feature/a", "feature-a"),
            make_worktree("feature.a", "feature-a"),
        ];
        let (kept, rejected) = reject_case_colliding_worktrees(wts);
        assert!(kept.is_empty());
        assert!(rejected.contains("feature-a"));
    }

    #[test]
    fn test_match_worktree_prefix() {
        let mut entry = make_entry("myapp");
        entry.worktrees = vec![make_worktree("feature/b", "feature-b")];
        entry
            .rejected_worktree_prefixes
            .insert("feature-a".to_string());

        assert!(matches!(
            match_worktree_prefix(&entry, "feature-b"),
            PrefixMatch::Worktree(_)
        ));
        // Host case does not matter for either outcome.
        assert!(matches!(
            match_worktree_prefix(&entry, "Feature-B"),
            PrefixMatch::Worktree(_)
        ));
        // A rejected prefix is refused, not served by the main checkout.
        assert!(matches!(
            match_worktree_prefix(&entry, "feature-a"),
            PrefixMatch::Ambiguous
        ));
        assert!(matches!(
            match_worktree_prefix(&entry, "FEATURE-A"),
            PrefixMatch::Ambiguous
        ));
        // An unrelated prefix is still an ordinary wildcard subdomain.
        assert!(matches!(
            match_worktree_prefix(&entry, "tenant"),
            PrefixMatch::Unknown
        ));
    }

    /// Each way of reaching the HTTP path with a passthrough hostname names its
    /// own remedy, since the fixes are different.
    #[test]
    fn test_passthrough_unroutable_message() {
        let no_tls = passthrough_unroutable_message("api.localhost", false);
        assert!(no_tls.contains("api.localhost"), "{no_tls}");
        assert!(no_tls.contains("settings.proxy.https = true"), "{no_tls}");

        // Over TLS this branch is reached when the handshake named a
        // different host than the request did — a connection with no host
        // name at all fails in the certificate resolver and never gets here.
        let mismatch = passthrough_unroutable_message("api.localhost", true);
        assert!(mismatch.contains("named a different host"), "{mismatch}");
        assert!(
            !mismatch.contains("settings.proxy.https"),
            "a host mismatch is not an HTTPS configuration problem: {mismatch}"
        );
    }

    /// Routing and the certificate resolver read one table, so they cannot
    /// disagree about a hostname's mode: the synchronous read returns the very
    /// same allocation the async one does.
    #[tokio::test]
    async fn test_slug_snapshot_is_the_cached_table() {
        let from_async = get_cached_slugs().await;
        let from_sync = slug_snapshot();
        assert!(
            Arc::ptr_eq(&from_async, &from_sync),
            "the synchronous read must see the same table routing does"
        );
    }

    /// An overlapping refresh that read older config does not reinstate it:
    /// for a hostname that just became passthrough, that would put the
    /// certificate resolver back to issuing for it.
    #[test]
    fn test_should_publish_slugs_keeps_the_newest_build() {
        let first = std::time::Instant::now();
        let second = first + std::time::Duration::from_millis(50);

        // Nothing published yet: anything is an improvement.
        assert!(should_publish_slugs(None, first));
        // A build that started later replaces one that started earlier.
        assert!(should_publish_slugs(Some(first), second));
        // A slower build that started earlier does not.
        assert!(!should_publish_slugs(Some(second), first));
        // A rebuild from the same instant may publish, so an equal timestamp
        // never wedges the cache.
        assert!(should_publish_slugs(Some(first), first));
    }

    /// The synchronous mode lookup the certificate resolver uses agrees with
    /// the async one: exact hosts, wildcard subdomains, worktree prefixes with
    /// their own setting, and anything that is not a slug at all.
    #[test]
    fn test_resolve_tls_mode_in() {
        let mut entries = std::collections::HashMap::new();

        let mut spliced = make_entry("spliced");
        spliced.tls = ProxyTlsRoute {
            mode: ProxyTlsMode::Passthrough,
            port: Some(8443),
        };
        spliced.worktrees = vec![
            make_worktree("feature/b", "feature-b"),
            make_worktree("feature/c", "feature-c"),
        ];
        spliced.worktree_tls.insert(
            "feature-b".to_string(),
            ProxyTlsRoute {
                mode: ProxyTlsMode::Terminate,
                port: None,
            },
        );
        entries.insert("spliced".to_string(), spliced);
        entries.insert("plain".to_string(), make_entry("plain"));

        let mode = |host: &str| resolve_tls_mode_in(host, "localhost", &entries);

        assert_eq!(mode("spliced.localhost"), ProxyTlsMode::Passthrough);
        // Host names are case-insensitive, and a wildcard subdomain inherits
        // the slug's mode.
        assert_eq!(mode("SPLICED.localhost"), ProxyTlsMode::Passthrough);
        assert_eq!(mode("tenant.spliced.localhost"), ProxyTlsMode::Passthrough);
        // A worktree with its own setting overrides the slug's …
        assert_eq!(mode("feature-b.spliced.localhost"), ProxyTlsMode::Terminate);
        // … and one without it inherits.
        assert_eq!(
            mode("feature-c.spliced.localhost"),
            ProxyTlsMode::Passthrough
        );
        // Everything else terminates: another slug, an unknown host, the
        // bare TLD, and a host outside the TLD.
        assert_eq!(mode("plain.localhost"), ProxyTlsMode::Terminate);
        assert_eq!(mode("unknown.localhost"), ProxyTlsMode::Terminate);
        assert_eq!(mode("localhost"), ProxyTlsMode::Terminate);
        assert_eq!(mode("spliced.example.com"), ProxyTlsMode::Terminate);
    }

    /// An empty table — the snapshot before any refresh — terminates rather
    /// than refusing certificates for hosts it knows nothing about.
    #[test]
    fn test_resolve_tls_mode_in_empty_table() {
        let entries = std::collections::HashMap::new();
        assert_eq!(
            resolve_tls_mode_in("spliced.localhost", "localhost", &entries),
            ProxyTlsMode::Terminate
        );
    }

    #[test]
    fn test_strip_dot_suffix_ignore_case() {
        assert_eq!(
            strip_dot_suffix_ignore_case("feature-a.myapp", "myapp"),
            Some("feature-a".to_string())
        );
        assert_eq!(
            strip_dot_suffix_ignore_case("Feature-A.MyApp", "myapp"),
            Some("Feature-A".to_string())
        );
        assert_eq!(
            strip_dot_suffix_ignore_case("feature-a.myapp", "MYAPP"),
            Some("feature-a".to_string())
        );
        // No dot separator, no prefix left, and a non-matching suffix all fail.
        assert_eq!(strip_dot_suffix_ignore_case("xmyapp", "myapp"), None);
        assert_eq!(strip_dot_suffix_ignore_case(".myapp", "myapp"), None);
        assert_eq!(strip_dot_suffix_ignore_case("myapp", "myapp"), None);
        assert_eq!(
            strip_dot_suffix_ignore_case("feature-a.other", "myapp"),
            None
        );
        // Multi-byte input must not panic on a mid-character split.
        assert_eq!(
            strip_dot_suffix_ignore_case("café.myapp", "myapp"),
            Some("café".to_string())
        );
        assert_eq!(strip_dot_suffix_ignore_case("café", "afé"), None);
    }

    /// Build a minimal ClientHello naming `host`, as it goes on the wire.
    #[cfg(feature = "proxy-tls")]
    fn client_hello_wire(host: &str) -> Vec<u8> {
        let mut entry = vec![0u8];
        entry.extend_from_slice(&(host.len() as u16).to_be_bytes());
        entry.extend_from_slice(host.as_bytes());
        let mut sni = (entry.len() as u16).to_be_bytes().to_vec();
        sni.extend_from_slice(&entry);

        let mut ext = vec![0x00, 0x00];
        ext.extend_from_slice(&(sni.len() as u16).to_be_bytes());
        ext.extend_from_slice(&sni);

        let mut body = vec![0x03, 0x03];
        body.extend_from_slice(&[0x22; 32]);
        body.push(0);
        body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]);
        body.extend_from_slice(&[0x01, 0x00]);
        body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
        body.extend_from_slice(&ext);

        let mut msg = vec![0x01];
        let len = body.len() as u32;
        msg.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
        msg.extend_from_slice(&body);

        let mut record = vec![0x16, 0x03, 0x01];
        record.extend_from_slice(&(msg.len() as u16).to_be_bytes());
        record.extend_from_slice(&msg);
        record
    }

    /// Accept one connection, hand the peeked verdict back, and report what a
    /// reader sees afterwards.
    #[cfg(feature = "proxy-tls")]
    async fn probe_over_socket(
        writes: Vec<Vec<u8>>,
        gap: std::time::Duration,
        timeout: std::time::Duration,
    ) -> (SniProbe, Vec<u8>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let total: usize = writes.iter().map(Vec::len).sum();

        let client = tokio::spawn(async move {
            let mut sock = TcpStream::connect(addr).await.unwrap();
            for chunk in writes {
                sock.write_all(&chunk).await.unwrap();
                sock.flush().await.unwrap();
                tokio::time::sleep(gap).await;
            }
            // Hold the connection open so the server can read back what it
            // only peeked at.
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        });

        let (stream, _) = listener.accept().await.unwrap();
        let probe = peek_sni_host(&stream, timeout).await;

        // Whatever was peeked must still be readable, byte for byte.
        let mut replayed = vec![0u8; total];
        let mut stream = stream;
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            stream.read_exact(&mut replayed),
        )
        .await;
        let replayed = match read {
            Ok(Ok(_)) => replayed,
            _ => vec![],
        };
        client.abort();
        (probe, replayed)
    }

    /// The peek loop reassembles a hello that arrives in many small writes,
    /// and leaves every byte in the socket for the path that handles the
    /// connection.
    #[cfg(feature = "proxy-tls")]
    #[tokio::test]
    async fn test_peek_sni_host_reads_a_hello_split_across_writes() {
        let wire = client_hello_wire("api.localhost");
        let writes: Vec<Vec<u8>> = wire.chunks(3).map(<[u8]>::to_vec).collect();
        let (probe, replayed) = probe_over_socket(
            writes,
            std::time::Duration::from_millis(5),
            std::time::Duration::from_secs(5),
        )
        .await;

        assert_eq!(probe, SniProbe::Host("api.localhost".to_string()));
        assert_eq!(replayed, wire, "peeked bytes must still be readable");
    }

    /// A hello that stops half way is reported as undetermined once the
    /// timeout expires, never as "no hostname": the caller must not terminate
    /// a connection that might belong to a passthrough daemon.
    #[cfg(feature = "proxy-tls")]
    #[tokio::test]
    async fn test_peek_sni_host_undetermined_when_a_hello_stalls() {
        let wire = client_hello_wire("api.localhost");
        let truncated = wire[..wire.len() / 2].to_vec();
        let (probe, _) = probe_over_socket(
            vec![truncated],
            std::time::Duration::ZERO,
            std::time::Duration::from_millis(150),
        )
        .await;

        assert_eq!(probe, SniProbe::Undetermined);
    }

    /// Something that is not a TLS handshake is a definite "no hostname", so
    /// the connection is still served rather than dropped.
    #[cfg(feature = "proxy-tls")]
    #[tokio::test]
    async fn test_peek_sni_host_reports_no_host_for_non_tls() {
        let (probe, _) = probe_over_socket(
            vec![b"GET / HTTP/1.1\r\n\r\n".to_vec()],
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(5),
        )
        .await;

        assert_eq!(probe, SniProbe::NoHost);
    }

    #[cfg(feature = "proxy-tls")]
    #[test]
    fn test_generate_ca() {
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("ca.pem");
        let key_path = dir.path().join("ca-key.pem");

        generate_ca(&cert_path, &key_path).unwrap();

        assert!(cert_path.exists(), "ca.pem should be created");
        assert!(key_path.exists(), "ca-key.pem should be created");

        let cert_pem = std::fs::read_to_string(&cert_path).unwrap();
        let key_pem = std::fs::read_to_string(&key_path).unwrap();

        assert!(cert_pem.contains("BEGIN CERTIFICATE"), "should be PEM cert");
        assert!(
            key_pem.contains("BEGIN") && key_pem.contains("PRIVATE KEY"),
            "should be PEM key"
        );
    }

    /// The raw `cookie` field values, in the order the map holds them.
    fn cookie_fields(headers: &HeaderMap) -> Vec<&[u8]> {
        headers
            .get_all(COOKIE)
            .iter()
            .map(HeaderValue::as_bytes)
            .collect()
    }

    /// Several fields become one, joined with `"; "`, and a comma inside a
    /// value is left alone.
    #[test]
    fn test_join_cookie_fields_joins_with_semicolon_space() {
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, HeaderValue::from_static("_session=abc123"));
        headers.append(COOKIE, HeaderValue::from_static("consent=ads,stats"));
        headers.append(COOKIE, HeaderValue::from_static("theme=dark"));

        join_cookie_fields(&mut headers);

        assert_eq!(
            cookie_fields(&headers),
            vec![&b"_session=abc123; consent=ads,stats; theme=dark"[..]]
        );
    }

    /// A UTF-8 cookie value is joined like any other, since the join works on
    /// bytes rather than on visible ASCII.
    #[test]
    fn test_join_cookie_fields_joins_bytes_outside_ascii() {
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, HeaderValue::from_static("_session=abc123"));
        headers.append(
            COOKIE,
            HeaderValue::from_bytes(b"name=Jos\xc3\xa9").unwrap(),
        );

        join_cookie_fields(&mut headers);

        assert_eq!(
            cookie_fields(&headers),
            vec![&b"_session=abc123; name=Jos\xc3\xa9"[..]]
        );
    }

    /// A request without cookies gains none.
    #[test]
    fn test_join_cookie_fields_without_cookies() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("app.localhost"));

        join_cookie_fields(&mut headers);

        assert!(headers.get(COOKIE).is_none());
    }
}
