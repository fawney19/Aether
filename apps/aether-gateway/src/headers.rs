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

const MAX_REQUEST_BODY_MB_ENV: &str = "AETHER_MAX_REQUEST_BODY_MB";
const MAX_REDACTED_SYNC_RESPONSE_BODY_MB_ENV: &str = "AETHER_MAX_REDACTED_SYNC_RESPONSE_BODY_MB";
const MAX_INTERNAL_BUFFERED_BODY_MB_ENV: &str = "AETHER_MAX_INTERNAL_BUFFERED_BODY_MB";
const TRUSTED_PROXY_CIDRS_ENV: &str = "AETHER_TRUSTED_PROXY_CIDRS";
const DEFAULT_MAX_REQUEST_BODY_BYTES: u64 = 256 * 1024 * 1024;
const DEFAULT_MAX_REDACTED_SYNC_RESPONSE_BODY_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_MAX_INTERNAL_BUFFERED_BODY_BYTES: u64 = 64 * 1024 * 1024;
// A finite ceiling remains in force even when an operator uses the historical
// `0`/oversized value to request an effectively unlimited body.  This protects
// direct execution-runtime and internal aggregation paths that do not hold a
// frontdoor body-budget permit.  It does not apply to streaming bodies.
const MAX_CONFIGURED_BUFFERED_BODY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_REQUEST_CONTENT_ENCODINGS: usize = 8;

/// Operator cap applied after Content-Encoding decoding, and to uncompressed
/// bodies as-is. A configured zero disables the optional lower cap, while the
/// finite safety ceiling still prevents an unbounded allocation.
static MAX_REQUEST_BODY_BYTES: LazyLock<u64> = LazyLock::new(|| {
    body_limit_bytes_from_env(MAX_REQUEST_BODY_MB_ENV, DEFAULT_MAX_REQUEST_BODY_BYTES)
});

static MAX_REDACTED_SYNC_RESPONSE_BODY_BYTES: LazyLock<u64> = LazyLock::new(|| {
    body_limit_bytes_from_env(
        MAX_REDACTED_SYNC_RESPONSE_BODY_MB_ENV,
        DEFAULT_MAX_REDACTED_SYNC_RESPONSE_BODY_BYTES,
    )
});

static MAX_INTERNAL_BUFFERED_BODY_BYTES: LazyLock<u64> = LazyLock::new(|| {
    body_limit_bytes_from_env(
        MAX_INTERNAL_BUFFERED_BODY_MB_ENV,
        DEFAULT_MAX_INTERNAL_BUFFERED_BODY_BYTES,
    )
});

fn body_limit_bytes_from_env(name: &str, default_bytes: u64) -> u64 {
    let value = std::env::var(name).ok();
    body_limit_bytes(value.as_deref(), default_bytes)
}

fn body_limit_bytes(value: Option<&str>, default_bytes: u64) -> u64 {
    let configured = match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some("0") => MAX_CONFIGURED_BUFFERED_BODY_BYTES,
        Some(value) => match value.parse::<u64>() {
            Ok(value) if value > 0 => value
                .checked_mul(1024 * 1024)
                .unwrap_or(MAX_CONFIGURED_BUFFERED_BODY_BYTES),
            _ => default_bytes,
        },
        None => default_bytes,
    };
    configured.min(MAX_CONFIGURED_BUFFERED_BODY_BYTES)
}

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
    usize::try_from(*MAX_INTERNAL_BUFFERED_BODY_BYTES)
        .unwrap_or(usize::MAX)
        .min(usize::try_from(MAX_CONFIGURED_BUFFERED_BODY_BYTES).unwrap_or(usize::MAX))
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
    pub(crate) forwarded_headers_trusted: bool,
}

pub(crate) fn request_origin_from_headers(headers: &http::HeaderMap) -> RequestOrigin {
    RequestOrigin {
        client_ip: None,
        user_agent: header_value_str(headers, http::header::USER_AGENT.as_str())
            .map(|value| truncate_chars(value.as_str(), 1_000)),
        forwarded_headers_trusted: false,
    }
}

