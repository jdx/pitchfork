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
use tokio::net::TcpListener;

use crate::daemon_id::DaemonId;
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
}

/// In-memory cache for the global slug registry + derived namespaces.
struct SlugCache {
    entries: Arc<std::collections::HashMap<String, CachedSlugEntry>>,
    expires_at: std::time::Instant,
}

static SLUG_CACHE: once_cell::sync::Lazy<tokio::sync::Mutex<SlugCache>> =
    once_cell::sync::Lazy::new(|| {
        tokio::sync::Mutex::new(SlugCache {
            entries: Arc::new(std::collections::HashMap::new()),
            expires_at: std::time::Instant::now(), // expired → will be populated on first access
        })
    });

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
        entries.insert(
            key,
            CachedSlugEntry {
                slug: slug.clone(),
                namespace: ns,
                daemon_name,
                dir: entry.resolve_dir().unwrap_or_default(),
                worktrees,
                rejected_worktree_prefixes,
            },
        );
    }
    entries
}

/// Return a snapshot of the cached slug table, refreshing from disk if expired.
///
/// The disk I/O happens *outside* the mutex to avoid blocking concurrent requests
/// during the refresh.  A short race window exists where two threads may both
/// refresh, but that is harmless (last writer wins with identical data).
pub async fn get_cached_slugs() -> Arc<std::collections::HashMap<String, CachedSlugEntry>> {
    // Fast path: cache still valid — just clone the Arc.
    {
        let cache = SLUG_CACHE.lock().await;
        if std::time::Instant::now() < cache.expires_at {
            return Arc::clone(&cache.entries);
        }
    } // lock released before disk I/O

    // Slow path: refresh from disk on a blocking thread (involves subprocess calls).
    let new_entries = Arc::new(
        tokio::task::spawn_blocking(build_slug_entries)
            .await
            .unwrap_or_else(|e| {
                log::warn!("Failed to refresh slug cache: {e}");
                std::collections::HashMap::new()
            }),
    );

    // Store the refreshed entries.
    {
        let mut cache = SLUG_CACHE.lock().await;
        cache.entries = Arc::clone(&new_entries);
        cache.expires_at = std::time::Instant::now() + SLUG_CACHE_TTL;
    }

    new_entries
}

// ─── Hostname registry cache ────────────────────────────────────────────────
//
// The automatic `<daemon>.<worktree>.<project>` hostnames are resolved against
// a registry built from every project pitchfork knows about.  Building it reads
// configuration files and enumerates git worktrees, so the result is cached
// with the same short TTL as the slug table.

struct RegistryCache {
    registry: Arc<crate::proxy::hostname::HostRegistry>,
    expires_at: std::time::Instant,
}

static HOST_REGISTRY: once_cell::sync::Lazy<tokio::sync::Mutex<RegistryCache>> =
    once_cell::sync::Lazy::new(|| {
        tokio::sync::Mutex::new(RegistryCache {
            registry: Arc::new(crate::proxy::hostname::HostRegistry::default()),
            expires_at: std::time::Instant::now(), // expired -> built on first access
        })
    });

