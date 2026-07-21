use std::{
    borrow::Cow,
    collections::BTreeMap,
    fmt,
    io::Read,
    net::{IpAddr, SocketAddr},
    sync::LazyLock,
};

use crate::constants::*;
use axum::body::Bytes;
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use serde_json::{Map, Value};
use uuid::Uuid;

const DEFAULT_MAX_REQUEST_BODY_MB: u64 = 64;
const MAX_REQUEST_BODY_MB_ENV: &str = "AETHER_MAX_REQUEST_BODY_MB";
const DEFAULT_MAX_REDACTED_SYNC_RESPONSE_BODY_MB: u64 = 64;
const MAX_REDACTED_SYNC_RESPONSE_BODY_MB_ENV: &str = "AETHER_MAX_REDACTED_SYNC_RESPONSE_BODY_MB";
const DEFAULT_MAX_INTERNAL_BUFFERED_BODY_MB: u64 = 128;
const MAX_INTERNAL_BUFFERED_BODY_MB_ENV: &str = "AETHER_MAX_INTERNAL_BUFFERED_BODY_MB";
const TRUSTED_PROXY_CIDRS_ENV: &str = "AETHER_TRUSTED_PROXY_CIDRS";

/// Upper bound applied to a request body after Content-Encoding decoding, and to
/// uncompressed bodies as-is. Guards against decompression bombs and oversized
/// request allocations. Overridable via `AETHER_MAX_REQUEST_BODY_MB`.
static MAX_REQUEST_BODY_BYTES: LazyLock<u64> = LazyLock::new(|| {
    std::env::var(MAX_REQUEST_BODY_MB_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_REQUEST_BODY_MB)
        .saturating_mul(1024 * 1024)
});

static MAX_REDACTED_SYNC_RESPONSE_BODY_BYTES: LazyLock<u64> = LazyLock::new(|| {
    std::env::var(MAX_REDACTED_SYNC_RESPONSE_BODY_MB_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_REDACTED_SYNC_RESPONSE_BODY_MB)
        .saturating_mul(1024 * 1024)
});

static MAX_INTERNAL_BUFFERED_BODY_BYTES: LazyLock<u64> = LazyLock::new(|| {
    std::env::var(MAX_INTERNAL_BUFFERED_BODY_MB_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_INTERNAL_BUFFERED_BODY_MB)
        .saturating_mul(1024 * 1024)
});

static TRUSTED_PROXY_CIDRS: LazyLock<Vec<String>> = LazyLock::new(|| {
    std::env::var(TRUSTED_PROXY_CIDRS_ENV)
        .unwrap_or_else(|_| "127.0.0.0/8,::1/128".to_string())
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty() && valid_ip_or_cidr(value))
        .map(ToOwned::to_owned)
        .collect()
});

pub(crate) fn max_request_body_bytes() -> u64 {
    *MAX_REQUEST_BODY_BYTES
}

pub(crate) fn max_redacted_sync_response_body_bytes() -> u64 {
    *MAX_REDACTED_SYNC_RESPONSE_BODY_BYTES
}

pub(crate) fn max_internal_buffered_body_bytes() -> usize {
    usize::try_from(*MAX_INTERNAL_BUFFERED_BODY_BYTES).unwrap_or(usize::MAX)
}

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
    RequestOrigin {
        client_ip: Some(effective_client_ip(headers, remote_addr).to_string()),
        user_agent: header_value_str(headers, http::header::USER_AGENT.as_str())
            .map(|value| truncate_chars(value.as_str(), 1_000)),
    }
}

