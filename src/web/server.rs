use crate::Result;
use crate::settings::settings;
use axum::{
    Router,
    body::Body,
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::{Redirect, Response},
    routing::{get, post},
};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::routes;
use super::static_files::{set_static_base, set_static_token, static_handler};

/// API token middleware - rejects requests without valid X-Pitchfork-Token header
/// when the server is bound to a non-loopback address and a token is configured.
async fn token_auth(
    request: Request<Body>,
    next: Next,
    expected_token: String,
) -> Result<Response, StatusCode> {
    if expected_token.is_empty() {
        return Ok(next.run(request).await);
    }
    let token = request
        .headers()
        .get("X-Pitchfork-Token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if token != expected_token {
        let addr: std::borrow::Cow<'_, str> = request
            .extensions()
            .get::<axum::extract::ConnectInfo<SocketAddr>>()
            .map(|a| a.0.to_string().into())
            .unwrap_or_else(|| "unknown".into());
        warn!(
            "API request rejected: invalid or missing X-Pitchfork-Token from {} to {}",
            addr,
            request.uri()
        );
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

/// Check if an IP address is loopback (127.0.0.1 or ::1).
fn is_loopback(addr: &str) -> bool {
    addr.parse::<SocketAddr>()
        .map(|a| a.ip().is_loopback())
        .unwrap_or_else(|_| {
            // Try parsing as just an IP without port
            addr.parse::<std::net::IpAddr>()
                .map(|ip| ip.is_loopback())
                .unwrap_or(false)
        })
}
/// Generate a random 32-byte hex token (64 characters).
fn generate_token() -> String {
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    format!("{}{}", a.simple(), b.simple())
}

/// Build the API router (no CSRF - SPA uses JSON).
fn api_router(token: String) -> Router {
    let token_clone = token.clone();
    Router::new()
        .route("/api/stats", get(routes::api::stats::stats))
        .route("/api/daemons", get(routes::api::daemons::list))
        .route("/api/daemons/{id}", get(routes::api::daemons::show))
        .route("/api/daemons/{id}/start", post(routes::api::daemons::start))
        .route("/api/daemons/{id}/stop", post(routes::api::daemons::stop))
        .route(
            "/api/daemons/{id}/restart",
            post(routes::api::daemons::restart),
        )
        .route(
            "/api/daemons/{id}/enable",
            post(routes::api::daemons::enable),
        )
        .route(
            "/api/daemons/{id}/disable",
            post(routes::api::daemons::disable),
        )
        .route("/api/logs/{id}/tail", get(routes::api::logs::tail))
        .route("/api/logs/{id}/loggers", get(routes::api::logs::loggers))
        .route(
            "/api/logs/{id}/field-keys",
            get(routes::api::logs::field_keys),
        )
        .route("/api/namespaces", get(routes::api::namespaces::list))
        .route("/api/namespaces", post(routes::api::namespaces::register))
        .route(
            "/api/namespaces/{name}",
            axum::routing::delete(routes::api::namespaces::remove),
        )
        .route("/api/proxies", get(routes::api::proxies::list))
        .route("/api/projects", get(routes::api::projects::list))
        .route("/api/projects/{project}", get(routes::api::projects::show))
        .route(
            "/api/projects/{project}/{worktree}",
            get(routes::api::projects::stack),
        )
        .route(
            "/api/processes/{id}/tree",
            get(routes::api::processes::tree),
        )
        .route("/logs/{id}/stream", get(routes::logs::stream_sse))
        .layer(middleware::from_fn(move |req, next| {
            let t = token_clone.clone();
            async move { token_auth(req, next, t).await }
        }))
}

/// How long a client gets to finish the TLS handshake before the connection
/// is dropped, so a peer that connects and goes quiet cannot hold a task
/// open indefinitely.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Why `web.tls_cert` and `web.tls_key` cannot be used as configured: they
/// are a pair, so both are set or neither is.
fn tls_pair_problem(cert: &str, key: &str) -> Option<String> {
    match (cert.is_empty(), key.is_empty()) {
        (false, true) => Some(
            "web.tls_cert is set but web.tls_key is empty; set both to serve HTTPS, or neither \
             for plain HTTP"
                .to_string(),
        ),
        (true, false) => Some(
            "web.tls_key is set but web.tls_cert is empty; set both to serve HTTPS, or neither \
             for plain HTTP"
                .to_string(),
        ),
        _ => None,
    }
}

/// Build the rustls server configuration for the web UI from the
/// `web.tls_cert` / `web.tls_key` pair. `None` when neither is set, so the
/// listener serves plain HTTP.
///
/// The key is checked against the certificate here, so a mismatch is reported
/// once at startup instead of failing every handshake.
async fn web_tls_config(cert: &str, key: &str) -> Result<Option<Arc<rustls::ServerConfig>>> {
    use rustls::pki_types::CertificateDer;
    use rustls_pemfile::{certs, private_key};

    if let Some(problem) = tls_pair_problem(cert, key) {
        miette::bail!("{problem}");
    }
    if cert.is_empty() {
        return Ok(None);
    }
    let cert_path = Path::new(cert);
    let key_path = Path::new(key);

    let cert_pem = tokio::fs::read(cert_path)
        .await
        .map_err(|e| miette::miette!("Failed to read web.tls_cert {}: {e}", cert_path.display()))?;
    let key_pem = tokio::fs::read(key_path)
        .await
        .map_err(|e| miette::miette!("Failed to read web.tls_key {}: {e}", key_path.display()))?;

    let cert_ders: Vec<CertificateDer<'static>> = certs(&mut cert_pem.as_slice())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| miette::miette!("Failed to parse {}: {e}", cert_path.display()))?;
    if cert_ders.is_empty() {
        miette::bail!("No certificates found in {}", cert_path.display());
    }
    let key_der = private_key(&mut key_pem.as_slice())
        .map_err(|e| miette::miette!("Failed to parse {}: {e}", key_path.display()))?
        .ok_or_else(|| miette::miette!("No private key found in {}", key_path.display()))?;

    // Install ring as the default CryptoProvider if none has been set yet.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
        .map_err(|e| miette::miette!("Failed to use the key in {}: {e}", key_path.display()))?;
    rustls::sign::CertifiedKey::new(cert_ders.clone(), signing_key)
        .keys_match()
        .map_err(|e| {
            miette::miette!(
                "web.tls_key {} does not match web.tls_cert {}: {e}",
                key_path.display(),
                cert_path.display()
            )
        })?;

    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_ders, key_der)
        .map_err(|e| miette::miette!("Failed to build TLS configuration: {e}"))?;
    // Advertise HTTP/2 and HTTP/1.1 via ALPN so browsers can multiplex the
    // dashboard's polling and SSE requests over one connection.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Some(Arc::new(config)))
}

/// Accept connections on `listener`, terminate TLS, and serve `app` on each.
///
/// `axum::serve` only speaks plain TCP, so this does the accept loop itself:
/// hyper's auto builder picks HTTP/1.1 or HTTP/2 from what ALPN negotiated.
async fn serve_tls(
    listener: tokio::net::TcpListener,
    app: Router,
    tls_config: Arc<rustls::ServerConfig>,
) -> Result<()> {
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::server::conn::auto::Builder;
    use hyper_util::service::TowerToHyperService;

    let acceptor = tokio_rustls::TlsAcceptor::from(tls_config);
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                // Usually a transient resource error (e.g. too many open
                // files); back off briefly rather than spin on it.
                warn!("Web UI failed to accept a connection: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let acceptor = acceptor.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let tls_stream =
                match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                    Ok(Ok(stream)) => stream,
                    Ok(Err(e)) => {
                        debug!("TLS handshake with {peer} failed: {e}");
                        return;
                    }
                    Err(_) => {
                        debug!("TLS handshake with {peer} timed out");
                        return;
                    }
                };
            let svc = TowerToHyperService::new(app);
            if let Err(e) = Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(TokioIo::new(tls_stream), svc)
                .await
            {
                // Browsers reset streams on navigation; nothing to act on.
                debug!("Web UI connection from {peer} ended with error: {e}");
            }
        });
    }
}

