use aether_contracts::{ExecutionError, ExecutionErrorKind, ExecutionPhase};
use serde_json::{json, Value};
use tracing::debug;

use crate::execution_runtime::transport::ExecutionRuntimeTransportError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HedgeEligibilityTag {
    pub(crate) source: &'static str,
    pub(crate) reason: &'static str,
}

impl HedgeEligibilityTag {
    const fn new(source: &'static str, reason: &'static str) -> Self {
        Self { source, reason }
    }

    fn to_report_value(self) -> Value {
        json!({
            "tag": "hedge_fast_fail_eligible",
            "eligible": true,
            "source": self.source,
            "reason": self.reason,
        })
    }
}

pub(crate) fn with_hedge_eligibility_report_context(
    report_context: Option<&Value>,
    tag: HedgeEligibilityTag,
) -> Option<Value> {
    let mut object = report_context?.as_object()?.clone();
    debug!(
        event_name = "hedge_eligible_fast_fail",
        log_type = "event",
        hedge_source = tag.source,
        hedge_reason = tag.reason,
        "gateway marked candidate fast failure as hedge eligible"
    );
    let tag_value = tag.to_report_value();
    object.insert("hedge_eligible".to_string(), Value::Bool(true));
    object.insert("hedge_eligibility".to_string(), tag_value.clone());
    if let Some(Value::Object(error_flow)) = object.get_mut("error_flow") {
        error_flow.insert("hedge_eligible".to_string(), Value::Bool(true));
        error_flow.insert("hedge_eligibility".to_string(), tag_value);
    }
    Some(Value::Object(object))
}

pub(crate) fn hedge_fast_failure_status_tag(status_code: u16) -> Option<HedgeEligibilityTag> {
    let reason = match status_code {
        401 => "http_401_unauthorized",
        403 => "http_403_forbidden",
        429 => "http_429_rate_limited",
        _ => return None,
    };
    Some(HedgeEligibilityTag::new("upstream_status", reason))
}

pub(crate) fn hedge_fast_failure_status_is_eligible(status_code: u16) -> bool {
    hedge_fast_failure_status_tag(status_code).is_some()
}

pub(crate) fn hedge_fast_failure_execution_error_tag(
    error: &ExecutionError,
) -> Option<HedgeEligibilityTag> {
    if let Some(status_code) = error.upstream_status {
        return hedge_fast_failure_status_tag(status_code);
    }

    if !execution_error_phase_can_be_fast_failure(&error.phase) {
        return None;
    }

    match error.kind {
        ExecutionErrorKind::TlsError => Some(HedgeEligibilityTag::new(
            "execution_error",
            "tls_or_handshake",
        )),
        ExecutionErrorKind::ConnectTimeout
        | ExecutionErrorKind::FirstByteTimeout
        | ExecutionErrorKind::ReadTimeout
        | ExecutionErrorKind::Upstream4xx
        | ExecutionErrorKind::Upstream5xx
        | ExecutionErrorKind::Cancelled => None,
        ExecutionErrorKind::ProxyError
        | ExecutionErrorKind::ProtocolError
        | ExecutionErrorKind::Internal => {
            local_transport_fast_failure_tag("execution_error", error.message.as_str())
        }
    }
}

pub(crate) fn hedge_fast_failure_execution_error_is_eligible(error: &ExecutionError) -> bool {
    hedge_fast_failure_execution_error_tag(error).is_some()
}

