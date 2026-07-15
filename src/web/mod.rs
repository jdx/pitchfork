mod routes;
mod server;
mod static_files;

pub use server::{serve, serve_api};

use std::sync::OnceLock;

static BASE_PATH: OnceLock<String> = OnceLock::new();

static WEB_PORT: OnceLock<u16> = OnceLock::new();

/// URL of the running web UI, derived from the actual bound address
/// (which may differ from settings due to port bumping).
static WEB_URL: OnceLock<String> = OnceLock::new();

pub fn port() -> Option<u16> {
    WEB_PORT.get().copied()
}

pub fn url() -> Option<String> {
    WEB_URL.get().cloned()
}

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
