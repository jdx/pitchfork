//! Shared helper functions for web routes.

use crate::daemon::Daemon;
use crate::daemon_id::DaemonId;
use crate::procs::PROCS;

/// HTML-escape a string to prevent XSS attacks.
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// URL-encode a string for use in URLs.
pub fn url_encode(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// Convert daemon ID to a CSS-selector-safe string.
///
/// Encodes special characters that are not valid in CSS selectors.
/// Uses a simple escape scheme: special characters are replaced with `-XX-`
/// where XX is the hex code of the character.
pub fn css_safe_id(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '/' => result.push_str("-2f-"),
            '.' => result.push_str("-2e-"),
            ':' => result.push_str("-3a-"),
            '@' => result.push_str("-40-"),
            '#' => result.push_str("-23-"),
            '[' => result.push_str("-5b-"),
            ']' => result.push_str("-5d-"),
            '(' => result.push_str("-28-"),
            ')' => result.push_str("-29-"),
            '!' => result.push_str("-21-"),
            '$' => result.push_str("-24-"),
            '%' => result.push_str("-25-"),
            '^' => result.push_str("-5e-"),
            '&' => result.push_str("-26-"),
            '*' => result.push_str("-2a-"),
            '+' => result.push_str("-2b-"),
            '=' => result.push_str("-3d-"),
            '|' => result.push_str("-7c-"),
            '\\' => result.push_str("-5c-"),
            '~' => result.push_str("-7e-"),
            '`' => result.push_str("-60-"),
            '<' => result.push_str("-3c-"),
            '>' => result.push_str("-3e-"),
            ',' => result.push_str("-2c-"),
            ' ' => result.push_str("-20-"),
            '"' => result.push_str("-22-"),
            '\'' => result.push_str("-27-"),
            _ => result.push(c),
        }
    }
    result
}

/// Format daemon ID with dim namespace for HTML display.
///
/// Returns HTML with the namespace wrapped in a span with class "daemon-ns" for CSS styling.
pub fn format_daemon_id_html(id: &DaemonId) -> String {
    format!(
        r#"<span class="daemon-ns">{}</span>/{}"#,
        html_escape(id.namespace()),
        html_escape(id.name())
    )
}

/// Generate an HTML table row for a daemon.
///
/// This is used by both the index page and the daemons list page.
pub fn daemon_row(id: &DaemonId, d: &Daemon, is_disabled: bool) -> String {
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