/// Bind a `TcpListener` and return `(listener, actual_port)`, trying
/// `port_attempts` ports starting from `port`.
async fn try_bind(
    bind_address: &str,
    port: u16,
    port_attempts: u16,
) -> Result<(tokio::net::TcpListener, u16)> {
    let ip_addr: std::net::IpAddr = bind_address
        .parse()
        .map_err(|e| miette::miette!("Invalid bind address '{}': {}", bind_address, e))?;

    let mut last_error = None;
    for offset in 0..port_attempts {
        let try_port = port.saturating_add(offset);
        let addr = SocketAddr::from((ip_addr, try_port));

        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                let actual_port = listener
                    .local_addr()
                    .map_err(|e| miette::miette!("Failed to inspect bound port: {}", e))?;
                return Ok((listener, actual_port.port()));
            }
            Err(e) => {
                debug!("Port {try_port} unavailable: {e}");
                last_error = Some(e);
            }
        }
    }

    Err(miette::miette!(
        "Failed to bind: tried ports {}-{}, all in use. Last error: {}",
        port,
        port.saturating_add(port_attempts - 1),
        last_error.map(|e| e.to_string()).unwrap_or_default()
    ))
}

pub async fn serve(port: u16, web_path: Option<String>) -> Result<()> {
    let base_path = super::normalize_base_path(web_path.as_deref())?;
    super::BASE_PATH
        .set(base_path.clone())
        .expect("BASE_PATH already set; serve() must only be called once per process");
    let s = settings();
    let bind_address = &s.web.bind_address;
    let port_attempts: u16 = u16::try_from(s.web.port_attempts)
        .unwrap_or_else(|_| {
            warn!(
                "web.port_attempts value {} is out of range (1-65535), clamping to 10",
                s.web.port_attempts
            );
            10
        })
        .max(1);

    // Determine token: use configured token, or auto-generate one if binding to non-loopback
    let mut token = s.api.token.clone();
    if token.is_empty() && !is_loopback(bind_address) {
        token = generate_token();
        info!(
            "Web UI bound to non-loopback address {}. Auto-generated API token: {}",
            bind_address, token
        );
        // Also print to stderr so it's visible even with log level filtering
        eprintln!("pitchfork API security token (auto-generated): {}", token);
    }

    // Resolve TLS before binding, so a bad certificate is reported without
    // having claimed the port first.
    let tls_config = web_tls_config(&s.web.tls_cert, &s.web.tls_key).await?;
    let scheme = if tls_config.is_some() {
        "https"
    } else {
        "http"
    };

    set_static_token(token.clone());
    set_static_base(base_path.clone());

    let inner = api_router(token.clone()).fallback(static_handler);

    let app = if base_path.is_empty() {
        inner
    } else {
        let redirect_target = format!("{base_path}/");
        Router::new()
            .route(
                "/",
                get(move || async move { Redirect::temporary(&redirect_target) }),
            )
            .nest(&base_path, inner)
    };

    let (listener, actual_port) = try_bind(bind_address, port, port_attempts).await?;
    let _ = super::WEB_PORT.set(actual_port);
    let actual_addr = listener.local_addr().unwrap();
    let url_host = match actual_addr.ip() {
        ip if ip.is_unspecified() => "localhost".to_string(),
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
        std::net::IpAddr::V4(ip) => ip.to_string(),
    };
    let _ = super::WEB_URL.set(format!("{scheme}://{url_host}:{actual_port}{base_path}"));
    if tls_config.is_some() {
        let _ = super::WEB_HTTPS_ORIGIN.set(format!("https://{url_host}:{actual_port}"));
    }

    info!("Web UI listening on {scheme}://{actual_addr}");

    match tls_config {
        Some(tls_config) => serve_tls(listener, app, tls_config).await,
        None => axum::serve(listener, app)
            .await
            .map_err(|e| miette::miette!("Web server error: {}", e)),
    }
}

