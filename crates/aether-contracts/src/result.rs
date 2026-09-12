use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ExecutionError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionResponseObservation {
    pub request_started_at_unix_ms: u64,
    pub response_headers_observed_at_unix_ms: u64,
    pub request_order_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionTelemetry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_bytes: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ResponseBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_bytes_b64: Option<String>,
}

impl fmt::Debug for ResponseBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResponseBody")
            .field("has_json_body", &self.json_body.is_some())
            .field(
                "json_body_bytes",
                &self
                    .json_body
                    .as_ref()
                    .and_then(|body| serde_json::to_vec(body).ok().map(|bytes| bytes.len())),
            )
            .field(
                "body_bytes_b64_len",
                &self.body_bytes_b64.as_ref().map(String::len),
            )
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionResult {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    pub status_code: u16,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_observation: Option<ExecutionResponseObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ResponseBody>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<ExecutionTelemetry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ExecutionError>,
}

impl fmt::Debug for ExecutionResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("ExecutionResult");
        debug
            .field("request_id", &self.request_id)
            .field("candidate_id", &self.candidate_id)
            .field("status_code", &self.status_code)
            .field("header_names", &self.headers.keys().collect::<Vec<_>>())
            .field("response_observation", &self.response_observation)
            .field("body", &self.body)
            .field("telemetry", &self.telemetry)
            .field("has_error", &self.error.is_some());
        if let Some(error) = self.error.as_ref() {
            // ExecutionError::message can contain an upstream response or URL.
            debug
                .field("error_kind", &error.kind)
                .field("error_phase", &error.phase)
                .field("error_upstream_status", &error.upstream_status)
                .field("error_retryable", &error.retryable)
                .field("error_failover_recommended", &error.failover_recommended);
        }
        debug.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{ExecutionResult, ResponseBody};
    use crate::{ExecutionError, ExecutionErrorKind, ExecutionPhase};
    use std::collections::BTreeMap;

    #[test]
    fn debug_does_not_render_response_headers_body_or_error_message() {
        let result = ExecutionResult {
            request_id: "request-1".into(),
            candidate_id: None,
            status_code: 401,
            headers: BTreeMap::from([(
                "set-cookie".into(),
                "session=response-header-secret".into(),
            )]),
            response_observation: None,
            body: Some(ResponseBody {
                json_body: Some(serde_json::json!({"access_token": "response-body-secret"})),
                body_bytes_b64: None,
            }),
            telemetry: None,
            error: Some(ExecutionError {
                kind: ExecutionErrorKind::Upstream4xx,
                phase: ExecutionPhase::Finalize,
                message: "upstream detail error-secret".into(),
                upstream_status: Some(401),
                retryable: false,
                failover_recommended: false,
            }),
        };

        let debug = format!("{result:?}");
        for secret in [
            "response-header-secret",
            "response-body-secret",
            "error-secret",
        ] {
            assert!(!debug.contains(secret), "debug leaked {secret}: {debug}");
        }
        assert!(debug.contains("header_names"));
        assert!(debug.contains("has_error"));
    }
}