/// Return a snapshot of the cached hostname registry, rebuilding if expired.
pub async fn get_cached_host_registry() -> Arc<crate::proxy::hostname::HostRegistry> {
    {
        let cache = HOST_REGISTRY.lock().await;
        if std::time::Instant::now() < cache.expires_at {
            return Arc::clone(&cache.registry);
        }
    } // lock released before disk I/O

    let registry = Arc::new(
        tokio::task::spawn_blocking(crate::proxy::hostname::HostRegistry::build)
            .await
            .unwrap_or_else(|e| {
                log::warn!("Failed to refresh hostname registry: {e}");
                crate::proxy::hostname::HostRegistry::default()
            }),
    );
    for err in &registry.errors {
        crate::proxy::hostname::warn_once(err);
    }

    {
        let mut cache = HOST_REGISTRY.lock().await;
        cache.registry = Arc::clone(&registry);
        cache.expires_at = std::time::Instant::now() + SLUG_CACHE_TTL;
    }

    registry
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
    /// The hostname is reserved for a project or stack page, which a later
    /// change will serve.  It must never fall through to a daemon.
    Page {
        project: String,
        worktree: Option<String>,
        daemons: Vec<String>,
    },
    /// The hostname named a project or daemon that does not exist.
    Unknown { heading: String, known: Vec<String> },
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
        serve_https_with_http_fallback(app, addr, &s, effective_port, bind_tx, cancel).await
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
    let resolver = SniCertResolver::new(&ca_cert_path, &ca_key_path)?;

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
                let (stream, peer_addr) = match accept_result {
                    Ok(conn) => conn,
                    Err(e) => {
                        log::warn!("Accept error (will retry): {e}");
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                };

                let acceptor = acceptor.clone();
                // This loop serves the router itself rather than going through
                // `into_make_service_with_connect_info`, so the peer address is
                // attached here. Handlers use it to decide how much of this
                // machine's configuration a response may describe.
                let app = app
                    .clone()
                    .layer(axum::Extension(axum::extract::ConnectInfo(peer_addr)));
                let redirect_app = redirect_app.clone();

                conn_tasks.spawn(async move {
                    // Peek at the first byte without consuming it.
                    // TLS ClientHello always starts with 0x16 (content type "handshake").
                    let mut peek_buf = [0u8; 1];
                    match stream.peek(&mut peek_buf).await {
                        Ok(0) | Err(_) => return,
                        _ => {}
                    }

                    if peek_buf[0] == 0x16 {
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

/// Fallback when proxy-tls feature is not enabled.
#[cfg(not(feature = "proxy-tls"))]
async fn serve_https_with_http_fallback(
    _app: Router,
    _addr: SocketAddr,
    _s: &crate::settings::Settings,
    _effective_port: u16,
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
    fn new(ca_cert_path: &std::path::Path, ca_key_path: &std::path::Path) -> crate::Result<Self> {
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

    let local_client = is_local_client(&req);

    // Intercept "pitchfork.<tld>" — route to the built-in web UI
    let target_port = if let Some(subdomain) = strip_tld(&host, &state.tld) {
        if subdomain.eq_ignore_ascii_case("pitchfork") {
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
        match resolve_target(&host, &state.tld).await {
            ResolveResult::Ready(port) => port,
            ResolveResult::Starting { slug } => {
                return starting_html_response(&slug, &raw_host);
            }
            ResolveResult::Page {
                project,
                worktree,
                daemons,
            } => {
                // A reserved name answers 200 while an unknown one answers 404,
                // which tells anything on the network which projects exist. Off
                // this machine the two look the same.
                if !local_client {
                    return unknown_host_response(&host, "Not found", &[]);
                }
                return page_placeholder_response(
                    &project,
                    worktree.as_deref(),
                    &daemons,
                    &state.tld,
                    &host_port_suffix(&raw_host),
                );
            }
            ResolveResult::Unknown { heading, known } => {
                // The heading says which project was recognised, which is one
                // more thing than a remote client needs to learn.
                return unknown_host_response(
                    &host,
                    if local_client { &heading } else { "Not found" },
                    if local_client { &known } else { &[] },
                );
            }
            ResolveResult::NotFound => {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    &format!(
                        "No daemon found for host '{host}'.\n\
                         A daemon is reachable once it configures a `port` and its project is \
                         known to pitchfork; run `pitchfork proxy status` to see the hostnames \
                         it serves.\n\
                         Expected format: <daemon>.<project>.{tld}",
                        tld = state.tld
                    ),
                );
            }
            ResolveResult::Error(msg) => {
                if local_client {
                    return error_response(StatusCode::BAD_GATEWAY, &msg);
                }
                // The message names directories on this machine.
                log::warn!("Refused '{host}' for a non-local client: {msg}");
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    &format!("'{host}' is not available."),
                );
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
    let Some(subdomain) = strip_tld(host, tld) else {
        return ResolveResult::NotFound;
    };

    let cached = cached_slug_lookup(&subdomain).await.filter(|cached| {
        // A slug too long for the configured TLD is not advertised as a URL, so
        // it does not take precedence over the daemon's automatic hostname
        // here either.
        if crate::proxy::hostname::hostname_fits(&cached.slug) {
            return true;
        }
        crate::proxy::hostname::warn_once(&format!(
            "Slug '{}' plus the configured proxy.tld is over the DNS length limit, so it is              not routed.",
            cached.slug
        ));
        false
    });
    let Some(cached) = cached else {
        // No legacy slug matched; fall through to the automatic
        // `<daemon>.<worktree>.<project>` hostnames.
        return resolve_registry_target(&subdomain).await;
    };

    // ─── Worktree prefix extraction ──────────────────────────────────────
    // When a wildcard subdomain like "feature-a.myapp" matched slug "myapp",
    // the prefix "feature-a" may correspond to a git worktree or jj workspace.
    let (expected_namespace, worktree_dir) = if !subdomain.eq_ignore_ascii_case(&cached.slug) {
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
                    (ns, Some(wt.path.clone()))
                }
                PrefixMatch::Ambiguous => {
                    return ResolveResult::Error(format!(
                        "'{host}' is ambiguous: more than one branch or workspace of '{slug}' \
                         sanitizes to the prefix '{p}', and host names are case-insensitive.\n\
                         Rename one of them so the prefixes differ by more than case, then \
                         reload.\n\
                         The supervisor log lists the colliding branches.",
                        slug = cached.slug,
                    ));
                }
                PrefixMatch::Unknown => (cached.namespace.clone(), None),
            },
            None => (cached.namespace.clone(), None),
        }
    } else {
        (cached.namespace.clone(), None)
    };

    let daemon_name = &cached.daemon_name;

    let daemons = {
        let state_file = SUPERVISOR.state_file.lock().await;
        state_file.daemons.clone()
    };

    let running_matches: Vec<(&DaemonId, &crate::daemon::Daemon)> = daemons
        .iter()
        .filter(|(id, d)| {
            id.name() == daemon_name
                && d.status.is_running()
                && match &expected_namespace {
                    Some(ns) => id.namespace() == ns,
                    None => true,
                }
        })
        .collect();

    match running_matches.as_slice() {
        [] => {
            try_auto_start(
                &cached.slug,
                &cached,
                worktree_dir.as_deref(),
                expected_namespace.as_deref(),
            )
            .await
        }
        [(_, d)] => {
            if let Some(port) = d.active_port.or_else(|| d.resolved_port.first().copied()) {
                ResolveResult::Ready(port)
            } else {
                ResolveResult::NotFound
            }
        }
        _ => {
            let d = running_matches[0].1;
            if let Some(port) = d.active_port.or_else(|| d.resolved_port.first().copied()) {
                ResolveResult::Ready(port)
            } else {
                ResolveResult::NotFound
            }
        }
    }
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
        try_auto_start_inner(slug, cached, &daemon_id, worktree_dir),
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

    // Render Tera templates and merge top-level env (per-daemon wins). Building
    // the template context reads configuration and derives hostnames, so it
    // runs on a blocking worker rather than on the thread serving the request.
    let rendered = {
        let id = daemon_id.clone();
        let mut config = daemon_config.clone();
        tokio::task::spawn_blocking(move || {
            crate::ipc::batch::render_daemon_config(&id, &mut config, &pt).map(|()| config)
        })
        .await
    };
    daemon_config = match rendered {
        Ok(Ok(config)) => config,
        Ok(Err(e)) => {
            log::warn!("Auto-start: failed to render templates for {daemon_id}: {e}");
            return ResolveResult::Error(format!("Failed to render templates: {e}"));
        }
        Err(e) => {
            log::warn!("Auto-start: template rendering task failed for {daemon_id}: {e}");
            return ResolveResult::Error(format!("Failed to render templates: {e}"));
        }
    };

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
                if let Some(port) = d.active_port.or_else(|| d.resolved_port.first().copied()) {
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

/// Resolve a hostname against the automatic hostname registry.
///
/// Runs only after the legacy `[slugs]` registry found no match, so a slug
/// keeps precedence over an automatic hostname that spells the same thing.
async fn resolve_registry_target(subdomain: &str) -> ResolveResult {
    let registry = get_cached_host_registry().await;
    if !crate::proxy::hostname::hostname_fits(subdomain) {
        // Nothing advertises a name this long, so nothing answers to one.
        return ResolveResult::Unknown {
            heading: "Host name too long".to_string(),
            known: registry.project_labels(),
        };
    }
    match registry.resolve(subdomain, settings().proxy.wildcard) {
        crate::proxy::hostname::HostTarget::Daemon {
            ref dir,
            ref namespace,
            ref daemon,
            ..
        } => {
            // When several checkouts share this daemon's namespace — in this
            // project or in another one, since namespaces come from directory
            // names — the ID no longer says which checkout is running, so the
            // request has to be matched to the directory it named.
            let per_checkout = registry.shares_daemon_id(namespace, daemon);
            resolve_registry_daemon(subdomain, dir, namespace, daemon, per_checkout).await
        }
        crate::proxy::hostname::HostTarget::ProjectPage { project } => {
            let daemons = registry
                .projects
                .get(&project)
                .map(|p| p.primary.labels())
                .unwrap_or_default();
            ResolveResult::Page {
                project,
                worktree: None,
                daemons,
            }
        }
        crate::proxy::hostname::HostTarget::WorktreePage { project, worktree } => {
            let daemons = registry
                .projects
                .get(&project)
                .and_then(|p| p.worktrees.get(&worktree))
                .map(|c| c.labels())
                .unwrap_or_default();
            ResolveResult::Page {
                project,
                worktree: Some(worktree),
                daemons,
            }
        }
        crate::proxy::hostname::HostTarget::UnknownProject { known } => ResolveResult::Unknown {
            heading: "Unknown project".to_string(),
            known,
        },
        crate::proxy::hostname::HostTarget::UnknownDaemon {
            project,
            worktree,
            known,
        } => ResolveResult::Unknown {
            heading: match worktree {
                Some(wt) => format!("Unknown daemon in '{wt}' of project '{project}'"),
                None => format!("Unknown daemon in project '{project}'"),
            },
            known,
        },
    }
}

/// Find the running daemon behind an automatic hostname, auto-starting it when
/// it is not running.
///
/// Several checkouts of one project can share a namespace when the project
/// declares one explicitly, so a daemon running in the matching directory is
/// preferred over one that merely shares the name.
async fn resolve_registry_daemon(
    host: &str,
    dir: &std::path::Path,
    namespace: &str,
    daemon: &str,
    per_checkout: bool,
) -> ResolveResult {
    let daemons = {
        let state_file = SUPERVISOR.state_file.lock().await;
        state_file.daemons.clone()
    };

    let mut matches: Vec<crate::daemon::Daemon> = daemons
        .iter()
        .filter(|(id, d)| {
            id.name() == daemon && id.namespace() == namespace && d.status.is_running()
        })
        .map(|(_, d)| d.clone())
        .collect();
    // Attributing a daemon to a checkout walks the filesystem, so it happens off
    // the request's worker thread.
    matches = sort_by_checkout(matches, dir).await;

    if let Some(d) = matches.first() {
        // A running daemon from another checkout would serve that checkout's
        // content under this one's hostname, so say what is wrong instead.
        if per_checkout && !runs_in_checkout(d.clone(), dir).await {
            return ResolveResult::Error(format!(
                "'{host}' belongs to the checkout at {}, but daemon '{namespace}/{daemon}' is \
                 running from {}.\n\
                 These checkouts share the namespace '{namespace}', so pitchfork cannot run \
                 both copies at once.\n\
                 Give each checkout its own top-level `namespace`, or stop the other one first.",
                dir.display(),
                d.dir
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "an unknown directory".to_string()),
            ));
        }
        return match d.active_port.or_else(|| d.resolved_port.first().copied()) {
            Some(port) => ResolveResult::Ready(port),
            None => ResolveResult::NotFound,
        };
    }

    let cached = CachedSlugEntry {
        slug: host.to_string(),
        namespace: Some(namespace.to_string()),
        daemon_name: daemon.to_string(),
        dir: dir.to_path_buf(),
        worktrees: vec![],
        rejected_worktree_prefixes: std::collections::HashSet::new(),
    };
    let result = try_auto_start(host, &cached, None, Some(namespace)).await;

    // The start can land on a record another checkout already owns, because the
    // supervisor refuses to run a second daemon under the same ID. Serving that
    // port would hand this hostname the other checkout's content.
    if per_checkout && let ResolveResult::Ready(_) = result {
        let started = {
            let state_file = SUPERVISOR.state_file.lock().await;
            state_file
                .daemons
                .iter()
                .find(|(id, _)| id.name() == daemon && id.namespace() == namespace)
                .map(|(_, d)| d.clone())
        };
        if let Some(d) = started
            && !runs_in_checkout(d.clone(), dir).await
        {
            return ResolveResult::Error(format!(
                "'{host}' belongs to the checkout at {}, but daemon '{namespace}/{daemon}' is \
                 running from {}.\n\
                 These checkouts share the namespace '{namespace}', so pitchfork cannot run \
                 both copies at once.\n\
                 Give each checkout its own top-level `namespace`, or stop the other one first.",
                dir.display(),
                d.dir
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "an unknown directory".to_string()),
            ));
        }
    }

    result
}

/// Whether the request came from this machine.
///
/// Names of other people's projects, daemon labels and absolute paths are
/// details of the developer's machine. They help whoever is sitting at it and
/// tell a device on the LAN things it has no business knowing, so pages spell
/// them out for loopback clients only.
fn is_local_client(req: &Request) -> bool {
    // A request whose peer is unknown is treated as remote: withholding detail
    // from a local client is a small loss, and the reverse is a leak.
    req.extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .is_some_and(|ci| ci.0.ip().is_loopback())
}

/// Whether a daemon is running from this checkout.
///
/// The daemon's directory is resolved to the checkout that contains it rather
/// than compared as a path prefix, so a worktree nested inside its primary
/// checkout is attributed to the worktree, and a symlinked or non-canonical
/// directory still matches. A daemon whose explicit `dir` lies outside every
/// checkout belongs to none of them, which keeps it reachable as long as its
/// hostname is unambiguous.
fn daemon_runs_in(daemon: &crate::daemon::Daemon, checkout: &std::path::Path) -> bool {
    daemon
        .dir
        .as_deref()
        .is_some_and(|d| crate::proxy::hostname::checkout_root_of(d) == checkout)
}

/// [`daemon_runs_in`] off the async worker, since it walks the filesystem.
async fn runs_in_checkout(daemon: crate::daemon::Daemon, checkout: &std::path::Path) -> bool {
    let checkout = checkout.to_path_buf();
    tokio::task::spawn_blocking(move || daemon_runs_in(&daemon, &checkout))
        .await
        .unwrap_or(false)
}

/// Order the candidates so that daemons running in this checkout come first.
async fn sort_by_checkout(
    daemons: Vec<crate::daemon::Daemon>,
    checkout: &std::path::Path,
) -> Vec<crate::daemon::Daemon> {
    if daemons.len() < 2 {
        return daemons;
    }
    let checkout = checkout.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut daemons = daemons;
        daemons.sort_by_key(|d| !daemon_runs_in(d, &checkout));
        daemons
    })
    .await
    .unwrap_or_default()
}

/// Escape the five characters that change the meaning of HTML text.
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Wrap body markup in the shared pitchfork page chrome.
fn html_page(status: StatusCode, title: &str, body: String) -> Response {
    let html = format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{title} — pitchfork</title>
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
        .container {{ max-width: 640px; padding: 2rem; }}
        h1 {{ font-size: 1.5rem; font-weight: 600; margin-bottom: 0.75rem; }}
        p {{ color: #8b949e; font-size: 0.9rem; margin-bottom: 0.75rem; }}
        ul {{ list-style: none; margin: 0.5rem 0 1rem; }}
        li {{ margin: 0.25rem 0; }}
        code, a {{
            color: #58a6ff;
            font-family: "SFMono-Regular", Consolas, "Liberation Mono", Menlo, monospace;
            text-decoration: none;
        }}
    </style>
</head>
<body>
    <div class="container">{body}</div>
</body>
</html>"##
    );
    Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(html))
        .unwrap_or_else(|_| (status, title.to_string()).into_response())
}

/// The `:port` part of a Host header, or an empty string when it carries none.
///
/// Links on pitchfork's own pages keep the port the request arrived on, so they
/// still work when the proxy listens somewhere other than 80 or 443.
fn host_port_suffix(raw_host: &str) -> String {
    let port = if raw_host.starts_with('[') {
        raw_host.split_once("]:").map(|(_, port)| port)
    } else {
        raw_host.rsplit_once(':').map(|(_, port)| port)
    };
    port.filter(|p| p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty())
        .map(|p| format!(":{p}"))
        .unwrap_or_default()
}

/// Serve the placeholder for a reserved project or stack hostname.
///
/// `<project>.<tld>` and `<worktree>.<project>.<tld>` belong to the project and
/// stack pages.  Until those pages exist this placeholder stands in, so the
/// hostname never resolves to whichever daemon shares its name.
fn page_placeholder_response(
    project: &str,
    worktree: Option<&str>,
    daemons: &[String],
    tld: &str,
    port_suffix: &str,
) -> Response {
    let heading = match worktree {
        Some(wt) => format!("{} · {}", escape_html(project), escape_html(wt)),
        None => escape_html(project),
    };
    let suffix = match worktree {
        Some(wt) => format!(
            "{}.{}.{}",
            escape_html(wt),
            escape_html(project),
            escape_html(tld)
        ),
        None => format!("{}.{}", escape_html(project), escape_html(tld)),
    };
    let list = if daemons.is_empty() {
        "<p>No daemon in this checkout has a port configured.</p>".to_string()
    } else {
        let items: String = daemons
            .iter()
            .map(|d| {
                let d = escape_html(d);
                format!("<li><a href=\"//{d}.{suffix}{port_suffix}\">{d}.{suffix}</a></li>")
            })
            .collect();
        format!("<p>Daemons here:</p><ul>{items}</ul>")
    };
    let body = format!(
        "<h1>{heading}</h1>\
         <p>This address is reserved for the {page} page, which is not built yet.</p>\
         {list}",
        page = if worktree.is_some() {
            "stack"
        } else {
            "project"
        },
    );
    html_page(StatusCode::OK, "pitchfork", body)
}

/// Serve the 404 page for a hostname whose project or daemon does not exist.
fn unknown_host_response(host: &str, heading: &str, known: &[String]) -> Response {
    let list = if known.is_empty() {
        "<p>Nothing is registered under this name yet.</p>".to_string()
    } else {
        let items: String = known
            .iter()
            .map(|k| format!("<li><code>{}</code></li>", escape_html(k)))
            .collect();
        format!("<p>Known names:</p><ul>{items}</ul>")
    };
    let body = format!(
        "<h1>{heading}</h1><p>No route for <code>{host}</code>.</p>{list}",
        heading = escape_html(heading),
        host = escape_html(host),
    );
    html_page(StatusCode::NOT_FOUND, "Not found", body)
}

/// Strip the TLD suffix from a hostname, returning the subdomain part.
///
/// Host names are case-insensitive (RFC 4343) and a browser passes on whatever
/// the user typed, so `API.MyProject.LOCALHOST` has to lose its TLD like any
/// other spelling.
///
/// Examples:
/// - `api.myproject.localhost` with tld `localhost` → `api.myproject`
/// - `api.LOCALHOST` with tld `localhost` → `api`
/// - `localhost` with tld `localhost` → `None` (no subdomain)
fn strip_tld(host: &str, tld: &str) -> Option<String> {
    strip_dot_suffix_ignore_case(host, tld)
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
        // Host names are case-insensitive, and browsers pass on what was typed.
        assert_eq!(
            strip_tld("API.MyProject.LOCALHOST", "localhost"),
            Some("API.MyProject".to_string())
        );
        assert_eq!(
            strip_tld("api.localhost", "LOCALHOST"),
            Some("api".to_string())
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
        }
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

    #[test]
    fn test_host_port_suffix() {
        assert_eq!(host_port_suffix("api.myproj.localhost:8088"), ":8088");
        assert_eq!(host_port_suffix("api.myproj.localhost"), "");
        assert_eq!(host_port_suffix("[::1]:8088"), ":8088");
        assert_eq!(host_port_suffix("[::1]"), "");
        // A non-numeric tail is not a port and must not reach a link.
        assert_eq!(host_port_suffix("host:notaport"), "");
    }

    /// A daemon belongs to the checkout that contains its working directory,
    /// which is the worktree rather than the primary when one is nested inside
    /// the other, and no checkout at all when its `dir` points elsewhere.
    #[test]
    fn test_daemon_runs_in() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("my-repo");
        std::fs::create_dir_all(repo.join(".git/worktrees/feature")).unwrap();
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        // A worktree checked out *inside* the primary's directory tree.
        let nested = repo.join(".worktrees/feature");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join(".git"),
            format!(
                "gitdir: {}\n",
                repo.join(".git/worktrees/feature").display()
            ),
        )
        .unwrap();

        let root = |p: &std::path::Path| crate::proxy::hostname::checkout_root_of(p);
        let repo_root = root(&repo);
        let nested_root = root(&nested);

        let mut daemon = crate::daemon::Daemon {
            dir: Some(repo.join("sub")),
            ..Default::default()
        };
        assert!(daemon_runs_in(&daemon, &repo_root));
        assert!(!daemon_runs_in(&daemon, &nested_root));

        // Lexically the nested worktree sits under the primary; by checkout it
        // does not, so the primary's hostname must not claim it.
        daemon.dir = Some(nested.clone());
        assert!(daemon_runs_in(&daemon, &nested_root));
        assert!(!daemon_runs_in(&daemon, &repo_root));

        // An explicit dir outside every checkout belongs to none of them.
        daemon.dir = Some(temp.path().join("elsewhere"));
        assert!(!daemon_runs_in(&daemon, &repo_root));

        daemon.dir = None;
        assert!(!daemon_runs_in(&daemon, &repo_root));
    }

    /// Only a request known to come from this machine gets the detailed pages;
    /// an unknown peer counts as remote.
    #[test]
    fn test_is_local_client() {
        let build = |info: Option<SocketAddr>| {
            let mut req = Request::new(Body::empty());
            if let Some(addr) = info {
                req.extensions_mut()
                    .insert(axum::extract::ConnectInfo(addr));
            }
            req
        };

        assert!(is_local_client(&build(Some(
            "127.0.0.1:5000".parse().unwrap()
        ))));
        assert!(is_local_client(&build(Some("[::1]:5000".parse().unwrap()))));
        assert!(!is_local_client(&build(Some(
            "192.168.1.42:5000".parse().unwrap()
        ))));
        assert!(!is_local_client(&build(None)));
    }
}