pub(crate) fn hedge_fast_failure_transport_error_tag(
    error: &ExecutionRuntimeTransportError,
) -> Option<HedgeEligibilityTag> {
    match error {
        ExecutionRuntimeTransportError::RequestBodyRequired
        | ExecutionRuntimeTransportError::BodyDecode(_)
        | ExecutionRuntimeTransportError::UnsupportedContentEncoding(_)
        | ExecutionRuntimeTransportError::ProxyUnsupported
        | ExecutionRuntimeTransportError::InvalidMethod(_)
        | ExecutionRuntimeTransportError::InvalidHeaderName(_)
        | ExecutionRuntimeTransportError::InvalidHeaderValue(_)
        | ExecutionRuntimeTransportError::InvalidProxy(_)
        | ExecutionRuntimeTransportError::UnsupportedTransportProfile(_)
        | ExecutionRuntimeTransportError::BodyEncode(_)
        | ExecutionRuntimeTransportError::ClientBuild(_) => Some(HedgeEligibilityTag::new(
            "transport_error",
            "presend_local_request_error",
        )),
        ExecutionRuntimeTransportError::UpstreamRequest(message)
        | ExecutionRuntimeTransportError::RelayError(message) => {
            local_transport_fast_failure_tag("transport_error", message)
        }
        ExecutionRuntimeTransportError::StreamUnsupported
        | ExecutionRuntimeTransportError::InvalidJson(_) => None,
    }
}

pub(crate) fn hedge_fast_failure_transport_error_is_eligible(
    error: &ExecutionRuntimeTransportError,
) -> bool {
    hedge_fast_failure_transport_error_tag(error).is_some()
}

pub(crate) fn hedge_fast_failure_candidate_status_tag(
    status_code: Option<u16>,
    error_type: Option<&str>,
    error_message: Option<&str>,
) -> Option<HedgeEligibilityTag> {
    if let Some(status_code) = status_code {
        return hedge_fast_failure_status_tag(status_code);
    }

    let combined = [error_type, error_message]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    local_transport_fast_failure_tag("candidate_status", combined.as_str())
}

pub(crate) fn hedge_fast_failure_candidate_status_is_eligible(
    status_code: Option<u16>,
    error_type: Option<&str>,
    error_message: Option<&str>,
) -> bool {
    hedge_fast_failure_candidate_status_tag(status_code, error_type, error_message).is_some()
}

fn local_transport_fast_failure_tag(
    source: &'static str,
    text: &str,
) -> Option<HedgeEligibilityTag> {
    let normalized = text.trim().to_ascii_lowercase();
    if normalized.is_empty()
        || text_indicates_timeout(normalized.as_str())
        || text_indicates_post_response_failure(normalized.as_str())
    {
        return None;
    }

    let reason = if text_indicates_tls_or_handshake(normalized.as_str()) {
        "tls_or_handshake"
    } else if text_indicates_connection_failure(normalized.as_str()) {
        "connection_failure"
    } else if text_indicates_presend_local_request_error(normalized.as_str()) {
        "presend_local_request_error"
    } else {
        return None;
    };
    Some(HedgeEligibilityTag::new(source, reason))
}

fn text_indicates_timeout(text: &str) -> bool {
    text.contains("timeout")
        || text.contains("timed out")
        || text.contains("first_byte_timeout")
        || text.contains("read_timeout")
        || text.contains("connect_timeout")
}

fn execution_error_phase_can_be_fast_failure(phase: &ExecutionPhase) -> bool {
    matches!(
        phase,
        ExecutionPhase::Connect
            | ExecutionPhase::Handshake
            | ExecutionPhase::Write
            | ExecutionPhase::FirstByte
    )
}

fn text_indicates_post_response_failure(text: &str) -> bool {
    text.contains("[kind=body")
        || text.contains("[kind=decode")
        || text.contains("streamread")
        || text.contains("stream read")
        || text.contains("after headers")
        || text.contains("after upstream stream had started")
}

fn text_indicates_tls_or_handshake(text: &str) -> bool {
    text.contains("tls")
        || text.contains("ssl")
        || text.contains("handshake")
        || text.contains("certificate")
        || text.contains("invalid peer cert")
}

fn text_indicates_connection_failure(text: &str) -> bool {
    text.contains("[kind=connect")
        || text.contains("connection refused")
        || text.contains("connection reset")
        || text.contains("connection closed")
        || text.contains("connection aborted")
        || text.contains("connect error")
        || text.contains("client error (connect")
        || text.contains("tcp connect")
        || text.contains("network unreachable")
        || text.contains("no route to host")
        || text.contains("could not resolve")
        || text.contains("failed to lookup")
        || text.contains("dns error")
        || text.contains("name resolution")
}

