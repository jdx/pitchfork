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
pub async fn serve() -> crate::Result<()> {
    let s = settings();
    let effective_port = match u16::try_from(s.proxy.port).ok().filter(|&p| p > 0) {
        Some(p) => p,
        None => {
            miette::bail!(
                "proxy.port {} is out of valid port range (1-65535), proxy server cannot start",
                s.proxy.port
            );
        }
    };

    let client = Client::builder(TokioExecutor::new()).build(HttpConnector::new());

    let state = ProxyState {
        client: Arc::new(client),
        tld: s.proxy.tld.clone(),
        is_tls: s.proxy.https,
        on_error: None,
    };

    let app = Router::new().fallback(proxy_handler).with_state(state);

    // Resolve bind address from settings (default: 127.0.0.1 for local-only access).
    let bind_ip: std::net::IpAddr = s
        .proxy
        .host
        .parse()
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    let addr = SocketAddr::from((bind_ip, effective_port));

    if s.proxy.https {
        serve_https_with_http_fallback(app, addr, s, effective_port).await
    } else {
        serve_http(app, addr, effective_port).await
    }
}

/// Serve plain HTTP.
async fn serve_http(app: Router, addr: SocketAddr, effective_port: u16) -> crate::Result<()> {
    let listener = TcpListener::bind(addr).await.map_err(|e| {
        miette::miette!(
            "Failed to bind proxy server to port {effective_port}: {e}\n\
             Hint: ports below 1024 require elevated privileges (sudo)."
        )
    })?;

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

    let listener = TcpListener::bind(addr).await.map_err(|e| {
        miette::miette!(
            "Failed to bind HTTPS proxy server to port {effective_port}: {e}\n\
             Hint: ports below 1024 require elevated privileges (sudo)."
        )
    })?;

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
) -> crate::Result<()> {
    miette::bail!(
        "HTTPS proxy support requires the `proxy-tls` feature.\n\
         Rebuild pitchfork with: cargo build --features proxy-tls"
    )
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
                let mut in_cert = false;
                let mut lines: Vec<&str> = Vec::new();
                for line in pem.lines() {
                    if line.starts_with("-----BEGIN CERTIFICATE-----") {
                        in_cert = true;
                    }
                    if in_cert {
                        lines.push(line);
                    }
                    if in_cert && line.starts_with("-----END CERTIFICATE-----") {
                        break;
                    }
                }
                lines.join("\n")
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
        // Using yesterday as not_before avoids clock-skew rejections on clients
        // whose clocks are slightly behind.
        {
            use chrono::{Datelike, Duration, Utc};
            let yesterday = Utc::now() - Duration::days(1);
            // Use 10 * 366 days (≈ 10 years + a few days) to ensure the cert
            // covers a full 10-year span even across leap years.
            let ten_years_later = Utc::now() + Duration::days(10 * 366);
            params.not_before = date_time_ymd(
                yesterday.year(),
                yesterday.month() as u8,
                yesterday.day() as u8,
            );
            params.not_after = date_time_ymd(
                ten_years_later.year(),
                ten_years_later.month() as u8,
                ten_years_later.day() as u8,
            );
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
/// Preserves any existing forwarded headers (appends to `X-Forwarded-For`).
fn inject_forwarded_headers(req: &mut Request, is_tls: bool, host_header: &str) {
    let remote_addr = req
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());

    let proto = if is_tls { "https" } else { "http" };
    let default_port = if is_tls { "443" } else { "80" };

    // Helper to read an existing header value as a string
    let get_header = |name: &str| -> Option<String> {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };

    let forwarded_for = get_header("x-forwarded-for")
        .map(|existing| format!("{existing}, {remote_addr}"))
        .unwrap_or_else(|| remote_addr.clone());

    let forwarded_proto = get_header("x-forwarded-proto").unwrap_or_else(|| proto.to_string());

    let forwarded_host = get_header("x-forwarded-host").unwrap_or_else(|| host_header.to_string());

    let forwarded_port = get_header("x-forwarded-port").unwrap_or_else(|| {
        host_header
            .rsplit_once(':')
            .map(|(_, port)| port.to_string())
            .unwrap_or_else(|| default_port.to_string())
    });

    let headers = [
        ("x-forwarded-for", forwarded_for),
        ("x-forwarded-proto", forwarded_proto),
        ("x-forwarded-host", forwarded_host),
        ("x-forwarded-port", forwarded_port),
    ];
    for (name, value) in headers {
        if let Ok(v) = HeaderValue::from_str(&value) {
            let header_name = axum::http::HeaderName::from_static(name);
            // Always use `insert` (overwrite) for all forwarded headers.
            //
            // For X-Forwarded-For we already built the full chain in the
            // `forwarded_for` string above ("<existing>, <remote_addr>"), so
            // we must use `insert` — not `append` — to avoid recording the
            // remote IP twice (once in the concatenated string and once as a
            // second header value).
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
    let raw_host = match get_request_host(&req) {
        Some(h) => h,
        None => return error_response(StatusCode::BAD_REQUEST, "Missing Host header"),
    };
    // Strip port from host for routing
    let host = raw_host.split(':').next().unwrap_or(&raw_host).to_string();

    // Loop detection: check hop count.
    //
    // Security: strip (zero out) the hop counter on the very first hop to
    // prevent external clients from forging a high value and triggering a
    // 508 Loop Detected response (denial-of-service).  A request is
    // considered "first hop" when it does not carry the `x-pitchfork`
    // response header that pitchfork adds to every proxied response — i.e.
    // it did not come from another pitchfork proxy instance.
    let is_from_pitchfork = req.headers().contains_key(PITCHFORK_HEADER);
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
    let target_port = match resolve_target_port(&host, &state.tld).await {
        Some(port) => port,
        None => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!(
                    "No daemon found for host '{host}'.\n\
                     Make sure the daemon is running and has a port configured.\n\
                     Expected format: <id>.<namespace>.{tld} or <slug>.{tld}",
                    tld = state.tld
                ),
            );
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

    // Forward the request (hyper client handles upgrade transparently)
    let result = state.client.request(req).await;

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

            // Strip hop-by-hop headers when serving HTTPS (HTTP/2 forbids them)
            if state.is_tls {
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

/// Resolve the target port for a given hostname.
///
/// Resolution order:
/// 1. Check if any daemon has a matching `slug` (and `proxy != Some(false)`)
/// 2. Parse `<id>.<namespace>.<tld>` pattern
/// 3. Parse `<id>.<tld>` pattern (global namespace)
///
/// Returns the `active_port` if available, otherwise falls back to `resolved_port[0]`.
///
/// # Locking
/// The state file lock is held only for the duration of the snapshot copy,
/// then released immediately to avoid serialising all proxy requests.
async fn resolve_target_port(host: &str, tld: &str) -> Option<u16> {
    // Strip the TLD suffix to get the subdomain part before acquiring the lock.
    let subdomain = strip_tld(host, tld)?;

    // Take a snapshot of the daemon map and release the lock immediately so
    // that concurrent requests are not serialised behind this one.
    let daemons = {
        let state_file = SUPERVISOR.state_file.lock().await;
        state_file.daemons.clone()
    };

    // First pass: check for slug match.
    // A daemon with proxy = false has explicitly opted out of proxying;
    // its slug should not be routable through the proxy.
    // Note: slugs containing dots are rejected at config validation time, so
    // there is no ambiguity with the id.namespace second pass below.
    for daemon in daemons.values() {
        if let Some(ref slug) = daemon.slug {
            if slug == &subdomain {
                // Respect the per-daemon proxy opt-out flag.
                if !daemon.proxy {
                    return None;
                }
                // Slug matched: return the port (or None if daemon has no port yet).
                // Do NOT fall through to the id.namespace pass — a slug match is
                // authoritative and should never accidentally route to a different daemon.
                return daemon
                    .active_port
                    .or_else(|| daemon.resolved_port.first().copied());
            }
        }
    }

    // Second pass: try `<id>.<namespace>` pattern.
    // The subdomain format is `<id>.<namespace>` where namespace is the
    // *last* dot-separated component (e.g. `api.myproject` → id=`api`, ns=`myproject`).
    // For a single component (no dot) treat it as the global namespace.
    let (id, namespace) = if let Some(dot_pos) = subdomain.find('.') {
        // First dot separates id from namespace.
        // `api.myproject` → id=`api`, namespace=`myproject`
        // `v2.api.myproject` is not a valid pattern; the daemon id cannot
        // contain dots, so we only split on the first dot.
        let id = &subdomain[..dot_pos];
        let namespace = &subdomain[dot_pos + 1..];
        (id.to_string(), namespace.to_string())
    } else {
        // <id> only — treat as global namespace
        (subdomain.to_string(), "global".to_string())
    };

    // Look up daemon by (namespace, id), respecting per-daemon proxy opt-out.
    let daemon_id = DaemonId::try_new(&namespace, &id).ok()?;
    let daemon = daemons.get(&daemon_id)?;

    if !daemon.proxy {
        return None;
    }

    daemon
        .active_port
        .or_else(|| daemon.resolved_port.first().copied())
}

/// Strip the TLD suffix from a hostname, returning the subdomain part.
///
/// Examples:
/// - `api.myproject.localhost` with tld `localhost` → `api.myproject`
/// - `api.localhost` with tld `localhost` → `api`
/// - `localhost` with tld `localhost` → `None` (no subdomain)
fn strip_tld(host: &str, tld: &str) -> Option<String> {
    let subdomain = host.strip_suffix(&format!(".{tld}"))?;
    if subdomain.is_empty() {
        None
    } else {
        Some(subdomain.to_string())
    }
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
