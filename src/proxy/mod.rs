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

pub mod hostname;
pub mod hosts;
pub mod lan_ip;
pub mod mdns;
pub mod server;
pub mod trust;
pub mod worktree;

/// Lowercased keys that more than one spelling in `keys` maps to.
///
/// Host names are case-insensitive (RFC 4343), so such keys are ambiguous as
/// routing targets no matter which spelling a request uses.
pub(crate) fn ascii_case_collisions<'a>(
    keys: impl Iterator<Item = &'a str>,
) -> std::collections::HashSet<String> {
    let mut seen = std::collections::HashSet::new();
    let mut collisions = std::collections::HashSet::new();
    for key in keys {
        let folded = key.to_ascii_lowercase();
        if !seen.insert(folded.clone()) {
            collisions.insert(folded);
        }
    }
    collisions
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ascii_case_collisions() {
        let none = ascii_case_collisions(["myapp", "other", "third"].into_iter());
        assert!(none.is_empty());

        let folded = ascii_case_collisions(["MyApp", "myapp", "other"].into_iter());
        assert_eq!(folded.len(), 1);
        assert!(folded.contains("myapp"));

        // Identical spellings collide too, not just case-only variants.
        let exact = ascii_case_collisions(["dup", "dup"].into_iter());
        assert!(exact.contains("dup"));

        // Folding is ASCII-only: DNS does not case-fold non-ASCII labels.
        let unicode = ascii_case_collisions(["café", "CAFÉ"].into_iter());
        assert!(unicode.is_empty());
    }
}