pub(crate) fn effective_client_ip(headers: &http::HeaderMap, remote_addr: &SocketAddr) -> IpAddr {
    let remote_ip = remote_addr.ip();
    if !trusted_proxy_ip(remote_ip) {
        return remote_ip;
    }

    if let Some(real_ip) =
        header_value_str(headers, "x-real-ip").and_then(|value| value.parse::<IpAddr>().ok())
    {
        return real_ip;
    }

    let forwarded_ips = header_value_str(headers, "x-forwarded-for")
        .map(|value| {
            value
                .split(',')
                .filter_map(|segment| segment.trim().parse::<IpAddr>().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    forwarded_ips
        .iter()
        .rev()
        .copied()
        .find(|ip| !trusted_proxy_ip(*ip))
        .or_else(|| forwarded_ips.first().copied())
        .unwrap_or(remote_ip)
}

fn trusted_proxy_ip(ip: IpAddr) -> bool {
    TRUSTED_PROXY_CIDRS
        .iter()
        .any(|pattern| ip_or_cidr_matches(pattern, ip))
}

fn valid_ip_or_cidr(value: &str) -> bool {
    if value.parse::<IpAddr>().is_ok() {
        return true;
    }
    let Some((network, prefix)) = value.split_once('/') else {
        return false;
    };
    let Ok(network) = network.trim().parse::<IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.trim().parse::<u8>() else {
        return false;
    };
    match network {
        IpAddr::V4(_) => prefix <= 32,
        IpAddr::V6(_) => prefix <= 128,
    }
}

fn ip_or_cidr_matches(pattern: &str, ip: IpAddr) -> bool {
    if let Ok(expected) = pattern.parse::<IpAddr>() {
        return expected == ip;
    }
    let Some((network, prefix)) = pattern.split_once('/') else {
        return false;
    };
    let Ok(prefix) = prefix.trim().parse::<u8>() else {
        return false;
    };
    match (network.trim().parse::<IpAddr>(), ip) {
        (Ok(IpAddr::V4(network)), IpAddr::V4(ip)) if prefix <= 32 => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            (u32::from(network) & mask) == (u32::from(ip) & mask)
        }
        (Ok(IpAddr::V6(network)), IpAddr::V6(ip)) if prefix <= 128 => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            (u128::from(network) & mask) == (u128::from(ip) & mask)
        }
        _ => false,
    }
}

pub(crate) fn request_origin_from_parts(parts: &http::request::Parts) -> RequestOrigin {
    parts
        .extensions
        .get::<RequestOrigin>()
        .cloned()
        .unwrap_or_else(|| request_origin_from_headers(&parts.headers))
}

pub(crate) fn tls_fingerprint_from_headers(headers: &http::HeaderMap) -> Option<Value> {
    let mut object = Map::new();

    copy_tls_header(headers, &mut object, "x-aether-tls-ja3", "ja3");
    copy_tls_header(headers, &mut object, "x-aether-tls-ja3-hash", "ja3_hash");
    copy_tls_header(headers, &mut object, "x-aether-tls-ja4", "ja4");
    copy_tls_header(headers, &mut object, "x-aether-tls-protocol", "protocol");
    copy_tls_header(headers, &mut object, "x-aether-tls-version", "tls_version");
    copy_tls_header(headers, &mut object, "x-aether-tls-cipher", "cipher");
    copy_tls_header(headers, &mut object, "x-aether-tls-sni", "sni");
    copy_tls_header(headers, &mut object, "x-aether-tls-alpn", "alpn");

    if object.is_empty() {
        return None;
    }

    let source = header_value_str(headers, "x-aether-tls-source")
        .unwrap_or_else(|| "forwarded_header".to_string());
    object.insert("source".to_string(), Value::String(source));

    Some(Value::Object(object))
}

fn copy_tls_header(
    headers: &http::HeaderMap,
    object: &mut Map<String, Value>,
    header_name: &str,
    field_name: &str,
) {
    let Some(value) = header_value_str(headers, header_name) else {
        return;
    };
    object.insert(
        field_name.to_string(),
        Value::String(truncate_chars(&value, 512)),
    );
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequestBodyNormalizationError {
    UnsupportedContentEncoding(String),
    DecodeFailed { encoding: String, reason: String },
    DecompressedBodyTooLarge { encoding: String, limit_bytes: u64 },
    RequestBodyTooLarge { limit_bytes: u64 },
}

impl RequestBodyNormalizationError {
    pub(crate) fn client_message(&self) -> String {
        match self {
            Self::UnsupportedContentEncoding(encoding) => {
                format!("Unsupported request Content-Encoding: {encoding}")
            }
            Self::DecodeFailed { encoding, .. } => {
                format!("Failed to decode request body with Content-Encoding: {encoding}")
            }
            Self::DecompressedBodyTooLarge {
                encoding,
                limit_bytes,
            } => format!(
                "Decoded request body with Content-Encoding {encoding} exceeds {limit_bytes} bytes"
            ),
            Self::RequestBodyTooLarge { limit_bytes } => {
                format!("Request body exceeds {limit_bytes} bytes")
            }
        }
    }

    pub(crate) fn http_status(&self) -> http::StatusCode {
        match self {
            Self::DecompressedBodyTooLarge { .. } | Self::RequestBodyTooLarge { .. } => {
                http::StatusCode::PAYLOAD_TOO_LARGE
            }
            Self::UnsupportedContentEncoding(_) | Self::DecodeFailed { .. } => {
                http::StatusCode::BAD_REQUEST
            }
        }
    }
}

impl fmt::Display for RequestBodyNormalizationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedContentEncoding(encoding) => {
                write!(f, "unsupported request Content-Encoding: {encoding}")
            }
            Self::DecodeFailed { encoding, reason } => {
                write!(
                    f,
                    "failed to decode request body with Content-Encoding {encoding}: {reason}"
                )
            }
            Self::DecompressedBodyTooLarge {
                encoding,
                limit_bytes,
            } => write!(
                f,
                "decoded request body with Content-Encoding {encoding} exceeds {limit_bytes} bytes"
            ),
            Self::RequestBodyTooLarge { limit_bytes } => {
                write!(f, "request body exceeds {limit_bytes} bytes")
            }
        }
    }
}

