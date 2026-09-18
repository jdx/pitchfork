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

/// The TLD the proxy actually serves on.
///
/// LAN mode forces `.local`, because mDNS publishes names in that domain.
pub fn effective_tld(s: &crate::settings::Settings) -> &str {
    if s.proxy.lan || !s.proxy.lan_ip.is_empty() {
        "local"
    } else {
        &s.proxy.tld
    }
}

/// Build the URL for a proxy hostname: a legacy slug, or an automatic
/// `<daemon>.<worktree>.<project>` host.
///
/// This is the one place a pitchfork URL is spelled out, so everything that
/// shows a URL, injects `PITCHFORK_URL`, or renders a template agrees with what
/// the proxy serves.
///
/// Returns `None` if:
/// - `host` is `None` (not proxied)
/// - Proxy is disabled in settings
/// - `proxy.port` is invalid (out of range or zero)
pub fn build_proxy_url(host: Option<&str>, s: &crate::settings::Settings) -> Option<String> {
    if !s.proxy.enable {
        return None;
    }
    let host = host?;

    let scheme = if s.proxy.https { "https" } else { "http" };
    let tld = effective_tld(s);
    let standard_port = if s.proxy.https { 443u16 } else { 80u16 };

    let effective_port = u16::try_from(s.proxy.port).ok().filter(|&p| p > 0)?;

    let authority = format!("{host}.{tld}");

    Some(if effective_port == standard_port {
        format!("{scheme}://{authority}")
    } else {
        format!("{scheme}://{authority}:{effective_port}")
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

    fn proxy_settings() -> crate::settings::Settings {
        let mut s = crate::settings::Settings::default();
        s.proxy.enable = true;
        s.proxy.https = true;
        s.proxy.port = 443;
        s.proxy.tld = "localhost".to_string();
        s
    }

    /// The standard port is omitted, a custom one is not.
    #[test]
    fn test_build_proxy_url_port_suffix() {
        let mut s = proxy_settings();
        assert_eq!(
            build_proxy_url(Some("api.myproj"), &s).as_deref(),
            Some("https://api.myproj.localhost")
        );
        s.proxy.port = 8088;
        assert_eq!(
            build_proxy_url(Some("api.myproj"), &s).as_deref(),
            Some("https://api.myproj.localhost:8088")
        );
        s.proxy.https = false;
        s.proxy.port = 80;
        assert_eq!(
            build_proxy_url(Some("api.myproj"), &s).as_deref(),
            Some("http://api.myproj.localhost")
        );
    }

    /// Nothing advertises a URL while the proxy is off, and an unrouted daemon
    /// has none either.
    #[test]
    fn test_build_proxy_url_requires_enabled_proxy_and_host() {
        let mut s = proxy_settings();
        assert_eq!(build_proxy_url(None, &s), None);
        s.proxy.enable = false;
        assert_eq!(build_proxy_url(Some("api.myproj"), &s), None);
    }

    /// LAN mode serves `.local`, whatever `proxy.tld` says, so URLs must follow.
    #[test]
    fn test_build_proxy_url_uses_lan_tld() {
        let mut s = proxy_settings();
        s.proxy.tld = "test".to_string();
        assert_eq!(effective_tld(&s), "test");

        s.proxy.lan = true;
        assert_eq!(effective_tld(&s), "local");
        assert_eq!(
            build_proxy_url(Some("api.myproj"), &s).as_deref(),
            Some("https://api.myproj.local")
        );

        s.proxy.lan = false;
        s.proxy.lan_ip = "192.168.1.42".to_string();
        assert_eq!(
            build_proxy_url(Some("api.myproj"), &s).as_deref(),
            Some("https://api.myproj.local")
        );
    }

    /// An out-of-range port has no URL to show rather than a broken one.
    #[test]
    fn test_build_proxy_url_rejects_invalid_port() {
        let mut s = proxy_settings();
        s.proxy.port = 0;
        assert_eq!(build_proxy_url(Some("api.myproj"), &s), None);
        s.proxy.port = 70000;
        assert_eq!(build_proxy_url(Some("api.myproj"), &s), None);
    }
}
