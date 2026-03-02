pub mod helpers;
mod routes;
mod server;
mod static_files;

pub use server::serve;

use std::sync::OnceLock;

static BASE_PATH: OnceLock<String> = OnceLock::new();

/// Returns the base path prefix for all web routes (e.g. "" or "/ps").
pub(crate) fn bp() -> &'static str {
    BASE_PATH.get().map(|s| s.as_str()).unwrap_or("")
}

/// Normalize a user-provided base path into "/prefix" form (no trailing slash).
/// Handles inputs like "ps", "/ps", "/ps/", etc.
/// Returns an error if the path contains characters other than alphanumeric, hyphens, or underscores.
pub(crate) fn normalize_base_path(path: Option<&str>) -> crate::Result<String> {
    match path {
        None => Ok(String::new()),
        Some(p) => {
            let trimmed = p.trim().trim_matches('/');
            if trimmed.is_empty() {
                Ok(String::new())
            } else if !trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                Err(miette::miette!(
                    "PITCHFORK_WEB_PATH must contain only alphanumeric characters, hyphens, or underscores, got: {trimmed:?}"
                ))
            } else {
                Ok(format!("/{trimmed}"))
            }
        }
    }
}
