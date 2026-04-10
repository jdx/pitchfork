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
use axum::http::{HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use hyper::header::HOST;

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
struct CachedSlugEntry {
    /// Expected namespace derived from `entry.dir` (None if derivation failed).
    namespace: Option<String>,
    /// Daemon short name (defaults to slug name when not explicitly set).
    daemon_name: String,
    /// Project directory for this slug (needed for auto-start).
    dir: std::path::PathBuf,
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

/// Build the slug lookup table from disk (expensive — involves file I/O).
/// Called outside the cache lock to avoid blocking concurrent proxy requests.
fn build_slug_entries() -> std::collections::HashMap<String, CachedSlugEntry> {
    let global_slugs = crate::pitchfork_toml::PitchforkToml::read_global_slugs();
    let mut entries = std::collections::HashMap::with_capacity(global_slugs.len());
    for (slug, entry) in &global_slugs {
        let ns = crate::pitchfork_toml::PitchforkToml::namespace_for_dir(&entry.dir).ok();
        let daemon_name = entry.daemon.as_deref().unwrap_or(slug).to_string();
        entries.insert(
            slug.clone(),
            CachedSlugEntry {
                namespace: ns,
                daemon_name,
                dir: entry.dir.clone(),
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
async fn get_cached_slugs() -> Arc<std::collections::HashMap<String, CachedSlugEntry>> {
    // Fast path: cache still valid — just clone the Arc.
    {
        let cache = SLUG_CACHE.lock().await;
        if std::time::Instant::now() < cache.expires_at {
            return Arc::clone(&cache.entries);
        }
    } // lock released before disk I/O

    // Slow path: refresh from disk (no lock held).
    let new_entries = Arc::new(build_slug_entries());

    // Store the refreshed entries.
    {
        let mut cache = SLUG_CACHE.lock().await;
        cache.entries = Arc::clone(&new_entries);
        cache.expires_at = std::time::Instant::now() + SLUG_CACHE_TTL;
    }

    new_entries
}

/// Look up a single slug in the cached table.
async fn cached_slug_lookup(subdomain: &str) -> Option<CachedSlugEntry> {
    let entries = get_cached_slugs().await;
    entries.get(subdomain).cloned()
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
    Ready(u16),
    /// Daemon was auto-started and is now ready on this port.
    Started(u16),
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
) -> crate::Result<()> {
    let s = settings();
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
        tld: s.proxy.tld.clone(),
        is_tls: s.proxy.https,
        on_error: None,
    };

    let app = Router::new().fallback(proxy_handler).with_state(state);

    // Resolve bind address from settings (default: 127.0.0.1 for local-only access).
    let bind_ip: std::net::IpAddr = match s.proxy.host.parse() {
        Ok(ip) => ip,
        Err(_) => {
            log::warn!(
                "proxy.host {:?} is not a valid IP address — falling back to 127.0.0.1. \
                 The proxy will only be reachable on the loopback interface.",
                s.proxy.host
            );
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        }
    };
    let addr = SocketAddr::from((bind_ip, effective_port));

    if s.proxy.https {
        serve_https_with_http_fallback(app, addr, s, effective_port, bind_tx).await
    } else {
        serve_http(app, addr, effective_port, bind_tx).await
    }
}

/// Serve plain HTTP.
async fn serve_http(
    app: Router,
    addr: SocketAddr,
    effective_port: u16,
    bind_tx: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
) -> crate::Result<()> {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => {
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
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(|e| miette::miette!("Proxy server error: {e}"))?;
    Ok(())
}

/// Serve HTTPS with automatic HTTP detection on the same port.
///
/// Peeks at the first byte of each incoming TCP connection:
/// - `0x16` (TLS ClientHello) → hand off to the TLS acceptor
/// - anything else → treat as plain HTTP (useful for health checks and
///   clients that haven't been configured to use HTTPS)
#[cfg(feature = "proxy-tls")]
async fn serve_https_with_http_fallback(
    app: Router,
    addr: SocketAddr,
    s: &crate::settings::Settings,
    effective_port: u16,
    bind_tx: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
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

    let tls_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver));

    let acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = match TcpListener::bind(addr).await {
        Ok(l) => {
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

    // Accept connections and sniff the first byte to decide TLS vs plain HTTP.
    loop {
        let (stream, _peer_addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                // Transient errors (e.g. EAGAIN, EMFILE) should not bring down
                // the entire proxy server.  Log and retry after a brief pause.
                log::warn!("Accept error (will retry): {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };

        let acceptor = acceptor.clone();
        let app = app.clone();

        tokio::spawn(async move {
            // Peek at the first byte without consuming it.
            // TLS ClientHello always starts with 0x16 (content type "handshake").
            let mut peek_buf = [0u8; 1];
            match stream.peek(&mut peek_buf).await {
                Ok(0) | Err(_) => return, // connection closed before sending anything
                _ => {}
            }

            if peek_buf[0] == 0x16 {
                // TLS handshake
                match acceptor.accept(stream).await {
                    Ok(tls_stream) => {
                        let io = hyper_util::rt::TokioIo::new(tls_stream);
                        let svc = hyper_util::service::TowerToHyperService::new(app);
                        let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                            .serve_connection_with_upgrades(io, svc)
                            .await;
                    }
                    Err(e) => {
                        log::debug!("TLS handshake error: {e}");
                    }
                }
            } else {
                // Plain HTTP on the TLS port
                let io = hyper_util::rt::TokioIo::new(stream);
                let svc = hyper_util::service::TowerToHyperService::new(app);
                let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                    .serve_connection_with_upgrades(io, svc)
                    .await;
            }
        });
    }
}

/// Fallback when proxy-tls feature is not enabled.
#[cfg(not(feature = "proxy-tls"))]
async fn serve_https_with_http_fallback(
    _app: Router,
    _addr: SocketAddr,
    _s: &crate::settings::Settings,
    _effective_port: u16,
    bind_tx: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
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
        use std::io::Write;
        #[cfg(unix)]
        {
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
    /// The CA key pair (used to sign leaf certs).
    ca_key: rcgen::KeyPair,
    /// The CA certificate (used as issuer for leaf certs).
    ca_cert: rcgen::Certificate,
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
        use rcgen::CertificateParams;

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

        // Parse the actual CA cert from disk using rcgen's from_ca_cert_pem.
        // We use `self_signed` here only to reconstruct the in-memory
        // `rcgen::Certificate` object that is needed to sign leaf certs via
        // `signed_by`.  The resulting object is used solely as the issuer
        // reference for leaf cert signing — it is never serialised back to
        // disk, so the new serial number / timestamps it carries do not matter.
        // Leaf certs signed by this object will chain to the on-disk CA that
        // browsers/OS have already trusted, because the public key and subject
        // are identical.
        let ca_cert = CertificateParams::from_ca_cert_pem(&ca_cert_pem)
            .map_err(|e| miette::miette!("Failed to parse CA cert params: {e}"))?
            .self_signed(&ca_key)
            .map_err(|e| miette::miette!("Failed to reconstruct CA cert: {e}"))?;

        // Ensure the host-certs directory exists
        let host_certs_dir = ca_cert_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("host-certs");
        std::fs::create_dir_all(&host_certs_dir)
            .map_err(|e| miette::miette!("Failed to create host-certs dir: {e}"))?;

        Ok(Self {
            ca_key,
            ca_cert,
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

        // Check that the first certificate has not expired.
        // rcgen's CertificateParams::not_after is a `time::OffsetDateTime`.
        // We compare it against the current UTC time to detect expired certs.
        {
            use rcgen::CertificateParams;
            // Re-parse the PEM to get the validity period.
            // CertificateParams::from_ca_cert_pem works for any DER-encoded cert.
            let first_pem_block: String = {
                let lines: Vec<&str> = pem
                    .lines()
                    .skip_while(|l| !l.starts_with("-----BEGIN CERTIFICATE-----"))
                    .take_while(|l| !l.starts_with("-----END CERTIFICATE-----"))
                    .collect();
                if lines.is_empty() {
                    String::new()
                } else {
                    // Re-attach the END line that take_while dropped
                    let end_line = "-----END CERTIFICATE-----";
                    format!("{}\n{end_line}", lines.join("\n"))
                }
            };
            if first_pem_block.is_empty() {
                // Could not extract a PEM block — treat as corrupt and force regeneration.
                miette::bail!(
                    "Could not extract PEM certificate block from {} — will regenerate",
                    path.display()
                );
            }
            // Propagate parse errors so the caller can fall through to regeneration
            // rather than silently serving a cert whose expiry we cannot verify.
            let params = CertificateParams::from_ca_cert_pem(&first_pem_block).map_err(|e| {
                miette::miette!("Failed to parse cert params from {}: {e}", path.display())
            })?;
            use chrono::Utc;
            let now_ts = Utc::now().timestamp();
            let not_after_ts = params.not_after.unix_timestamp();
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
            .signed_by(&leaf_key, &self.ca_cert, &self.ca_key)
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
            use std::io::Write;
            #[cfg(unix)]
            {
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

    // Resolve the target port from the host
    let target_port = match resolve_target(&host, &state.tld).await {
        ResolveResult::Ready(port) | ResolveResult::Started(port) => port,
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
/// - `ResolveResult::Ready(port)`       — daemon already running, forward to this port
/// - `ResolveResult::Started(port)`     — daemon was auto-started and is now ready
/// - `ResolveResult::Starting { slug }` — daemon start in progress (show waiting page)
/// - `ResolveResult::NotFound`          — no daemon matched
/// - `ResolveResult::Error(msg)`        — routing refused with a descriptive reason
///
/// # Locking
/// The state file lock is held only for the duration of the snapshot copy,
/// then released immediately to avoid serialising all proxy requests.
async fn resolve_target(host: &str, tld: &str) -> ResolveResult {
    // Strip the TLD suffix to get the subdomain part.
    let Some(subdomain) = strip_tld(host, tld) else {
        return ResolveResult::NotFound;
    };

    // Look up the slug via the in-memory cache (refreshed every SLUG_CACHE_TTL).
    let Some(cached) = cached_slug_lookup(&subdomain).await else {
        return ResolveResult::NotFound;
    };

    let daemon_name = &cached.daemon_name;
    let expected_namespace = &cached.namespace;

    // Find matching daemons in the state file.
    let daemons = {
        let state_file = SUPERVISOR.state_file.lock().await;
        state_file.daemons.clone()
    };

    // Find running daemons whose short name matches the slug's daemon name,
    // scoped to the slug's registered project namespace when available.
    let running_matches: Vec<(&DaemonId, &crate::daemon::Daemon)> = daemons
        .iter()
        .filter(|(id, d)| {
            id.name() == daemon_name
                && d.status.is_running()
                && match expected_namespace {
                    Some(ns) => id.namespace() == ns,
                    None => true,
                }
        })
        .collect();

    match running_matches.as_slice() {
        [] => {
            // Daemon not running — try auto-start if enabled.
            try_auto_start(&subdomain, &cached).await
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

/// Attempt to auto-start a daemon for the given slug.
///
/// If `proxy.auto_start` is disabled, returns `NotFound`.
/// Uses a dedup set to prevent concurrent starts for the same daemon.
/// Calls `SUPERVISOR.run()` with `wait_ready = true` so the daemon goes
/// through the same readiness lifecycle as `pf start`, then polls for the
/// active port.
async fn try_auto_start(slug: &str, cached: &CachedSlugEntry) -> ResolveResult {
    let s = settings();
    if !s.proxy.auto_start {
        return ResolveResult::NotFound;
    }

    // Resolve the daemon ID from the slug's project directory.
    let ns = match &cached.namespace {
        Some(ns) => ns.clone(),
        None => return ResolveResult::NotFound,
    };
    let daemon_id = match DaemonId::try_new(&ns, &cached.daemon_name) {
        Ok(id) => id,
        Err(_) => return ResolveResult::NotFound,
    };

    // Check if another request is already starting this daemon.
    {
        let in_progress = AUTO_START_IN_PROGRESS.lock().await;
        if in_progress.contains(&daemon_id) {
            return ResolveResult::Starting {
                slug: slug.to_string(),
            };
        }
    }

    // Load config from the slug's project directory to find the daemon definition.
    let pt = match crate::pitchfork_toml::PitchforkToml::all_merged_from(&cached.dir) {
        Ok(pt) => pt,
        Err(e) => {
            log::warn!(
                "Auto-start: failed to load config from {}: {e}",
                cached.dir.display()
            );
            return ResolveResult::NotFound;
        }
    };

    let daemon_config = match pt.daemons.get(&daemon_id) {
        Some(cfg) => cfg,
        None => {
            log::debug!(
                "Auto-start: daemon {daemon_id} not found in config at {}",
                cached.dir.display()
            );
            return ResolveResult::NotFound;
        }
    };

    // Build run options — keep wait_ready=true (set by build_run_options) so
    // SUPERVISOR.run() waits for the daemon's readiness signal before returning,
    // matching the same lifecycle as `pf start` via IPC.
    let opts = crate::ipc::batch::StartOptions::default();
    let mut run_opts = match crate::ipc::batch::build_run_options(&daemon_id, daemon_config, &opts)
    {
        Ok(o) => o,
        Err(e) => {
            log::warn!("Auto-start: failed to build run options for {daemon_id}: {e}");
            return ResolveResult::Error(format!("Failed to build run options: {e}"));
        }
    };

    if run_opts.dir.as_os_str().is_empty() {
        run_opts.dir = cached.dir.clone();
    }

    // Mark as in-progress.
    {
        let mut in_progress = AUTO_START_IN_PROGRESS.lock().await;
        if !in_progress.insert(daemon_id.clone()) {
            // Race: another task inserted between our check and this insert.
            return ResolveResult::Starting {
                slug: slug.to_string(),
            };
        }
    }

    log::info!("Auto-start: starting daemon {daemon_id} for slug '{slug}'");

    // Trigger the start and wait for daemon readiness.
    let run_result = SUPERVISOR.run(run_opts).await;
    AUTO_START_IN_PROGRESS.lock().await.remove(&daemon_id);

    if let Err(e) = run_result {
        log::warn!("Auto-start: failed to start daemon {daemon_id}: {e}");
        return ResolveResult::Error(format!("Failed to start daemon: {e}"));
    }

    // Daemon is ready. Poll briefly for the active_port to be detected
    // (detect_and_store_active_port runs asynchronously after readiness).
    let timeout = s.proxy_auto_start_timeout();
    let start = std::time::Instant::now();
    let poll_interval = std::time::Duration::from_millis(250);

    loop {
        let daemons = {
            let sf = SUPERVISOR.state_file.lock().await;
            sf.daemons.clone()
        };

        if let Some(d) = daemons.get(&daemon_id) {
            if d.status.is_running() {
                if let Some(port) = d.active_port.or_else(|| d.resolved_port.first().copied()) {
                    log::info!("Auto-start: daemon {daemon_id} is ready on port {port}");
                    return ResolveResult::Started(port);
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
        }

        if start.elapsed() >= timeout {
            log::warn!(
                "Auto-start: daemon {daemon_id} ready but no port detected within {timeout:?}"
            );
            return ResolveResult::Error(format!(
                "Daemon '{daemon_id}' started but no port was detected within {timeout:?}.\n\
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
        .replace('"', "&quot;");
    let escaped_host = raw_host
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");

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
}
