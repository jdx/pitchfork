use axum::{
    extract::{Path, Query},
    response::Html,
};
use serde::Deserialize;

use crate::daemon::is_valid_daemon_id;
use crate::daemon_list::get_all_daemons_direct;
use crate::env;
use crate::ipc::batch::{StartOptions, build_run_options};
use crate::pitchfork_toml::PitchforkToml;
use crate::procs::PROCS;
use crate::state_file::StateFile;
use crate::supervisor::SUPERVISOR;

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn url_encode(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// Get daemon command from the stored cmd field
fn get_daemon_command(daemon: &crate::daemon::Daemon) -> String {
    daemon
        .cmd
        .as_ref()
        .map(shell_words::join)
        .unwrap_or_else(|| "-".to_string())
}

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
    <script src="https://unpkg.com/lucide@latest"></script>
    <link rel="stylesheet" href="/static/style.css">
</head>
<body>
    <nav>
        <a href="/" class="nav-brand"><img src="/static/logo.png" alt="pitchfork" class="logo-icon"> pitchfork</a>
        <div class="nav-links">
            <a href="/">Dashboard</a>
            <a href="/logs">Logs</a>
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
        
        // Optimize HTMX updates to reduce flicker
        document.body.addEventListener('htmx:beforeSwap', function(evt) {{
            // Get the new content
            const newContent = evt.detail.xhr.responseText.trim();
            const currentContent = evt.detail.target.innerHTML.trim();
            
            // Normalize whitespace for comparison
            const normalize = (str) => str.replace(/\\s+/g, ' ').trim();
            
            // Only swap if content actually changed
            if (normalize(newContent) === normalize(currentContent)) {{
                evt.detail.shouldSwap = false;
                evt.preventDefault();
            }}
        }});
    </script>
</body>
</html>"#
    )
}

fn daemon_row(id: &str, d: &crate::daemon::Daemon, is_disabled: bool) -> String {
    let safe_id = html_escape(id);
    let url_id = url_encode(id);
    let status_class = match &d.status {
        crate::daemon_status::DaemonStatus::Running => "running",
        crate::daemon_status::DaemonStatus::Stopped => "stopped",
        crate::daemon_status::DaemonStatus::Waiting => "waiting",
        crate::daemon_status::DaemonStatus::Stopping => "stopping",
        crate::daemon_status::DaemonStatus::Failed(_) => "failed",
        crate::daemon_status::DaemonStatus::Errored(_) => "errored",
    };

    let pid_display = d
        .pid
        .map(|p| p.to_string())
        .unwrap_or_else(|| "-".to_string());

    // Get process stats (CPU, memory, uptime)
    let stats = d.pid.and_then(|pid| PROCS.get_stats(pid));
    let cpu_display = stats
        .map(|s| s.cpu_display())
        .unwrap_or_else(|| "-".to_string());
    let mem_display = stats
        .map(|s| s.memory_display())
        .unwrap_or_else(|| "-".to_string());
    let uptime_display = stats
        .map(|s| s.uptime_display())
        .unwrap_or_else(|| "-".to_string());

    let error_msg = html_escape(&d.status.error_message().unwrap_or_default());
    let disabled_badge = if is_disabled {
        r#"<span class="badge disabled">disabled</span>"#
    } else {
        ""
    };

    let actions = if d.status.is_running() {
        format!(
            r##"
            <button hx-post="/daemons/{url_id}/stop" hx-target="#daemon-{safe_id}" hx-swap="outerHTML" hx-confirm="Stop daemon '{safe_id}'?" class="btn btn-sm"><i data-lucide="square" class="icon"></i> Stop</button>
            <button hx-post="/daemons/{url_id}/restart" hx-target="#daemon-{safe_id}" hx-swap="outerHTML" hx-confirm="Restart daemon '{safe_id}'?" class="btn btn-sm"><i data-lucide="refresh-cw" class="icon"></i> Restart</button>
        "##
        )
    } else {
        format!(
            r##"
            <button hx-post="/daemons/{url_id}/start" hx-target="#daemon-{safe_id}" hx-swap="outerHTML" class="btn btn-sm btn-primary"><i data-lucide="play" class="icon"></i> Start</button>
        "##
        )
    };

    let toggle_btn = if is_disabled {
        format!(
            r##"<button hx-post="/daemons/{url_id}/enable" hx-target="#daemon-{safe_id}" hx-swap="outerHTML" class="btn btn-sm"><i data-lucide="check" class="icon"></i> Enable</button>"##
        )
    } else {
        format!(
            r##"<button hx-post="/daemons/{url_id}/disable" hx-target="#daemon-{safe_id}" hx-swap="outerHTML" hx-confirm="Disable daemon '{safe_id}'?" class="btn btn-sm"><i data-lucide="x" class="icon"></i> Disable</button>"##
        )
    };

    format!(
        r#"<tr id="daemon-{safe_id}" class="clickable-row" onclick="window.location.href='/daemons/{url_id}'">
        <td><a href="/daemons/{url_id}" class="daemon-name" onclick="event.stopPropagation()">{safe_id}</a> {disabled_badge}</td>
        <td>{pid_display}</td>
        <td><span class="status {status_class}">{}</span></td>
        <td>{cpu_display}</td>
        <td>{mem_display}</td>
        <td>{uptime_display}</td>
        <td class="error-msg">{error_msg}</td>
        <td class="actions" onclick="event.stopPropagation()">{actions} {toggle_btn} <a href="/logs/{url_id}" class="btn btn-sm"><i data-lucide="file-text" class="icon"></i> Logs</a></td>
    </tr>"#,
        d.status
    )
}

