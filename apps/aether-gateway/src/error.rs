use axum::body::Body;
use axum::http::{Response, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;
use tracing::warn;

use crate::ai_serving::AiSurfaceFinalizeError;
use crate::constants::*;
use crate::insert_header_if_missing;

#[derive(Debug, Clone)]
pub(crate) enum GatewayError {
    UpstreamUnavailable {
        trace_id: String,
        message: String,
    },
    ControlUnavailable {
        trace_id: String,
        message: String,
    },
    LocalExecutionPlanningTimeout {
        trace_id: String,
        phase: &'static str,
        timeout_ms: u64,
    },
    AdmissionTimeout {
        trace_id: String,
        gate: &'static str,
        queue_budget_ms: u64,
    },
    Client {
        status: StatusCode,
        message: String,
    },
    Internal(String),
}

impl GatewayError {
    pub(crate) fn into_message(self) -> String {
        match self {
            Self::UpstreamUnavailable { message, .. }
            | Self::ControlUnavailable { message, .. }
            | Self::Client { message, .. }
            | Self::Internal(message) => message,
            Self::LocalExecutionPlanningTimeout {
                phase, timeout_ms, ..
            } => {
                format!("local execution planning timed out in {phase} after {timeout_ms}ms")
            }
            Self::AdmissionTimeout {
                gate,
                queue_budget_ms,
                ..
            } => {
                format!("gateway admission gate {gate} timed out after {queue_budget_ms}ms")
            }
        }
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response<Body> {
        match self {
            Self::UpstreamUnavailable { trace_id, message } => {
                warn!(trace_id = %trace_id, error = %message, "gateway proxy unavailable");
                let body = Json(json!({
                    "error": {
                        "message": "gateway proxy unavailable",
                        "trace_id": trace_id,
                    }
                }));
                let mut response = (StatusCode::BAD_GATEWAY, body).into_response();
                let _ =
                    insert_header_if_missing(response.headers_mut(), TRACE_ID_HEADER, &trace_id);
                let _ = insert_header_if_missing(
                    response.headers_mut(),
                    GATEWAY_HEADER,
                    "rust-phase3b",
                );
                response
            }
            Self::ControlUnavailable { trace_id, message } => {
                warn!(trace_id = %trace_id, error = %message, "gateway control unavailable");
                let body = Json(json!({
                    "error": {
                        "message": "gateway control unavailable",
                        "trace_id": trace_id,
                    }
                }));
                let mut response = (StatusCode::BAD_GATEWAY, body).into_response();
                let _ =
                    insert_header_if_missing(response.headers_mut(), TRACE_ID_HEADER, &trace_id);
                let _ = insert_header_if_missing(
                    response.headers_mut(),
                    GATEWAY_HEADER,
                    "rust-phase3b",
                );
                response
            }
            Self::LocalExecutionPlanningTimeout {
                trace_id,
                phase,
                timeout_ms,
            } => {
                warn!(
                    trace_id = %trace_id,
                    phase,
                    timeout_ms,
                    "gateway local execution planning timed out"
                );
                let body = Json(json!({
                    "error": {
                        "message": "gateway local execution planning timed out",
                        "trace_id": trace_id,
                    }
                }));
                let mut response = (StatusCode::GATEWAY_TIMEOUT, body).into_response();
                let _ =
                    insert_header_if_missing(response.headers_mut(), TRACE_ID_HEADER, &trace_id);
                let _ = insert_header_if_missing(
                    response.headers_mut(),
                    GATEWAY_HEADER,
                    "rust-phase3b",
                );
                response
            }
            Self::AdmissionTimeout {
                trace_id,
                gate,
                queue_budget_ms,
            } => {
                tracing::debug!(
                    trace_id = %trace_id,
                    gate,
                    queue_budget_ms,
                    "gateway admission gate timed out"
                );
                let body = Json(json!({
                    "error": {
                        "message": "gateway admission queue timed out",
                        "trace_id": trace_id,
                    }
                }));
                let mut response = (StatusCode::TOO_MANY_REQUESTS, body).into_response();
                let _ =
                    insert_header_if_missing(response.headers_mut(), TRACE_ID_HEADER, &trace_id);
                let _ = insert_header_if_missing(
                    response.headers_mut(),
                    GATEWAY_HEADER,
                    "rust-phase3b",
                );
                let _ = insert_header_if_missing(response.headers_mut(), "Retry-After", "1");
                response
            }
            Self::Client { status, message } => (
                status,
                Json(json!({
                    "error": {
                        "message": message,
                    }
                })),
            )
                .into_response(),
            Self::Internal(message) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": {
                        "message": message,
                    }
                })),
            )
                .into_response(),
        }
    }
}

impl From<AiSurfaceFinalizeError> for GatewayError {
    fn from(error: AiSurfaceFinalizeError) -> Self {
        GatewayError::Internal(error.0)
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{header::RETRY_AFTER, StatusCode};
    use axum::response::IntoResponse;

    use crate::constants::TRACE_ID_HEADER;

    use super::GatewayError;

    #[test]
    fn admission_timeout_returns_429_with_retry_after_without_panicking() {
        let trace_id = "trace-admission-timeout".to_string();

        let response = GatewayError::AdmissionTimeout {
            trace_id: trace_id.clone(),
            gate: "gateway_upstream_execution",
            queue_budget_ms: 250,
        }
        .into_response();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("1")
        );
        assert_eq!(
            response
                .headers()
                .get(TRACE_ID_HEADER)
                .and_then(|v| v.to_str().ok()),
            Some(trace_id.as_str())
        );
    }
}
