use crate::Result;
use axum::{
    routing::{get, post},
    Router,
};
use std::net::SocketAddr;

use super::routes;
use super::static_files::static_handler;

/// Number of ports to try before giving up
const PORT_ATTEMPTS: u16 = 10;

pub async fn serve(port: u16) -> Result<()> {
    let app = Router::new()
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
        .route("/static/{*path}", get(static_handler));

    // Try up to PORT_ATTEMPTS ports starting from the given port
    let mut last_error = None;
    for offset in 0..PORT_ATTEMPTS {
        let try_port = port.saturating_add(offset);
        let addr = SocketAddr::from(([127, 0, 0, 1], try_port));

        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                if offset > 0 {
                    info!("Port {} was in use, using port {} instead", port, try_port);
                }
                info!("Web UI listening on http://{}", addr);

                return axum::serve(listener, app)
                    .await
                    .map_err(|e| miette::miette!("Web server error: {}", e));
            }
            Err(e) => {
                debug!("Port {} unavailable: {}", try_port, e);
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
