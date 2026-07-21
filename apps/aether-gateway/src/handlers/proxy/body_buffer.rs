use crate::api::response::build_local_http_error_response;
use crate::control::GatewayPublicRequestContext;
use crate::headers::RequestBodyNormalizationError;
use crate::{AppState, GatewayError};
use aether_gateway_frontdoor::{BodyBufferError, BodyBufferPolicy as FrontdoorBodyBufferPolicy};
use axum::body::{Body, Bytes};
use axum::http::{self, Response};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{info, warn};

const REQUEST_BODY_READ_TIMEOUT_DETAIL: &str =
    "Request body read timed out before the gateway could route the request";
const REQUEST_BODY_READ_FAILED_DETAIL: &str = "Failed to read request body";

#[derive(Debug, Clone)]
pub(super) struct RequestBodyBufferPolicy {
    inner: FrontdoorBodyBufferPolicy,
}

impl RequestBodyBufferPolicy {
    pub(super) fn from_state(state: &AppState) -> Self {
        Self {
            inner: FrontdoorBodyBufferPolicy::with_permit_bytes(
                crate::headers::max_request_body_bytes(),
                state.frontdoor_runtime_guards.request_body_read_timeout,
                state.frontdoor_runtime_guards.internal_gate_queue_budget,
                state
                    .frontdoor_runtime_guards
                    .request_body_buffer_budget_bytes,
                crate::state::REQUEST_BODY_BUFFER_PERMIT_BYTES,
                Arc::clone(&state.request_body_buffer_budget),
            ),
        }
    }

    #[cfg(test)]
    pub(super) fn for_tests(max_bytes: u64, read_timeout: Duration) -> Self {
        let budget_bytes = usize::try_from(max_bytes)
            .unwrap_or(usize::MAX)
            .max(crate::state::REQUEST_BODY_BUFFER_PERMIT_BYTES);
        Self {
            inner: FrontdoorBodyBufferPolicy::with_permit_bytes(
                max_bytes,
                read_timeout,
                read_timeout,
                budget_bytes,
                crate::state::REQUEST_BODY_BUFFER_PERMIT_BYTES,
                Arc::new(Semaphore::new(
                    budget_bytes.saturating_add(crate::state::REQUEST_BODY_BUFFER_PERMIT_BYTES - 1)
                        / crate::state::REQUEST_BODY_BUFFER_PERMIT_BYTES,
                )),
            ),
        }
    }

    #[cfg(test)]
    pub(super) fn for_tests_with_budget(
        max_bytes: u64,
        read_timeout: Duration,
        queue_timeout: Duration,
        budget_bytes: usize,
        budget: Arc<Semaphore>,
    ) -> Self {
        Self {
            inner: FrontdoorBodyBufferPolicy::with_permit_bytes(
                max_bytes,
                read_timeout,
                queue_timeout,
                budget_bytes,
                crate::state::REQUEST_BODY_BUFFER_PERMIT_BYTES,
                budget,
            ),
        }
    }

    fn max_bytes(&self) -> u64 {
        self.inner.max_bytes()
    }

    fn budget_bytes(&self) -> usize {
        self.inner.budget_bytes()
    }

    fn read_timeout(&self) -> Duration {
        self.inner.read_timeout()
    }

    async fn reserve(
        &self,
        headers: &http::HeaderMap,
    ) -> Result<aether_gateway_frontdoor::BodyBufferReservation, BodyBufferError> {
        self.inner.reserve(headers).await
    }
}

#[derive(Debug)]
pub(super) enum RequestBodyBufferError {
    Normalization(RequestBodyNormalizationError),
    TooLarge {
        limit_bytes: u64,
    },
    Overloaded {
        requested_bytes: usize,
        budget_bytes: usize,
        timeout_ms: u64,
    },
    Timeout {
        timeout_ms: u64,
    },
    ReadFailed {
        message: String,
    },
}

impl RequestBodyBufferError {
    pub(super) fn http_status(&self) -> http::StatusCode {
        match self {
            Self::Normalization(error) => error.http_status(),
            Self::TooLarge { .. } => http::StatusCode::PAYLOAD_TOO_LARGE,
            Self::Overloaded { .. } => http::StatusCode::SERVICE_UNAVAILABLE,
            Self::Timeout { .. } => http::StatusCode::REQUEST_TIMEOUT,
            Self::ReadFailed { .. } => http::StatusCode::BAD_REQUEST,
        }
    }

    fn client_message(&self) -> String {
        match self {
            Self::Normalization(error) => error.client_message(),
            Self::TooLarge { limit_bytes } => format!("Request body exceeds {limit_bytes} bytes"),
            Self::Overloaded { .. } => {
                "Request body buffering capacity is temporarily exhausted".to_string()
            }
            Self::Timeout { .. } => REQUEST_BODY_READ_TIMEOUT_DETAIL.to_string(),
            Self::ReadFailed { .. } => REQUEST_BODY_READ_FAILED_DETAIL.to_string(),
        }
    }

