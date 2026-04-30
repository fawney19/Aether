use std::{collections::BTreeMap, net::SocketAddr};

use crate::constants::*;
use uuid::Uuid;

pub(crate) fn extract_or_generate_trace_id(headers: &http::HeaderMap) -> String {
    header_value_str(headers, TRACE_ID_HEADER).unwrap_or_else(|| Uuid::new_v4().to_string())
}

pub(crate) fn header_value_str(headers: &http::HeaderMap, key: &str) -> Option<String> {
    headers
        .get(key)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn header_value_u64(headers: &http::HeaderMap, key: &str) -> Option<u64> {
    header_value_str(headers, key).and_then(|value| value.parse::<u64>().ok())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RequestOrigin {
    pub(crate) client_ip: Option<String>,
    pub(crate) user_agent: Option<String>,
}

pub(crate) fn request_origin_from_headers(headers: &http::HeaderMap) -> RequestOrigin {
    RequestOrigin {
        client_ip: client_ip_from_headers(headers),
        user_agent: header_value_str(headers, http::header::USER_AGENT.as_str())
            .map(|value| truncate_chars(value.as_str(), 1_000)),
    }
}

pub(crate) fn request_origin_from_headers_and_remote_addr(
    headers: &http::HeaderMap,
    remote_addr: &SocketAddr,
) -> RequestOrigin {
    let mut origin = request_origin_from_headers(headers);
    if origin.client_ip.is_none() {
        origin.client_ip = Some(remote_addr.ip().to_string());
    }
    origin
}

pub(crate) fn request_origin_from_parts(parts: &http::request::Parts) -> RequestOrigin {
    parts
        .extensions
        .get::<RequestOrigin>()
        .cloned()
        .unwrap_or_else(|| request_origin_from_headers(&parts.headers))
}

fn client_ip_from_headers(headers: &http::HeaderMap) -> Option<String> {
    header_value_str(headers, "x-forwarded-for")
        .and_then(|value| {
            value
                .split(',')
                .map(str::trim)
                .find(|segment| !segment.is_empty() && !segment.eq_ignore_ascii_case("unknown"))
                .map(|segment| truncate_chars(segment, 45))
        })
        .or_else(|| {
            header_value_str(headers, "x-real-ip").and_then(|value| {
                let value = value.trim();
                (!value.is_empty() && !value.eq_ignore_ascii_case("unknown"))
                    .then(|| truncate_chars(value, 45))
            })
        })
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(crate) fn should_skip_request_header(name: &str) -> bool {
    crate::provider_transport::should_skip_request_header(name)
}

pub(crate) fn should_skip_upstream_passthrough_header(name: &str) -> bool {
    crate::provider_transport::should_skip_upstream_passthrough_header(name)
}

pub(crate) fn should_skip_response_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "x-aether-control-executed"
            | "x-aether-control-action"
    )
}

pub(crate) fn collect_control_headers(headers: &http::HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect()
}

pub(crate) fn is_json_request(headers: &http::HeaderMap) -> bool {
    header_value_str(headers, http::header::CONTENT_TYPE.as_str())
        .map(|value| value.to_ascii_lowercase().contains("application/json"))
        .unwrap_or(false)
}

pub(crate) fn header_equals(
    headers: &reqwest::header::HeaderMap,
    key: &'static str,
    expected: &str,
) -> bool {
    headers
        .get(key)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{
        request_origin_from_headers, request_origin_from_headers_and_remote_addr, RequestOrigin,
    };
    use http::{HeaderMap, HeaderValue};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn request_origin_prefers_first_forwarded_for_ip() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static(" 203.0.113.8, 10.0.0.1 "),
        );
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.4"));
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("Claude-Code/1.0"),
        );

        assert_eq!(
            request_origin_from_headers(&headers),
            RequestOrigin {
                client_ip: Some("203.0.113.8".to_string()),
                user_agent: Some("Claude-Code/1.0".to_string()),
            }
        );
    }

    #[test]
    fn request_origin_uses_real_ip_after_empty_forwarded_for_segments() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static(" , unknown "));
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.4"));

        assert_eq!(
            request_origin_from_headers(&headers).client_ip.as_deref(),
            Some("198.51.100.4")
        );
    }

    #[test]
    fn request_origin_falls_back_to_remote_addr() {
        let headers = HeaderMap::new();
        let remote_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)), 443);

        assert_eq!(
            request_origin_from_headers_and_remote_addr(&headers, &remote_addr)
                .client_ip
                .as_deref(),
            Some("192.0.2.10")
        );
    }
}
