use aether_contracts::{ExecutionPlan, ExecutionResult};

use crate::constants::TRACE_ID_HEADER;
use crate::{AppState, GatewayError};

fn remote_runtime_request_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_request() {
        "request"
    } else if error.is_body() {
        "body"
    } else if error.is_decode() {
        "decode"
    } else {
        "transport"
    }
}

fn build_remote_execution_runtime_request(
    state: &AppState,
    remote_execution_runtime_base_url: &str,
    path: &str,
    trace_id: Option<&str>,
    plan: &ExecutionPlan,
) -> Result<reqwest::RequestBuilder, GatewayError> {
    let envelope_limit = crate::execution_runtime::transport::execution_result_envelope_limit_bytes(
        crate::headers::max_internal_buffered_body_bytes(),
    );
    let body = crate::execution_runtime::transport::serialize_serializable_with_limit(
        plan,
        envelope_limit,
    )
    .map_err(|error| {
        let kind = match error {
            crate::execution_runtime::transport::ExecutionRuntimeTransportError::BodyTooLarge {
                ..
            } => "too_large",
            _ => "encode",
        };
        GatewayError::Internal(format!(
            "remote execution runtime request body failed ({kind})"
        ))
    })?;
    let mut request = state
        .client
        .post(format!("{remote_execution_runtime_base_url}{path}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body);
    if let Some(trace_id) = trace_id.map(str::trim).filter(|value| !value.is_empty()) {
        request = request.header(TRACE_ID_HEADER, trace_id);
    }
    Ok(request)
}

pub(crate) async fn post_sync_plan_to_remote_execution_runtime(
    state: &AppState,
    remote_execution_runtime_base_url: &str,
    trace_id: Option<&str>,
    plan: &ExecutionPlan,
) -> Result<reqwest::Response, GatewayError> {
    build_remote_execution_runtime_request(
        state,
        remote_execution_runtime_base_url,
        "/v1/execute/sync",
        trace_id,
        plan,
    )?
    .send()
    .await
    .map_err(|err| {
        GatewayError::Internal(format!(
            "remote execution runtime request failed ({})",
            remote_runtime_request_error_kind(&err)
        ))
    })
}

pub(crate) async fn post_stream_plan_to_remote_execution_runtime(
    state: &AppState,
    remote_execution_runtime_base_url: &str,
    trace_id: Option<&str>,
    plan: &ExecutionPlan,
) -> Result<reqwest::Response, GatewayError> {
    build_remote_execution_runtime_request(
        state,
        remote_execution_runtime_base_url,
        "/v1/execute/stream",
        trace_id,
        plan,
    )?
    .send()
    .await
    .map_err(|err| {
        GatewayError::Internal(format!(
            "remote execution runtime request failed ({})",
            remote_runtime_request_error_kind(&err)
        ))
    })
}

pub(crate) async fn execute_sync_plan_via_remote_execution_runtime(
    state: &AppState,
    remote_execution_runtime_base_url: &str,
    trace_id: Option<&str>,
    plan: &ExecutionPlan,
) -> Result<ExecutionResult, GatewayError> {
    let response = post_sync_plan_to_remote_execution_runtime(
        state,
        remote_execution_runtime_base_url,
        trace_id,
        plan,
    )
    .await?;
    if response.status() != http::StatusCode::OK {
        return Err(GatewayError::Internal(format!(
            "execution runtime returned HTTP {}",
            response.status()
        )));
    }

    let body = aether_http::read_response_bytes_with_limit(
        response,
        crate::execution_runtime::transport::execution_result_envelope_limit_bytes(
            crate::headers::max_internal_buffered_body_bytes(),
        ),
    )
    .await
    .map_err(|err| {
        GatewayError::Internal(format!(
            "remote execution runtime response body failed ({})",
            match err {
                aether_http::ResponseBodyReadError::TooLarge { .. } => "too_large",
                aether_http::ResponseBodyReadError::Read(error) => {
                    remote_runtime_request_error_kind(&error)
                }
            }
        ))
    })?;
    serde_json::from_slice::<ExecutionResult>(&body).map_err(|_| {
        GatewayError::Internal(
            "remote execution runtime returned invalid execution JSON".to_string(),
        )
    })
}
