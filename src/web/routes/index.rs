use axum::response::Html;
use std::collections::HashSet;

use crate::daemon_id::DaemonId;
use crate::env;
use crate::pitchfork_toml::PitchforkToml;
use crate::procs::PROCS;
use crate::state_file::StateFile;
use crate::web::bp;
use crate::web::helpers::{css_safe_id, daemon_row, format_daemon_id_html, url_encode};

fn get_stats() -> crate::Result<(usize, usize, usize, usize)> {
    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    let pt = PitchforkToml::all_merged()?;

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

    Ok((total, running, stopped, errored))
}

pub async fn stats_partial() -> Html<String> {
    let (total, running_count, stopped_count, errored_count) = match get_stats() {
        Ok(stats) => stats,
        Err(e) => {
            return Html(format!(
                r#"<div class="error">Failed to load configuration: {}</div>"#,
                crate::web::helpers::html_escape(&e.to_string())
            ));
        }
    };

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
    let bp = bp();

    // Refresh process info for accurate CPU/memory stats
    PROCS.refresh_processes();

    let state = StateFile::read(&*env::PITCHFORK_STATE_FILE)
        .unwrap_or_else(|_| StateFile::new(env::PITCHFORK_STATE_FILE.clone()));
    let pt = match PitchforkToml::all_merged() {
        Ok(pt) => pt,
        Err(e) => {
            return Html(format!(
                r#"<h1>Error</h1><p class="error">Failed to load configuration: {}</p>"#,
                crate::web::helpers::html_escape(&e.to_string())
            ));
        }
    };

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
                r##"<tr id="daemon-{safe_id}" class="clickable-row" onclick="window.location.href='{bp}/daemons/{url_id}'">
                <td><a href="{bp}/daemons/{url_id}" class="daemon-name" onclick="event.stopPropagation()">{display_html}</a></td>
                <td>-</td>
                <td><span class="status available">available</span></td>
                <td>-</td>
                <td>-</td>
                <td>-</td>
                <td></td>
                <td class="actions" onclick="event.stopPropagation()">
                    <button hx-post="{bp}/daemons/{url_id}/start" hx-target="#daemon-{safe_id}" hx-swap="outerHTML" class="btn btn-sm btn-primary"><i data-lucide="play" class="icon"></i> Start</button>
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
    <link rel="icon" type="image/x-icon" href="{bp}/static/favicon.ico">
    <script src="https://unpkg.com/htmx.org@2.0.4"></script>
    <script src="https://unpkg.com/lucide@0.474.0"></script>
    <link rel="stylesheet" href="{bp}/static/style.css">
</head>
<body>
    <nav>
        <a href="{bp}/" class="nav-brand"><img src="{bp}/static/logo.png" alt="pitchfork" class="logo-icon"> pitchfork</a>
        <div class="nav-links">
            <a href="{bp}/" class="active">Dashboard</a>
            <a href="{bp}/logs">Logs</a>
            <a href="{bp}/config">Config</a>
        </div>
    </nav>
    <main>
        <div class="stats-grid" hx-get="{bp}/_stats" hx-trigger="every 5s" hx-swap="innerHTML swap:0.2s settle:0.2s">
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
            <tbody id="daemon-list" hx-get="{bp}/daemons/_list" hx-trigger="every 5s" hx-swap="innerHTML swap:0.1s settle:0.1s">
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
