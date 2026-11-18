//! A slug's public URL: the address an upstream tunnel or reverse proxy serves
//! the slug under, such as `https://dev.example.com`.
//!
//! The proxy only ever sees the slug's local hostname, so a backend behind a
//! tunnel would otherwise build redirects, absolute links and CSRF origin
//! checks from `<slug>.<tld>` over plain HTTP. With a public URL set, requests
//! for the slug carry that URL's host, scheme and port instead.

use axum::http::Uri;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicUrl {
    /// Host, plus the port when it is not the scheme's default, as a `Host`
    /// header carries it: `dev.example.com` or `dev.example.com:8443`.
    pub authority: String,
    /// `http` or `https`.
    pub proto: &'static str,
    /// The port, the scheme's default when the URL names none.
    pub port: u16,
}

impl PublicUrl {
    pub fn parse(value: &str) -> Result<Self, String> {
        let uri: Uri = value
            .parse()
            .map_err(|e| format!("'{value}' is not a valid URL: {e}"))?;
        let (proto, default_port) = match uri.scheme_str() {
            Some(s) if s.eq_ignore_ascii_case("https") => ("https", 443),
            Some(s) if s.eq_ignore_ascii_case("http") => ("http", 80),
            _ => return Err(format!("'{value}' must start with http:// or https://")),
        };
        let Some(authority) = uri.authority() else {
            return Err(format!("'{value}' has no host"));
        };
        if authority.as_str().contains('@') {
            return Err(format!("'{value}' must not contain credentials"));
        }
        let host = authority.host().to_ascii_lowercase();
        if host.is_empty() {
            return Err(format!("'{value}' has no host"));
        }
        if let Some(pq) = uri.path_and_query()
            && !matches!(pq.as_str(), "" | "/")
        {
            return Err(format!(
                "'{value}' must be an origin only, without a path or query"
            ));
        }
        // `http` parses an out-of-range port such as `:65536` but cannot report
        // it, so tell an absent port apart from an invalid one.
        let port = match authority.as_str().rsplit_once(':') {
            // An empty suffix (`https://host:`) means the scheme default.
            Some((_, "")) => default_port,
            Some((_, p)) if !p.contains(']') => p
                .parse::<u16>()
                .map_err(|_| format!("'{value}' has an invalid port"))?,
            _ => default_port,
        };
        let authority = if port == default_port {
            host
        } else {
            format!("{host}:{port}")
        };
        Ok(Self {
            authority,
            proto,
            port,
        })
    }

    /// The URL in its canonical form, as stored and displayed.
    pub fn as_url(&self) -> String {
        format!("{}://{}", self.proto, self.authority)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_https_origin_with_the_default_port() {
        let url = PublicUrl::parse("https://Dev.Example.com").unwrap();
        assert_eq!(url.authority, "dev.example.com");
        assert_eq!(url.proto, "https");
        assert_eq!(url.port, 443);
        assert_eq!(url.as_url(), "https://dev.example.com");
    }

    #[test]
    fn accepts_a_trailing_slash() {
        let url = PublicUrl::parse("http://dev.example.com/").unwrap();
        assert_eq!(url.authority, "dev.example.com");
        assert_eq!(url.proto, "http");
        assert_eq!(url.port, 80);
    }

    #[test]
    fn keeps_a_non_default_port_in_the_authority() {
        let url = PublicUrl::parse("https://dev.example.com:8443").unwrap();
        assert_eq!(url.authority, "dev.example.com:8443");
        assert_eq!(url.port, 8443);
    }

    #[test]
    fn keeps_ipv6_hosts_bracketed() {
        let url = PublicUrl::parse("https://[::1]").unwrap();
        assert_eq!(url.authority, "[::1]");
        assert_eq!(url.as_url(), "https://[::1]");
        let url = PublicUrl::parse("https://[::1]:8443").unwrap();
        assert_eq!(url.authority, "[::1]:8443");
        assert_eq!(url.port, 8443);
        assert_eq!(PublicUrl::parse(&url.as_url()).unwrap(), url);
    }

    #[test]
    fn an_empty_port_means_the_default() {
        assert_eq!(
            PublicUrl::parse("https://dev.example.com:").unwrap().port,
            443
        );
        assert_eq!(
            PublicUrl::parse("http://dev.example.com:").unwrap().port,
            80
        );
        let url = PublicUrl::parse("https://[::1]:").unwrap();
        assert_eq!(url.authority, "[::1]");
        assert_eq!(url.port, 443);
    }

    #[test]
    fn drops_an_explicit_default_port() {
        let url = PublicUrl::parse("https://dev.example.com:443").unwrap();
        assert_eq!(url.authority, "dev.example.com");
    }

    #[test]
    fn rejects_anything_but_an_http_origin() {
        for value in [
            "dev.example.com",
            "ftp://dev.example.com",
            "https://dev.example.com/app",
            "https://dev.example.com/?a=1",
            "https://user:pass@dev.example.com",
            "https://dev.example.com:65536",
            "",
        ] {
            assert!(PublicUrl::parse(value).is_err(), "{value}");
        }
    }
}
