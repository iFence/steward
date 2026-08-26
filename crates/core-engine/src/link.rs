//! Link detection: recognizing when the launcher query is a URL that should
//! be opened in the default browser instead of searched as an app name.
//!
//! Mirrors the built-in calculator: pure, dependency-free detection that the
//! app layer turns into an "Open in browser" command row. Lives here so the
//! recognition rules are unit-testable without any UI.

/// Whether `query` is a URL worth offering as an "open in browser" command,
/// and if so the URL to open.
///
/// Scheme-less addresses are handled without forcing `https://`:
/// - a `host:port` (e.g. `172.20.2.14:1230`) passes through verbatim — such
///   addresses are almost always plain-HTTP intranet servers, so the opener
///   supplies `http://` the way a browser address bar would;
/// - a bare host (a domain, `localhost` or an IPv4 address) gets `https://`
///   prepended.
///
/// Returns `None` for anything else: empty/whitespace input, queries with
/// spaces, unknown schemes, and dotless single words that could be app names.
pub fn try_openable(query: &str) -> Option<String> {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    if let Some((scheme, rest)) = trimmed.split_once("://") {
        if is_web_scheme(scheme) && !rest.is_empty() {
            return Some(trimmed.to_owned());
        }
        return None;
    }
    let (host, has_port) = split_host_port(trimmed);
    if host.is_empty() || !is_host(host) {
        return None;
    }
    if has_port {
        Some(trimmed.to_owned())
    } else {
        Some(format!("https://{host}"))
    }
}

/// `http`/`https`/`ftp`/`ftps` are the schemes the default browser opens;
/// anything else (e.g. `vscode:`) belongs to a specific app, not the browser.
fn is_web_scheme(scheme: &str) -> bool {
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "ftp" | "ftps"
    )
}

/// Split a scheme-less address into its host part and whether a `:port`
/// follows.
fn split_host_port(input: &str) -> (&str, bool) {
    match input.rsplit_once(':') {
        Some((host, port)) if is_valid_port(port) => (host, true),
        _ => (input, false),
    }
}

/// Whether `host` is safe to hand to the browser: an IPv4 address,
/// `localhost`, or a dotted domain. Dotless words are rejected so plain
/// app-name queries stay ordinary searches.
fn is_host(host: &str) -> bool {
    is_ipv4(host) || host.eq_ignore_ascii_case("localhost") || is_domain(host)
}

fn is_valid_port(port: &str) -> bool {
    !port.is_empty() && port.parse::<u16>().is_ok()
}

/// Four decimal octets in 0..=255, e.g. `172.20.2.14`.
fn is_ipv4(host: &str) -> bool {
    let octets: Vec<&str> = host.split('.').collect();
    octets.len() == 4
        && octets.iter().all(|o| {
            !o.is_empty() && o.bytes().all(|b| b.is_ascii_digit()) && o.parse::<u8>().is_ok()
        })
}

/// A dotted hostname: at least one label-separating dot, labels of 1-63
/// alphanumeric or hyphen characters, never starting/ending with a hyphen.
fn is_domain(host: &str) -> bool {
    if !host.contains('.') {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    // A dotted-decimal that is not a valid IPv4 address (e.g. `1.2.3`) is not
    // a hostname — reject it rather than guessing at the intended site.
    if labels
        .iter()
        .all(|label| !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    labels.iter().all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_urls_are_openable() {
        assert_eq!(
            try_openable("https://github.com"),
            Some("https://github.com".into())
        );
        assert_eq!(
            try_openable("http://example.com/path?q=1"),
            Some("http://example.com/path?q=1".into())
        );
        assert_eq!(
            try_openable("HTTPS://GITHUB.COM"),
            Some("HTTPS://GITHUB.COM".into())
        );
        assert_eq!(
            try_openable("ftp://mirror.example.com"),
            Some("ftp://mirror.example.com".into())
        );
        // A path/query containing a port-like colon must not be mangled.
        assert_eq!(
            try_openable("https://example.com:8080/path"),
            Some("https://example.com:8080/path".into())
        );
    }

    #[test]
    fn ipv4_addresses_open_verbatim_or_with_https() {
        // A `host:port` address passes through as typed; the opener adds the
        // http scheme itself (see `open_url`), like a browser address bar.
        assert_eq!(
            try_openable("172.20.2.14:1230"),
            Some("172.20.2.14:1230".into())
        );
        assert_eq!(
            try_openable("127.0.0.1:8080"),
            Some("127.0.0.1:8080".into())
        );
        // A bare address without a port still needs a scheme to open.
        assert_eq!(
            try_openable("192.168.1.1"),
            Some("https://192.168.1.1".into())
        );
    }

    #[test]
    fn bare_domains_and_localhost_are_openable() {
        assert_eq!(
            try_openable("github.com"),
            Some("https://github.com".into())
        );
        // host:port passes through verbatim.
        assert_eq!(
            try_openable("example.com:8443"),
            Some("example.com:8443".into())
        );
        assert_eq!(
            try_openable("localhost:3000"),
            Some("localhost:3000".into())
        );
    }

    #[test]
    fn non_links_stay_ordinary_search() {
        assert_eq!(try_openable(""), None);
        assert_eq!(try_openable("   "), None);
        assert_eq!(try_openable("calc"), None);
        assert_eq!(try_openable("firefox"), None);
        assert_eq!(try_openable("hello world"), None);
        assert_eq!(try_openable("foo://bar"), None);
        assert_eq!(try_openable("https://"), None);
        assert_eq!(try_openable("1.2.3"), None);
        assert_eq!(try_openable("999.1.1.1"), None);
        assert_eq!(try_openable("exa mple.com"), None);
    }

    #[test]
    fn malformed_hosts_are_rejected() {
        assert_eq!(try_openable("-foo.com"), None);
        assert_eq!(try_openable("foo..com"), None);
        assert_eq!(try_openable("example.com:port"), None);
        assert_eq!(try_openable("example.com:99999"), None);
        assert_eq!(try_openable("a"), None);
    }
}
