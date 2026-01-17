use axum::{extract::Query, response::Html, Form};
use serde::Deserialize;
use std::path::PathBuf;

use crate::pitchfork_toml::PitchforkToml;

fn base_html(title: &str, content: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{title} - Pitchfork</title>
    <script src="https://unpkg.com/htmx.org@2.0.4"></script>
    <link rel="stylesheet" href="/static/style.css">
</head>
<body>
    <nav>
        <a href="/" class="nav-brand">Pitchfork</a>
        <div class="nav-links">
            <a href="/">Dashboard</a>
            <a href="/daemons">Daemons</a>
            <a href="/logs">Logs</a>
            <a href="/config" class="active">Config</a>
        </div>
    </nav>
    <main>
        {content}
    </main>
</body>
</html>"#
    )
}

fn get_allowed_paths() -> Vec<PathBuf> {
    PitchforkToml::list_paths()
}

fn is_path_allowed(path: &PathBuf) -> bool {
    let allowed = get_allowed_paths();
    allowed.iter().any(|p| p == path)
}

pub async fn list() -> Html<String> {
    let paths = get_allowed_paths();

    let mut file_list = String::new();
    for path in paths {
        let exists = path.exists();
        let display = path.display();
        let exists_badge = if exists {
            r#"<span class="badge exists">exists</span>"#
        } else {
            r#"<span class="badge">not created</span>"#
        };

        let path_str = path.to_string_lossy();
        let encoded_path = urlencoding::encode(&path_str);
        file_list.push_str(&format!(
            r#"
            <li>
                <a href="/config/edit?path={encoded_path}">{display}</a>
                {exists_badge}
            </li>
        "#
        ));
    }

    let content = format!(
        r#"
        <h1>Configuration Files</h1>
        <p>Pitchfork loads configuration from these locations (in order of precedence):</p>
        <ul class="config-file-list">
            {file_list}
        </ul>
        <p class="help-text">Later files override earlier ones. Click a file to edit it.</p>
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

    if !is_path_allowed(&path) {
        let content = r#"
            <h1>Error</h1>
            <p class="error">This file path is not allowed.</p>
            <a href="/config" class="btn">Back to Config List</a>
        "#;
        return Html(base_html("Error", content));
    }

    let content_value = if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(c) => html_escape(&c),
            Err(e) => format!("# Error reading file: {}", e),
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
                <a href="/config" class="btn btn-sm">Back</a>
            </div>
        </div>
        <form hx-post="/config/save" hx-target="#save-result">
            <input type="hidden" name="path" value="{encoded_path}">
            <div class="form-group">
                <textarea name="content" id="config-editor" rows="25">{content_value}</textarea>
            </div>
            <div class="form-actions">
                <button type="button" hx-post="/config/validate" hx-include="#config-editor, input[name=path]" hx-target="#validation-result" class="btn">Validate</button>
                <button type="submit" class="btn btn-primary">Save</button>
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
    match toml::from_str::<PitchforkToml>(&form.content) {
        Ok(config) => {
            let daemon_count = config.daemons.len();
            let daemon_names: Vec<&String> = config.daemons.keys().collect();
            Html(format!(
                r#"
                <div class="validation-success">
                    <strong>Valid TOML!</strong>
                    <p>Found {daemon_count} daemon(s): {}</p>
                </div>
            "#,
                daemon_names
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
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

    if !is_path_allowed(&path) {
        return Html(r#"<div class="error">This file path is not allowed.</div>"#.to_string());
    }

    // Validate TOML first
    if let Err(e) = toml::from_str::<PitchforkToml>(&form.content) {
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

    // Create parent directories if needed
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Html(format!(
                    r#"<div class="error">Failed to create directory: {}</div>"#,
                    e
                ));
            }
        }
    }

    // Write the file
    match std::fs::write(&path, &form.content) {
        Ok(_) => Html(format!(
            r#"
            <div class="save-success">
                <strong>Saved successfully!</strong>
                <p>Configuration saved to {}</p>
            </div>
        "#,
            html_escape(&path.display().to_string())
        )),
        Err(e) => Html(format!(
            r#"
            <div class="error">
                <strong>Failed to save</strong>
                <p>{}</p>
            </div>
        "#,
            e
        )),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
