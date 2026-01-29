use axum::{
    extract::Path,
    response::{
        Html,
        sse::{Event, KeepAlive, Sse},
    },
};
use std::convert::Infallible;
use std::time::Duration;

use crate::daemon::is_valid_daemon_id;
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
    <title>{title} - pitchfork</title>
    <link rel="icon" type="image/x-icon" href="/static/favicon.ico">
    <script src="https://unpkg.com/htmx.org@2.0.4"></script>
    <script src="https://unpkg.com/htmx-ext-sse@2.2.2/sse.js"></script>
    <script src="https://unpkg.com/lucide@0.474.0"></script>
    <link rel="stylesheet" href="/static/style.css">
</head>
<body>
    <nav>
        <a href="/" class="nav-brand"><img src="/static/logo.png" alt="pitchfork" class="logo-icon"> pitchfork</a>
        <div class="nav-links">
            <a href="/">Dashboard</a>
            <a href="/logs" class="active">Logs</a>
            <a href="/config">Config</a>
        </div>
    </nav>
    <main>
        {content}
    </main>
    <script>
        // Initialize Lucide icons on page load
        lucide.createIcons();
        
        // Re-initialize Lucide icons after HTMX swaps content
        document.body.addEventListener('htmx:afterSwap', function(evt) {{
            lucide.createIcons();
        }});
    </script>
</body>
</html>"#
    )
}

