use axum::{
    body::Body,
    extract::{Path, Query},
    http::StatusCode,
    response::Response,
};
use serde::Deserialize;
use std::convert::Infallible;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Deserialize)]
pub struct TailQuery {
    lines: Option<usize>,
}

pub async fn tail(Path(id): Path<String>, Query(query): Query<TailQuery>) -> Response<Body> {
    let daemon_id = match crate::daemon_id::DaemonId::parse(&id) {
        Ok(id) => id,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/plain")
                .body(Body::from("invalid daemon id"))
                .unwrap();
        }
    };

    let log_path = daemon_id.log_path();
    let history_lines = query.lines.unwrap_or(100);

    let mut file = match tokio::fs::File::open(&log_path).await {
        Ok(f) => f,
        Err(e) => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("content-type", "text/plain")
                .body(Body::from(format!("failed to open log file: {e}")))
                .unwrap();
        }
    };

    let stream = async_stream::stream! {
        const MAX_HISTORY_BYTES: u64 = 256 * 1024;
        let file_len = match file.metadata().await.map(|m| m.len()) {
            Ok(len) => len,
            Err(e) => {
                log::warn!("failed to read log metadata for {daemon_id}: {e}");
                return;
            }
        };
        let start_offset = file_len.saturating_sub(MAX_HISTORY_BYTES);

        if let Err(e) = file.seek(std::io::SeekFrom::Start(start_offset)).await {
            log::warn!("failed to seek log file for {daemon_id}: {e}");
            return;
        }

        let to_read = (file_len - start_offset) as usize;
        if to_read > 0 {
            let mut buf = vec![0u8; to_read];
            match file.read_exact(&mut buf).await {
                Ok(_) => {
                    let text = String::from_utf8_lossy(&buf);
                    let mut lines: Vec<&str> = text.split('\n').collect();
                    // First element may be a partial line when we started mid-file
                    if start_offset > 0 && !lines.is_empty() {
                        lines.remove(0);
                    }
                    // Last element may be a partial line if file doesn't end with newline
                    if !buf.is_empty() && buf.last() != Some(&b'\n') && !lines.is_empty() {
                        lines.pop();
                    }
                    let history = lines.into_iter().rev().take(history_lines).collect::<Vec<_>>();
                    let mut out = Vec::new();
                    for line in history.into_iter().rev() {
                        out.extend_from_slice(line.as_bytes());
                        out.push(b'\n');
                    }
                    if !out.is_empty() {
                        yield Ok::<Vec<u8>, Infallible>(out);
                    }
                }
                Err(e) => {
                    log::warn!("failed to read log file for {daemon_id}: {e}");
                    return;
                }
            }
        }

        // Tail -f from end of file
        if let Err(e) = file.seek(std::io::SeekFrom::End(0)).await {
            log::warn!("failed to seek log file for {daemon_id}: {e}");
            return;
        }

        let mut buf = vec![0u8; 8192];
        loop {
            match file.read(&mut buf).await {
                Ok(0) => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    continue;
                }
                Ok(n) => {
                    yield Ok::<Vec<u8>, Infallible>(buf[..n].to_vec());
                }
                Err(e) => {
                    log::warn!("failed to read log file for {daemon_id}: {e}");
                    return;
                }
            }
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/plain")
        .body(Body::from_stream(stream))
        .unwrap()
}