impl std::error::Error for RequestBodyNormalizationError {}

pub(crate) fn normalize_request_body_headers_and_bytes(
    headers: &mut http::HeaderMap,
    body_bytes: Bytes,
) -> Result<Bytes, RequestBodyNormalizationError> {
    normalize_request_body_headers_and_bytes_with_limit(
        headers,
        body_bytes,
        max_request_body_bytes(),
    )
}

pub(crate) fn normalize_request_body_headers_and_bytes_with_limit(
    headers: &mut http::HeaderMap,
    body_bytes: Bytes,
    limit_bytes: u64,
) -> Result<Bytes, RequestBodyNormalizationError> {
    let body_was_encoded = !request_content_encodings(headers).is_empty();
    let decoded = decoded_request_body_bytes_with_limit(headers, body_bytes.as_ref(), limit_bytes)?;
    if !body_was_encoded {
        return Ok(body_bytes);
    }

    headers.remove(http::header::CONTENT_ENCODING);
    headers.remove(http::header::CONTENT_LENGTH);
    Ok(Bytes::from(decoded.into_owned()))
}

/// Rejects a request whose declared `Content-Length` already exceeds the body
/// limit, before the body is buffered into memory. Chunked or length-less
/// requests pass this check and stay bounded by the post-decode guard instead.
pub(crate) fn check_request_content_length(
    headers: &http::HeaderMap,
) -> Result<(), RequestBodyNormalizationError> {
    check_request_content_length_with_limit(headers, max_request_body_bytes())
}

pub(crate) fn check_request_content_length_with_limit(
    headers: &http::HeaderMap,
    limit: u64,
) -> Result<(), RequestBodyNormalizationError> {
    let declared = header_value_str(headers, http::header::CONTENT_LENGTH.as_str())
        .and_then(|value| value.trim().parse::<u64>().ok());
    if declared.is_some_and(|value| value > limit) {
        return Err(RequestBodyNormalizationError::RequestBodyTooLarge { limit_bytes: limit });
    }
    Ok(())
}

pub(crate) fn decoded_request_body_bytes<'a>(
    headers: &http::HeaderMap,
    body_bytes: &'a [u8],
) -> Result<Cow<'a, [u8]>, RequestBodyNormalizationError> {
    decoded_request_body_bytes_with_limit(headers, body_bytes, max_request_body_bytes())
}

pub(crate) fn decoded_request_body_bytes_with_limit<'a>(
    headers: &http::HeaderMap,
    body_bytes: &'a [u8],
    limit: u64,
) -> Result<Cow<'a, [u8]>, RequestBodyNormalizationError> {
    let encodings = request_content_encodings(headers);
    if encodings.is_empty() {
        if body_bytes.len() as u64 > limit {
            return Err(RequestBodyNormalizationError::RequestBodyTooLarge { limit_bytes: limit });
        }
        return Ok(Cow::Borrowed(body_bytes));
    }

    let mut decoded = body_bytes.to_vec();
    for encoding in encodings.iter().rev() {
        decoded = decode_single_request_body_with_limit(encoding, decoded.as_slice(), limit)?;
    }
    Ok(Cow::Owned(decoded))
}

fn request_content_encodings(headers: &http::HeaderMap) -> Vec<String> {
    header_value_str(headers, http::header::CONTENT_ENCODING.as_str())
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_ascii_lowercase)
                .filter(|value| value != "identity")
                .collect()
        })
        .unwrap_or_default()
}

