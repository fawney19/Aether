use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::Response;
use axum::http::StatusCode;
use serde_json::json;

use crate::constants::*;
use crate::control::GatewayControlDecision;
use crate::control::GatewayLocalAuthRejection;
use crate::headers::should_skip_response_header;
use crate::rate_limit::FrontdoorUserRpmRejection;
use crate::{insert_header_if_missing, GatewayError};

fn execution_runtime_candidate_header_value(decision: &GatewayControlDecision) -> &'static str {
    if decision.is_execution_runtime_candidate() {
        "true"
    } else {
        "false"
    }
}

fn insert_execution_runtime_candidate_headers(
    headers: &mut http::HeaderMap,
    decision: &GatewayControlDecision,
) -> Result<(), GatewayError> {
    let value = execution_runtime_candidate_header_value(decision);
    insert_header_if_missing(headers, CONTROL_EXECUTION_RUNTIME_HEADER, value)
}

fn response_is_sse(headers: &http::HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"))
}

pub(crate) fn apply_streaming_response_headers(headers: &mut http::HeaderMap) {
    if !response_is_sse(headers) {
        return;
    }

    headers.insert(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert(
        HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
}

pub(crate) fn build_client_response(
    upstream_response: reqwest::Response,
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
) -> Result<Response<Body>, GatewayError> {
    let status = upstream_response.status();
    let upstream_headers = upstream_response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let upstream_stream = upstream_response.bytes_stream();
    build_client_response_from_parts(
        status.as_u16(),
        &upstream_headers,
        Body::from_stream(upstream_stream),
        trace_id,
        control_decision,
    )
}

pub(crate) fn build_client_response_from_parts(
    status_code: u16,
    upstream_headers: &BTreeMap<String, String>,
    body: Body,
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
) -> Result<Response<Body>, GatewayError> {
    let mut response = Response::builder()
        .status(status_code)
        .body(body)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;

    for (name, value) in upstream_headers {
        if should_skip_response_header(name.as_str()) {
            continue;
        }
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let header_value =
            HeaderValue::from_str(value).map_err(|err| GatewayError::Internal(err.to_string()))?;
        response.headers_mut().insert(header_name, header_value);
    }
    apply_streaming_response_headers(response.headers_mut());
    insert_header_if_missing(response.headers_mut(), TRACE_ID_HEADER, trace_id)?;
    insert_header_if_missing(response.headers_mut(), GATEWAY_HEADER, "rust-phase3b")?;
    if let Some(decision) = control_decision {
        insert_header_if_missing(
            response.headers_mut(),
            CONTROL_ROUTE_CLASS_HEADER,
            decision.route_class.as_deref().unwrap_or("passthrough"),
        )?;
        insert_execution_runtime_candidate_headers(response.headers_mut(), decision)?;
        if let Some(route_family) = decision.route_family.as_deref() {
            insert_header_if_missing(
                response.headers_mut(),
                CONTROL_ROUTE_FAMILY_HEADER,
                route_family,
            )?;
        }
        if let Some(route_kind) = decision.route_kind.as_deref() {
            insert_header_if_missing(
                response.headers_mut(),
                CONTROL_ROUTE_KIND_HEADER,
                route_kind,
            )?;
        }
    }
    Ok(response)
}

pub(crate) fn insert_candidate_id_header_if_present(
    headers: &mut http::HeaderMap,
    candidate_id: Option<&str>,
) -> Result<(), GatewayError> {
    let Some(candidate_id) = candidate_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    insert_header_if_missing(headers, CONTROL_CANDIDATE_ID_HEADER, candidate_id)
}

pub(crate) fn insert_request_id_header_if_present(
    headers: &mut http::HeaderMap,
    request_id: Option<&str>,
) -> Result<(), GatewayError> {
    let Some(request_id) = request_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    insert_header_if_missing(headers, CONTROL_REQUEST_ID_HEADER, request_id)
}

pub(crate) fn attach_control_metadata_headers(
    mut response: Response<Body>,
    request_id: Option<&str>,
    candidate_id: Option<&str>,
) -> Result<Response<Body>, GatewayError> {
    insert_request_id_header_if_present(response.headers_mut(), request_id)?;
    insert_candidate_id_header_if_present(response.headers_mut(), candidate_id)?;
    Ok(response)
}

pub(crate) fn build_local_balance_denied_response(
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
    balance_remaining: Option<f64>,
) -> Result<Response<Body>, GatewayError> {
    let message = match balance_remaining {
        Some(remaining) => format!("余额不足（剩余: ${remaining:.2}）"),
        None => "余额不足".to_string(),
    };
    let payload = json!({
        "error": {
            "type": "balance_exceeded",
            "message": message,
            "details": {
                "balance_type": "USD",
                "remaining": balance_remaining,
            }
        }
    });
    let body =
        serde_json::to_vec(&payload).map_err(|err| GatewayError::Internal(err.to_string()))?;
    let headers = BTreeMap::from([("content-type".to_string(), "application/json".to_string())]);
    build_client_response_from_parts(
        StatusCode::TOO_MANY_REQUESTS.as_u16(),
        &headers,
        Body::from(body),
        trace_id,
        control_decision,
    )
}

pub(crate) fn build_local_user_rpm_limited_response(
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
    rejection: &FrontdoorUserRpmRejection,
) -> Result<Response<Body>, GatewayError> {
    let payload = json!({
        "error": {
            "type": "rate_limit_exceeded",
            "message": "请求过于频繁，请稍后重试",
        }
    });
    let body =
        serde_json::to_vec(&payload).map_err(|err| GatewayError::Internal(err.to_string()))?;
    let headers = BTreeMap::from([
        ("content-type".to_string(), "application/json".to_string()),
        ("Retry-After".to_string(), rejection.retry_after.to_string()),
        ("X-RateLimit-Limit".to_string(), rejection.limit.to_string()),
        ("X-RateLimit-Remaining".to_string(), "0".to_string()),
        ("X-RateLimit-Scope".to_string(), rejection.scope.to_string()),
    ]);
    build_client_response_from_parts(
        StatusCode::TOO_MANY_REQUESTS.as_u16(),
        &headers,
        Body::from(body),
        trace_id,
        control_decision,
    )
}

pub(crate) fn build_local_http_error_response(
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
    status_code: StatusCode,
    message: &str,
) -> Result<Response<Body>, GatewayError> {
    let payload = json!({
        "error": {
            "type": "http_error",
            "message": message,
        }
    });
    let body =
        serde_json::to_vec(&payload).map_err(|err| GatewayError::Internal(err.to_string()))?;
    let headers = BTreeMap::from([("content-type".to_string(), "application/json".to_string())]);
    build_client_response_from_parts(
        status_code.as_u16(),
        &headers,
        Body::from(body),
        trace_id,
        control_decision,
    )
}

pub(crate) fn build_local_auth_rejection_response(
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
    rejection: &GatewayLocalAuthRejection,
) -> Result<Response<Body>, GatewayError> {
    match rejection {
        GatewayLocalAuthRejection::InvalidApiKey => build_local_http_error_response(
            trace_id,
            control_decision,
            StatusCode::UNAUTHORIZED,
            "无效的API密钥",
        ),
        GatewayLocalAuthRejection::LockedApiKey => build_local_http_error_response(
            trace_id,
            control_decision,
            StatusCode::FORBIDDEN,
            "该密钥已被管理员锁定，请联系管理员",
        ),
        GatewayLocalAuthRejection::WalletUnavailable => build_local_http_error_response(
            trace_id,
            control_decision,
            StatusCode::FORBIDDEN,
            "钱包不可用",
        ),
        GatewayLocalAuthRejection::BalanceDenied { remaining } => {
            build_local_balance_denied_response(trace_id, control_decision, *remaining)
        }
        GatewayLocalAuthRejection::ProviderNotAllowed { provider } => {
            build_local_http_error_response(
                trace_id,
                control_decision,
                StatusCode::FORBIDDEN,
                &format!("当前密钥不允许访问 {provider} 提供商"),
            )
        }
        GatewayLocalAuthRejection::ApiFormatNotAllowed { api_format } => {
            build_local_http_error_response(
                trace_id,
                control_decision,
                StatusCode::FORBIDDEN,
                &format!("当前密钥不允许访问 {api_format} 格式"),
            )
        }
        GatewayLocalAuthRejection::ModelNotAllowed { model } => build_local_http_error_response(
            trace_id,
            control_decision,
            StatusCode::FORBIDDEN,
            &format!("当前密钥不允许访问模型 {model}"),
        ),
    }
}

pub(crate) fn build_local_overloaded_response(
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
    gate: &str,
    limit: usize,
) -> Result<Response<Body>, GatewayError> {
    let payload = json!({
        "error": {
            "type": "overloaded",
            "message": "服务繁忙，请稍后重试",
            "details": {
                "gate": gate,
                "limit": limit,
            }
        }
    });
    let body =
        serde_json::to_vec(&payload).map_err(|err| GatewayError::Internal(err.to_string()))?;
    let headers = BTreeMap::from([("content-type".to_string(), "application/json".to_string())]);
    build_client_response_from_parts(
        StatusCode::SERVICE_UNAVAILABLE.as_u16(),
        &headers,
        Body::from(body),
        trace_id,
        control_decision,
    )
}

#[cfg(test)]
mod tests {
    use super::build_client_response_from_parts;
    use axum::body::Body;
    use std::collections::BTreeMap;

    #[test]
    fn sse_responses_disable_proxy_buffering() {
        let response = build_client_response_from_parts(
            200,
            &BTreeMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
            Body::from("data: hello\n\n"),
            "trace-sse-buffering-1",
            None,
        )
        .expect("response should build");

        assert_eq!(
            response
                .headers()
                .get(http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache, no-transform")
        );
        assert_eq!(
            response
                .headers()
                .get("x-accel-buffering")
                .and_then(|value| value.to_str().ok()),
            Some("no")
        );
    }
}
