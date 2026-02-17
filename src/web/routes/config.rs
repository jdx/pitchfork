use axum::{Form, extract::Query, response::Html};
use indexmap::IndexMap;
use serde::Deserialize;
use std::path::PathBuf;

use crate::pitchfork_toml::PitchforkToml;
use crate::web::helpers::html_escape;

/// Simple struct for TOML validation in the web UI
/// This mirrors PitchforkTomlRaw but is only used for syntax validation
#[derive(Deserialize)]
struct ConfigTomlForValidation {
    #[serde(default)]
    daemons: IndexMap<String, toml::Value>,
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
            <a href="/config" class="active">Config</a>
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

fn get_allowed_paths() -> Vec<PathBuf> {
    PitchforkToml::list_paths()
}

/// Canonicalize a path, handling both existing and non-existing files/directories.
/// Walks up the path tree to find an existing ancestor and canonicalizes from there.
fn safe_canonicalize(path: &PathBuf) -> Option<PathBuf> {
    // First try to canonicalize the full path (works for existing files)
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return Some(canonical);
    }

    // For non-existing paths, walk up the tree to find an existing ancestor
    let mut existing_ancestor = path.clone();
    let mut non_existing_parts: Vec<std::ffi::OsString> = Vec::new();

    while !existing_ancestor.exists() {
        if let Some(file_name) = existing_ancestor.file_name() {
            non_existing_parts.push(file_name.to_os_string());
        } else {
            // Reached root without finding existing ancestor
            return None;
        }
        existing_ancestor = existing_ancestor.parent()?.to_path_buf();
    }

    // Canonicalize the existing ancestor
    let canonical_base = std::fs::canonicalize(&existing_ancestor).ok()?;

    // Rebuild the path with non-existing parts
    let mut result = canonical_base;
    for part in non_existing_parts.into_iter().rev() {
        result = result.join(part);
    }

    Some(result)
}

/// Check if a path is in the allowed list and return the canonical path if valid.
/// Returns the canonical path to use for file operations, preventing TOCTOU attacks.
fn validate_path(path: &PathBuf) -> Option<PathBuf> {
    let allowed = get_allowed_paths();

    // Canonicalize the input path
    let canonical_input = safe_canonicalize(path)?;

    // Check against canonicalized allowed paths
    let is_allowed = allowed.iter().any(|allowed_path| {
        safe_canonicalize(allowed_path)
            .map(|canonical_allowed| canonical_allowed == canonical_input)
            .unwrap_or(false)
    });

    if is_allowed {
        Some(canonical_input)
    } else {
        None
    }
}

pub async fn list() -> Html<String> {
    let paths = get_allowed_paths();

    let mut file_list = String::new();
    for path in paths {
        let exists = path.exists();
        let display = html_escape(&path.display().to_string());
        let status_class = if exists { "exists" } else { "not-created" };
        let status_text = if exists { "EXISTS" } else { "NOT CREATED" };

        let path_str = path.to_string_lossy();
        let encoded_path = urlencoding::encode(&path_str);
        file_list.push_str(&format!(
            r#"
            <div class="config-card {status_class}">
                <div class="config-path">{display}</div>
                <div class="config-status">
                    <span class="status-badge {status_class}">{status_text}</span>
                <a href="/config/edit?path={encoded_path}" class="btn btn-sm"><i data-lucide="edit" class="icon"></i> Edit</a>
                </div>
            </div>
        "#
        ));
    }

    let content = format!(
        r#"
        <div class="page-header">
            <div>
                <h1>Configuration Files</h1>
                <p class="subtitle">Pitchfork loads configuration from these locations (in order of precedence)</p>
            </div>
        </div>
        <div class="config-list">
            {file_list}
        </div>
        <div class="help-box">
            <strong>💡 Note:</strong> Later files override earlier ones. Click Edit to modify a configuration file.
        </div>
    "#
    );

    Html(base_html("Configuration", &content))
}

#[derive(Deserialize)]
pub struct EditQuery {
    path: String,
}

