use std::collections::{BTreeMap, BTreeSet};

use aether_contracts::USAGE_SERVER_NOW_UNIX_MS_HEADER;

/// Return the case-insensitive field names nominated by all `Connection`
/// header values in a request.  Those fields are hop-by-hop even when their
/// names are application-defined and therefore must not cross to a provider.
pub(crate) fn declared_connection_header_names(
    headers: &http::HeaderMap,
    extra_headers: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let mut values = headers
        .get_all(http::header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    values.extend(
        extra_headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case(http::header::CONNECTION.as_str()))
            .map(|(_, value)| value.as_str()),
    );
    aether_http::connection_declared_header_names(values)
}

pub(crate) fn is_declared_connection_header(
    name: &str,
    declared_connection_headers: &BTreeSet<String>,
) -> bool {
    declared_connection_headers.contains(&name.trim().to_ascii_lowercase())
}

const UPSTREAM_CREDENTIAL_HEADER_NAMES: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "api-key",
    "x-api-key",
    "x-goog-api-key",
    "cookie",
    "cookie2",
    "set-cookie",
];

pub(crate) fn upstream_credential_header_names() -> &'static [&'static str] {
    UPSTREAM_CREDENTIAL_HEADER_NAMES
}

pub(crate) fn is_upstream_credential_header(name: &str) -> bool {
    let name = name.trim();
    UPSTREAM_CREDENTIAL_HEADER_NAMES
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

pub(crate) fn is_aether_internal_header(name: &str) -> bool {
    name.trim().to_ascii_lowercase().starts_with("x-aether-")
}

pub fn should_skip_request_header(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    if is_aether_internal_header(&normalized)
        || is_untrusted_forwarding_metadata_header(&normalized)
        || is_untrusted_routing_override_header(&normalized)
    {
        return true;
    }
    matches!(
        normalized.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "set-cookie"
            | USAGE_SERVER_NOW_UNIX_MS_HEADER
    )
}

fn is_untrusted_forwarding_metadata_header(normalized_name: &str) -> bool {
    matches!(normalized_name, "forwarded" | "via")
        || normalized_name.starts_with("x-forwarded-")
        || normalized_name.starts_with("x_forwarded_")
        || normalized_name.starts_with("x-real-")
        || normalized_name.starts_with("x_real_")
}

fn is_untrusted_routing_override_header(normalized_name: &str) -> bool {
    matches!(
        normalized_name,
        "x-http-method"
            | "x-http-method-override"
            | "x-method-override"
            | "x-override-url"
            | "x-rewrite-url"
    ) || normalized_name.starts_with("x-original-")
        || normalized_name.starts_with("x_original_")
        || normalized_name.starts_with("x-envoy-original-")
}

pub fn should_skip_upstream_passthrough_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    // Anthropic SDK (stainless) client metadata and Anthropic-specific headers
    // (anthropic-version / anthropic-beta / anthropic-dangerous-direct-browser-access / ...).
    // These are only meaningful when the upstream is Anthropic. The Claude Code
    // adapter re-reads what it needs directly from the original HeaderMap and
    // injects its own values *after* this filter runs, so stripping both prefixes
    // here prevents leakage to any other upstream (OpenAI/Gemini/Codex/...).
    if lower.starts_with("x-stainless-") || lower.starts_with("anthropic-") {
        return true;
    }
    is_upstream_credential_header(&lower)
        || matches!(
            lower.as_str(),
            "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "content-encoding"
            | "x-real-ip"
            | "x-real-proto"
            | "x-forwarded-for"
            | "x-forwarded-proto"
            | "x-forwarded-scheme"
            | "x-forwarded-host"
            | "x-forwarded-port"
            // Claude CLI client identifier; re-injected by the Claude Code adapter
            // when the upstream is Anthropic, filtered for everybody else.
            | "x-app"
        )
        || should_skip_request_header(name)
}

pub(crate) fn should_skip_upstream_passthrough_header_with_connection(
    name: &str,
    declared_connection_headers: &BTreeSet<String>,
) -> bool {
    should_skip_upstream_passthrough_header(name)
        || is_declared_connection_header(name, declared_connection_headers)
}

pub(crate) fn should_skip_upstream_complete_passthrough_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    is_upstream_credential_header(&lower)
        || matches!(
            lower.as_str(),
            "host"
                | "content-length"
                | "transfer-encoding"
                | "connection"
                | "content-encoding"
                | "x-real-ip"
                | "x-real-proto"
                | "x-forwarded-for"
                | "x-forwarded-proto"
                | "x-forwarded-scheme"
                | "x-forwarded-host"
                | "x-forwarded-port"
        )
        || should_skip_request_header(name)
}

pub(crate) fn should_skip_upstream_complete_passthrough_header_with_connection(
    name: &str,
    declared_connection_headers: &BTreeSet<String>,
) -> bool {
    should_skip_upstream_complete_passthrough_header(name)
        || is_declared_connection_header(name, declared_connection_headers)
}