fn decode_single_request_body(
    encoding: &str,
    body_bytes: &[u8],
) -> Result<Vec<u8>, RequestBodyNormalizationError> {
    decode_single_request_body_with_limit(encoding, body_bytes, max_request_body_bytes())
}

fn decode_single_request_body_with_limit(
    encoding: &str,
    body_bytes: &[u8],
    limit: u64,
) -> Result<Vec<u8>, RequestBodyNormalizationError> {
    match encoding {
        "gzip" | "x-gzip" => decode_gzip_body_with_limit(encoding, body_bytes, limit),
        "deflate" => decode_deflate_body_with_limit(encoding, body_bytes, limit),
        "zstd" => decode_zstd_body_with_limit(encoding, body_bytes, limit),
        _ => Err(RequestBodyNormalizationError::UnsupportedContentEncoding(
            encoding.to_string(),
        )),
    }
}

fn decode_gzip_body(
    encoding: &str,
    body_bytes: &[u8],
) -> Result<Vec<u8>, RequestBodyNormalizationError> {
    decode_gzip_body_with_limit(encoding, body_bytes, max_request_body_bytes())
}

fn decode_gzip_body_with_limit(
    encoding: &str,
    body_bytes: &[u8],
    limit: u64,
) -> Result<Vec<u8>, RequestBodyNormalizationError> {
    let mut decoder = GzDecoder::new(body_bytes);
    read_request_decoder_to_end_with_limit(encoding, &mut decoder, limit)
}

fn decode_deflate_body(
    encoding: &str,
    body_bytes: &[u8],
) -> Result<Vec<u8>, RequestBodyNormalizationError> {
    decode_deflate_body_with_limit(encoding, body_bytes, max_request_body_bytes())
}

fn decode_deflate_body_with_limit(
    encoding: &str,
    body_bytes: &[u8],
    limit: u64,
) -> Result<Vec<u8>, RequestBodyNormalizationError> {
    let mut zlib_decoder = ZlibDecoder::new(body_bytes);
    match read_request_decoder_to_end_with_limit(encoding, &mut zlib_decoder, limit) {
        Ok(decoded) => Ok(decoded),
        Err(err @ RequestBodyNormalizationError::DecompressedBodyTooLarge { .. }) => Err(err),
        Err(zlib_error) => {
            let mut raw_decoder = DeflateDecoder::new(body_bytes);
            read_request_decoder_to_end_with_limit(encoding, &mut raw_decoder, limit).map_err(
                |raw_error| RequestBodyNormalizationError::DecodeFailed {
                    encoding: encoding.to_string(),
                    reason: format!("{zlib_error}; raw deflate fallback failed: {raw_error}"),
                },
            )
        }
    }
}

fn decode_zstd_body(
    encoding: &str,
    body_bytes: &[u8],
) -> Result<Vec<u8>, RequestBodyNormalizationError> {
    decode_zstd_body_with_limit(encoding, body_bytes, max_request_body_bytes())
}

fn decode_zstd_body_with_limit(
    encoding: &str,
    body_bytes: &[u8],
    limit: u64,
) -> Result<Vec<u8>, RequestBodyNormalizationError> {
    let mut decoder = zstd::stream::read::Decoder::new(body_bytes).map_err(|err| {
        RequestBodyNormalizationError::DecodeFailed {
            encoding: encoding.to_string(),
            reason: err.to_string(),
        }
    })?;
    read_request_decoder_to_end_with_limit(encoding, &mut decoder, limit)
}

fn read_request_decoder_to_end(
    encoding: &str,
    decoder: &mut impl Read,
) -> Result<Vec<u8>, RequestBodyNormalizationError> {
    read_request_decoder_to_end_with_limit(encoding, decoder, max_request_body_bytes())
}

