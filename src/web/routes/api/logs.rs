use axum::{
    body::Body,
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Local, TimeZone};
use serde::Deserialize;
use std::convert::Infallible;

use crate::cli::json_output::JsonLogEntry;
use crate::daemon_id::DaemonId;
use crate::log_store::sqlite::LOG_STORE;
use crate::log_store::{FieldFilter, LogQuery, LogStore, MessageFilter};

#[derive(Deserialize)]
pub struct TailQuery {
    lines: Option<usize>,
    since: Option<String>,
    until: Option<String>,
    level: Option<String>,
    grep: Option<String>,
    regex: Option<String>,
    logger: Option<String>,
    /// Structured field filters in "KEY=VALUE" format (can be repeated).
    field: Option<Vec<String>>,
    /// Whether grep should be case-sensitive. Default: false.
    case_sensitive: Option<bool>,
    /// jq expression for advanced filtering.
    jq: Option<String>,
}

/// Parse a datetime string from the query params.
/// Accepts ISO 8601 (e.g. "2026-07-22T17:00:00") or "YYYY-MM-DD HH:MM:SS".
fn parse_datetime(s: &str) -> Option<DateTime<Local>> {
    // Try ISO 8601 first
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Local));
    }
    // Try "YYYY-MM-DD HH:MM:SS"
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Local.from_local_datetime(&naive).single();
    }
    // Try "YYYY-MM-DDTHH:MM:SS" (datetime-local input format)
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Local.from_local_datetime(&naive).single();
    }
    None
}

