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

pub mod activity;
pub mod dns;
pub mod doctor;
pub mod hostname;
pub mod hosts;
pub mod lan_ip;
pub mod mdns;
pub mod pac;
pub mod server;
pub mod setup;
pub mod sni;
pub mod trust;
pub mod worktree;

/// Rate limiter for a log line that an outside party can trigger at will.
///
/// The proxy and the resolver both refuse work under load, and a client can
/// provoke those refusals as fast as it can open sockets. Logging each one
/// hands that client a way to fill the disk, so the message is emitted at most
/// once per interval and carries the number suppressed since.
pub(crate) struct LogThrottle {
    last: std::sync::Mutex<Option<std::time::Instant>>,
    suppressed: std::sync::atomic::AtomicU64,
}

impl LogThrottle {
    pub(crate) const fn new() -> Self {
        Self {
            last: std::sync::Mutex::new(None),
            suppressed: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Whether to log now, and how many were suppressed since the last time.
    ///
    /// `None` means stay quiet. A poisoned lock logs rather than goes silent,
    /// since losing the message entirely is the worse failure.
    pub(crate) fn allow(&self, every: std::time::Duration) -> Option<u64> {
        use std::sync::atomic::Ordering;
        let now = std::time::Instant::now();
        let mut last = match self.last.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        match *last {
            Some(t) if now.duration_since(t) < every => {
                self.suppressed.fetch_add(1, Ordering::Relaxed);
                None
            }
            _ => {
                *last = Some(now);
                Some(self.suppressed.swap(0, Ordering::Relaxed))
            }
        }
    }
}

/// Whether `name` is the TLD itself or a name beneath it.
///
/// One definition shared by the DNS responder, which uses it to decide what it
/// is authoritative for, and the certificate resolver, which uses it to decide
/// what the local CA is allowed to sign. Those two answers must agree: a name
/// the proxy will not resolve is a name it must not issue a certificate for.
///
/// Comparison is ASCII case-insensitive, per RFC 4343, and a trailing root dot
/// is ignored.
pub(crate) fn owns_name(tld: &str, name: &str) -> bool {
    let name = name.trim_end_matches('.');
    let tld = tld.trim_matches('.');
    if tld.is_empty() || name.is_empty() {
        return false;
    }
    if name.eq_ignore_ascii_case(tld) {
        return true;
    }
    // Byte comparison: a DNS label may hold non-UTF-8 data, so slicing a
    // lossily-decoded string could land mid-character.
    let (name, tld) = (name.as_bytes(), tld.as_bytes());
    name.len() > tld.len() + 1
        && name[name.len() - tld.len() - 1] == b'.'
        && name[name.len() - tld.len()..].eq_ignore_ascii_case(tld)
}

/// Whether `name` sits strictly beneath `tld`, rather than being the TLD itself.
///
/// Used for the sibling wildcard on a minted certificate: `*.<tld>` would cover
/// the entire TLD, which is broader than the one host the certificate is for.
pub(crate) fn is_strictly_under_tld(tld: &str, name: &str) -> bool {
    !name
        .trim_end_matches('.')
        .eq_ignore_ascii_case(tld.trim_matches('.'))
        && owns_name(tld, name)
}

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
    fn a_throttled_message_reports_what_it_suppressed() {
        use std::time::Duration;
        let throttle = LogThrottle::new();
        // First call goes through, with nothing suppressed yet.
        assert_eq!(throttle.allow(Duration::from_secs(60)), Some(0));
        // Everything inside the window stays quiet.
        for _ in 0..5 {
            assert_eq!(throttle.allow(Duration::from_secs(60)), None);
        }
        // A zero window always allows, and reports the five it swallowed.
        assert_eq!(throttle.allow(Duration::ZERO), Some(5));
        // The count resets after being reported.
        assert_eq!(throttle.allow(Duration::ZERO), Some(0));
    }

    #[test]
    fn owns_name_matches_the_apex_and_names_beneath_it() {
        assert!(owns_name("localhost", "localhost"));
        assert!(owns_name("localhost", "api.localhost"));
        assert!(owns_name("localhost", "core.fix-refs.proj.localhost"));
        assert!(owns_name("localhost", "API.LocalHost"));
        assert!(owns_name("localhost", "api.localhost."));
        assert!(owns_name("dev.internal", "api.dev.internal"));

        assert!(!owns_name("localhost", "example.com"));
        // Ends with the letters but not at a label boundary.
        assert!(!owns_name("localhost", "notlocalhost"));
        assert!(!owns_name("localhost", "localhost.evil.com"));
        assert!(!owns_name("localhost", ""));
        assert!(!owns_name("", "api.localhost"));
    }

    #[test]
    fn is_strictly_under_tld_excludes_the_apex() {
        assert!(is_strictly_under_tld("localhost", "api.localhost"));
        // The apex itself is not "under" the TLD: a wildcard there would cover
        // every name in it.
        assert!(!is_strictly_under_tld("localhost", "localhost"));
        assert!(!is_strictly_under_tld("dev.internal", "dev.internal"));
        assert!(is_strictly_under_tld("dev.internal", "a.dev.internal"));
        assert!(!is_strictly_under_tld("localhost", "example.com"));
    }

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