pub async fn list() -> Html<String> {
    let content = list_content().await;
    Html(base_html("Daemons", &content))
}

async fn list_content() -> String {
    // Refresh process info for accurate CPU/memory stats
    PROCS.refresh_processes();

    let entries = get_all_daemons_direct(&SUPERVISOR)
        .await
        .unwrap_or_default();
    let mut rows = String::new();

    for entry in entries {
        if entry.is_available {
            // Show available (config-only) daemons
            let safe_id = html_escape(&entry.id);
            let url_id = url_encode(&entry.id);
            rows.push_str(&format!(r##"<tr id="daemon-{safe_id}" class="clickable-row" onclick="window.location.href='/daemons/{url_id}'">
                <td><a href="/daemons/{url_id}" class="daemon-name" onclick="event.stopPropagation()">{safe_id}</a></td>
                <td>-</td>
                <td><span class="status available">available</span></td>
                <td>-</td>
                <td>-</td>
                <td>-</td>
                <td></td>
                <td class="actions" onclick="event.stopPropagation()">
                    <button hx-post="/daemons/{url_id}/start" hx-target="#daemon-{safe_id}" hx-swap="outerHTML" class="btn btn-sm btn-primary"><i data-lucide="play" class="icon"></i> Start</button>
                    <a href="/logs/{url_id}" class="btn btn-sm"><i data-lucide="file-text" class="icon"></i> Logs</a>
                </td>
            </tr>"##));
        } else {
            // Show active daemons from state file
            rows.push_str(&daemon_row(&entry.id, &entry.daemon, entry.is_disabled));
        }
    }

    if rows.is_empty() {
        rows = r#"<tr><td colspan="8" class="empty">No daemons configured. Add some to pitchfork.toml</td></tr>"#.to_string();
    }

    format!(
        r##"
        <div class="page-header">
            <h1>Daemons</h1>
            <div class="header-actions">
                <button hx-get="/daemons/_list" hx-target="#daemon-list" hx-swap="innerHTML" class="btn btn-sm">Refresh</button>
            </div>
        </div>
        <table class="daemon-table">
            <thead>
                <tr>
                    <th>Name</th>
                    <th>PID</th>
                    <th>Status</th>
                    <th>CPU</th>
                    <th>Mem</th>
                    <th>Uptime</th>
                    <th>Error</th>
                    <th>Actions</th>
                </tr>
            </thead>
            <tbody id="daemon-list" hx-get="/daemons/_list" hx-trigger="every 5s" hx-swap="innerHTML swap:0.1s settle:0.1s">
            {rows}
            </tbody>
        </table>
    "##
    )
}

pub async fn list_partial() -> Html<String> {
    // Refresh process info for accurate CPU/memory stats
    PROCS.refresh_processes();

    let entries = get_all_daemons_direct(&SUPERVISOR)
        .await
        .unwrap_or_default();
    let mut rows = String::new();

    for entry in entries {
        if entry.is_available {
            // Show available (config-only) daemons
            let safe_id = html_escape(&entry.id);
            let url_id = url_encode(&entry.id);
            rows.push_str(&format!(r##"<tr id="daemon-{safe_id}" class="clickable-row" onclick="window.location.href='/daemons/{url_id}'">
                <td><a href="/daemons/{url_id}" class="daemon-name" onclick="event.stopPropagation()">{safe_id}</a></td>
                <td>-</td>
                <td><span class="status available">available</span></td>
                <td>-</td>
                <td>-</td>
                <td>-</td>
                <td></td>
                <td class="actions" onclick="event.stopPropagation()">
                    <button hx-post="/daemons/{url_id}/start" hx-target="#daemon-{safe_id}" hx-swap="outerHTML" class="btn btn-sm btn-primary"><i data-lucide="play" class="icon"></i> Start</button>
                    <a href="/logs/{url_id}" class="btn btn-sm"><i data-lucide="file-text" class="icon"></i> Logs</a>
                </td>
            </tr>"##));
        } else {
            // Show active daemons from state file
            rows.push_str(&daemon_row(&entry.id, &entry.daemon, entry.is_disabled));
        }
    }

    if rows.is_empty() {
        rows = r#"<tr><td colspan="8" class="empty">No daemons configured</td></tr>"#.to_string();
    }

    Html(rows)
}

pub async fn show(Path(id): Path<String>) -> Html<String> {
    // Validate daemon ID
    if !is_valid_daemon_id(&id) {
        let content = r#"<h1>Error</h1><p class="error">Invalid daemon ID.</p><a href="/" class="btn">Back</a>"#;
        return Html(base_html("Error", content));
    }

    // Refresh process info for accurate stats
    PROCS.refresh_processes();

    let safe_id = html_escape(&id);
    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    let pt = PitchforkToml::all_merged();

    let daemon_info = state.daemons.get(&id);
    let config_info = pt.daemons.get(&id);
    let is_disabled = state.disabled.contains(&id);

    let url_id = url_encode(&id);
    let content = if let Some(d) = daemon_info {
        let status_class = match &d.status {
            crate::daemon_status::DaemonStatus::Running => "running",
            crate::daemon_status::DaemonStatus::Stopped => "stopped",
            _ => "other",
        };

        // Get extended process info if we have a PID
        let process_section = if let Some(pid) = d.pid {
            if let Some(stats) = PROCS.get_extended_stats(pid) {
                format!(
                    r#"
                    <h2>Process Information</h2>
                    <div class="process-info-grid">
                        <div class="process-info-card">
                            <div class="label">CPU Usage</div>
                            <div class="value">{}</div>
                        </div>
                        <div class="process-info-card">
                            <div class="label">Memory (RSS)</div>
                            <div class="value">{}</div>
                        </div>
                        <div class="process-info-card">
                            <div class="label">Virtual Memory</div>
                            <div class="value">{}</div>
                        </div>
                        <div class="process-info-card">
                            <div class="label">Uptime</div>
                            <div class="value">{}</div>
                        </div>
                        <div class="process-info-card">
                            <div class="label">Threads</div>
                            <div class="value">{}</div>
                        </div>
                        <div class="process-info-card">
                            <div class="label">Disk Read</div>
                            <div class="value">{}</div>
                        </div>
                        <div class="process-info-card">
                            <div class="label">Disk Write</div>
                            <div class="value">{}</div>
                        </div>
                        <div class="process-info-card">
                            <div class="label">Process Status</div>
                            <div class="value">{}</div>
                        </div>
                    </div>
                    <div class="detail-section">
                        <dl>
                            <dt>Process Name</dt><dd><code>{}</code></dd>
                            <dt>Executable</dt><dd><code>{}</code></dd>
                            <dt>Working Dir</dt><dd><code>{}</code></dd>
                            <dt>Start Time</dt><dd>{}</dd>
                            <dt>Parent PID</dt><dd>{}</dd>
                            <dt>User</dt><dd>{}</dd>
                        </dl>
                    </div>
                    {}
                "#,
                    stats.cpu_display(),
                    stats.memory_display(),
                    stats.virtual_memory_display(),
                    stats.uptime_display(),
                    stats.thread_count,
                    stats.disk_read_display(),
                    stats.disk_write_display(),
                    html_escape(&stats.status),
                    html_escape(&stats.name),
                    html_escape(stats.exe_path.as_deref().unwrap_or("-")),
                    html_escape(stats.cwd.as_deref().unwrap_or("-")),
                    stats.start_time_display(),
                    stats
                        .parent_pid
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    html_escape(stats.user_id.as_deref().unwrap_or("-")),
                    if !stats.environ.is_empty() {
                        format!(
                            r#"<h2>Environment Variables (first 20)</h2>
                            <div class="detail-section">
                                <div class="env-list">{}</div>
                            </div>"#,
                            stats
                                .environ
                                .iter()
                                .map(|e| format!("<div>{}</div>", html_escape(e)))
                                .collect::<Vec<_>>()
                                .join("")
                        )
                    } else {
                        String::new()
                    }
                )
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let config_section = if let Some(cfg) = config_info {
            format!(
                r#"
                <h2>Configuration</h2>
                <div class="detail-section">
                    <dl>
                        <dt>Command</dt><dd><code>{}</code></dd>
                        <dt>Retry</dt><dd>{}</dd>
                        <dt>Ready Delay</dt><dd>{}</dd>
                        <dt>Ready Output</dt><dd>{}</dd>
                        <dt>Ready HTTP</dt><dd>{}</dd>
                    </dl>
                </div>
            "#,
                html_escape(&cfg.run),
                cfg.retry,
                cfg.ready_delay
                    .map(|d| format!("{d}s"))
                    .unwrap_or_else(|| "-".into()),
                html_escape(cfg.ready_output.as_deref().unwrap_or("-")),
                html_escape(cfg.ready_http.as_deref().unwrap_or("-")),
            )
        } else {
            String::new()
        };

        format!(
            r#"
            <div class="page-header">
                <div>
                    <h1><span class="daemon-label">DAEMON:</span> <span class="daemon-name">{safe_id}</span></h1>
                </div>
                <div class="header-actions">
                    <a href="/logs/{url_id}" class="btn btn-sm"><i data-lucide="file-text" class="icon"></i> View Logs</a>
                    <a href="/" class="btn btn-sm"><i data-lucide="arrow-left" class="icon"></i> Back</a>
                </div>
            </div>
            <div class="daemon-detail">
                <h2>Status</h2>
                <div class="detail-section">
                    <dl>
                        <dt>Status</dt><dd><span class="status {status_class}">{}</span></dd>
                        <dt>PID</dt><dd>{}</dd>
                        <dt>Directory</dt><dd>{}</dd>
                        <dt>Command</dt><dd><code>{}</code></dd>
                        <dt>Ad-hoc</dt><dd>{}</dd>
                        <dt>Disabled</dt><dd>{}</dd>
                        <dt>Retry Count</dt><dd>{} / {}</dd>
                    </dl>
                </div>
                {process_section}
                {config_section}
            </div>
        "#,
            d.status,
            d.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
            html_escape(
                &d.dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "-".into())
            ),
            html_escape(&get_daemon_command(d)),
            if config_info.is_none() { "Yes" } else { "No" },
            if is_disabled { "Yes" } else { "No" },
            d.retry_count,
            d.retry,
        )
    } else if config_info.is_some() {
        format!(
            r##"
            <h1>Daemon: {safe_id}</h1>
            <p>This daemon is configured but has not been started yet.</p>
            <div class="actions">
                <button hx-post="/daemons/{url_id}/start?from=detail" hx-target="#start-result" hx-swap="innerHTML" class="btn btn-primary">Start</button>
                <a href="/" class="btn">Back to List</a>
            </div>
            <div id="start-result"></div>
        "##
        )
    } else {
        format!(
            r#"
            <h1>Daemon Not Found</h1>
            <p>No daemon with ID "{safe_id}" exists.</p>
            <a href="/" class="btn">Back to List</a>
        "#
        )
    };

    Html(base_html(&format!("Daemon: {safe_id}"), &content))
}

#[derive(Deserialize, Default)]
pub struct StartQuery {
    #[serde(default)]
    from: Option<String>,
}

pub async fn start(Path(id): Path<String>, Query(query): Query<StartQuery>) -> Html<String> {
    // Validate daemon ID
    if !is_valid_daemon_id(&id) {
        return Html(r#"<div class="error">Invalid daemon ID</div>"#.to_string());
    }

    let safe_id = html_escape(&id);
    let pt = PitchforkToml::all_merged();
    let from_detail = query.from.as_deref() == Some("detail");

    let start_error = if let Some(daemon_config) = pt.daemons.get(&id) {
        // Use shared helper to build RunOptions from config
        let opts = StartOptions::default();
        let mut run_opts = match build_run_options(&id, daemon_config, &opts) {
            Ok(opts) => opts,
            Err(e) => {
                return if from_detail {
                    Html(format!(r#"<div class="error">{}</div>"#, html_escape(&e)))
                } else {
                    Html(format!(
                        r#"<tr id="daemon-{safe_id}"><td colspan="8" class="error">{}</td></tr>"#,
                        html_escape(&e)
                    ))
                };
            }
        };

        // Web UI specific: don't block on ready check, use CWD if no path
        run_opts.wait_ready = false;
        if run_opts.dir.as_os_str().is_empty() {
            run_opts.dir = env::CWD.clone();
        }

        match SUPERVISOR.run(run_opts).await {
            Ok(_) => None,
            Err(e) => Some(format!("Failed to start: {e}")),
        }
    } else {
        Some(format!("Daemon '{id}' not found in config"))
    };

    // Return updated row
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));

    // Return different content based on context
    if from_detail {
        if let Some(err) = start_error {
            Html(format!(r#"<div class="error">{}</div>"#, html_escape(&err)))
        } else if let Some(daemon) = state.daemons.get(&id) {
            let status = &daemon.status;
            Html(format!(
                r#"<div class="success">Started! Status: {status}</div><script>setTimeout(function(){{ window.location.href='/'; }}, 1000);</script>"#
            ))
        } else {
            Html(r#"<div>Starting...</div><script>setTimeout(function(){ window.location.href='/'; }, 1000);</script>"#.to_string())
        }
    } else {
        // Return table row for list page
        if let Some(daemon) = state.daemons.get(&id) {
            let is_disabled = state.disabled.contains(&id);
            Html(daemon_row(&id, daemon, is_disabled))
        } else if let Some(err) = start_error {
            Html(format!(
                r#"<tr id="daemon-{safe_id}"><td colspan="8" class="error">{}</td></tr>"#,
                html_escape(&err)
            ))
        } else {
            Html(format!(
                r#"<tr id="daemon-{safe_id}"><td colspan="8">Starting {safe_id}...</td></tr>"#
            ))
        }
    }
}

pub async fn stop(Path(id): Path<String>) -> Html<String> {
    // Validate daemon ID
    if !is_valid_daemon_id(&id) {
        return Html(r#"<div class="error">Invalid daemon ID</div>"#.to_string());
    }

    let safe_id = html_escape(&id);
    let _ = SUPERVISOR.stop(&id).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));

    if let Some(daemon) = state.daemons.get(&id) {
        let is_disabled = state.disabled.contains(&id);
        Html(daemon_row(&id, daemon, is_disabled))
    } else {
        Html(format!(
            r#"<tr id="daemon-{safe_id}"><td colspan="8">Stopped</td></tr>"#
        ))
    }
}

pub async fn restart(Path(id): Path<String>) -> Html<String> {
    // Validate daemon ID
    if !is_valid_daemon_id(&id) {
        return Html(r#"<div class="error">Invalid daemon ID</div>"#.to_string());
    }

    let _ = SUPERVISOR.stop(&id).await;
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    start(Path(id), Query(StartQuery::default())).await
}

pub async fn enable(Path(id): Path<String>) -> Html<String> {
    // Validate daemon ID
    if !is_valid_daemon_id(&id) {
        return Html(r#"<div class="error">Invalid daemon ID</div>"#.to_string());
    }

    let safe_id = html_escape(&id);
    let _ = SUPERVISOR.enable(id.clone()).await;

    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    if let Some(daemon) = state.daemons.get(&id) {
        let is_disabled = state.disabled.contains(&id);
        Html(daemon_row(&id, daemon, is_disabled))
    } else {
        Html(format!(
            r#"<tr id="daemon-{safe_id}"><td colspan="8">Enabled</td></tr>"#
        ))
    }
}

pub async fn disable(Path(id): Path<String>) -> Html<String> {
    // Validate daemon ID
    if !is_valid_daemon_id(&id) {
        return Html(r#"<div class="error">Invalid daemon ID</div>"#.to_string());
    }

    let safe_id = html_escape(&id);
    let _ = SUPERVISOR.disable(id.clone()).await;

    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    if let Some(daemon) = state.daemons.get(&id) {
        let is_disabled = state.disabled.contains(&id);
        Html(daemon_row(&id, daemon, is_disabled))
    } else {
        Html(format!(
            r#"<tr id="daemon-{safe_id}"><td colspan="8">Disabled</td></tr>"#
        ))
    }
}
