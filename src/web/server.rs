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
        .route("/api/namespaces", get(routes::api::namespaces::list))
        .route("/api/namespaces", post(routes::api::namespaces::register))
        .route(
            "/api/namespaces/{name}",
            axum::routing::delete(routes::api::namespaces::remove),
        )
        .route("/api/proxies", get(routes::api::proxies::list))
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
    // The scheme is hardcoded because the web server only speaks plain HTTP.
    // If TLS support is ever added, this must pick the scheme accordingly or
    // the URL reported by `supervisor status` will be wrong.
    let _ = super::WEB_URL.set(format!("http://{url_host}:{actual_port}{base_path}"));

    info!("Web UI listening on http://{actual_addr}");

    axum::serve(listener, app)
        .await
        .map_err(|e| miette::miette!("Web server error: {}", e))
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