fn text_indicates_presend_local_request_error(text: &str) -> bool {
    text.contains("[kind=request")
        || text.contains("request body must contain")
        || text.contains("request body base64 is invalid")
        || text.contains("request content-encoding is not supported")
        || text.contains("proxy execution is not supported")
        || text.contains("invalid method")
        || text.contains("invalid upstream header")
        || text.contains("invalid header")
        || text.contains("invalid proxy configuration")
        || text.contains("unsupported transport profile")
        || text.contains("failed to encode request body")
        || text.contains("failed to build http client")
}

#[cfg(test)]
mod tests {
    use aether_contracts::{ExecutionError, ExecutionErrorKind, ExecutionPhase};

    use super::*;

    fn execution_error(
        kind: ExecutionErrorKind,
        message: &str,
        upstream_status: Option<u16>,
    ) -> ExecutionError {
        ExecutionError {
            kind,
            phase: ExecutionPhase::Connect,
            message: message.to_string(),
            upstream_status,
            retryable: true,
            failover_recommended: true,
        }
    }

    #[test]
    fn hedge_fast_failure_marks_only_auth_and_rate_limit_statuses() {
        assert!(hedge_fast_failure_status_is_eligible(401));
        assert!(hedge_fast_failure_status_is_eligible(403));
        assert!(hedge_fast_failure_status_is_eligible(429));
        assert!(!hedge_fast_failure_status_is_eligible(400));
        assert!(!hedge_fast_failure_status_is_eligible(500));

        for status_code in [401, 403, 429] {
            assert!(hedge_fast_failure_candidate_status_is_eligible(
                Some(status_code),
                None,
                None,
            ));
        }
    }

    #[test]
    fn hedge_fast_failure_treats_execution_error_status_as_authoritative() {
        let tls_with_500 = execution_error(
            ExecutionErrorKind::TlsError,
            "TLS handshake failed",
            Some(500),
        );
        let stream_read_after_200 = execution_error(
            ExecutionErrorKind::TlsError,
            "connection closed after headers",
            Some(200),
        );
        let auth_status = execution_error(ExecutionErrorKind::Upstream4xx, "expired", Some(401));

        assert!(!hedge_fast_failure_execution_error_is_eligible(
            &tls_with_500
        ));
        assert!(!hedge_fast_failure_execution_error_is_eligible(
            &stream_read_after_200
        ));
        assert!(hedge_fast_failure_execution_error_is_eligible(&auth_status));
    }

    #[test]
    fn hedge_fast_failure_marks_connection_errors() {
        let error = ExecutionRuntimeTransportError::UpstreamRequest(
            "error sending request: connection refused [kind=connect,request]".to_string(),
        );

        assert!(hedge_fast_failure_transport_error_is_eligible(&error));
    }

    #[test]
    fn hedge_fast_failure_marks_tls_like_errors() {
        let error = execution_error(
            ExecutionErrorKind::ProtocolError,
            "TLS handshake failed: invalid peer certificate",
            None,
        );

        assert!(hedge_fast_failure_execution_error_is_eligible(&error));
    }

    #[test]
    fn hedge_fast_failure_marks_presend_local_request_errors() {
        let error = ExecutionRuntimeTransportError::InvalidHeaderName("x bad header".to_string());

        assert!(hedge_fast_failure_transport_error_is_eligible(&error));
    }

    #[test]
    fn hedge_fast_failure_rejects_http_5xx() {
        assert!(!hedge_fast_failure_candidate_status_is_eligible(
            Some(500),
            Some("retryable_upstream_status"),
            Some("upstream failed"),
        ));
    }

    #[test]
    fn hedge_fast_failure_rejects_timeout_slow_death() {
        let error = execution_error(
            ExecutionErrorKind::FirstByteTimeout,
            "first byte timeout waiting for upstream",
            None,
        );

        assert!(!hedge_fast_failure_execution_error_is_eligible(&error));
        assert!(!hedge_fast_failure_candidate_status_is_eligible(
            None,
            Some("local_stream_candidate_watchdog_timeout"),
            Some("candidate exceeded watchdog timeout"),
        ));
    }

