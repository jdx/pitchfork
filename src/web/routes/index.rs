use axum::response::Html;
use std::collections::HashSet;

use crate::env;
use crate::pitchfork_toml::PitchforkToml;
use crate::state_file::StateFile;

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

pub async fn index() -> Html<String> {
    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    let pt = PitchforkToml::all_merged();

    // Exclude the "pitchfork" supervisor daemon from counts
    let user_daemons: Vec<_> = state
        .daemons
        .iter()
        .filter(|(id, _)| *id != "pitchfork")
        .collect();

    let running_count = user_daemons
        .iter()
        .filter(|(_, d)| d.status.is_running())
        .count();
    let stopped_count = user_daemons
        .iter()
        .filter(|(_, d)| d.status.is_stopped())
        .count();
    let errored_count = user_daemons
        .iter()
        .filter(|(_, d)| d.status.is_errored())
        .count();

    // Total includes both state file daemons and configured-but-not-started daemons
    let mut all_ids: HashSet<&String> = user_daemons.iter().map(|(id, _)| *id).collect();
    for id in pt.daemons.keys() {
        all_ids.insert(id);
    }
    let total = all_ids.len();

    // Build daemon table rows
    let mut rows = String::new();
    for (id, daemon) in &state.daemons {
        if id == "pitchfork" {
            continue;
        }
        let is_disabled = state.disabled.contains(id);
        rows.push_str(&daemon_row(id, daemon, is_disabled));
    }

    // Add configured-but-not-started daemons
    for id in pt.daemons.keys() {
        if !state.daemons.contains_key(id) {
            rows.push_str(&format!(
                r##"<tr id="daemon-{id}">
                <td><a href="/daemons/{id}">{id}</a> <span class="badge">not started</span></td>
                <td>-</td>
                <td><span class="status stopped">not started</span></td>
                <td></td>
                <td class="actions">
                    <button hx-post="/daemons/{id}/start" hx-target="#daemon-{id}" hx-swap="outerHTML" class="btn btn-sm btn-primary">Start</button>
                </td>
            </tr>"##
            ));
        }
    }

    if rows.is_empty() {
        rows = r#"<tr><td colspan="5" class="empty">No daemons configured. Add some to pitchfork.toml</td></tr>"#.to_string();
    }

    let html = format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Pitchfork</title>
    <script src="https://unpkg.com/htmx.org@2.0.4"></script>
    <link rel="stylesheet" href="/static/style.css">
</head>
<body>
    <nav>
        <a href="/" class="nav-brand">Pitchfork</a>
        <div class="nav-links">
            <a href="/" class="active">Dashboard</a>
            <a href="/logs">Logs</a>
            <a href="/config">Config</a>
        </div>
    </nav>
    <main>
        <div class="stats-grid">
            <div class="stat-card">
                <div class="stat-value">{total}</div>
                <div class="stat-label">Total</div>
            </div>
            <div class="stat-card running">
                <div class="stat-value">{running_count}</div>
                <div class="stat-label">Running</div>
            </div>
            <div class="stat-card stopped">
                <div class="stat-value">{stopped_count}</div>
                <div class="stat-label">Stopped</div>
            </div>
            <div class="stat-card errored">
                <div class="stat-value">{errored_count}</div>
                <div class="stat-label">Errored</div>
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
    </main>
</body>
</html>"##
    );

    Html(html)
}