pub(crate) fn request_origin_from_trusted_headers(headers: &http::HeaderMap) -> RequestOrigin {
    RequestOrigin {
        client_ip: client_ip_from_headers(headers),
        user_agent: header_value_str(headers, http::header::USER_AGENT.as_str())
            .map(|value| truncate_chars(value.as_str(), 1_000)),
        forwarded_headers_trusted: true,
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
        forwarded_headers_trusted: trusted_proxy_ip(remote_addr.ip()),
    }
}

pub(crate) fn effective_client_ip(headers: &http::HeaderMap, remote_addr: &SocketAddr) -> IpAddr {
    let remote_ip = remote_addr.ip();
    if !trusted_proxy_ip(remote_ip) {
        return remote_ip;
    }

    let forwarded_ips = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|segment| segment.trim().parse::<IpAddr>().ok())
        .collect::<Vec<_>>();
    if let Some(client_ip) = forwarded_ips
        .iter()
        .rev()
        .copied()
        .find(|ip| !trusted_proxy_ip(*ip))
    {
        return client_ip;
    }

    let mut real_ip_values = headers.get_all("x-real-ip").iter();
    let real_ip = real_ip_values
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<IpAddr>().ok());
    if real_ip_values.next().is_none() {
        return real_ip.unwrap_or(remote_ip);
    }

    remote_ip
}

pub(crate) fn trusted_proxy_ip(ip: IpAddr) -> bool {
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
    let name = name.to_ascii_lowercase();
    name == "set-cookie"
        || name.starts_with("x-aether-")
        // CORS is a gateway policy. If an upstream can supply these fields,
        // it can opt an otherwise-disallowed browser origin into reading a
        // credentialed gateway response after the CORS middleware declines
        // to add its own headers.
        || name.starts_with("access-control-")
        // Browser security policy is owned by the gateway.  Besides weakening
        // active-content protections, an upstream-controlled report-only
        // policy / Reporting API endpoint can make a browser disclose gateway
        // URLs and diagnostics to an attacker-controlled collector.
        || name.starts_with("content-security-policy")
        // These response headers are interpreted by common reverse proxies
        // and application servers as privileged internal redirects or local
        // file-send instructions. Upstream providers are untrusted at this
        // boundary and must not be able to make the gateway's front proxy
        // fetch an internal URL or disclose a local file.
        || name.starts_with("x-accel-")
        || matches!(
            name.as_str(),
            "accept-ch"
                | "alt-svc"
                | "authentication-info"
                | "connection"
                | "clear-site-data"
                | "content-length"
                | "critical-ch"
                | "keep-alive"
                | "nel"
                | "proxy-authenticate"
                | "proxy-authentication-info"
                | "proxy-authorization"
                | "proxy-connection"
                | "referrer-policy"
                | "refresh"
                | "report-to"
                | "reporting-endpoints"
                | "strict-transport-security"
                | "te"
                | "timing-allow-origin"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "x-content-type-options"
                | "x-httpd-send-file"
                | "x-lighttpd-send-file"
                | "x-litespeed-location"
                | "x-reproxy-url"
                | "x-send-file"
                | "x-sendfile"
                | "x-sendfile2"
        )
}