    fn reason(&self) -> &'static str {
        match self {
            Self::Normalization(error) => match error {
                RequestBodyNormalizationError::UnsupportedContentEncoding(_) => {
                    "unsupported_content_encoding"
                }
                RequestBodyNormalizationError::DecodeFailed { .. } => "decode_failed",
                RequestBodyNormalizationError::DecompressedBodyTooLarge { .. } => {
                    "decompressed_body_too_large"
                }
                RequestBodyNormalizationError::RequestBodyTooLarge { .. } => {
                    "request_body_too_large"
                }
            },
            Self::TooLarge { .. } => "request_body_too_large",
            Self::Overloaded { .. } => "request_body_buffer_overloaded",
            Self::Timeout { .. } => "request_body_read_timeout",
            Self::ReadFailed { .. } => "request_body_read_failed",
        }
    }
}

impl From<BodyBufferError> for RequestBodyBufferError {
    fn from(error: BodyBufferError) -> Self {
        match error {
            BodyBufferError::TooLarge { limit_bytes } => Self::TooLarge { limit_bytes },
            BodyBufferError::Overloaded {
                requested_bytes,
                budget_bytes,
                timeout_ms,
            } => Self::Overloaded {
                requested_bytes,
                budget_bytes,
                timeout_ms,
            },
            BodyBufferError::Timeout { timeout_ms } => Self::Timeout { timeout_ms },
            BodyBufferError::ReadFailed { message } => Self::ReadFailed { message },
        }
    }
}

pub(super) async fn buffer_and_normalize_request_body(
    request_body: &mut Option<Body>,
    headers: &mut http::HeaderMap,
    body_owner_expectation: &'static str,
    trace_id: &str,
    method: &http::Method,
    path_and_query: &str,
    phase: &'static str,
    policy: RequestBodyBufferPolicy,
) -> Result<Bytes, RequestBodyBufferError> {
    let reservation = policy
        .reserve(headers)
        .await
        .map_err(RequestBodyBufferError::from)?;
    let reservation_bytes = reservation.requested_bytes();

    info!(
        event_name = "frontdoor_request_body_buffer_started",
        log_type = "event",
        trace_id,
        method = %method,
        path = %path_and_query,
        phase,
        max_body_bytes = policy.max_bytes(),
        reserved_body_bytes = reservation_bytes,
        body_buffer_budget_bytes = policy.budget_bytes(),
        timeout_ms = policy.read_timeout().as_millis() as u64,
        "gateway started buffering request body"
    );

    let buffered = reservation
        .collect(request_body.take().expect(body_owner_expectation))
        .await
        .map_err(RequestBodyBufferError::from)?;
    let elapsed_ms = buffered.elapsed().as_millis() as u64;
    let normalized = buffered
        .try_map(|body| {
            crate::headers::normalize_request_body_headers_and_bytes_with_limit(
                headers,
                body,
                policy.max_bytes(),
            )
        })
        .map_err(RequestBodyBufferError::Normalization)?;
    info!(
        event_name = "frontdoor_request_body_buffer_completed",
        log_type = "event",
        trace_id,
        method = %method,
        path = %path_and_query,
        phase,
        body_bytes = normalized.len(),
        elapsed_ms,
        "gateway completed buffering request body"
    );
    Ok(normalized)
}

pub(super) fn build_request_body_buffer_error_response(
    trace_id: &str,
    request_context: &GatewayPublicRequestContext,
    error: &RequestBodyBufferError,
) -> Result<Response<Body>, GatewayError> {
    warn!(
        event_name = "frontdoor_request_body_buffer_failed",
        log_type = "ops",
        trace_id,
        method = %request_context.request_method,
        path = %request_context.request_path_and_query(),
        status_code = error.http_status().as_u16(),
        reason = error.reason(),
        detail = %error.client_message(),
        read_error = match error {
            RequestBodyBufferError::ReadFailed { message } => message.as_str(),
            _ => "",
        },
        buffer_requested_bytes = match error {
            RequestBodyBufferError::Overloaded { requested_bytes, .. } => *requested_bytes,
            _ => 0,
        },
        buffer_budget_bytes = match error {
            RequestBodyBufferError::Overloaded { budget_bytes, .. } => *budget_bytes,
            _ => 0,
        },
        buffer_queue_timeout_ms = match error {
            RequestBodyBufferError::Overloaded { timeout_ms, .. } => *timeout_ms,
            _ => 0,
        },
        "gateway rejected request body before local execution planning"
    );
    build_local_http_error_response(
        trace_id,
        request_context.control_decision.as_ref(),
        error.http_status(),
        error.client_message().as_str(),
    )
}
