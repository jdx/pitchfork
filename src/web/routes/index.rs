use axum::response::Html;
use std::collections::HashSet;

use crate::env;
use crate::pitchfork_toml::PitchforkToml;
use crate::state_file::StateFile;

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

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Pitchfork Dashboard</title>
    <script src="/static/htmx.min.js"></script>
    <link rel="stylesheet" href="/static/style.css">
</head>
<body>
    <nav>
        <div class="nav-brand">Pitchfork</div>
        <div class="nav-links">
            <a href="/" class="active">Dashboard</a>
            <a href="/daemons">Daemons</a>
            <a href="/logs">Logs</a>
            <a href="/config">Config</a>
        </div>
    </nav>
    <main>
        <h1>Dashboard</h1>
        <div class="stats-grid">
            <div class="stat-card">
                <div class="stat-value">{total}</div>
                <div class="stat-label">Total Daemons</div>
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

        <h2>Quick Actions</h2>
        <div class="actions">
            <a href="/daemons" class="btn">Manage Daemons</a>
            <a href="/logs" class="btn">View Logs</a>
            <a href="/config" class="btn">Edit Config</a>
        </div>
    </main>
</body>
</html>"#
    );

    Html(html)
}
