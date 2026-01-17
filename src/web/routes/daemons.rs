use axum::{extract::Path, response::Html};

use crate::daemon::RunOptions;
use crate::env;
use crate::pitchfork_toml::PitchforkToml;
use crate::state_file::StateFile;
use crate::supervisor::SUPERVISOR;

fn base_html(title: &str, content: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{title} - Pitchfork</title>
    <script src="/static/htmx.min.js"></script>
    <link rel="stylesheet" href="/static/style.css">
</head>
<body>
    <nav>
        <a href="/" class="nav-brand">Pitchfork</a>
        <div class="nav-links">
            <a href="/">Dashboard</a>
            <a href="/daemons" class="active">Daemons</a>
            <a href="/logs">Logs</a>
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

fn daemon_row(id: &str, d: &crate::daemon::Daemon, is_disabled: bool) -> String {
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
    let error_msg = d.status.error_message().unwrap_or_default();
    let disabled_badge = if is_disabled {
        r#"<span class="badge disabled">disabled</span>"#
    } else {
        ""
    };

    let actions = if d.status.is_running() {
        format!(
            r##"
            <button hx-post="/daemons/{id}/stop" hx-target="#daemon-{id}" hx-swap="outerHTML" class="btn btn-sm">Stop</button>
            <button hx-post="/daemons/{id}/restart" hx-target="#daemon-{id}" hx-swap="outerHTML" class="btn btn-sm">Restart</button>
        "##
        )
    } else {
        format!(
            r##"
            <button hx-post="/daemons/{id}/start" hx-target="#daemon-{id}" hx-swap="outerHTML" class="btn btn-sm btn-primary">Start</button>
        "##
        )
    };

    let toggle_btn = if is_disabled {
        format!(
            r##"<button hx-post="/daemons/{id}/enable" hx-target="#daemon-{id}" hx-swap="outerHTML" class="btn btn-sm">Enable</button>"##
        )
    } else {
        format!(
            r##"<button hx-post="/daemons/{id}/disable" hx-target="#daemon-{id}" hx-swap="outerHTML" class="btn btn-sm">Disable</button>"##
        )
    };

    format!(
        r#"<tr id="daemon-{id}">
        <td><a href="/daemons/{id}">{id}</a> {disabled_badge}</td>
        <td>{pid_display}</td>
        <td><span class="status {status_class}">{}</span></td>
        <td class="error-msg">{error_msg}</td>
        <td class="actions">{actions} {toggle_btn} <a href="/logs/{id}" class="btn btn-sm">Logs</a></td>
    </tr>"#,
        d.status
    )
}

pub async fn list() -> Html<String> {
    let content = list_content().await;
    Html(base_html("Daemons", &content))
}

async fn list_content() -> String {
    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    let pt = PitchforkToml::all_merged();

    let mut rows = String::new();

    // Show daemons from state file
    for (id, daemon) in &state.daemons {
        if id == "pitchfork" {
            continue; // Skip supervisor itself
        }
        let is_disabled = state.disabled.contains(id);
        rows.push_str(&daemon_row(id, daemon, is_disabled));
    }

    // Show daemons from config that aren't in state yet
    for id in pt.daemons.keys() {
        if !state.daemons.contains_key(id) {
            rows.push_str(&format!(r##"<tr id="daemon-{id}">
                <td><a href="/daemons/{id}">{id}</a> <span class="badge">not started</span></td>
                <td>-</td>
                <td><span class="status stopped">not started</span></td>
                <td></td>
                <td class="actions">
                    <button hx-post="/daemons/{id}/start" hx-target="#daemon-{id}" hx-swap="outerHTML" class="btn btn-sm btn-primary">Start</button>
                </td>
            </tr>"##));
        }
    }

    if rows.is_empty() {
        rows = r#"<tr><td colspan="5" class="empty">No daemons configured. Add some to pitchfork.toml</td></tr>"#.to_string();
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
                    <th>Error</th>
                    <th>Actions</th>
                </tr>
            </thead>
            <tbody id="daemon-list" hx-get="/daemons/_list" hx-trigger="every 5s" hx-swap="innerHTML">
                {rows}
            </tbody>
        </table>
    "##
    )
}

pub async fn list_partial() -> Html<String> {
    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    let pt = PitchforkToml::all_merged();

    let mut rows = String::new();

    for (id, daemon) in &state.daemons {
        if id == "pitchfork" {
            continue;
        }
        let is_disabled = state.disabled.contains(id);
        rows.push_str(&daemon_row(id, daemon, is_disabled));
    }

    for id in pt.daemons.keys() {
        if !state.daemons.contains_key(id) {
            rows.push_str(&format!(r##"<tr id="daemon-{id}">
                <td><a href="/daemons/{id}">{id}</a> <span class="badge">not started</span></td>
                <td>-</td>
                <td><span class="status stopped">not started</span></td>
                <td></td>
                <td class="actions">
                    <button hx-post="/daemons/{id}/start" hx-target="#daemon-{id}" hx-swap="outerHTML" class="btn btn-sm btn-primary">Start</button>
                </td>
            </tr>"##));
        }
    }

    if rows.is_empty() {
        rows = r#"<tr><td colspan="5" class="empty">No daemons configured</td></tr>"#.to_string();
    }

    Html(rows)
}

pub async fn show(Path(id): Path<String>) -> Html<String> {
    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    let pt = PitchforkToml::all_merged();

    let daemon_info = state.daemons.get(&id);
    let config_info = pt.daemons.get(&id);
    let is_disabled = state.disabled.contains(&id);

    let content = if let Some(d) = daemon_info {
        let status_class = match &d.status {
            crate::daemon_status::DaemonStatus::Running => "running",
            crate::daemon_status::DaemonStatus::Stopped => "stopped",
            _ => "other",
        };

        let config_section = if let Some(cfg) = config_info {
            format!(
                r#"
                <h2>Configuration</h2>
                <dl>
                    <dt>Command</dt><dd><code>{}</code></dd>
                    <dt>Retry</dt><dd>{}</dd>
                    <dt>Ready Delay</dt><dd>{}</dd>
                    <dt>Ready Output</dt><dd>{}</dd>
                    <dt>Ready HTTP</dt><dd>{}</dd>
                </dl>
            "#,
                cfg.run,
                cfg.retry,
                cfg.ready_delay
                    .map(|d| format!("{}s", d))
                    .unwrap_or_else(|| "-".into()),
                cfg.ready_output.as_deref().unwrap_or("-"),
                cfg.ready_http.as_deref().unwrap_or("-"),
            )
        } else {
            String::new()
        };

        format!(
            r#"
            <h1>Daemon: {id}</h1>
            <div class="daemon-detail">
                <h2>Status</h2>
                <dl>
                    <dt>Status</dt><dd><span class="status {status_class}">{}</span></dd>
                    <dt>PID</dt><dd>{}</dd>
                    <dt>Directory</dt><dd>{}</dd>
                    <dt>Disabled</dt><dd>{}</dd>
                    <dt>Retry Count</dt><dd>{} / {}</dd>
                </dl>
                {config_section}
                <div class="actions">
                    <a href="/logs/{id}" class="btn">View Logs</a>
                    <a href="/daemons" class="btn">Back to List</a>
                </div>
            </div>
        "#,
            d.status,
            d.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
            d.dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "-".into()),
            if is_disabled { "Yes" } else { "No" },
            d.retry_count,
            d.retry,
        )
    } else if config_info.is_some() {
        format!(
            r#"
            <h1>Daemon: {id}</h1>
            <p>This daemon is configured but has not been started yet.</p>
            <div class="actions">
                <button hx-post="/daemons/{id}/start" class="btn btn-primary">Start</button>
                <a href="/daemons" class="btn">Back to List</a>
            </div>
        "#
        )
    } else {
        format!(
            r#"
            <h1>Daemon Not Found</h1>
            <p>No daemon with ID "{id}" exists.</p>
            <a href="/daemons" class="btn">Back to List</a>
        "#
        )
    };

    Html(base_html(&format!("Daemon: {}", id), &content))
}

pub async fn start(Path(id): Path<String>) -> Html<String> {
    let pt = PitchforkToml::all_merged();

    let start_error = if let Some(daemon_config) = pt.daemons.get(&id) {
        let dir = daemon_config
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| env::CWD.clone());

        let opts = RunOptions {
            id: id.clone(),
            cmd: shell_words::split(&daemon_config.run).unwrap_or_default(),
            force: false,
            shell_pid: None,
            dir,
            autostop: false,
            cron_schedule: daemon_config.cron.as_ref().map(|c| c.schedule.clone()),
            cron_retrigger: daemon_config.cron.as_ref().map(|c| c.retrigger),
            retry: daemon_config.retry,
            retry_count: 0,
            ready_delay: daemon_config.ready_delay.or(Some(3)),
            ready_output: daemon_config.ready_output.clone(),
            ready_http: daemon_config.ready_http.clone(),
            wait_ready: false, // Don't block web request
        };

        match SUPERVISOR.run(opts).await {
            Ok(_) => None,
            Err(e) => Some(format!("Failed to start: {}", e)),
        }
    } else {
        Some(format!("Daemon '{}' not found in config", id))
    };

    // Return updated row
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));

    if let Some(daemon) = state.daemons.get(&id) {
        let is_disabled = state.disabled.contains(&id);
        Html(daemon_row(&id, daemon, is_disabled))
    } else if let Some(err) = start_error {
        Html(format!(
            r#"<tr id="daemon-{id}"><td colspan="5" class="error">{err}</td></tr>"#
        ))
    } else {
        Html(format!(
            r#"<tr id="daemon-{id}"><td colspan="5">Starting {id}...</td></tr>"#
        ))
    }
}

pub async fn stop(Path(id): Path<String>) -> Html<String> {
    let _ = SUPERVISOR.stop(&id).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));

    if let Some(daemon) = state.daemons.get(&id) {
        let is_disabled = state.disabled.contains(&id);
        Html(daemon_row(&id, daemon, is_disabled))
    } else {
        Html(format!(
            r#"<tr id="daemon-{id}"><td colspan="5">Stopped</td></tr>"#
        ))
    }
}

pub async fn restart(Path(id): Path<String>) -> Html<String> {
    let _ = SUPERVISOR.stop(&id).await;
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    start(Path(id)).await
}

pub async fn enable(Path(id): Path<String>) -> Html<String> {
    let _ = SUPERVISOR.enable(id.clone()).await;

    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    if let Some(daemon) = state.daemons.get(&id) {
        let is_disabled = state.disabled.contains(&id);
        Html(daemon_row(&id, daemon, is_disabled))
    } else {
        Html(format!(
            r#"<tr id="daemon-{id}"><td colspan="5">Enabled</td></tr>"#
        ))
    }
}

pub async fn disable(Path(id): Path<String>) -> Html<String> {
    let _ = SUPERVISOR.disable(id.clone()).await;

    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    if let Some(daemon) = state.daemons.get(&id) {
        let is_disabled = state.disabled.contains(&id);
        Html(daemon_row(&id, daemon, is_disabled))
    } else {
        Html(format!(
            r#"<tr id="daemon-{id}"><td colspan="5">Disabled</td></tr>"#
        ))
    }
}