/// Serve the API on a dedicated port, separate from the web UI.
/// Called by the supervisor when `settings.api.bind_port` is configured.
pub async fn serve_api(port: u16, _web_path: Option<String>) -> Result<()> {
    let s = settings();
    let bind_address = &s.api.bind_address;
    let port_attempts: u16 = u16::try_from(s.api.port_attempts)
        .unwrap_or_else(|_| {
            warn!(
                "api.port_attempts value {} is out of range (1-65535), clamping to 10",
                s.api.port_attempts
            );
            10
        })
        .max(1);

    // Determine token for standalone API server
    let mut token = s.api.token.clone();
    if token.is_empty() && !is_loopback(bind_address) {
        token = generate_token();
        info!(
            "API server bound to non-loopback address {}. Auto-generated API token: {}",
            bind_address, token
        );
        eprintln!("pitchfork API security token (auto-generated): {}", token);
    }

    let app = api_router(token);

    let (listener, _actual_port) = try_bind(bind_address, port, port_attempts).await?;
    let actual_addr = listener.local_addr().unwrap();
    info!("API server listening on http://{actual_addr}");

    axum::serve(listener, app)
        .await
        .map_err(|e| miette::miette!("API server error: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_pair_must_be_both_or_neither() {
        assert!(tls_pair_problem("", "").is_none());
        assert!(tls_pair_problem("/c.pem", "/k.pem").is_none());
        assert!(tls_pair_problem("/c.pem", "").is_some());
        assert!(tls_pair_problem("", "/k.pem").is_some());
    }

    #[tokio::test]
    async fn no_tls_config_when_unset() {
        assert!(web_tls_config("", "").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn half_configured_pair_is_an_error() {
        let err = web_tls_config("/c.pem", "").await.unwrap_err().to_string();
        assert!(err.contains("web.tls_key is empty"), "{err}");
        let err = web_tls_config("", "/k.pem").await.unwrap_err().to_string();
        assert!(err.contains("web.tls_cert is empty"), "{err}");
    }

    #[tokio::test]
    async fn missing_cert_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("missing.crt");
        let key = dir.path().join("missing.key");
        let err = web_tls_config(cert.to_str().unwrap(), key.to_str().unwrap())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Failed to read web.tls_cert"), "{err}");
    }

    /// Self-signed certificate for `localhost`, written as a PEM pair.
    #[cfg(feature = "proxy-tls")]
    fn write_self_signed(
        dir: &Path,
        stem: &str,
    ) -> (std::path::PathBuf, std::path::PathBuf, String) {
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let cert_path = dir.join(format!("{stem}.crt"));
        let key_path = dir.join(format!("{stem}.key"));
        std::fs::write(&cert_path, cert.pem()).unwrap();
        std::fs::write(&key_path, key.serialize_pem()).unwrap();
        (cert_path, key_path, cert.pem())
    }

    #[cfg(feature = "proxy-tls")]
    #[tokio::test]
    async fn mismatched_key_is_reported_at_load() {
        let dir = tempfile::tempdir().unwrap();
        let (cert_a, _, _) = write_self_signed(dir.path(), "a");
        let (_, key_b, _) = write_self_signed(dir.path(), "b");
        let err = web_tls_config(cert_a.to_str().unwrap(), key_b.to_str().unwrap())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not match web.tls_cert"), "{err}");
    }

    /// A `/ping` router behind the TLS accept loop, on a fresh self-signed
    /// certificate. Returns the address, the certificate PEM a client must
    /// trust, and the server task.
    #[cfg(feature = "proxy-tls")]
    async fn start_tls_server(
        dir: &Path,
    ) -> (SocketAddr, String, tokio::task::JoinHandle<Result<()>>) {
        let (cert_path, key_path, cert_pem) = write_self_signed(dir, "web");
        let tls_config = web_tls_config(cert_path.to_str().unwrap(), key_path.to_str().unwrap())
            .await
            .unwrap()
            .expect("configured pair yields a TLS config");
        let app = Router::new().route("/ping", get(|| async { "pong" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(serve_tls(listener, app, tls_config));
        (addr, cert_pem, server)
    }

    /// A TLS connection to `addr` that trusts exactly `cert_pem`, offering
    /// `alpn` (none when empty).
    #[cfg(feature = "proxy-tls")]
    async fn connect_tls(
        addr: SocketAddr,
        cert_pem: &str,
        alpn: Vec<Vec<u8>>,
    ) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut cert_pem.as_bytes()) {
            roots.add(cert.unwrap()).unwrap();
        }
        let mut client_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        client_config.alpn_protocols = alpn;
        let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        connector.connect(server_name, tcp).await.unwrap()
    }

    /// End to end: the TLS accept loop terminates a real handshake and hands
    /// an HTTP/1.1 request to the router.
    #[cfg(feature = "proxy-tls")]
    #[tokio::test]
    async fn serves_router_over_tls() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let dir = tempfile::tempdir().unwrap();
        let (addr, cert_pem, server) = start_tls_server(dir.path()).await;
        let mut tls = connect_tls(addr, &cert_pem, Vec::new()).await;

        tls.write_all(b"GET /ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        tls.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8_lossy(&response);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("pong"), "{response}");

        server.abort();
    }

    /// A client offering only `h2` gets HTTP/2 negotiated through ALPN and
    /// served, so the dashboard can multiplex over one connection.
    #[cfg(feature = "proxy-tls")]
    #[tokio::test]
    async fn negotiates_http2_over_tls() {
        use http_body_util::{BodyExt, Empty};
        use hyper_util::rt::{TokioExecutor, TokioIo};

        let dir = tempfile::tempdir().unwrap();
        let (addr, cert_pem, server) = start_tls_server(dir.path()).await;
        let tls = connect_tls(addr, &cert_pem, vec![b"h2".to_vec()]).await;
        assert_eq!(tls.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));

        let (mut sender, conn) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
                .await
                .unwrap();
        tokio::spawn(conn);
        let req = hyper::Request::builder()
            .uri("https://localhost/ping")
            .body(Empty::<hyper::body::Bytes>::new())
            .unwrap();
        let res = sender.send_request(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"pong");

        server.abort();
    }
}