pub(crate) fn remove_declared_connection_headers(
    headers: &mut BTreeMap<String, String>,
    declared_connection_headers: &BTreeSet<String>,
) {
    let mut all_declared = declared_connection_headers.clone();
    let connection_values = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .map(|(_, value)| value.as_str())
        .collect::<Vec<_>>();
    all_declared.extend(aether_http::connection_declared_header_names(
        connection_values,
    ));
    headers.retain(|name, _| {
        !name.eq_ignore_ascii_case("connection")
            && !is_declared_connection_header(name, &all_declared)
    });
}

pub fn normalize_upstream_accept_encoding(value: &str) -> Option<String> {
    let mut accepted = Vec::new();
    let mut wildcard_allowed = false;
    let mut gzip_disabled = false;
    let mut deflate_disabled = false;
    let mut identity_disabled = false;

    for item in value.split(',') {
        let Some((token, normalized_item, enabled)) = parse_accept_encoding_item(item) else {
            continue;
        };
        match token.as_str() {
            "gzip" if enabled => accepted.push(normalized_item),
            "gzip" => gzip_disabled = true,
            "deflate" if enabled => accepted.push(normalized_item),
            "deflate" => deflate_disabled = true,
            "identity" if enabled => accepted.push(normalized_item),
            "identity" => identity_disabled = true,
            "*" if enabled => wildcard_allowed = true,
            _ => {}
        }
    }

    if !accepted.is_empty() {
        return Some(accepted.join(", "));
    }

    if wildcard_allowed && !gzip_disabled {
        Some("gzip".to_string())
    } else if wildcard_allowed && !deflate_disabled {
        Some("deflate".to_string())
    } else if wildcard_allowed && !identity_disabled {
        Some("identity".to_string())
    } else {
        None
    }
}

fn parse_accept_encoding_item(raw_item: &str) -> Option<(String, String, bool)> {
    let mut parts = raw_item.trim().split(';');
    let token = parts.next()?.trim().to_ascii_lowercase();
    if token.is_empty() {
        return None;
    }

    let mut enabled = true;
    let mut normalized = token.clone();
    for raw_param in parts {
        let param = raw_param.trim();
        if param.is_empty() {
            continue;
        }
        let Some((name, value)) = param.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("q") {
            let value = value.trim();
            if q_value_is_zero(value) {
                enabled = false;
                continue;
            }
            normalized.push_str(";q=");
            normalized.push_str(value);
        }
    }

    Some((token, normalized, enabled))
}

fn q_value_is_zero(value: &str) -> bool {
    value
        .trim_matches('"')
        .parse::<f32>()
        .is_ok_and(|q| q <= 0.0)
}

pub fn force_identity_accept_encoding(headers: &mut BTreeMap<String, String>) {
    if let Some(existing_key) = headers
        .keys()
        .find(|key| key.eq_ignore_ascii_case("accept-encoding"))
        .cloned()
    {
        headers.remove(&existing_key);
    }
    headers.insert("accept-encoding".to_string(), "identity".to_string());
}

#[cfg(test)]
mod tests {
    use super::{
        force_identity_accept_encoding, is_upstream_credential_header,
        normalize_upstream_accept_encoding, should_skip_request_header,
        should_skip_upstream_complete_passthrough_header, should_skip_upstream_passthrough_header,
    };
    use aether_contracts::USAGE_SERVER_NOW_UNIX_MS_HEADER;
    use std::collections::BTreeMap;

    #[test]
    fn strips_all_stainless_headers() {
        let stainless = [
            "x-stainless-arch",
            "x-stainless-lang",
            "x-stainless-os",
            "x-stainless-package-version",
            "x-stainless-retry-count",
            "x-stainless-runtime",
            "x-stainless-runtime-version",
            "x-stainless-timeout",
            "x-stainless-helper-method",
            "X-Stainless-Arch",
            "X-STAINLESS-FUTURE-HEADER",
        ];
        for h in stainless {
            assert!(
                should_skip_upstream_passthrough_header(h),
                "should skip {h}"
            );
        }
    }

    #[test]
    fn accept_encoding_is_not_classified_as_hop_by_hop_passthrough_skip() {
        assert!(!should_skip_upstream_passthrough_header("accept-encoding"));
        assert!(!should_skip_upstream_complete_passthrough_header(
            "accept-encoding"
        ));
    }

    #[test]
    fn normalizes_accept_encoding_to_supported_upstream_codecs() {
        assert_eq!(
            normalize_upstream_accept_encoding("gzip, br").as_deref(),
            Some("gzip")
        );
        assert_eq!(
            normalize_upstream_accept_encoding("br, deflate").as_deref(),
            Some("deflate")
        );
        assert_eq!(
            normalize_upstream_accept_encoding("identity").as_deref(),
            Some("identity")
        );
        assert_eq!(
            normalize_upstream_accept_encoding("gzip;q=0.5, br").as_deref(),
            Some("gzip;q=0.5")
        );
        assert_eq!(
            normalize_upstream_accept_encoding("gzip;q=0, br, deflate").as_deref(),
            Some("deflate")
        );
        assert_eq!(
            normalize_upstream_accept_encoding("gzip;q=0, br").as_deref(),
            None
        );
        assert_eq!(
            normalize_upstream_accept_encoding("*").as_deref(),
            Some("gzip")
        );
        assert_eq!(normalize_upstream_accept_encoding("br"), None);
    }

