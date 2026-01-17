use crate::Result;
use axum::{
    routing::{get, post},
    Router,
};
use std::net::SocketAddr;

use super::routes;
use super::static_files::static_handler;

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

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| miette::miette!("Failed to bind web server to {}: {}", addr, e))?;

    info!("Web UI listening on http://{}", addr);

    axum::serve(listener, app)
        .await
        .map_err(|e| miette::miette!("Web server error: {}", e))?;

    Ok(())
}