    #[test]
    fn hedge_fast_failure_rejects_stream_after_success_failures() {
        assert!(!hedge_fast_failure_candidate_status_is_eligible(
            Some(200),
            Some("execution_runtime_stream_chunk_decode_error"),
            Some("failed after upstream stream had started"),
        ));
    }

    #[test]
    fn hedge_fast_failure_rejects_midstream_transport_errors_without_status_fast_fail_signal() {
        assert!(!hedge_fast_failure_candidate_status_is_eligible(
            Some(200),
            Some("read_timeout"),
            Some("stream read timed out after headers"),
        ));
    }

    #[test]
    fn hedge_fast_failure_tags_report_context_for_retry_consumers() {
        let context = serde_json::json!({
            "candidate_index": 0,
            "error_flow": {
                "decision": "retry_next_candidate"
            }
        });
        let tagged = with_hedge_eligibility_report_context(
            Some(&context),
            hedge_fast_failure_status_tag(429).expect("429 should be eligible"),
        )
        .expect("context should be tagged");

        assert_eq!(tagged.get("hedge_eligible"), Some(&serde_json::json!(true)));
        assert_eq!(
            tagged
                .get("hedge_eligibility")
                .and_then(|value| value.get("reason")),
            Some(&serde_json::json!("http_429_rate_limited"))
        );
        assert_eq!(
            tagged
                .get("error_flow")
                .and_then(|value| value.get("hedge_eligible")),
            Some(&serde_json::json!(true))
        );
    }

    #[test]
    fn hedge_fast_failure_tags_connection_and_tls_reasons() {
        let connection = ExecutionRuntimeTransportError::UpstreamRequest(
            "error sending request: connection refused [kind=connect,request]".to_string(),
        );
        let tls = execution_error(
            ExecutionErrorKind::ProtocolError,
            "TLS handshake failed: invalid peer certificate",
            None,
        );

        assert_eq!(
            hedge_fast_failure_transport_error_tag(&connection)
                .expect("connection should be eligible")
                .reason,
            "connection_failure"
        );
        assert_eq!(
            hedge_fast_failure_execution_error_tag(&tls)
                .expect("TLS should be eligible")
                .reason,
            "tls_or_handshake"
        );
    }

    #[test]
    fn hedge_fast_failure_rejects_post_header_body_read_errors() {
        let error = ExecutionRuntimeTransportError::UpstreamRequest(
            "error reading a body from connection: connection closed before message completed [kind=body]".to_string(),
        );
        let stream_read_error = execution_error(
            ExecutionErrorKind::ProtocolError,
            "connection closed after headers",
            None,
        );
        let stream_read_error = ExecutionError {
            phase: ExecutionPhase::StreamRead,
            ..stream_read_error
        };

        assert!(!hedge_fast_failure_transport_error_is_eligible(&error));
        assert!(!hedge_fast_failure_execution_error_is_eligible(
            &stream_read_error
        ));
    }

    #[test]
    fn hedge_fast_failure_rejects_non_fast_transport_errors() {
        let stream_unsupported = ExecutionRuntimeTransportError::StreamUnsupported;
        let invalid_json = ExecutionRuntimeTransportError::InvalidJson(
            serde_json::from_str::<serde_json::Value>("not-json")
                .expect_err("invalid json should fail"),
        );
        let connect_timeout = ExecutionRuntimeTransportError::UpstreamRequest(
            "connection timed out while connecting [kind=connect,timeout]".to_string(),
        );

        assert!(!hedge_fast_failure_transport_error_is_eligible(
            &stream_unsupported
        ));
        assert!(!hedge_fast_failure_transport_error_is_eligible(
            &invalid_json
        ));
        assert!(!hedge_fast_failure_transport_error_is_eligible(
            &connect_timeout
        ));
    }
}
