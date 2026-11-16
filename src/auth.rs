//! Authentication and origin checks, matching ttyd's semantics: either a basic
//! credential (`-c`) or a header injected by an authenticating reverse proxy (`-H`).

use axum::http::{HeaderMap, header};

use crate::cli::Config;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    /// Request is allowed. `user` is set when an auth proxy identified the client.
    Allowed { user: Option<String> },
    /// `-c` credential missing or wrong: 401 with a basic challenge.
    Unauthorized,
    /// `-H` auth header missing: 407 with a proxy challenge.
    ProxyUnauthorized,
}

/// Checks a request the way ttyd's HTTP and websocket handlers do.
pub fn check(cfg: &Config, headers: &HeaderMap) -> Auth {
    if let Some(name) = &cfg.auth_header {
        return match header_value(headers, name) {
            Some(value) => Auth::Allowed { user: Some(value) },
            None => Auth::ProxyUnauthorized,
        };
    }
    if let Some(credential) = &cfg.credential {
        let authorized = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Basic "))
            .map(|value| value.trim() == credential)
            .unwrap_or(false);
        return if authorized { Auth::Allowed { user: None } } else { Auth::Unauthorized };
    }
    Auth::Allowed { user: None }
}

/// Header lookup by configured name; header names are case-insensitive.
fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
}

/// `-O`: refuse websocket upgrades whose `Origin` does not match `Host`.
pub fn origin_matches_host(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let Some(host) = headers.get(header::HOST).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let Some(origin_host) = origin_authority(origin) else {
        return false;
    };
    normalize_authority(origin_host).eq_ignore_ascii_case(&normalize_authority(host))
}

/// Extracts `host[:port]` from an absolute URI such as `http://example.com:8080`.
fn origin_authority(origin: &str) -> Option<&str> {
    let rest = origin.split_once("://")?.1;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Drops a default port so `host` and `host:80` compare equal.
fn normalize_authority(authority: &str) -> String {
    // Bracket-aware so IPv6 literals survive.
    let (host, port) = if let Some(end) = authority.rfind(']') {
        match authority[end..].strip_prefix("]:") {
            Some(port) => (&authority[..=end], Some(port)),
            None => (authority, None),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    match port {
        Some("80") | Some("443") | None => host.to_string(),
        Some(port) => format!("{host}:{port}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn config_with_credential() -> Config {
        Config { credential: Some("dXNlcjpwYXNz".into()), ..Config::default() }
    }

    fn config_with_auth_header() -> Config {
        Config { auth_header: Some("X-Forwarded-User".into()), ..Config::default() }
    }

    #[test]
    fn no_auth_configured_allows_everything() {
        assert_eq!(check(&Config::default(), &HeaderMap::new()), Auth::Allowed { user: None });
    }

    #[test]
    fn basic_credential_must_match() {
        let cfg = config_with_credential();
        let mut headers = HeaderMap::new();
        assert_eq!(check(&cfg, &headers), Auth::Unauthorized);
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic nope"));
        assert_eq!(check(&cfg, &headers), Auth::Unauthorized);
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic dXNlcjpwYXNz"));
        assert_eq!(check(&cfg, &headers), Auth::Allowed { user: None });
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer dXNlcjpwYXNz"));
        assert_eq!(check(&cfg, &headers), Auth::Unauthorized);
    }

    #[test]
    fn auth_header_presence_authenticates_and_identifies() {
        let cfg = config_with_auth_header();
        let mut headers = HeaderMap::new();
        assert_eq!(check(&cfg, &headers), Auth::ProxyUnauthorized);
        headers.insert("x-forwarded-user", HeaderValue::from_static("alice"));
        assert_eq!(check(&cfg, &headers), Auth::Allowed { user: Some("alice".into()) });
    }

    #[test]
    fn origin_must_match_host() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static("http://example.com"));
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(origin_matches_host(&headers));

        headers.insert(header::ORIGIN, HeaderValue::from_static("http://example.com:80"));
        assert!(origin_matches_host(&headers));

        headers.insert(header::ORIGIN, HeaderValue::from_static("http://evil.com"));
        assert!(!origin_matches_host(&headers));

        headers.insert(header::ORIGIN, HeaderValue::from_static("http://example.com:8443"));
        assert!(!origin_matches_host(&headers));
    }

    #[test]
    fn origin_check_needs_both_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(!origin_matches_host(&headers));
        headers.remove(header::HOST);
        headers.insert(header::ORIGIN, HeaderValue::from_static("http://example.com"));
        assert!(!origin_matches_host(&headers));
    }

    #[test]
    fn ipv6_authorities_normalize() {
        assert_eq!(normalize_authority("[::1]:8080"), "[::1]:8080");
        assert_eq!(normalize_authority("[::1]:80"), "[::1]");
        assert_eq!(normalize_authority("localhost:443"), "localhost");
        assert_eq!(normalize_authority("localhost"), "localhost");
    }
}
