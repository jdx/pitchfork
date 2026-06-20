use axum::{
    extract::Path,
    response::sse::{Event, KeepAlive, Sse},
};
use std::convert::Infallible;

use crate::daemon::is_valid_daemon_id;
use crate::daemon_id::DaemonId;
use crate::log_store::sqlite::LOG_STORE;
use crate::log_store::{LogQuery, LogStore};
use crate::settings::settings;
use console;

pub async fn stream_sse(
    Path(id): Path<String>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let sse_poll_interval = settings().web_sse_poll_interval();

    let stream = async_stream::stream! {
        if !is_valid_daemon_id(&id) {
            yield Ok(Event::default().event("error").data("invalid daemon id"));
            return;
        }

        let daemon_id = match DaemonId::parse(&id) {
            Ok(d) => d,
            Err(_) => {
                yield Ok(Event::default().event("error").data("invalid daemon id"));
                return;
            }
        };

        let mut last_id: i64 = match tokio::task::spawn_blocking({
            let d = daemon_id.clone();
            move || LOG_STORE.query(&LogQuery {
                daemon_ids: vec![d.qualified()],
                from: None,
                to: None,
                limit: Some(1),
                order_desc: true,
                after_id: None,
                message_filters: Vec::new(),
            })
        }).await {
            Ok(Ok(entries)) => entries.first().map(|e| e.id).unwrap_or(0),
            _ => 0,
        };

        let mut last_clear_gen: u64 = match tokio::task::spawn_blocking({
            let d = daemon_id.clone();
            move || LOG_STORE.last_clear_generation(&d)
        }).await {
            Ok(Ok(Some(g))) => g,
            _ => 0,
        };

        loop {
            tokio::time::sleep(sse_poll_interval).await;

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
                yield Ok(Event::default().event("clear").data(""));
                continue;
            }

            const BATCH_SIZE: usize = 500;
            let entries = match tokio::task::spawn_blocking({
                let d = daemon_id.clone();
                move || LOG_STORE.query(&LogQuery {
                    daemon_ids: vec![d.qualified()],
                    from: None,
                    to: None,
                    limit: Some(BATCH_SIZE),
                    order_desc: false,
                    after_id: Some(last_id),
                    message_filters: Vec::new(),
                })
            }).await {
                Ok(Ok(e)) => e,
                _ => continue,
            };

            for entry in entries {
                last_id = entry.id;
                let ts = entry.timestamp.format("%Y-%m-%d %H:%M:%S");
                let stripped = console::strip_ansi_codes(&entry.message);
                yield Ok(Event::default().event("message").data(format!("{ts} {stripped}")));
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}
