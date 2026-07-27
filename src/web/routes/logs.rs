use axum::{
    extract::Path,
    response::sse::{Event, KeepAlive, Sse},
};
use std::convert::Infallible;

use crate::cli::json_output::JsonLogEntry;
use crate::daemon::is_valid_daemon_id;
use crate::daemon_id::DaemonId;
use crate::log_store::sqlite::LOG_STORE;
use crate::log_store::{LogQuery, LogStore};
use crate::settings::settings;

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

        // Capture the starting cursor (last existing row id) and clear
        // generation atomically so a clear between them can't pair a stale
        // cursor with the new generation.
        let (mut last_id, mut last_clear_gen) = match tokio::task::spawn_blocking({
            let d = daemon_id.clone();
            move || LOG_STORE.query_with_generation(
                &LogQuery {
                    daemon_ids: vec![d.qualified()],
                    from: None,
                    to: None,
                    limit: Some(1),
                    order_desc: true,
                    after_id: None,
                    before_id: None,
                    message_filters: Vec::new(),
                    field_filters: Vec::new(),
                    include_structured: false,
                },
                &d,
            )
        })
        .await
        {
            Ok(Ok((entries, generation))) => {
                (entries.first().map(|e| e.id).unwrap_or(0), generation.unwrap_or(0))
            }
            _ => {
                // Initialization failed — don't fall back to cursor 0 which
                // would replay the entire history on the next poll.
                yield Ok(Event::default().event("error").data("failed to initialize log stream"));
                return;
            }
        };

        loop {
            tokio::time::sleep(sse_poll_interval).await;

            // Read new rows and current generation atomically so a clear
            // cannot interleave between the generation check and the row
            // query, which would produce duplicate streamed entries.
            const BATCH_SIZE: usize = 500;
            let poll_result = match tokio::task::spawn_blocking({
                let d = daemon_id.clone();
                move || LOG_STORE.query_with_generation(
                    &LogQuery {
                        daemon_ids: vec![d.qualified()],
                        from: None,
                        to: None,
                        limit: Some(BATCH_SIZE),
                        order_desc: false,
                        after_id: Some(last_id),
                        before_id: None,
                        message_filters: Vec::new(),
                        field_filters: Vec::new(),
                        include_structured: true,
                    },
                    &d,
                )
            })
            .await
            {
                Ok(Ok((entries, generation))) => (entries, generation.unwrap_or(0)),
                _ => continue,
            };

            let (entries, current_gen) = poll_result;

            if current_gen != last_clear_gen {
                last_clear_gen = current_gen;
                last_id = 0;
                yield Ok(Event::default().event("clear").data(""));
                continue;
            }

            for entry in entries {
                last_id = entry.id;
                let json_entry: JsonLogEntry = entry.into();
                let json_str = serde_json::to_string(&json_entry).unwrap_or_default();
                yield Ok(Event::default().event("message").data(json_str));
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}
