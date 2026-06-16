use std::collections::BTreeMap;

use aether_contracts::USAGE_SERVER_NOW_UNIX_MS_HEADER;

pub fn should_skip_request_header(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
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
            | "x-aether-execution-path"
            | "x-aether-dependency-reason"
            | "x-aether-execution-loop-guard"
            | "x-aether-control-execute-fallback"
            | "x-aether-rate-limit-preflight"
            | USAGE_SERVER_NOW_UNIX_MS_HEADER
    )
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
    matches!(
        lower.as_str(),
        "authorization"
            | "x-api-key"
            | "x-goog-api-key"
            | "host"
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
    ) || should_skip_request_header(name)
}

pub(crate) fn should_skip_upstream_complete_passthrough_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "authorization"
            | "x-api-key"
            | "x-goog-api-key"
            | "host"
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
    ) || should_skip_request_header(name)
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
        force_identity_accept_encoding, normalize_upstream_accept_encoding,
        should_skip_request_header, should_skip_upstream_complete_passthrough_header,
        should_skip_upstream_passthrough_header,
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
}