fn read_request_decoder_to_end_with_limit(
    encoding: &str,
    decoder: &mut impl Read,
    limit: u64,
) -> Result<Vec<u8>, RequestBodyNormalizationError> {
    let mut limited = decoder.take(limit.saturating_add(1));
    let mut out = Vec::new();
    limited
        .read_to_end(&mut out)
        .map_err(|err| RequestBodyNormalizationError::DecodeFailed {
            encoding: encoding.to_string(),
            reason: err.to_string(),
        })?;
    if out.len() as u64 > limit {
        return Err(RequestBodyNormalizationError::DecompressedBodyTooLarge {
            encoding: encoding.to_string(),
            limit_bytes: limit,
        });
    }
    Ok(out)
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
        decoded_request_body_bytes, effective_client_ip, normalize_request_body_headers_and_bytes,
        request_origin_from_headers, request_origin_from_headers_and_remote_addr,
        tls_fingerprint_from_headers, RequestBodyNormalizationError, RequestOrigin,
    };
    use flate2::{
        write::{DeflateEncoder, GzEncoder, ZlibEncoder},
        Compression,
    };
    use http::{HeaderMap, HeaderValue};
    use serde_json::json;
    use std::{
        io::Write,
        net::{IpAddr, Ipv4Addr, SocketAddr},
    };

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
    fn effective_client_ip_ignores_forwarded_headers_from_untrusted_peers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.4"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.8"));
        let remote_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)), 443);

        assert_eq!(
            effective_client_ip(&headers, &remote_addr),
            remote_addr.ip()
        );
    }

    #[test]
    fn effective_client_ip_accepts_real_ip_from_trusted_loopback_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.4"));
        let remote_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 443);

        assert_eq!(
            effective_client_ip(&headers, &remote_addr),
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 4))
        );
    }

    #[test]
    fn effective_client_ip_walks_forwarded_chain_from_trusted_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.8, 127.0.0.2"),
        );
        let remote_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 443);

        assert_eq!(
            effective_client_ip(&headers, &remote_addr),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 8))
        );
    }

    #[test]
    fn decoded_request_body_bytes_decodes_zstd() {
        let payload = br#"{"model":"gpt-5.4"}"#;
        let encoded =
            zstd::stream::encode_all(payload.as_slice(), 0).expect("zstd body should encode");
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_static("zstd"),
        );

        let decoded =
            decoded_request_body_bytes(&headers, encoded.as_slice()).expect("body should decode");

        assert_eq!(decoded.as_ref(), payload);
    }

    #[test]
    fn decoded_request_body_bytes_decodes_x_gzip() {
        let payload = br#"{"model":"gpt-5.4"}"#;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).expect("gzip body should write");
        let encoded = encoder.finish().expect("gzip body should finish");
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_static("x-gzip"),
        );

        let decoded =
            decoded_request_body_bytes(&headers, encoded.as_slice()).expect("body should decode");

        assert_eq!(decoded.as_ref(), payload);
    }

    #[test]
    fn decoded_request_body_bytes_decodes_zlib_wrapped_deflate() {
        let payload = br#"{"model":"gpt-5.4"}"#;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(payload)
            .expect("deflate body should write");
        let encoded = encoder.finish().expect("deflate body should finish");
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_static("deflate"),
        );

        let decoded =
            decoded_request_body_bytes(&headers, encoded.as_slice()).expect("body should decode");

        assert_eq!(decoded.as_ref(), payload);
    }

    #[test]
    fn decoded_request_body_bytes_decodes_raw_deflate_fallback() {
        let payload = br#"{"model":"gpt-5.4"}"#;
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(payload)
            .expect("deflate body should write");
        let encoded = encoder.finish().expect("deflate body should finish");
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_static("deflate"),
        );

        let decoded =
            decoded_request_body_bytes(&headers, encoded.as_slice()).expect("body should decode");

        assert_eq!(decoded.as_ref(), payload);
    }

    #[test]
    fn decoded_request_body_bytes_decodes_multiple_chained_encodings() {
        let payload = br#"{"model":"gpt-5.4"}"#;
        let mut gzip_encoder = GzEncoder::new(Vec::new(), Compression::default());
        gzip_encoder
            .write_all(payload)
            .expect("gzip body should write");
        let gzipped = gzip_encoder.finish().expect("gzip body should finish");
        let encoded =
            zstd::stream::encode_all(gzipped.as_slice(), 0).expect("zstd body should encode");
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_static("gzip, zstd"),
        );

        let decoded =
            decoded_request_body_bytes(&headers, encoded.as_slice()).expect("body should decode");

        assert_eq!(decoded.as_ref(), payload);
    }

    #[test]
    fn decoded_request_body_bytes_rejects_corrupt_encoded_body() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_static("zstd"),
        );

        let err = decoded_request_body_bytes(&headers, br#"{"model":"gpt-5.4"}"#.as_slice())
            .expect_err("corrupt body should fail");

        assert!(matches!(
            err,
            RequestBodyNormalizationError::DecodeFailed { .. }
        ));
    }

    #[test]
    fn normalize_request_body_headers_and_bytes_clears_encoding_headers() {
        let payload = br#"{"model":"gpt-5.4"}"#;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).expect("gzip body should write");
        let encoded = encoder.finish().expect("gzip body should finish");
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_static("x-gzip"),
        );
        headers.insert(
            http::header::CONTENT_LENGTH,
            HeaderValue::from_static("999"),
        );

        let decoded = normalize_request_body_headers_and_bytes(
            &mut headers,
            axum::body::Bytes::from(encoded),
        )
        .expect("body should normalize");

        assert_eq!(decoded.as_ref(), payload);
        assert!(!headers.contains_key(http::header::CONTENT_ENCODING));
        assert!(!headers.contains_key(http::header::CONTENT_LENGTH));
    }

    #[test]
    fn decoded_request_body_bytes_rejects_unsupported_encoding() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_static("br"),
        );

        let err = decoded_request_body_bytes(&headers, br#"{"model":"gpt-5.4"}"#.as_slice())
            .expect_err("unsupported encoding should fail");

        assert_eq!(
            err,
            RequestBodyNormalizationError::UnsupportedContentEncoding("br".to_string())
        );
    }

    #[test]
    fn decoded_request_body_bytes_rejects_oversized_uncompressed_body() {
        let limit = *super::MAX_REQUEST_BODY_BYTES;
        let oversized = vec![b'a'; limit as usize + 1];
        let headers = HeaderMap::new();

        let err = decoded_request_body_bytes(&headers, oversized.as_slice())
            .expect_err("oversized uncompressed body should fail");

        assert_eq!(
            err,
            RequestBodyNormalizationError::RequestBodyTooLarge { limit_bytes: limit }
        );
    }

    #[test]
    fn check_request_content_length_rejects_oversized_declared_length() {
        let limit = *super::MAX_REQUEST_BODY_BYTES;
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_LENGTH,
            HeaderValue::from_str(&(limit + 1).to_string()).expect("length header should build"),
        );

        let err = super::check_request_content_length(&headers)
            .expect_err("oversized declared length should fail");

        assert_eq!(
            err,
            RequestBodyNormalizationError::RequestBodyTooLarge { limit_bytes: limit }
        );
    }

    #[test]
    fn request_body_normalization_error_maps_http_status() {
        assert_eq!(
            RequestBodyNormalizationError::RequestBodyTooLarge { limit_bytes: 1 }.http_status(),
            http::StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            RequestBodyNormalizationError::DecompressedBodyTooLarge {
                encoding: "zstd".to_string(),
                limit_bytes: 1,
            }
            .http_status(),
            http::StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            RequestBodyNormalizationError::UnsupportedContentEncoding("br".to_string())
                .http_status(),
            http::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            RequestBodyNormalizationError::DecodeFailed {
                encoding: "gzip".to_string(),
                reason: "bad".to_string(),
            }
            .http_status(),
            http::StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn check_request_content_length_allows_missing_or_within_limit() {
        let empty = HeaderMap::new();
        assert!(super::check_request_content_length(&empty).is_ok());

        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_LENGTH,
            HeaderValue::from_static("1024"),
        );
        assert!(super::check_request_content_length(&headers).is_ok());
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

    #[test]
    fn tls_fingerprint_from_headers_collects_forwarded_tls_fields() {
        let mut headers = HeaderMap::new();
        headers.insert("x-aether-tls-ja3", HeaderValue::from_static("ja3-value"));
        headers.insert(
            "x-aether-tls-ja3-hash",
            HeaderValue::from_static("ja3-hash"),
        );
        headers.insert("x-aether-tls-ja4", HeaderValue::from_static("ja4-value"));
        headers.insert("x-aether-tls-protocol", HeaderValue::from_static("TLSv1.3"));
        headers.insert(
            "x-aether-tls-cipher",
            HeaderValue::from_static("TLS_AES_128_GCM_SHA256"),
        );
        headers.insert(
            "x-aether-tls-sni",
            HeaderValue::from_static("api.example.com"),
        );
        headers.insert("x-aether-tls-alpn", HeaderValue::from_static("h2"));
        headers.insert("x-aether-tls-source", HeaderValue::from_static("nginx"));

        assert_eq!(
            tls_fingerprint_from_headers(&headers),
            Some(json!({
                "source": "nginx",
                "ja3": "ja3-value",
                "ja3_hash": "ja3-hash",
                "ja4": "ja4-value",
                "protocol": "TLSv1.3",
                "cipher": "TLS_AES_128_GCM_SHA256",
                "sni": "api.example.com",
                "alpn": "h2"
            }))
        );
    }
}
