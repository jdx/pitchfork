use axum::response::Html;
use std::collections::HashSet;

use crate::daemon_id::DaemonId;
use crate::env;
use crate::pitchfork_toml::PitchforkToml;
use crate::procs::PROCS;
use crate::state_file::StateFile;
use crate::web::helpers::{css_safe_id, format_daemon_id_html, html_escape, url_encode};

fn daemon_row(id: &DaemonId, d: &crate::daemon::Daemon, is_disabled: bool) -> String {
    let id_str = id.to_string();
    let safe_id = css_safe_id(&id_str);
    let confirm_id = html_escape(&id_str); // For display in confirm dialogs
    let url_id = url_encode(&id_str);
    let display_html = format_daemon_id_html(id);
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
            <button hx-post="/daemons/{url_id}/stop" hx-target="#daemon-{safe_id}" hx-swap="outerHTML" hx-confirm="Stop daemon '{confirm_id}'?" class="btn btn-sm"><i data-lucide="square" class="icon"></i> Stop</button>
            <button hx-post="/daemons/{url_id}/restart" hx-target="#daemon-{safe_id}" hx-swap="outerHTML" hx-confirm="Restart daemon '{confirm_id}'?" class="btn btn-sm"><i data-lucide="refresh-cw" class="icon"></i> Restart</button>
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
            r##"<button hx-post="/daemons/{url_id}/disable" hx-target="#daemon-{safe_id}" hx-swap="outerHTML" hx-confirm="Disable daemon '{confirm_id}'?" class="btn btn-sm"><i data-lucide="x" class="icon"></i> Disable</button>"##
        )
    };

    format!(
        r#"<tr id="daemon-{safe_id}" class="clickable-row" onclick="window.location.href='/daemons/{url_id}'">
        <td><a href="/daemons/{url_id}" class="daemon-name" onclick="event.stopPropagation()">{display_html}</a> {disabled_badge}</td>
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

fn get_stats() -> (usize, usize, usize, usize) {
    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    let pt = PitchforkToml::all_merged();

    let pitchfork_id = DaemonId::pitchfork();
    let user_daemons: Vec<_> = state
        .daemons
        .iter()
        .filter(|(id, _)| **id != pitchfork_id)
        .collect();

    let running = user_daemons
        .iter()
        .filter(|(_, d)| d.status.is_running())
        .count();
    let stopped = user_daemons
        .iter()
        .filter(|(_, d)| d.status.is_stopped())
        .count();
    let errored = user_daemons
        .iter()
        .filter(|(_, d)| d.status.is_errored())
        .count();

    let mut all_ids: HashSet<&DaemonId> = user_daemons.iter().map(|(id, _)| *id).collect();
    for id in pt.daemons.keys() {
        all_ids.insert(id);
    }
    let total = all_ids.len();

    (total, running, stopped, errored)
}

pub async fn stats_partial() -> Html<String> {
    let (total, running_count, stopped_count, errored_count) = get_stats();

    Html(format!(
        r#"<div class="stat-card">
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
        </div>"#
    ))
}

pub async fn index() -> Html<String> {
    // Refresh process info for accurate CPU/memory stats
    PROCS.refresh_processes();

    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    let pt = PitchforkToml::all_merged();

    // Exclude the "pitchfork" supervisor daemon from counts
    let pitchfork_id = DaemonId::pitchfork();
    let user_daemons: Vec<_> = state
        .daemons
        .iter()
        .filter(|(id, _)| **id != pitchfork_id)
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
    let mut all_ids: HashSet<&DaemonId> = user_daemons.iter().map(|(id, _)| *id).collect();
    for id in pt.daemons.keys() {
        all_ids.insert(id);
    }
    let total = all_ids.len();

    // Build daemon table rows
    let mut rows = String::new();
    for (id, daemon) in &state.daemons {
        if *id == pitchfork_id {
            continue;
        }
        let is_disabled = state.disabled.contains(id);
        rows.push_str(&daemon_row(id, daemon, is_disabled));
    }

    // Add configured-but-not-started daemons
    for id in pt.daemons.keys() {
        if !state.daemons.contains_key(id) {
            let id_str = id.to_string();
            let safe_id = css_safe_id(&id_str);
            let url_id = url_encode(&id_str);
            let display_html = format_daemon_id_html(id);
            rows.push_str(&format!(
                r##"<tr id="daemon-{safe_id}" class="clickable-row" onclick="window.location.href='/daemons/{url_id}'">
                <td><a href="/daemons/{url_id}" class="daemon-name" onclick="event.stopPropagation()">{display_html}</a></td>
                <td>-</td>
                <td><span class="status available">available</span></td>
                <td>-</td>
                <td>-</td>
                <td>-</td>
                <td></td>
                <td class="actions" onclick="event.stopPropagation()">
                    <button hx-post="/daemons/{url_id}/start" hx-target="#daemon-{safe_id}" hx-swap="outerHTML" class="btn btn-sm btn-primary"><i data-lucide="play" class="icon"></i> Start</button>
                </td>
            </tr>"##
            ));
        }
    }

    if rows.is_empty() {
        rows = r#"<tr><td colspan="8" class="empty">No daemons configured. Add some to pitchfork.toml</td></tr>"#.to_string();
    }

    let html = format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>pitchfork</title>
    <link rel="icon" type="image/x-icon" href="/static/favicon.ico">
    <script src="https://unpkg.com/htmx.org@2.0.4"></script>
    <script src="https://unpkg.com/lucide@0.474.0"></script>
    <link rel="stylesheet" href="/static/style.css">
</head>
<body>
    <nav>
        <a href="/" class="nav-brand"><img src="/static/logo.png" alt="pitchfork" class="logo-icon"> pitchfork</a>
        <div class="nav-links">
            <a href="/" class="active">Dashboard</a>
            <a href="/logs">Logs</a>
            <a href="/config">Config</a>
        </div>
    </nav>
    <main>
        <div class="stats-grid" hx-get="/_stats" hx-trigger="every 5s" hx-swap="innerHTML swap:0.2s settle:0.2s">
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
        
        // Add smooth fade effect for stats updates
        document.body.addEventListener('htmx:beforeSwap', function(evt) {{
            if (evt.detail.target.classList.contains('stats-grid')) {{
                evt.detail.target.style.opacity = '0.7';
            }}
        }});
        
        document.body.addEventListener('htmx:afterSwap', function(evt) {{
            if (evt.detail.target.classList.contains('stats-grid')) {{
                setTimeout(() => {{
                    evt.detail.target.style.opacity = '1';
                }}, 50);
            }}
        }});
    </script>
</body>
</html>"##
    );

    Html(html)
}
