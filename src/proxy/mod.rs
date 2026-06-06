//! Reverse proxy server for pitchfork daemons.
//!
//! Routes `<slug>.<tld>:<port>` to the daemon's actual listening port.
//! Slugs are defined in the global config (`~/.config/pitchfork/config.toml`)
//! under `[slugs]`. Each slug maps to a project directory and daemon name.
//!
//! # URL Routing
//!
//! ```text
//! myapp.localhost:7777          →  localhost:8080  (via slug)
//! ```

pub mod hosts;
pub mod lan_ip;
pub mod mdns;
pub mod server;
pub mod trust;
pub mod worktree;

/// Build a proxy URL from an optional slug and settings.
///
/// Returns `None` if:
/// - `slug` is `None` (not proxied)
/// - Proxy is disabled in settings
/// - `proxy.port` is invalid (out of range or zero)
pub fn build_proxy_url(slug: Option<&str>, s: &crate::settings::Settings) -> Option<String> {
    if !s.proxy.enable {
        return None;
    }
    let slug = slug?;

    let scheme = if s.proxy.https { "https" } else { "http" };
    let tld = &s.proxy.tld;
    let standard_port = if s.proxy.https { 443u16 } else { 80u16 };

    let effective_port = u16::try_from(s.proxy.port).ok().filter(|&p| p > 0)?;

    let host = format!("{slug}.{tld}");

    Some(if effective_port == standard_port {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{effective_port}")
    })
}
