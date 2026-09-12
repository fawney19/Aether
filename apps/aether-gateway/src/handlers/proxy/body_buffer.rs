use crate::api::response::build_local_http_error_response;
use crate::control::GatewayPublicRequestContext;
use crate::headers::RequestBodyNormalizationError;
use crate::{AppState, GatewayError};
use aether_gateway_frontdoor::{BodyBufferError, BodyBufferPolicy as FrontdoorBodyBufferPolicy};
use aether_usage_runtime::MAX_INTERNAL_REPORT_BODY_BYTES;
use axum::body::{Body, Bytes};
use axum::http::{self, Response};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{info, warn};

const REQUEST_BODY_READ_TIMEOUT_DETAIL: &str =
    "Request body read timed out before the gateway could route the request";
const REQUEST_BODY_READ_FAILED_DETAIL: &str = "Failed to read request body";
// A report can carry two 64 MiB decoded provider/client bodies.  Base64 expands
// each body by roughly one third, so the request envelope needs about 180 MiB
// plus JSON metadata.  Keep a bounded 192 MiB control-plane ceiling rather
// than inheriting the generic 256 MiB public request limit.
const MAX_INTERNAL_REPORT_REQUEST_BODY_BYTES: u64 = 192 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(super) struct RequestBodyBufferPolicy {
    inner: FrontdoorBodyBufferPolicy,
}

impl RequestBodyBufferPolicy {
    pub(super) fn from_state(state: &AppState) -> Self {
        Self::from_state_with_max_bytes(state, crate::headers::max_request_body_bytes())
    }

    pub(super) fn for_internal_report(state: &AppState) -> Self {
        // Keep the envelope limit tied to the per-field decoded limit.  The
        // explicit constant leaves room for base64 expansion and metadata.
        let envelope_limit = MAX_INTERNAL_REPORT_REQUEST_BODY_BYTES
            .min((MAX_INTERNAL_REPORT_BODY_BYTES as u64).saturating_mul(3));
        Self::from_state_with_max_bytes(state, envelope_limit)
    }

    pub(super) fn for_request_context(
        state: &AppState,
        request_context: &GatewayPublicRequestContext,
    ) -> Self {
        let is_internal_report =
            request_context
                .control_decision
                .as_ref()
                .is_some_and(|decision| {
                    decision.route_class.as_deref() == Some("internal_proxy")
                        && decision.route_family.as_deref() == Some("internal_gateway")
                        && matches!(
                            decision.route_kind.as_deref(),
                            Some("report_sync" | "report_stream" | "finalize_sync")
                        )
                });
        if is_internal_report {
            Self::for_internal_report(state)
        } else {
            Self::from_state(state)
        }
    }

    fn from_state_with_max_bytes(state: &AppState, max_bytes: u64) -> Self {
        Self {
            inner: FrontdoorBodyBufferPolicy::with_optional_read_timeout_and_permit_bytes(
                max_bytes,
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
    pub(super) fn for_tests_without_read_timeout(max_bytes: u64) -> Self {
        let budget_bytes = usize::try_from(max_bytes)
            .unwrap_or(usize::MAX)
            .max(crate::state::REQUEST_BODY_BUFFER_PERMIT_BYTES);
        Self {
            inner: FrontdoorBodyBufferPolicy::with_optional_read_timeout_and_permit_bytes(
                max_bytes,
                None,
                Duration::from_secs(1),
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

    fn effective_max_bytes(&self) -> u64 {
        self.inner.effective_max_bytes()
    }

    fn budget_bytes(&self) -> usize {
        self.inner.budget_bytes()
    }

    fn read_timeout(&self) -> Option<Duration> {
        self.inner.optional_read_timeout()
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
    InvalidHeaders {
        message: String,
    },
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
            Self::InvalidHeaders { .. } => http::StatusCode::BAD_REQUEST,
            Self::Normalization(error) => error.http_status(),
            Self::TooLarge { .. } => http::StatusCode::PAYLOAD_TOO_LARGE,
            Self::Overloaded { .. } => http::StatusCode::SERVICE_UNAVAILABLE,
            Self::Timeout { .. } => http::StatusCode::REQUEST_TIMEOUT,
            Self::ReadFailed { .. } => http::StatusCode::BAD_REQUEST,
        }
    }

    pub(super) fn client_message(&self) -> String {
        match self {
            Self::InvalidHeaders { .. } => "Invalid request body headers".to_string(),
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
            Self::InvalidHeaders { .. } => "invalid_request_body_headers",
            Self::Normalization(error) => match error {
                RequestBodyNormalizationError::InvalidBodyFraming
                | RequestBodyNormalizationError::AmbiguousBodyFraming => {
                    "invalid_request_body_headers"
                }
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
                RequestBodyNormalizationError::BodyBufferOverloaded { .. } => {
                    "request_body_buffer_overloaded"
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
            BodyBufferError::InvalidHeaders { message } => Self::InvalidHeaders { message },
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
    let sanitized_path_and_query = crate::middleware::sanitize_access_log_path(path_and_query);
    let reservation = policy
        .reserve(headers)
        .await
        .map_err(RequestBodyBufferError::from)?;
    let reservation_bytes = reservation.requested_bytes();
    let read_timeout = policy.read_timeout();

    info!(
        event_name = "frontdoor_request_body_buffer_started",
        log_type = "event",
        trace_id,
        method = %method,
        path = %sanitized_path_and_query,
        phase,
        max_body_bytes = policy.max_bytes(),
        reserved_body_bytes = reservation_bytes,
        body_buffer_budget_bytes = policy.budget_bytes(),
        timeout_enabled = read_timeout.is_some(),
        timeout_ms = read_timeout.map(|timeout| timeout.as_millis() as u64).unwrap_or(0),
        "gateway started buffering request body"
    );

    let buffered = reservation
        .collect(request_body.take().expect(body_owner_expectation))
        .await
        .map_err(RequestBodyBufferError::from)?;
    let elapsed_ms = buffered.elapsed().as_millis() as u64;
    let retained_input_capacity = buffered
        .requested_bytes()
        .saturating_sub(buffered.bytes().len());
    let normalized = buffered
        .try_map_with_budget(|body, memory| {
            crate::headers::normalize_request_body_headers_and_bytes_with_budget(
                headers,
                body,
                policy.effective_max_bytes(),
                &mut |requested_bytes| {
                    let requested_bytes = requested_bytes.saturating_add(retained_input_capacity);
                    memory.try_reserve_bytes(requested_bytes).map_err(|_| {
                        RequestBodyNormalizationError::BodyBufferOverloaded {
                            requested_bytes,
                            budget_bytes: policy.budget_bytes(),
                        }
                    })
                },
            )
        })
        .map_err(|error| match error {
            RequestBodyNormalizationError::BodyBufferOverloaded {
                requested_bytes,
                budget_bytes,
            } => RequestBodyBufferError::Overloaded {
                requested_bytes,
                budget_bytes,
                timeout_ms: 0,
            },
            error => RequestBodyBufferError::Normalization(error),
        })?;
    info!(
        event_name = "frontdoor_request_body_buffer_completed",
        log_type = "event",
        trace_id,
        method = %method,
        path = %sanitized_path_and_query,
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
    let sanitized_path_and_query =
        crate::middleware::sanitize_access_log_path(&request_context.request_path_and_query());
    warn!(
        event_name = "frontdoor_request_body_buffer_failed",
        log_type = "ops",
        trace_id,
        method = %request_context.request_method,
        path = %sanitized_path_and_query,
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
