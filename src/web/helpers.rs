//! Shared helper functions for web routes.

use crate::daemon_id::DaemonId;

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
