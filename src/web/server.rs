use crate::Result;
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
    middleware::{self, Next},
    response::{Redirect, Response},
    routing::{get, post},
};
use std::net::SocketAddr;

use super::routes;
use super::static_files::static_handler;

/// CSRF protection middleware - requires HX-Request header on POST requests.
/// This prevents cross-origin form submissions since custom headers trigger CORS preflight.
async fn csrf_protection(request: Request<Body>, next: Next) -> Result<Response, StatusCode> {
    if request.method() == Method::POST {
        // htmx automatically sends HX-Request header on all requests
        // Cross-origin form submissions cannot set custom headers
        if !request.headers().contains_key("hx-request") {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(next.run(request).await)
}

/// Number of ports to try before giving up
const PORT_ATTEMPTS: u16 = 10;

pub async fn serve(port: u16, web_path: Option<String>) -> Result<()> {
    let base_path = super::normalize_base_path(web_path.as_deref());
    let _ = super::BASE_PATH.set(base_path.clone());

    let inner = Router::new()
        // Dashboard
        .route("/", get(routes::index::index))
        .route("/_stats", get(routes::index::stats_partial))
        .route("/health", get(|| async { "OK" }))
        // Daemons
        .route("/daemons", get(routes::daemons::list))
        .route("/daemons/_list", get(routes::daemons::list_partial))
        .route("/daemons/{id}", get(routes::daemons::show))
        .route("/daemons/{id}/start", post(routes::daemons::start))
        .route("/daemons/{id}/stop", post(routes::daemons::stop))
        .route("/daemons/{id}/restart", post(routes::daemons::restart))
        .route("/daemons/{id}/enable", post(routes::daemons::enable))
        .route("/daemons/{id}/disable", post(routes::daemons::disable))
        // Logs
        .route("/logs", get(routes::logs::index))
        .route("/logs/{id}", get(routes::logs::show))
        .route("/logs/{id}/_lines", get(routes::logs::lines_partial))
        .route("/logs/{id}/stream", get(routes::logs::stream_sse))
        .route("/logs/{id}/clear", post(routes::logs::clear))
        // Config
        .route("/config", get(routes::config::list))
        .route("/config/edit", get(routes::config::edit))
        .route("/config/validate", post(routes::config::validate))
        .route("/config/save", post(routes::config::save))
        // Static files
        .route("/static/{*path}", get(static_handler))
        // CSRF protection for all POST endpoints
        .layer(middleware::from_fn(csrf_protection));

    let app = if base_path.is_empty() {
        inner
    } else {
        let redirect_target = format!("{base_path}/");
        Router::new()
            .route(
                "/",
                get(move || async move { Redirect::permanent(&redirect_target) }),
            )
            .nest(&base_path, inner)
    };

    // Try up to PORT_ATTEMPTS ports starting from the given port
    let mut last_error = None;
    for offset in 0..PORT_ATTEMPTS {
        let try_port = port.saturating_add(offset);
        let addr = SocketAddr::from(([127, 0, 0, 1], try_port));

        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                if offset > 0 {
                    info!("Port {port} was in use, using port {try_port} instead");
                }
                if base_path.is_empty() {
                    info!("Web UI listening on http://{addr}");
                } else {
                    info!("Web UI listening on http://{addr}{base_path}/");
                }

                return axum::serve(listener, app)
                    .await
                    .map_err(|e| miette::miette!("Web server error: {}", e));
            }
            Err(e) => {
                debug!("Port {try_port} unavailable: {e}");
                last_error = Some(e);
            }
        }
    }

    Err(miette::miette!(
        "Failed to bind web server: tried ports {}-{}, all in use. Last error: {}",
        port,
        port.saturating_add(PORT_ATTEMPTS - 1),
        last_error.map(|e| e.to_string()).unwrap_or_default()
    ))
}
