use axum::{
    extract::Path,
    response::{
        sse::{Event, KeepAlive, Sse},
        Html,
    },
};
use std::convert::Infallible;
use std::time::Duration;

use crate::env;
use crate::pitchfork_toml::PitchforkToml;
use crate::state_file::StateFile;

fn base_html(title: &str, content: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{title} - Pitchfork</title>
    <script src="https://unpkg.com/htmx.org@2.0.4"></script>
    <script src="https://unpkg.com/htmx-ext-sse@2.2.2/sse.js"></script>
    <link rel="stylesheet" href="/static/style.css">
</head>
<body>
    <nav>
        <a href="/" class="nav-brand">Pitchfork</a>
        <div class="nav-links">
            <a href="/">Dashboard</a>
            <a href="/logs" class="active">Logs</a>
            <a href="/config">Config</a>
        </div>
    </nav>
    <main>
        {content}
    </main>
</body>
</html>"#
    )
}

pub async fn index() -> Html<String> {
    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    let pt = PitchforkToml::all_merged();

    let mut daemon_list = String::new();

    // Collect all daemon IDs
    let mut ids: Vec<String> = state
        .daemons
        .keys()
        .filter(|id| *id != "pitchfork")
        .cloned()
        .collect();

    for id in pt.daemons.keys() {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }

    ids.sort();

    for id in ids {
        daemon_list.push_str(&format!(
            r#"
            <li><a href="/logs/{id}">{id}</a></li>
        "#
        ));
    }

    if daemon_list.is_empty() {
        daemon_list = "<li>No daemons available</li>".to_string();
    }

    let content = format!(
        r#"
        <h1>Logs</h1>
        <p>Select a daemon to view its logs:</p>
        <ul class="daemon-log-list">
            {daemon_list}
        </ul>
    "#
    );

    Html(base_html("Logs", &content))
}

pub async fn show(Path(id): Path<String>) -> Html<String> {
    let log_path = env::PITCHFORK_LOGS_DIR
        .join(&id)
        .join(format!("{}.log", id));

    let initial_logs = if log_path.exists() {
        match std::fs::read_to_string(&log_path) {
            Ok(content) => {
                // Get last 100 lines
                let lines: Vec<&str> = content.lines().collect();
                let start = if lines.len() > 100 {
                    lines.len() - 100
                } else {
                    0
                };
                html_escape(&lines[start..].join("\n"))
            }
            Err(_) => String::new(),
        }
    } else {
        "No logs available yet.".to_string()
    };

    let content = format!(
        r#"
        <div class="page-header">
            <h1>Logs: {id}</h1>
            <div class="header-actions">
                <button hx-post="/logs/{id}/clear" hx-swap="none" class="btn btn-sm"
                    hx-confirm="Are you sure you want to clear the logs?">Clear Logs</button>
                <a href="/logs" class="btn btn-sm">Back</a>
            </div>
        </div>
        <div class="log-viewer">
            <pre id="log-output" hx-ext="sse" sse-connect="/logs/{id}/stream" sse-swap="message" hx-swap="beforeend scroll:bottom">{initial_logs}</pre>
        </div>
        <script>
            // Auto-scroll to bottom on load
            document.getElementById('log-output').scrollTop = document.getElementById('log-output').scrollHeight;
        </script>
    "#
    );

    Html(base_html(&format!("Logs: {}", id), &content))
}

pub async fn lines_partial(Path(id): Path<String>) -> Html<String> {
    let log_path = env::PITCHFORK_LOGS_DIR
        .join(&id)
        .join(format!("{}.log", id));

    let logs = if log_path.exists() {
        match std::fs::read_to_string(&log_path) {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let start = if lines.len() > 100 {
                    lines.len() - 100
                } else {
                    0
                };
                html_escape(&lines[start..].join("\n"))
            }
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };

    Html(logs)
}

pub async fn stream_sse(
    Path(id): Path<String>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let log_path = env::PITCHFORK_LOGS_DIR
        .join(&id)
        .join(format!("{}.log", id));

    // Track file position
    let initial_size = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);

    let stream = async_stream::stream! {
        let mut last_size = initial_size;

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;

            if let Ok(metadata) = std::fs::metadata(&log_path) {
                let current_size = metadata.len();

                if current_size > last_size {
                    // Read new content
                    if let Ok(file) = std::fs::File::open(&log_path) {
                        use std::io::{Read, Seek, SeekFrom};
                        let mut file = file;
                        if file.seek(SeekFrom::Start(last_size)).is_ok() {
                            let mut new_content = String::new();
                            if file.read_to_string(&mut new_content).is_ok() && !new_content.is_empty() {
                                let escaped = html_escape(&new_content);
                                yield Ok(Event::default().data(escaped));
                            }
                        }
                    }
                    last_size = current_size;
                } else if current_size < last_size {
                    // File was truncated (cleared), reset
                    last_size = current_size;
                }
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub async fn clear(Path(id): Path<String>) -> Html<String> {
    let log_path = env::PITCHFORK_LOGS_DIR
        .join(&id)
        .join(format!("{}.log", id));

    if log_path.exists() {
        let _ = std::fs::write(&log_path, "");
    }

    Html("".to_string())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
