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
pub(crate) fn normalize_base_path(path: Option<&str>) -> String {
    match path {
        None => String::new(),
        Some(p) => {
            let trimmed = p.trim().trim_matches('/');
            if trimmed.is_empty() {
                String::new()
            } else {
                format!("/{trimmed}")
            }
        }
    }
}
