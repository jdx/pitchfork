use axum::{
    body::Body,
    extract::{Path, Query},
    http::StatusCode,
    response::Response,
};
use serde::Deserialize;
use std::convert::Infallible;

use crate::daemon_id::DaemonId;
use crate::log_store::sqlite::LOG_STORE;
use crate::log_store::{LogQuery, LogStore};

#[derive(Deserialize)]
pub struct TailQuery {
    lines: Option<usize>,
}

pub async fn tail(Path(id): Path<String>, Query(query): Query<TailQuery>) -> Response<Body> {
    let daemon_id = match DaemonId::parse(&id) {
        Ok(id) => id,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/plain")
                .body(Body::from("invalid daemon id"))
                .unwrap();
        }
    };

    let history_lines = query.lines.unwrap_or(100);
    let qualified = daemon_id.qualified();

    // Fetch initial history from the SQLite log store.
    let initial = match tokio::task::spawn_blocking({
        let q = qualified.clone();
        move || {
            LOG_STORE.query(&LogQuery {
                daemon_ids: vec![q],
                from: None,
                to: None,
                limit: Some(history_lines),
                order_desc: true,
                after_id: None,
                message_filters: Vec::new(),
                field_filters: Vec::new(),
                include_structured: false,
            })
        }
    })
    .await
    {
        Ok(Ok(entries)) => entries,
        Ok(Err(e)) => {
            log::warn!("failed to query logs for {daemon_id}: {e}");
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("content-type", "text/plain")
                .body(Body::from(format!("failed to query logs: {e}")))
                .unwrap();
        }
        Err(e) => {
            log::warn!("log query task panicked for {daemon_id}: {e}");
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "text/plain")
                .body(Body::from("log query failed"))
                .unwrap();
        }
    };

    // Capture cursor from initial entries before reversing.
    let cursor_id = initial.first().map(|e| e.id).unwrap_or(0);

    // Reverse so oldest lines are yielded first.
    let initial: Vec<String> = initial
        .into_iter()
        .rev()
        .map(|e| {
            let ts = e.timestamp.format("%Y-%m-%d %H:%M:%S");
            format!("{ts} {msg}\n", msg = e.message)
        })
        .collect();

    let qualified_clone = qualified.clone();
    let stream = async_stream::stream! {
        // Yield history
        for line in initial {
            yield Ok::<Vec<u8>, Infallible>(line.into_bytes());
        }

        let mut last_id: i64 = cursor_id;

        let mut last_clear_gen: u64 = match tokio::task::spawn_blocking({
            let d = daemon_id.clone();
            move || LOG_STORE.last_clear_generation(&d)
        }).await {
            Ok(Ok(Some(g))) => g,
            _ => 0,
        };

        const BATCH_SIZE: usize = 500;
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            // Detect log clear
            let current_gen: u64 = match tokio::task::spawn_blocking({
                let d = daemon_id.clone();
                move || LOG_STORE.last_clear_generation(&d)
            }).await {
                Ok(Ok(Some(g))) => g,
                _ => 0,
            };

            if current_gen != last_clear_gen {
                last_clear_gen = current_gen;
                last_id = 0;
                continue;
            }

            let entries = match tokio::task::spawn_blocking({
                let q = qualified_clone.clone();
                move || LOG_STORE.query(&LogQuery {
                    daemon_ids: vec![q],
                    from: None,
                    to: None,
                    limit: Some(BATCH_SIZE),
                    order_desc: false,
                    after_id: Some(last_id),
                    message_filters: Vec::new(),
                    field_filters: Vec::new(),
                    include_structured: false,
                })
            }).await {
                Ok(Ok(e)) => e,
                _ => continue,
            };

            for entry in entries {
                last_id = entry.id;
                let ts = entry.timestamp.format("%Y-%m-%d %H:%M:%S");
                yield Ok::<Vec<u8>, Infallible>(format!("{ts} {msg}\n", msg = entry.message).into_bytes());
            }
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/plain")
        .body(Body::from_stream(stream))
        .unwrap()
}