pub(crate) fn collect_control_headers(headers: &http::HeaderMap) -> BTreeMap<String, String> {
    let connection_declared = aether_http::connection_declared_header_names(
        headers
            .get_all(http::header::CONNECTION)
            .iter()
            .filter_map(|value| value.to_str().ok()),
    );
    headers
        .iter()
        .filter_map(|(name, value)| {
            let normalized = name.as_str().to_ascii_lowercase();
            if normalized == http::header::CONNECTION.as_str()
                || connection_declared.contains(&normalized)
            {
                return None;
            }
            value
                .to_str()
                .ok()
                .map(|value| (normalized, value.trim().to_string()))
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
    InvalidBodyFraming,
    AmbiguousBodyFraming,
    UnsupportedContentEncoding(String),
    DecodeFailed {
        encoding: String,
        reason: String,
    },
    DecompressedBodyTooLarge {
        encoding: String,
        limit_bytes: u64,
    },
    RequestBodyTooLarge {
        limit_bytes: u64,
    },
    BodyBufferOverloaded {
        requested_bytes: usize,
        budget_bytes: usize,
    },
}

impl RequestBodyNormalizationError {
    pub(crate) fn client_message(&self) -> String {
        match self {
            Self::InvalidBodyFraming | Self::AmbiguousBodyFraming => {
                "Invalid request body framing".to_string()
            }
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
            Self::BodyBufferOverloaded { .. } => {
                "Request body buffering capacity is temporarily exhausted".to_string()
            }
        }
    }

    pub(crate) fn http_status(&self) -> http::StatusCode {
        match self {
            Self::InvalidBodyFraming | Self::AmbiguousBodyFraming => http::StatusCode::BAD_REQUEST,
            Self::DecompressedBodyTooLarge { .. } | Self::RequestBodyTooLarge { .. } => {
                http::StatusCode::PAYLOAD_TOO_LARGE
            }
            Self::UnsupportedContentEncoding(_) | Self::DecodeFailed { .. } => {
                http::StatusCode::BAD_REQUEST
            }
            Self::BodyBufferOverloaded { .. } => http::StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

impl fmt::Display for RequestBodyNormalizationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBodyFraming => write!(f, "invalid request body framing"),
            Self::AmbiguousBodyFraming => write!(f, "ambiguous request body framing"),
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
            Self::BodyBufferOverloaded { requested_bytes, budget_bytes } => write!(
                f,
                "request body buffering needs {requested_bytes} bytes of a {budget_bytes} byte budget"
            ),
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
    normalize_request_body_headers_and_bytes_with_budget(
        headers,
        body_bytes,
        limit_bytes,
        &mut |_| Ok(()),
    )
}

pub(crate) fn normalize_request_body_headers_and_bytes_with_budget(
    headers: &mut http::HeaderMap,
    body_bytes: Bytes,
    limit_bytes: u64,
    budget: &mut impl FnMut(usize) -> Result<(), RequestBodyNormalizationError>,
) -> Result<Bytes, RequestBodyNormalizationError> {
    let body_was_encoded = !request_content_encodings(headers).is_empty();
    let decoded =
        decoded_request_body_bytes_with_budget(headers, body_bytes.as_ref(), limit_bytes, budget)?;
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
    validate_request_body_framing(headers)?;
    let declared = declared_request_content_length(headers)?;
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
    decoded_request_body_bytes_with_budget(headers, body_bytes, limit, &mut |_| Ok(()))
}

fn decoded_request_body_bytes_with_budget<'a>(
    headers: &http::HeaderMap,
    body_bytes: &'a [u8],
    limit: u64,
    budget: &mut impl FnMut(usize) -> Result<(), RequestBodyNormalizationError>,
) -> Result<Cow<'a, [u8]>, RequestBodyNormalizationError> {
    validate_request_body_framing(headers)?;
    let encodings = request_content_encodings(headers);
    if encodings.is_empty() {
        if body_bytes.len() as u64 > limit {
            return Err(RequestBodyNormalizationError::RequestBodyTooLarge { limit_bytes: limit });
        }
        return Ok(Cow::Borrowed(body_bytes));
    }

    let mut decoded = Cow::Borrowed(body_bytes);
    for encoding in encodings.iter().rev() {
        let retained_input_bytes = body_bytes.len().saturating_add(match &decoded {
            Cow::Borrowed(_) => 0,
            Cow::Owned(bytes) => bytes.capacity(),
        });
        decoded = Cow::Owned(decode_single_request_body_with_budget(
            encoding,
            decoded.as_ref(),
            limit,
            retained_input_bytes,
            budget,
        )?);
    }
    Ok(decoded)
}

fn request_content_encodings(headers: &http::HeaderMap) -> Vec<String> {
    headers
        .get_all(http::header::CONTENT_ENCODING)
        .iter()
        .flat_map(|value| value.to_str().unwrap_or_default().split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .filter(|value| value != "identity")
        .collect()
}

fn validate_request_body_framing(
    headers: &http::HeaderMap,
) -> Result<(), RequestBodyNormalizationError> {
    let _ = declared_request_content_length(headers)?;
    if headers.contains_key(http::header::CONTENT_LENGTH)
        && headers.contains_key(http::header::TRANSFER_ENCODING)
    {
        return Err(RequestBodyNormalizationError::AmbiguousBodyFraming);
    }
    let mut encoding_count = 0usize;
    if headers
        .get_all(http::header::CONTENT_ENCODING)
        .iter()
        .nth(1)
        .is_some()
    {
        return Err(RequestBodyNormalizationError::AmbiguousBodyFraming);
    }
    for value in headers.get_all(http::header::CONTENT_ENCODING).iter() {
        let value = value
            .to_str()
            .map_err(|_| RequestBodyNormalizationError::InvalidBodyFraming)?;
        for encoding in value.split(',') {
            if encoding.trim().is_empty() {
                return Err(RequestBodyNormalizationError::InvalidBodyFraming);
            }
            encoding_count = encoding_count.saturating_add(1);
            if encoding_count > MAX_REQUEST_CONTENT_ENCODINGS {
                return Err(RequestBodyNormalizationError::InvalidBodyFraming);
            }
        }
    }
    Ok(())
}

fn declared_request_content_length(
    headers: &http::HeaderMap,
) -> Result<Option<u64>, RequestBodyNormalizationError> {
    let mut values = headers.get_all(http::header::CONTENT_LENGTH).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(RequestBodyNormalizationError::AmbiguousBodyFraming);
    }
    let value = value
        .to_str()
        .map_err(|_| RequestBodyNormalizationError::InvalidBodyFraming)?
        .trim();
    if value.is_empty() || value.contains(',') {
        return Err(RequestBodyNormalizationError::AmbiguousBodyFraming);
    }
    value
        .parse::<u64>()
        .map(Some)
        .map_err(|_| RequestBodyNormalizationError::InvalidBodyFraming)
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
    decode_single_request_body_with_budget(
        encoding,
        body_bytes,
        limit,
        body_bytes.len(),
        &mut |_| Ok(()),
    )
}

fn decode_single_request_body_with_budget(
    encoding: &str,
    body_bytes: &[u8],
    limit: u64,
    retained_input_bytes: usize,
    budget: &mut impl FnMut(usize) -> Result<(), RequestBodyNormalizationError>,
) -> Result<Vec<u8>, RequestBodyNormalizationError> {
    match encoding {
        "gzip" | "x-gzip" => {
            let mut decoder = GzDecoder::new(body_bytes);
            read_request_decoder_to_end_with_budget(
                encoding,
                &mut decoder,
                limit,
                retained_input_bytes,
                budget,
            )
        }
        "deflate" => decode_deflate_body_with_budget(
            encoding,
            body_bytes,
            limit,
            retained_input_bytes,
            budget,
        ),
        "zstd" => {
            let mut decoder = zstd::stream::read::Decoder::new(body_bytes).map_err(|err| {
                RequestBodyNormalizationError::DecodeFailed {
                    encoding: encoding.to_string(),
                    reason: err.to_string(),
                }
            })?;
            read_request_decoder_to_end_with_budget(
                encoding,
                &mut decoder,
                limit,
                retained_input_bytes,
                budget,
            )
        }
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
    decode_deflate_body_with_budget(encoding, body_bytes, limit, body_bytes.len(), &mut |_| {
        Ok(())
    })
}

fn decode_deflate_body_with_budget(
    encoding: &str,
    body_bytes: &[u8],
    limit: u64,
    retained_input_bytes: usize,
    budget: &mut impl FnMut(usize) -> Result<(), RequestBodyNormalizationError>,
) -> Result<Vec<u8>, RequestBodyNormalizationError> {
    let mut zlib_decoder = ZlibDecoder::new(body_bytes);
    match read_request_decoder_to_end_with_budget(
        encoding,
        &mut zlib_decoder,
        limit,
        retained_input_bytes,
        budget,
    ) {
        Ok(decoded) => Ok(decoded),
        Err(zlib_error @ RequestBodyNormalizationError::DecodeFailed { .. }) => {
            let mut raw_decoder = DeflateDecoder::new(body_bytes);
            read_request_decoder_to_end_with_budget(
                encoding,
                &mut raw_decoder,
                limit,
                retained_input_bytes,
                budget,
            )
            .map_err(|raw_error| match raw_error {
                RequestBodyNormalizationError::DecodeFailed { .. } => {
                    RequestBodyNormalizationError::DecodeFailed {
                        encoding: encoding.to_string(),
                        reason: format!("{zlib_error}; raw deflate fallback failed: {raw_error}"),
                    }
                }
                error => error,
            })
        }
        Err(error) => Err(error),
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
    read_request_decoder_to_end_with_budget(encoding, decoder, limit, 0, &mut |_| Ok(()))
}

fn read_request_decoder_to_end_with_budget(
    encoding: &str,
    decoder: &mut impl Read,
    limit: u64,
    retained_input_bytes: usize,
    budget: &mut impl FnMut(usize) -> Result<(), RequestBodyNormalizationError>,
) -> Result<Vec<u8>, RequestBodyNormalizationError> {
    let capacity_limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut scratch = [0_u8; 8 * 1024];
    let mut out = Vec::new();
    loop {
        let remaining = limit.saturating_sub(out.len() as u64).saturating_add(1);
        let read_limit = scratch
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let read = decoder.read(&mut scratch[..read_limit]).map_err(|err| {
            RequestBodyNormalizationError::DecodeFailed {
                encoding: encoding.to_string(),
                reason: err.to_string(),
            }
        })?;
        if read == 0 {
            return Ok(out);
        }
        let next_len = out.len().saturating_add(read);
        if next_len as u64 > limit {
            return Err(RequestBodyNormalizationError::DecompressedBodyTooLarge {
                encoding: encoding.to_string(),
                limit_bytes: limit,
            });
        }
        if next_len > out.capacity() {
            let mut capacity = out
                .capacity()
                .saturating_mul(2)
                .max(next_len)
                .min(capacity_limit);
            // The encoded body and previous decoding layer remain alive during growth.
            match budget(retained_input_bytes.saturating_add(capacity)) {
                Ok(()) => {}
                Err(RequestBodyNormalizationError::BodyBufferOverloaded { .. })
                    if capacity > next_len =>
                {
                    // Rejected reservations leave the budget unchanged. Spare capacity
                    // must not reject a body whose actual bytes still fit.
                    capacity = next_len;
                    budget(retained_input_bytes.saturating_add(capacity))?;
                }
                Err(error) => return Err(error),
            }
            out.try_reserve_exact(capacity.saturating_sub(out.len()))
                .map_err(|err| RequestBodyNormalizationError::DecodeFailed {
                    encoding: encoding.to_string(),
                    reason: err.to_string(),
                })?;
            if out.capacity() > capacity {
                budget(retained_input_bytes.saturating_add(out.capacity()))?;
            }
        }
        out.extend_from_slice(&scratch[..read]);
    }
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
        request_origin_from_trusted_headers, should_skip_response_header,
        tls_fingerprint_from_headers, RequestBodyNormalizationError, RequestOrigin,
    };

    #[test]
    fn upstream_response_header_filter_blocks_privileged_server_control_headers() {
        for name in [
            "set-cookie",
            "Set-Cookie",
            "x-aether-control-executed",
            "X-Aether-Future-Control",
            "Access-Control-Allow-Origin",
            "Access-Control-Allow-Credentials",
            "Access-Control-Expose-Headers",
            "Accept-CH",
            "Alt-Svc",
            "Authentication-Info",
            "Content-Security-Policy",
            "Content-Security-Policy-Report-Only",
            "Clear-Site-Data",
            "Content-Length",
            "Critical-CH",
            "NEL",
            "Proxy-Authentication-Info",
            "Referrer-Policy",
            "Refresh",
            "Report-To",
            "Reporting-Endpoints",
            "Strict-Transport-Security",
            "Timing-Allow-Origin",
            "X-Content-Type-Options",
            "X-Accel-Redirect",
            "x-accel-expires",
            "X-Sendfile",
            "X-Sendfile2",
            "X-Send-File",
            "X-HTTPD-Send-File",
            "X-LIGHTTPD-send-file",
            "X-LiteSpeed-Location",
            "X-Reproxy-URL",
        ] {
            assert!(
                should_skip_response_header(name),
                "upstream response header should be blocked: {name}"
            );
        }
        assert!(!should_skip_response_header("content-type"));
        assert!(!should_skip_response_header("x-proxy-timing"));
        // WWW-Authenticate is an end-to-end challenge used by legitimate
        // provider APIs (for example Bearer realm/error challenges). It is not
        // a proxy control header and must remain available to SDK clients.
        assert!(!should_skip_response_header("WWW-Authenticate"));
    }
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
    fn trusted_request_origin_prefers_first_forwarded_for_ip() {
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
            request_origin_from_trusted_headers(&headers),
            RequestOrigin {
                client_ip: Some("203.0.113.8".to_string()),
                user_agent: Some("Claude-Code/1.0".to_string()),
                forwarded_headers_trusted: true,
            }
        );
        assert_eq!(request_origin_from_headers(&headers).client_ip, None);
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
        assert!(
            request_origin_from_headers_and_remote_addr(&headers, &remote_addr)
                .forwarded_headers_trusted
        );
    }

    #[test]
    fn request_origin_does_not_trust_forwarded_metadata_from_public_peer() {
        let headers = HeaderMap::new();
        let remote_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)), 443);

        assert!(
            !request_origin_from_headers_and_remote_addr(&headers, &remote_addr)
                .forwarded_headers_trusted
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
    fn effective_client_ip_does_not_trust_all_trusted_forwarded_chain() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("127.0.0.2, 127.0.0.3"),
        );
        let remote_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 443);

        assert_eq!(
            effective_client_ip(&headers, &remote_addr),
            remote_addr.ip(),
            "a chain containing only trusted proxy addresses cannot establish the client IP"
        );
    }

    #[test]
    fn effective_client_ip_prefers_forwarded_chain_over_conflicting_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.8, 127.0.0.2"),
        );
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.4"));
        let remote_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 443);

        assert_eq!(
            effective_client_ip(&headers, &remote_addr),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 8))
        );
    }

    #[test]
    fn effective_client_ip_rejects_ambiguous_real_ip_headers() {
        let mut headers = HeaderMap::new();
        headers.append("x-real-ip", HeaderValue::from_static("198.51.100.4"));
        headers.append("x-real-ip", HeaderValue::from_static("203.0.113.8"));
        let remote_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 443);

        assert_eq!(
            effective_client_ip(&headers, &remote_addr),
            remote_addr.ip()
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
    fn explicit_limit_rejects_oversized_uncompressed_body() {
        let limit = 4;
        let oversized = vec![b'a'; limit as usize + 1];
        let headers = HeaderMap::new();

        let err =
            super::decoded_request_body_bytes_with_limit(&headers, oversized.as_slice(), limit)
                .expect_err("oversized uncompressed body should fail");

        assert_eq!(
            err,
            RequestBodyNormalizationError::RequestBodyTooLarge { limit_bytes: limit }
        );
    }

    #[test]
    fn explicit_limit_rejects_oversized_declared_length() {
        let limit = 4;
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_LENGTH,
            HeaderValue::from_str(&(limit + 1).to_string()).expect("length header should build"),
        );

        let err = super::check_request_content_length_with_limit(&headers, limit)
            .expect_err("oversized declared length should fail");

        assert_eq!(
            err,
            RequestBodyNormalizationError::RequestBodyTooLarge { limit_bytes: limit }
        );
    }

    #[test]
    fn request_body_framing_rejects_duplicate_content_length() {
        let mut headers = HeaderMap::new();
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("4"));
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("4"));

        let err = super::check_request_content_length_with_limit(&headers, 10)
            .expect_err("duplicate Content-Length fields must be rejected");
        assert_eq!(err, RequestBodyNormalizationError::AmbiguousBodyFraming);
    }

    #[test]
    fn request_body_framing_rejects_content_length_and_transfer_encoding() {
        let mut headers = HeaderMap::new();
        headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from_static("4"));
        headers.insert(
            http::header::TRANSFER_ENCODING,
            HeaderValue::from_static("chunked"),
        );

        let err = super::decoded_request_body_bytes_with_limit(&headers, b"body", 10)
            .expect_err("Content-Length and Transfer-Encoding must not be combined");
        assert_eq!(err, RequestBodyNormalizationError::AmbiguousBodyFraming);
    }

    #[test]
    fn request_body_framing_rejects_duplicate_content_encoding_fields() {
        let mut headers = HeaderMap::new();
        headers.append(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_static("gzip"),
        );
        headers.append(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_static("identity"),
        );

        let err = super::decoded_request_body_bytes_with_limit(&headers, b"body", 10)
            .expect_err("duplicate Content-Encoding fields must be rejected");
        assert_eq!(err, RequestBodyNormalizationError::AmbiguousBodyFraming);
    }

    #[test]
    fn body_limits_use_safe_default_and_finite_unlimited_override() {
        let default = 64 * 1024 * 1024;
        assert_eq!(super::body_limit_bytes(None, default), default);
        assert_eq!(super::body_limit_bytes(Some("invalid"), default), default);
        assert_eq!(
            super::body_limit_bytes(Some("0"), default),
            super::MAX_CONFIGURED_BUFFERED_BODY_BYTES
        );
        assert_eq!(
            super::body_limit_bytes(Some("999999999999"), default),
            super::MAX_CONFIGURED_BUFFERED_BODY_BYTES
        );
    }

    #[test]
    fn positive_body_limit_is_converted_from_mibibytes() {
        assert_eq!(
            super::body_limit_bytes(Some(" 8 "), 64 * 1024 * 1024),
            8 * 1024 * 1024
        );
    }

    #[test]
    fn finite_unlimited_limit_rejects_values_above_safety_ceiling() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_LENGTH,
            HeaderValue::from_static("268435457"),
        );
        super::check_request_content_length_with_limit(
            &headers,
            super::MAX_CONFIGURED_BUFFERED_BODY_BYTES,
        )
        .expect_err("body above the finite safety ceiling should be rejected");

        let body = b"body remains bounded by the finite safety ceiling";
        let decoded = super::decoded_request_body_bytes_with_limit(
            &HeaderMap::new(),
            body,
            super::MAX_CONFIGURED_BUFFERED_BODY_BYTES,
        )
        .expect("body within the finite safety ceiling should pass");
        assert_eq!(decoded.as_ref(), body);
    }

    #[test]
    fn request_body_normalization_error_maps_http_status() {
        assert_eq!(
            RequestBodyNormalizationError::BodyBufferOverloaded {
                requested_bytes: 2,
                budget_bytes: 1,
            }
            .http_status(),
            http::StatusCode::SERVICE_UNAVAILABLE
        );
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
    fn budgeted_normalization_accounts_for_encoded_input_and_output_capacity() {
        let payload = vec![b'a'; 150_000];
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&payload).expect("gzip payload");
        let encoded = encoder.finish().expect("gzip finish");
        let encoded_len = encoded.len();
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_static("gzip"),
        );
        let mut reservations = Vec::new();

        let decoded = super::normalize_request_body_headers_and_bytes_with_budget(
            &mut headers,
            encoded.into(),
            256 * 1024,
            &mut |bytes| {
                reservations.push(bytes);
                Ok(())
            },
        )
        .expect("budgeted gzip should decode");

        assert_eq!(decoded.as_ref(), payload.as_slice());
        assert!(!headers.contains_key(http::header::CONTENT_ENCODING));
        assert_eq!(reservations[0], encoded_len + 8 * 1024);
        assert!(reservations.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(reservations.last().copied().unwrap() >= encoded_len + payload.len());
        assert!(reservations.last().copied().unwrap() <= encoded_len + payload.len() * 2);
        assert!(
            reservations.len() <= 8,
            "output growth should remain geometric"
        );
    }

    #[test]
    fn budgeted_normalization_accounts_for_retained_encoding_layers() {
        let payload = b"small chained request body";
        let mut inner = GzEncoder::new(Vec::new(), Compression::default());
        inner.write_all(payload).expect("inner gzip payload");
        let inner = inner.finish().expect("inner gzip finish");
        let mut outer = GzEncoder::new(Vec::new(), Compression::default());
        outer.write_all(&inner).expect("outer gzip payload");
        let encoded = outer.finish().expect("outer gzip finish");
        let encoded_len = encoded.len();
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_static("gzip, gzip"),
        );
        let mut reservations = Vec::new();

        let decoded = super::normalize_request_body_headers_and_bytes_with_budget(
            &mut headers,
            encoded.into(),
            256,
            &mut |bytes| {
                reservations.push(bytes);
                Ok(())
            },
        )
        .expect("chained gzip should decode");

        assert_eq!(decoded.as_ref(), payload);
        assert_eq!(
            reservations,
            vec![
                encoded_len + inner.len(),
                encoded_len + inner.len() + payload.len(),
            ]
        );
    }

    #[test]
    fn budgeted_decoder_stops_before_collecting_when_capacity_is_exhausted() {
        let source = vec![b'a'; 150_000];
        let mut decoder = std::io::Cursor::new(source);
        let error = super::read_request_decoder_to_end_with_budget(
            "test",
            &mut decoder,
            256 * 1024,
            100,
            &mut |requested_bytes| {
                Err(RequestBodyNormalizationError::BodyBufferOverloaded {
                    requested_bytes,
                    budget_bytes: 100,
                })
            },
        )
        .expect_err("budget rejection must stop output growth");

        assert_eq!(decoder.position(), 8 * 1024);
        assert_eq!(
            error,
            RequestBodyNormalizationError::BodyBufferOverloaded {
                requested_bytes: 100 + 8 * 1024,
                budget_bytes: 100,
            }
        );
    }

    #[test]
    fn budgeted_decoder_accepts_actual_output_when_geometric_growth_does_not_fit() {
        let source = vec![b'a'; 100_000];
        let mut decoder = std::io::Cursor::new(&source);
        let retained_input_bytes = 100;
        let budget_bytes = retained_input_bytes + source.len();
        let mut reserved_bytes = retained_input_bytes;
        let mut rejected_growth = false;

        let decoded = super::read_request_decoder_to_end_with_budget(
            "test",
            &mut decoder,
            256 * 1024,
            retained_input_bytes,
            &mut |requested_bytes| {
                if requested_bytes > budget_bytes {
                    rejected_growth = true;
                    return Err(RequestBodyNormalizationError::BodyBufferOverloaded {
                        requested_bytes,
                        budget_bytes,
                    });
                }
                reserved_bytes = reserved_bytes.max(requested_bytes);
                Ok(())
            },
        )
        .expect("actual output within the budget should finish decoding");

        assert!(
            rejected_growth,
            "test must exercise oversized spare capacity"
        );
        assert_eq!(decoded, source);
        assert_eq!(reserved_bytes, budget_bytes);
        assert_eq!(decoded.capacity() + retained_input_bytes, budget_bytes);
    }

    #[test]
    fn budgeted_deflate_preserves_capacity_and_size_rejections() {
        let payload = [b'a'; 128];
        let mut wrapped = ZlibEncoder::new(Vec::new(), Compression::default());
        wrapped.write_all(&payload).expect("zlib payload");
        let wrapped = wrapped.finish().expect("zlib finish");
        let mut raw = DeflateEncoder::new(Vec::new(), Compression::default());
        raw.write_all(&payload).expect("raw deflate payload");
        let raw = raw.finish().expect("raw deflate finish");

        for encoded in [wrapped, raw] {
            let mut headers = HeaderMap::new();
            headers.insert(
                http::header::CONTENT_ENCODING,
                HeaderValue::from_static("deflate"),
            );
            let rejection = RequestBodyNormalizationError::BodyBufferOverloaded {
                requested_bytes: 300,
                budget_bytes: 200,
            };
            let error = super::normalize_request_body_headers_and_bytes_with_budget(
                &mut headers,
                encoded.clone().into(),
                256,
                &mut |_| Err(rejection.clone()),
            )
            .expect_err("capacity rejection must remain an overload error");
            assert_eq!(error, rejection);
            assert!(headers.contains_key(http::header::CONTENT_ENCODING));

            let error = super::normalize_request_body_headers_and_bytes_with_budget(
                &mut headers,
                encoded.into(),
                64,
                &mut |_| Ok(()),
            )
            .expect_err("size rejection must remain a size error");
            assert!(matches!(
                error,
                RequestBodyNormalizationError::DecompressedBodyTooLarge {
                    limit_bytes: 64,
                    ..
                }
            ));
        }
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
    fn trusted_request_origin_uses_real_ip_after_empty_forwarded_for_segments() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static(" , unknown "));
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.4"));

        assert_eq!(
            request_origin_from_trusted_headers(&headers)
                .client_ip
                .as_deref(),
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