/// Build message and field filters from query params.
/// Returns an error string if the regex pattern is invalid.
fn build_filters(
    query: &TailQuery,
) -> Result<(Vec<MessageFilter>, Vec<FieldFilter>), String> {
    let mut message_filters = Vec::new();
    let mut field_filters = Vec::new();

    if let Some(grep) = query.grep.as_deref().filter(|s| !s.is_empty()) {
        message_filters.push(MessageFilter::Contains {
            pattern: grep.to_string(),
            case_sensitive: query.case_sensitive.unwrap_or(false),
        });
    }

    if let Some(regex) = query.regex.as_deref().filter(|s| !s.is_empty()) {
        // Pre-validate regex so the user gets a clear 400 error instead of
        // a SQLite user-function failure at query time (which the polling
        // loop would silently swallow, stalling the stream).
        if let Err(e) = regex::Regex::new(regex) {
            return Err(format!("invalid regex pattern: {e}"));
        }
        message_filters.push(MessageFilter::Regex {
            pattern: regex.to_string(),
        });
    }

    if let Some(level) = query.level.as_deref().filter(|s| !s.is_empty()) {
        if let Some(normalized) = crate::log_parse::normalize_level_str(level) {
            field_filters.push(FieldFilter::LevelMin(normalized));
        }
    }

    if let Some(logger) = query.logger.as_deref().filter(|s| !s.is_empty()) {
        field_filters.push(FieldFilter::LoggerContains(logger.to_string()));
    }

    // Parse "KEY=VALUE" field filters (can be repeated).
    if let Some(fields) = &query.field {
        for pair in fields {
            if let Some((key, value)) = pair.split_once('=') {
                field_filters.push(FieldFilter::FieldEq {
                    key: key.to_string(),
                    value: value.to_string(),
                });
            }
        }
    }

    Ok((message_filters, field_filters))
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

    let from = query.since.as_deref().and_then(parse_datetime);
    let to = query.until.as_deref().and_then(parse_datetime);
    let (message_filters, field_filters) = match build_filters(&query) {
        Ok(v) => v,
        Err(e) => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/plain")
                .body(Body::from(e))
                .unwrap();
        }
    };

    // Compile jq filter early so parse errors surface before any query.
    let jq_filter = match query.jq.as_deref().filter(|s| !s.is_empty()) {
        Some(expr) => match crate::log_jq::JqFilter::new(expr) {
            Ok(f) => Some(f),
            Err(e) => {
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("content-type", "text/plain")
                    .body(Body::from(format!("invalid jq expression: {e}")))
                    .unwrap();
            }
        },
        None => None,
    };

    // Fetch initial history from the SQLite log store.
    let initial = match tokio::task::spawn_blocking({
        let q = qualified.clone();
        let mf = message_filters.clone();
        let ff = field_filters.clone();
        move || {
            LOG_STORE.query(&LogQuery {
                daemon_ids: vec![q],
                from,
                to,
                limit: Some(history_lines),
                order_desc: true,
                after_id: None,
                message_filters: mf,
                field_filters: ff,
                include_structured: true,
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

    // Capture cursor from raw query (before jq filtering) so the polling
    // loop doesn't rescan jq-filtered-out rows on every poll.
    let cursor_id = initial.first().map(|e| e.id).unwrap_or(0);

    // Apply jq filter if present.
    let initial = if let Some(jq) = &jq_filter {
        jq.filter(initial)
    } else {
        initial
    };

    // Reverse so oldest lines are yielded first.
    let initial: Vec<String> = initial
        .into_iter()
        .rev()
        .map(|e| {
            let entry: JsonLogEntry = e.into();
            serde_json::to_string(&entry).unwrap_or_default() + "\n"
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

            let raw_entries = match tokio::task::spawn_blocking({
                let q = qualified_clone.clone();
                let mf = message_filters.clone();
                let ff = field_filters.clone();
                move || LOG_STORE.query(&LogQuery {
                    daemon_ids: vec![q],
                    from,
                    to,
                    limit: Some(BATCH_SIZE),
                    order_desc: false,
                    after_id: Some(last_id),
                    message_filters: mf,
                    field_filters: ff,
                    include_structured: true,
                })
            }).await {
                Ok(Ok(e)) => e,
                _ => continue,
            };

            // Advance cursor past all raw entries (not just jq-matched ones)
            // so filtered-out rows aren't re-scanned on every poll.
            if let Some(last) = raw_entries.last() {
                last_id = last.id;
            }

            // Apply jq filter if present.
            let entries = if let Some(jq) = &jq_filter {
                jq.filter(raw_entries)
            } else {
                raw_entries
            };

            for entry in entries {
                let json_entry: JsonLogEntry = entry.into();
                let line = serde_json::to_string(&json_entry).unwrap_or_default() + "\n";
                yield Ok::<Vec<u8>, Infallible>(line.into_bytes());
            }
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/x-ndjson")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// Return distinct logger names for a daemon, for populating filter dropdowns.
pub async fn loggers(Path(id): Path<String>) -> Response<Body> {
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

    let qualified = daemon_id.qualified();
    let loggers = match tokio::task::spawn_blocking(move || {
        LOG_STORE.distinct_loggers(&qualified)
    })
    .await
    {
        Ok(Ok(loggers)) => loggers,
        Ok(Err(e)) => {
            log::warn!("failed to query loggers for {daemon_id}: {e}");
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "text/plain")
                .body(Body::from("failed to query loggers"))
                .unwrap();
        }
        Err(e) => {
            log::warn!("loggers query task panicked for {daemon_id}: {e}");
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "text/plain")
                .body(Body::from("loggers query failed"))
                .unwrap();
        }
    };

    (
        StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string(&loggers).unwrap_or_else(|_| "[]".to_string()),
    )
        .into_response()
}

/// Return distinct structured field keys for a daemon, for jq autocomplete.
pub async fn field_keys(Path(id): Path<String>) -> Response<Body> {
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

    let qualified = daemon_id.qualified();
    let keys = match tokio::task::spawn_blocking(move || {
        LOG_STORE.distinct_field_keys(&qualified)
    })
    .await
    {
        Ok(Ok(keys)) => keys,
        Ok(Err(e)) => {
            log::warn!("failed to query field keys for {daemon_id}: {e}");
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "text/plain")
                .body(Body::from("failed to query field keys"))
                .unwrap();
        }
        Err(e) => {
            log::warn!("field keys query task panicked for {daemon_id}: {e}");
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "text/plain")
                .body(Body::from("field keys query failed"))
                .unwrap();
        }
    };

    (
        StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string(&keys).unwrap_or_else(|_| "[]".to_string()),
    )
        .into_response()
}