pub async fn index() -> Html<String> {
    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    let pt = PitchforkToml::all_merged();

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

    let content = if ids.is_empty() {
        r#"
        <h1>Logs</h1>
        <div class="empty-state">
            <h2>No daemons available</h2>
            <p>Configure daemons in your pitchfork.toml to view their logs.</p>
        </div>
        "#
        .to_string()
    } else {
        let mut tabs = String::new();
        let mut tab_contents = String::new();

        for (idx, id) in ids.iter().enumerate() {
            let safe_id = html_escape(id);
            let js_id = js_escape(id);
            let url_id = url_encode(id);
            let is_first = idx == 0;
            let active_class = if is_first { " active" } else { "" };

            // Tab button - use js_id for onclick to prevent JS injection
            tabs.push_str(&format!(
                r#"<button class="tab{active_class}" onclick="switchTab('{js_id}', event)">{safe_id}</button>"#
            ));

            // Tab content
            let log_path = env::PITCHFORK_LOGS_DIR.join(id).join(format!("{id}.log"));

            let initial_logs = if log_path.exists() {
                match std::fs::read(&log_path) {
                    Ok(bytes) => {
                        let content = String::from_utf8_lossy(&bytes);
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

            tab_contents.push_str(&format!(
                r#"
                <div id="tab-{safe_id}" class="tab-content{active_class}">
                    <div class="page-header">
                        <h2>DAEMON: {safe_id}</h2>
                        <div class="header-actions">
                            <button hx-post="/logs/{url_id}/clear" hx-swap="none" class="btn btn-sm"
                                hx-confirm="Are you sure you want to clear the logs for {safe_id}?"
                                onclick="clearTabLogs('{js_id}')"><i data-lucide="trash-2" class="icon"></i> Clear Logs</button>
                        </div>
                    </div>
                    <div class="log-viewer">
                        <pre id="log-output-{safe_id}" hx-ext="sse" sse-connect="/logs/{url_id}/stream" sse-swap="message" hx-swap="beforeend scroll:bottom">{initial_logs}</pre>
                    </div>
                </div>
                "#
            ));
        }

        format!(
            r#"
            <div class="page-header logs-header">
                <h1><i data-lucide="file-text" class="icon" style="width: 28px; height: 28px; vertical-align: middle;"></i> Logs</h1>
            </div>
            <div class="tabs">
                {}
            </div>
            {}
            <script>
                function switchTab(tabId, evt) {{
                    // Hide all tabs
                    document.querySelectorAll('.tab-content').forEach(el => el.classList.remove('active'));
                    document.querySelectorAll('.tab').forEach(el => el.classList.remove('active'));
                    
                    // Show selected tab
                    document.getElementById('tab-' + tabId).classList.add('active');
                    evt.currentTarget.classList.add('active');
                    
                    // Scroll to bottom
                    const logOutput = document.getElementById('log-output-' + tabId);
                    if (logOutput) {{
                        logOutput.scrollTop = logOutput.scrollHeight;
                    }}
                }}
                
                function clearTabLogs(tabId) {{
                    const logOutput = document.getElementById('log-output-' + tabId);
                    if (logOutput) {{
                        setTimeout(() => {{
                            logOutput.textContent = '';
                        }}, 100);
                    }}
                }}
                
                // Auto-scroll first tab on load
                window.addEventListener('load', function() {{
                    const firstLog = document.querySelector('.tab-content.active pre');
                    if (firstLog) {{
                        firstLog.scrollTop = firstLog.scrollHeight;
                    }}
                }});
                
                // Setup clear event listeners for all tabs
                {}
            </script>
            "#,
            tabs,
            tab_contents,
            ids.iter()
                .enumerate()
                .map(|(idx, id)| {
                    let js_id = js_escape(id);
                    let url_id = url_encode(id);
                    format!(
                        r#"
                var clearSource_{idx} = new EventSource('/logs/{url_id}/stream');
                clearSource_{idx}.addEventListener('clear', function(e) {{
                    document.getElementById('log-output-' + '{js_id}').textContent = '';
                }});
                "#
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    Html(base_html("Logs", &content))
}

pub async fn show(Path(id): Path<String>) -> Html<String> {
    // Validate daemon ID to prevent path traversal
    if !is_valid_daemon_id(&id) {
        let content = r#"<h1>Error</h1><p class="error">Invalid daemon ID.</p><a href="/logs" class="btn"><i data-lucide="arrow-left" class="icon"></i> Back</a>"#;
        return Html(base_html("Error", content));
    }

    let safe_id = html_escape(&id);
    let url_id = url_encode(&id);
    let log_path = env::PITCHFORK_LOGS_DIR.join(&id).join(format!("{id}.log"));

    let initial_logs = if log_path.exists() {
        match std::fs::read(&log_path) {
            Ok(bytes) => {
                // Use lossy conversion to handle invalid UTF-8
                let content = String::from_utf8_lossy(&bytes);
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
            <h1>Logs: {safe_id}</h1>
            <div class="header-actions">
            <button hx-post="/logs/{url_id}/clear" hx-swap="none" class="btn btn-sm"
                hx-confirm="Are you sure you want to clear the logs for {safe_id}?"><i data-lucide="trash-2" class="icon"></i> Clear Logs</button>
            <a href="/logs" class="btn btn-sm"><i data-lucide="arrow-left" class="icon"></i> Back</a>            </div>
        </div>
        <div class="log-viewer">
            <pre id="log-output" hx-ext="sse" sse-connect="/logs/{url_id}/stream" sse-swap="message" hx-swap="beforeend scroll:bottom">{initial_logs}</pre>
        </div>
        <script>
            // Auto-scroll to bottom on load
            document.getElementById('log-output').scrollTop = document.getElementById('log-output').scrollHeight;
            // Listen for clear event using native EventSource (htmx-ext-sse only handles 'message' events)
            var clearSource = new EventSource('/logs/{url_id}/stream');
            clearSource.addEventListener('clear', function(e) {{
                document.getElementById('log-output').textContent = '';
            }});
        </script>
    "#
    );

    Html(base_html(&format!("Logs: {safe_id}"), &content))
}

pub async fn lines_partial(Path(id): Path<String>) -> Html<String> {
    // Validate daemon ID to prevent path traversal
    if !is_valid_daemon_id(&id) {
        return Html(String::new());
    }

    let log_path = env::PITCHFORK_LOGS_DIR.join(&id).join(format!("{id}.log"));

    let logs = if log_path.exists() {
        match std::fs::read(&log_path) {
            Ok(bytes) => {
                // Use lossy conversion to handle invalid UTF-8
                let content = String::from_utf8_lossy(&bytes);
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
    // Validate daemon ID to prevent path traversal
    let valid_id = is_valid_daemon_id(&id);

    let log_path = if valid_id {
        env::PITCHFORK_LOGS_DIR.join(&id).join(format!("{id}.log"))
    } else {
        // Return a dummy path that won't exist - stream will just be empty
        std::path::PathBuf::from("/dev/null/invalid")
    };

    // Track file position
    let initial_size = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);

    let stream = async_stream::stream! {
        let mut last_size = initial_size;

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;

            if let Ok(metadata) = std::fs::metadata(&log_path) {
                let current_size = metadata.len();

                if current_size > last_size {
                    // Read new content as bytes to handle invalid UTF-8
                    if let Ok(file) = std::fs::File::open(&log_path) {
                        use std::io::{Read, Seek, SeekFrom};
                        let mut file = file;
                        if file.seek(SeekFrom::Start(last_size)).is_ok() {
                            let mut buffer = Vec::new();
                            if file.read_to_end(&mut buffer).is_ok() && !buffer.is_empty() {
                                // Use lossy conversion to handle invalid UTF-8 gracefully
                                let new_content = String::from_utf8_lossy(&buffer);
                                let escaped = html_escape(&new_content);
                                yield Ok(Event::default().event("message").data(escaped));
                            }
                            // Always update last_size to avoid stalling on invalid content
                            last_size = current_size;
                        }
                    }
                } else if current_size < last_size {
                    // File was truncated (cleared), send clear event and reset
                    yield Ok(Event::default().event("clear").data(""));
                    last_size = current_size;
                }
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub async fn clear(Path(id): Path<String>) -> Html<String> {
    // Validate daemon ID to prevent path traversal
    if !is_valid_daemon_id(&id) {
        return Html("".to_string());
    }

    let log_path = env::PITCHFORK_LOGS_DIR.join(&id).join(format!("{id}.log"));

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

/// Escape a string for use inside JavaScript single-quoted string literals.
/// This prevents breaking out of the string when the value contains quotes.
fn js_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn url_encode(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}