    #[test]
    fn force_identity_accept_encoding_replaces_existing_casing() {
        let mut headers = BTreeMap::from([("Accept-Encoding".to_string(), "gzip".to_string())]);

        force_identity_accept_encoding(&mut headers);

        assert_eq!(
            headers.get("accept-encoding").map(String::as_str),
            Some("identity")
        );
        assert!(!headers.contains_key("Accept-Encoding"));
    }

    #[test]
    fn strips_anthropic_and_claude_cli_identity_headers() {
        let anthropic = [
            "anthropic-version",
            "anthropic-beta",
            "anthropic-dangerous-direct-browser-access",
            "Anthropic-Version",
            "ANTHROPIC-FUTURE-HEADER",
            "x-app",
            "X-App",
        ];
        for h in anthropic {
            assert!(
                should_skip_upstream_passthrough_header(h),
                "should skip {h}"
            );
        }
    }

    #[test]
    fn strips_all_client_credential_carriers_from_passthrough() {
        for header in [
            "authorization",
            "proxy-authorization",
            "api-key",
            "x-api-key",
            "x-goog-api-key",
            "cookie",
            "cookie2",
            "set-cookie",
            "Authorization",
            "COOKIE",
        ] {
            assert!(is_upstream_credential_header(header), "credential {header}");
            assert!(
                should_skip_upstream_passthrough_header(header),
                "normal passthrough should strip {header}"
            );
            assert!(
                should_skip_upstream_complete_passthrough_header(header),
                "complete passthrough should strip {header}"
            );
        }
    }

    #[test]
    fn strips_all_aether_owned_headers_from_provider_requests() {
        for header in [
            "x-aether-gateway",
            "x-aether-auth-user-id",
            "x-aether-auth-api-key-id",
            "x-aether-auth-balance-remaining",
            "X-Aether-Tunnel-Forwarded-By",
        ] {
            assert!(should_skip_request_header(header));
            assert!(should_skip_upstream_passthrough_header(header));
            assert!(should_skip_upstream_complete_passthrough_header(header));
        }
    }

    #[test]
    fn strips_all_client_supplied_forwarding_metadata() {
        for header in [
            "Forwarded",
            "Via",
            "X-Forwarded-For",
            "X-Forwarded-Host",
            "X-Forwarded-Prefix",
            "X-Forwarded-Server",
            "x_forwarded_for",
            "X-Real-IP",
            "X-Real-Host",
            "x_real_ip",
        ] {
            assert!(should_skip_request_header(header));
            assert!(should_skip_upstream_passthrough_header(header));
            assert!(should_skip_upstream_complete_passthrough_header(header));
        }
    }

    #[test]
    fn strips_client_supplied_upstream_routing_overrides() {
        for header in [
            "X-HTTP-Method-Override",
            "X-Method-Override",
            "X-Original-URL",
            "X-Original-URI",
            "x_original_url",
            "X-Rewrite-URL",
            "X-Override-URL",
            "X-Envoy-Original-Path",
        ] {
            assert!(should_skip_request_header(header));
            assert!(should_skip_upstream_passthrough_header(header));
            assert!(should_skip_upstream_complete_passthrough_header(header));
        }
    }

    #[test]
    fn strips_usage_server_time_header_from_provider_requests() {
        for h in [
            USAGE_SERVER_NOW_UNIX_MS_HEADER,
            "X-Aether-Server-Now-Unix-Ms",
        ] {
            assert!(should_skip_request_header(h), "should skip {h}");
            assert!(
                should_skip_upstream_passthrough_header(h),
                "should skip passthrough {h}"
            );
            assert!(
                should_skip_upstream_complete_passthrough_header(h),
                "should skip complete passthrough {h}"
            );
        }
    }

    #[test]
    fn allows_normal_headers_through() {
        let allowed = ["user-agent", "accept", "content-type", "x-custom-header"];
        for h in allowed {
            assert!(
                !should_skip_upstream_passthrough_header(h),
                "should allow {h}"
            );
        }
    }

    #[test]
    fn declared_connection_names_are_case_insensitive_and_multi_line() {
        let mut headers = http::HeaderMap::new();
        headers.append(
            http::header::CONNECTION,
            http::HeaderValue::from_static("X-Hop, keep-alive"),
        );
        headers.append(
            http::header::CONNECTION,
            http::HeaderValue::from_static("x-other-hop"),
        );

        let names = super::declared_connection_header_names(&headers, &BTreeMap::new());
        assert!(names.contains("x-hop"));
        assert!(names.contains("keep-alive"));
        assert!(names.contains("x-other-hop"));
        assert!(super::is_declared_connection_header("X-HOP", &names));
    }
}