pub async fn edit(Query(query): Query<EditQuery>) -> Html<String> {
    let path = PathBuf::from(&query.path);

    // Validate and get canonical path to prevent TOCTOU attacks
    let canonical_path = match validate_path(&path) {
        Some(p) => p,
        None => {
            let content = r#"
                <h1>Error</h1>
                <p class="error">This file path is not allowed.</p>
        <a href="/config" class="btn"><i data-lucide="arrow-left" class="icon"></i> Back to Config List</a>
            "#;
            return Html(base_html("Error", content));
        }
    };

    // Use canonical path for file operations
    let content_value = if canonical_path.exists() {
        match std::fs::read_to_string(&canonical_path) {
            Ok(c) => html_escape(&c),
            Err(e) => format!("# Error reading file: {e}"),
        }
    } else {
        r#"# New pitchfork.toml configuration
# Example:
#
# [daemons.myapp]
# run = "npm start"
# retry = 3
# ready_delay = 5
"#
        .to_string()
    };

    let encoded_path = html_escape(&query.path);
    let display_path = html_escape(&path.display().to_string());

    let content = format!(
        r##"
        <div class="page-header">
            <h1>Edit: {display_path}</h1>
            <div class="header-actions">
            <a href="/config" class="btn btn-sm"><i data-lucide="arrow-left" class="icon"></i> Back</a>
            </div>
        </div>
        <form hx-post="/config/save" hx-target="#save-result">
            <input type="hidden" name="path" value="{encoded_path}">
            <div class="form-group">
                <textarea name="content" id="config-editor" rows="25">{content_value}</textarea>
            </div>
            <div class="form-actions">
            <button type="button" hx-post="/config/validate" hx-include="#config-editor, input[name=path]" hx-target="#validation-result" class="btn"><i data-lucide="check-circle" class="icon"></i> Validate</button>
            <button type="submit" class="btn btn-primary"><i data-lucide="save" class="icon"></i> Save</button>
            </div>
            <div id="validation-result"></div>
            <div id="save-result"></div>
        </form>
    "##
    );

    Html(base_html(&format!("Edit: {}", path.display()), &content))
}

#[derive(Deserialize)]
pub struct ConfigForm {
    path: String,
    content: String,
}

pub async fn validate(Form(form): Form<ConfigForm>) -> Html<String> {
    match toml::from_str::<ConfigTomlForValidation>(&form.content) {
        Ok(config) => {
            let daemon_count = config.daemons.len();
            let daemon_names: Vec<String> = config.daemons.keys().map(|s| html_escape(s)).collect();
            Html(format!(
                r#"
                <div class="validation-success">
                    <strong>Valid TOML!</strong>
                    <p>Found {daemon_count} daemon(s): {}</p>
                </div>
            "#,
                daemon_names.join(", ")
            ))
        }
        Err(e) => Html(format!(
            r#"
                <div class="validation-error">
                    <strong>Invalid TOML</strong>
                    <pre>{}</pre>
                </div>
            "#,
            html_escape(&e.to_string())
        )),
    }
}

pub async fn save(Form(form): Form<ConfigForm>) -> Html<String> {
    let path = PathBuf::from(&form.path);

    // Validate and get canonical path to prevent TOCTOU attacks
    let canonical_path = match validate_path(&path) {
        Some(p) => p,
        None => {
            return Html(r#"<div class="error">This file path is not allowed.</div>"#.to_string());
        }
    };

    // Validate TOML first
    if let Err(e) = toml::from_str::<ConfigTomlForValidation>(&form.content) {
        return Html(format!(
            r#"
            <div class="validation-error">
                <strong>Cannot save: Invalid TOML</strong>
                <pre>{}</pre>
            </div>
        "#,
            html_escape(&e.to_string())
        ));
    }

    // Create parent directories if needed (using canonical path)
    if let Some(parent) = canonical_path.parent()
        && !parent.exists()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return Html(format!(
            r#"<div class="error">Failed to create directory: {}</div>"#,
            html_escape(&e.to_string())
        ));
    }

    // Write the file using canonical path to prevent symlink attacks
    match std::fs::write(&canonical_path, &form.content) {
        Ok(_) => Html(format!(
            r#"
            <div class="save-success">
                <strong>Saved successfully!</strong>
                <p>Configuration saved to {}</p>
            </div>
        "#,
            html_escape(&canonical_path.display().to_string())
        )),
        Err(e) => Html(format!(
            r#"
            <div class="error">
                <strong>Failed to save</strong>
                <p>{}</p>
            </div>
        "#,
            html_escape(&e.to_string())
        )),
    }
}
