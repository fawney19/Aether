use super::support_auth::{auth_refresh_cookie_name, auth_refresh_cookie_secure};
use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Response};
use std::net::SocketAddr;
use url::Url;

pub(super) fn finalize_refresh_cookie(
    mut response: Response<Body>,
    headers: &HeaderMap,
    host_header: Option<&str>,
    remote_addr: &SocketAddr,
) -> Response<Body> {
    if !response.headers().contains_key(header::SET_COOKIE) {
        return response;
    }
    let cookie_name = auth_refresh_cookie_name();
    let explicit_secure = std::env::var("AUTH_REFRESH_COOKIE_SECURE").ok();
    let public_base_url = std::env::var("AETHER_PUBLIC_BASE_URL")
        .ok()
        .or_else(|| std::env::var("PUBLIC_BASE_URL").ok());
    let secure = refresh_cookie_secure_for_request(
        headers,
        host_header,
        crate::headers::trusted_proxy_ip(remote_addr.ip()),
        explicit_secure.as_deref(),
        public_base_url.as_deref(),
        auth_refresh_cookie_secure(),
    );
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|cookie| rewrite_refresh_cookie(cookie, &cookie_name, secure))
        .collect::<Vec<_>>();
    response.headers_mut().remove(header::SET_COOKIE);
    for cookie in cookies {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

fn refresh_cookie_secure_for_request(
    headers: &HeaderMap,
    host_header: Option<&str>,
    trusted_proxy: bool,
    explicit_secure: Option<&str>,
    public_base_url: Option<&str>,
    fallback_secure: bool,
) -> bool {
    if let Some(value) = explicit_secure {
        return !value.trim().eq_ignore_ascii_case("false");
    }

    let origin = single_header(headers, header::ORIGIN.as_str()).and_then(parse_origin);
    let public_url = public_base_url.and_then(parse_http_url);
    let forwarded_proto = trusted_proxy.then(|| forwarded_proto(headers)).flatten();
    if origin.as_ref().is_some_and(|url| url.scheme() == "https")
        || public_url
            .as_ref()
            .is_some_and(|url| url.scheme() == "https")
        || forwarded_proto == Some("https")
    {
        return true;
    }
    if trusted_proxy && headers.contains_key("x-forwarded-proto") {
        return forwarded_proto != Some("http");
    }
    if public_url
        .as_ref()
        .is_some_and(|url| url.scheme() == "http")
    {
        return false;
    }
    if let (Some(origin), Some(host)) = (origin, host_header) {
        let request_origin = parse_origin(&format!("{}://{host}", origin.scheme()));
        if request_origin.is_some_and(|url| url.origin() == origin.origin()) {
            return false;
        }
    }
    fallback_secure
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?.trim();
    (values.next().is_none() && !value.is_empty()).then_some(value)
}

fn parse_origin(value: &str) -> Option<Url> {
    let url = parse_http_url(value)?;
    (url.path() == "/").then_some(url)
}

fn parse_http_url(value: &str) -> Option<Url> {
    let url = Url::parse(value.trim()).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

fn forwarded_proto(headers: &HeaderMap) -> Option<&'static str> {
    let value = headers
        .get_all("x-forwarded-proto")
        .iter()
        .next_back()?
        .to_str()
        .ok()?
        .rsplit(',')
        .next()?
        .trim();
    if value.eq_ignore_ascii_case("https") {
        Some("https")
    } else if value.eq_ignore_ascii_case("http") {
        Some("http")
    } else {
        None
    }
}

fn rewrite_refresh_cookie(cookie: &HeaderValue, cookie_name: &str, secure: bool) -> HeaderValue {
    let Ok(value) = cookie.to_str() else {
        return cookie.clone();
    };
    let mut attributes = value.split(';').map(str::trim);
    let Some(pair) = attributes.next() else {
        return cookie.clone();
    };
    if pair.split_once('=').map(|(name, _)| name) != Some(cookie_name) {
        return cookie.clone();
    }
    let secure =
        secure || cookie_name.starts_with("__Secure-") || cookie_name.starts_with("__Host-");
    let mut parts = vec![pair.to_string()];
    for attribute in attributes {
        if attribute.eq_ignore_ascii_case("Secure") {
            continue;
        }
        if !secure
            && attribute.split_once('=').is_some_and(|(name, value)| {
                name.trim().eq_ignore_ascii_case("SameSite")
                    && value.trim().eq_ignore_ascii_case("None")
            })
        {
            parts.push("SameSite=Lax".to_string());
        } else {
            parts.push(attribute.to_string());
        }
    }
    if secure {
        parts.push("Secure".to_string());
    }
    let Ok(mut rewritten) = HeaderValue::from_str(&parts.join("; ")) else {
        return cookie.clone();
    };
    rewritten.set_sensitive(cookie.is_sensitive());
    rewritten
}

#[cfg(test)]
mod tests {
    use super::{refresh_cookie_secure_for_request, rewrite_refresh_cookie};
    use axum::http::{header, HeaderMap, HeaderValue};

    fn headers(origin: Option<&str>, forwarded_proto: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(origin) = origin {
            headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        if let Some(proto) = forwarded_proto {
            headers.insert("x-forwarded-proto", HeaderValue::from_str(proto).unwrap());
        }
        headers
    }

    #[test]
    fn refresh_cookie_auto_detects_same_origin_http_and_https() {
        for (origin, host, secure) in [
            ("http://aether.test:8084", "aether.test:8084", false),
            ("http://aether.test", "aether.test:80", false),
            ("http://[2001:db8::1]:8084", "[2001:db8::1]:8084", false),
            ("https://aether.test", "aether.test", true),
        ] {
            assert_eq!(
                refresh_cookie_secure_for_request(
                    &headers(Some(origin), None),
                    Some(host),
                    false,
                    None,
                    None,
                    true,
                ),
                secure,
                "{origin}",
            );
        }
        assert!(refresh_cookie_secure_for_request(
            &headers(Some("https://aether.test"), None),
            Some("aether.test"),
            false,
            None,
            None,
            false,
        ));
    }

    #[test]
    fn refresh_cookie_does_not_infer_http_from_other_or_invalid_origins() {
        for origin in [
            "http://other.test",
            "http://aether.test:8085",
            "null",
            "http://user@aether.test:8084",
            "http://aether.test:8084/path",
            "http://aether.test:8084?query",
            "http://aether.test:8084#fragment",
            "http://aether.test:8084, https://aether.test:8084",
            "file:///tmp/test",
        ] {
            assert!(
                refresh_cookie_secure_for_request(
                    &headers(Some(origin), None),
                    Some("aether.test:8084"),
                    false,
                    None,
                    None,
                    true,
                ),
                "{origin}"
            );
        }
        let mut duplicate = headers(Some("http://aether.test:8084"), None);
        duplicate.append(
            header::ORIGIN,
            HeaderValue::from_static("https://aether.test:8084"),
        );
        assert!(refresh_cookie_secure_for_request(
            &duplicate,
            Some("aether.test:8084"),
            false,
            None,
            None,
            true,
        ));
    }

    #[test]
    fn refresh_cookie_only_trusts_forwarded_protocol_from_trusted_peers() {
        for (proto, trusted, secure) in [
            ("http", true, false),
            ("https", true, true),
            ("http", false, true),
            ("https", false, true),
            ("https, http", true, false),
            ("http, https", true, true),
            ("ftp", true, true),
            ("http,", true, true),
        ] {
            assert_eq!(
                refresh_cookie_secure_for_request(
                    &headers(None, Some(proto)),
                    Some("aether.test"),
                    trusted,
                    None,
                    None,
                    true,
                ),
                secure,
                "{proto}, trusted={trusted}"
            );
        }
        let mut chained = headers(None, Some("http, http"));
        chained.append("x-forwarded-proto", HeaderValue::from_static("https"));
        assert!(refresh_cookie_secure_for_request(
            &chained,
            Some("aether.test"),
            true,
            None,
            None,
            true,
        ));
    }

    #[test]
    fn refresh_cookie_https_evidence_prevents_automatic_downgrade() {
        for (origin, proto, public_url) in [
            ("https://aether.test", "http", None),
            ("http://aether.test", "https", None),
            ("http://aether.test", "http", Some("https://aether.test")),
        ] {
            assert!(refresh_cookie_secure_for_request(
                &headers(Some(origin), Some(proto)),
                Some("aether.test"),
                true,
                None,
                public_url,
                true,
            ));
        }
    }

    #[test]
    fn refresh_cookie_preserves_explicit_overrides_and_unknown_defaults() {
        for (explicit, secure) in [
            ("true", true),
            ("FALSE", false),
            ("invalid", true),
            ("", true),
        ] {
            assert_eq!(
                refresh_cookie_secure_for_request(
                    &headers(Some("http://aether.test"), None),
                    Some("aether.test"),
                    false,
                    Some(explicit),
                    None,
                    true,
                ),
                secure
            );
        }
        assert!(!refresh_cookie_secure_for_request(
            &headers(Some("https://aether.test"), None),
            Some("aether.test"),
            false,
            Some("false"),
            None,
            true,
        ));
        for fallback in [false, true] {
            assert_eq!(
                refresh_cookie_secure_for_request(
                    &HeaderMap::new(),
                    Some("aether.test"),
                    false,
                    None,
                    None,
                    fallback,
                ),
                fallback
            );
        }
    }

    #[test]
    fn refresh_cookie_accepts_an_explicit_public_http_origin() {
        assert!(!refresh_cookie_secure_for_request(
            &HeaderMap::new(),
            Some("internal:8084"),
            false,
            None,
            Some("http://aether.test"),
            true,
        ));
        assert!(refresh_cookie_secure_for_request(
            &headers(Some("https://aether.test"), None),
            Some("internal:8084"),
            false,
            None,
            Some("http://aether.test"),
            true,
        ));
    }

    #[test]
    fn refresh_cookie_rewrite_preserves_secret_path_expiry_and_httponly() {
        let mut cookie = HeaderValue::from_static(
            "aether_refresh_token=secret; Path=/api/auth; HttpOnly; SameSite=None; Max-Age=604800; Secure",
        );
        cookie.set_sensitive(true);
        let rewritten = rewrite_refresh_cookie(&cookie, "aether_refresh_token", false);
        assert_eq!(
            rewritten.to_str().unwrap(),
            "aether_refresh_token=secret; Path=/api/auth; HttpOnly; SameSite=Lax; Max-Age=604800"
        );
        assert!(rewritten.is_sensitive());
        assert_eq!(
            rewrite_refresh_cookie(&cookie, "aether_refresh_token", true),
            cookie
        );
    }

    #[test]
    fn refresh_cookie_rewrite_also_clears_http_cookies() {
        let cookie = HeaderValue::from_static(
            "aether_refresh_token=; Path=/api/auth; HttpOnly; SameSite=None; Max-Age=0; Secure",
        );
        assert_eq!(
            rewrite_refresh_cookie(&cookie, "aether_refresh_token", false)
                .to_str()
                .unwrap(),
            "aether_refresh_token=; Path=/api/auth; HttpOnly; SameSite=Lax; Max-Age=0"
        );
    }

    #[test]
    fn refresh_cookie_rewrite_preserves_other_cookies_and_strict_policy() {
        let unrelated = HeaderValue::from_static("oauth_binding=secret; Path=/; Secure; HttpOnly");
        assert_eq!(
            rewrite_refresh_cookie(&unrelated, "aether_refresh_token", false),
            unrelated
        );
        let strict = HeaderValue::from_static(
            "custom_refresh=secret; Path=/api/auth; HttpOnly; SameSite=Strict",
        );
        assert_eq!(
            rewrite_refresh_cookie(&strict, "custom_refresh", false),
            strict
        );
        assert!(rewrite_refresh_cookie(&strict, "custom_refresh", true)
            .to_str()
            .unwrap()
            .ends_with("; Secure"));
        let prefixed =
            HeaderValue::from_static("__Secure-refresh=secret; HttpOnly; SameSite=None; Secure");
        assert_eq!(
            rewrite_refresh_cookie(&prefixed, "__Secure-refresh", false),
            prefixed
        );
    }
}
