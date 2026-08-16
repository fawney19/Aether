use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::Response;
use axum::http::StatusCode;
use serde_json::json;

use crate::ai_serving::{build_core_error_body_for_client_format, LocalCoreSyncErrorKind};
use crate::constants::*;
use crate::control::GatewayControlDecision;
use crate::control::GatewayLocalAuthRejection;
use crate::daily_usage_limit::{DailyUsageLimitedResponse, FrontdoorDailyUsageRejection};
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
    build_client_response_from_parts_with_mutator(
        status_code,
        upstream_headers,
        body,
        trace_id,
        control_decision,
        |_| Ok(()),
    )
}

pub(crate) fn build_client_response_from_parts_with_mutator<F>(
    status_code: u16,
    upstream_headers: &BTreeMap<String, String>,
    body: Body,
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
    mutate_headers: F,
) -> Result<Response<Body>, GatewayError>
where
    F: FnOnce(&mut http::HeaderMap) -> Result<(), GatewayError>,
{
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
    mutate_headers(response.headers_mut())?;
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
    let fallback_payload = json!({
        "error": {
            "type": "balance_exceeded",
            "message": message,
            "details": {
                "balance_type": "USD",
                "remaining": balance_remaining,
            }
        }
    });
    let payload = build_local_error_payload(
        control_decision,
        None,
        &message,
        LocalCoreSyncErrorKind::RateLimit,
        fallback_payload,
    );
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
    let message = "请求过于频繁，请稍后重试";
    let fallback_payload = json!({
        "error": {
            "type": "rate_limit_exceeded",
            "message": message,
        }
    });
    let payload = build_local_error_payload(
        control_decision,
        None,
        message,
        LocalCoreSyncErrorKind::RateLimit,
        fallback_payload,
    );
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

pub(crate) fn build_local_daily_usage_limited_response(
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
    rejection: &FrontdoorDailyUsageRejection,
) -> Result<Response<Body>, GatewayError> {
    let message = "已达到每日使用上限，请在额度重置后重试";
    let fallback_payload = json!({
        "error": {
            "type": "daily_usage_limit_exceeded",
            "message": message,
            "details": {
                "limit_usd": rejection.limit_usd,
                "used_usd": rejection.used_usd,
                "remaining_usd": rejection.remaining_usd,
                "scope": rejection.scope,
                "reset_at": rejection.reset_at_unix_secs,
                "timezone": rejection.timezone,
            }
        }
    });
    let payload = if local_error_uses_claude_format(control_decision, None) {
        build_core_error_body_for_client_format(
            "claude:messages",
            message,
            Some("daily_usage_limit_exceeded"),
            LocalCoreSyncErrorKind::RateLimit,
        )
        .unwrap_or(fallback_payload)
    } else {
        fallback_payload
    };
    let body =
        serde_json::to_vec(&payload).map_err(|err| GatewayError::Internal(err.to_string()))?;
    let headers = BTreeMap::from([
        ("content-type".to_string(), "application/json".to_string()),
        ("Retry-After".to_string(), rejection.retry_after.to_string()),
        (
            "X-Daily-Usage-Limit-USD".to_string(),
            format!("{:.8}", rejection.limit_usd),
        ),
        (
            "X-Daily-Usage-Used-USD".to_string(),
            format!("{:.8}", rejection.used_usd),
        ),
        (
            "X-Daily-Usage-Remaining-USD".to_string(),
            format!("{:.8}", rejection.remaining_usd),
        ),
        (
            "X-Daily-Usage-Scope".to_string(),
            rejection.scope.to_string(),
        ),
        (
            "X-Daily-Usage-Reset".to_string(),
            rejection.reset_at_unix_secs.to_string(),
        ),
    ]);
    let mut response = build_client_response_from_parts(
        StatusCode::TOO_MANY_REQUESTS.as_u16(),
        &headers,
        Body::from(body),
        trace_id,
        control_decision,
    )?;
    response.extensions_mut().insert(DailyUsageLimitedResponse);
    Ok(response)
}

pub(crate) fn build_local_http_error_response(
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
    status_code: StatusCode,
    message: &str,
) -> Result<Response<Body>, GatewayError> {
    build_local_http_error_response_with_request_path(
        trace_id,
        control_decision,
        None,
        status_code,
        message,
    )
}

pub(crate) fn build_local_http_error_response_with_request_path(
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
    request_path: Option<&str>,
    status_code: StatusCode,
    message: &str,
) -> Result<Response<Body>, GatewayError> {
    let fallback_payload = json!({
        "error": {
            "type": "http_error",
            "message": message,
        }
    });
    let payload = build_local_error_payload(
        control_decision,
        request_path,
        message,
        local_error_kind_for_status(status_code),
        fallback_payload,
    );
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
    const ACCESS_POLICY_SUBJECT: &str = "当前用户、用户组或密钥的访问控制策略";

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
                &format!("{ACCESS_POLICY_SUBJECT}不允许访问 {provider} 提供商"),
            )
        }
        GatewayLocalAuthRejection::ApiFormatNotAllowed { api_format } => {
            build_local_http_error_response(
                trace_id,
                control_decision,
                StatusCode::FORBIDDEN,
                &format!("{ACCESS_POLICY_SUBJECT}不允许访问 {api_format} 格式"),
            )
        }
        GatewayLocalAuthRejection::ModelNotAllowed { model } => build_local_http_error_response(
            trace_id,
            control_decision,
            StatusCode::FORBIDDEN,
            &format!("{ACCESS_POLICY_SUBJECT}不允许访问模型 {model}"),
        ),
        GatewayLocalAuthRejection::IpNotAllowed { remote_ip } => build_local_http_error_response(
            trace_id,
            control_decision,
            StatusCode::UNAUTHORIZED,
            &format!("API Key 不允许从当前 IP 访问: {remote_ip}"),
        ),
    }
}

pub(crate) fn build_local_overloaded_response(
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
    request_path: Option<&str>,
    gate: &str,
    limit: usize,
) -> Result<Response<Body>, GatewayError> {
    let message = "服务繁忙，请稍后重试";
    let fallback_payload = json!({
        "error": {
            "type": "overloaded",
            "message": message,
            "details": {
                "gate": gate,
                "limit": limit,
            }
        }
    });
    let payload = build_local_error_payload(
        control_decision,
        request_path,
        message,
        LocalCoreSyncErrorKind::Overloaded,
        fallback_payload,
    );
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

fn build_local_error_payload(
    control_decision: Option<&GatewayControlDecision>,
    request_path: Option<&str>,
    message: &str,
    kind: LocalCoreSyncErrorKind,
    fallback_payload: serde_json::Value,
) -> serde_json::Value {
    if !local_error_uses_claude_format(control_decision, request_path) {
        return fallback_payload;
    }

    build_core_error_body_for_client_format("claude:messages", message, None, kind)
        .unwrap_or(fallback_payload)
}

fn local_error_uses_claude_format(
    control_decision: Option<&GatewayControlDecision>,
    request_path: Option<&str>,
) -> bool {
    control_decision.is_some_and(|decision| {
        decision.route_family.as_deref() == Some("claude")
            || decision
                .auth_endpoint_signature
                .as_deref()
                .is_some_and(|format| {
                    crate::ai_serving::normalize_api_format_alias(format)
                        .eq_ignore_ascii_case("claude:messages")
                })
    }) || request_path.is_some_and(|path| {
        matches!(
            path.trim_end_matches('/'),
            "/v1/messages" | "/v1/messages/count_tokens"
        )
    })
}

fn local_error_kind_for_status(status: StatusCode) -> LocalCoreSyncErrorKind {
    match status.as_u16() {
        400 | 405 | 422 => LocalCoreSyncErrorKind::InvalidRequest,
        401 => LocalCoreSyncErrorKind::Authentication,
        403 => LocalCoreSyncErrorKind::PermissionDenied,
        404 => LocalCoreSyncErrorKind::NotFound,
        413 => LocalCoreSyncErrorKind::RequestTooLarge,
        429 => LocalCoreSyncErrorKind::RateLimit,
        503 | 529 => LocalCoreSyncErrorKind::Overloaded,
        _ => LocalCoreSyncErrorKind::ServerError,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_client_response_from_parts, build_local_auth_rejection_response,
        build_local_daily_usage_limited_response,
        build_local_http_error_response_with_request_path, build_local_overloaded_response,
        build_local_user_rpm_limited_response,
    };
    use crate::control::{GatewayControlDecision, GatewayLocalAuthRejection};
    use crate::daily_usage_limit::FrontdoorDailyUsageRejection;
    use crate::rate_limit::FrontdoorUserRpmRejection;
    use axum::body::{to_bytes, Body};
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

    fn claude_decision() -> GatewayControlDecision {
        GatewayControlDecision::synthetic(
            "/v1/messages",
            Some("ai_public".to_string()),
            Some("claude".to_string()),
            Some("messages".to_string()),
            Some("claude:messages".to_string()),
        )
    }

    async fn response_json(response: http::Response<Body>) -> serde_json::Value {
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        serde_json::from_slice(&body).expect("response body should be JSON")
    }

    #[tokio::test]
    async fn claude_local_errors_use_anthropic_envelopes() {
        let decision = claude_decision();
        let invalid_key = build_local_auth_rejection_response(
            "trace-auth",
            Some(&decision),
            &GatewayLocalAuthRejection::InvalidApiKey,
        )
        .expect("invalid-key response should build");
        let invalid_key = response_json(invalid_key).await;
        assert_eq!(invalid_key["type"], "error");
        assert_eq!(invalid_key["error"]["type"], "authentication_error");

        let rpm = build_local_user_rpm_limited_response(
            "trace-rpm",
            Some(&decision),
            &FrontdoorUserRpmRejection {
                scope: "api_key",
                limit: 1,
                retry_after: 60,
            },
        )
        .expect("RPM response should build");
        let rpm = response_json(rpm).await;
        assert_eq!(rpm["type"], "error");
        assert_eq!(rpm["error"]["type"], "rate_limit_error");

        let overloaded = build_local_overloaded_response(
            "trace-overload",
            None,
            Some("/v1/messages/count_tokens"),
            "requests",
            10,
        )
        .expect("overload response should build");
        let overloaded = response_json(overloaded).await;
        assert_eq!(overloaded["type"], "error");
        assert_eq!(overloaded["error"]["type"], "overloaded_error");
    }

    #[tokio::test]
    async fn daily_usage_limit_response_has_dedicated_error_and_reset_headers() {
        let response = build_local_daily_usage_limited_response(
            "trace-daily-limit",
            None,
            &FrontdoorDailyUsageRejection {
                scope: "user",
                limit_usd: 10.0,
                used_usd: 10.25,
                remaining_usd: 0.0,
                retry_after: 120,
                reset_at_unix_secs: 1_800_000_000,
                timezone: "Asia/Shanghai".to_string(),
            },
        )
        .expect("daily limit response should build");

        assert_eq!(response.status(), http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "120");
        assert_eq!(response.headers()["x-daily-usage-limit-usd"], "10.00000000");
        assert_eq!(response.headers()["x-daily-usage-used-usd"], "10.25000000");
        assert_eq!(
            response.headers()["x-daily-usage-remaining-usd"],
            "0.00000000"
        );
        assert_eq!(response.headers()["x-daily-usage-scope"], "user");
        assert_eq!(response.headers()["x-daily-usage-reset"], "1800000000");

        let payload = response_json(response).await;
        assert_eq!(payload["error"]["type"], "daily_usage_limit_exceeded");
        assert_eq!(payload["error"]["details"]["timezone"], "Asia/Shanghai");
    }

    #[tokio::test]
    async fn claude_path_shapes_pre_control_http_errors_and_413() {
        for path in ["/v1/messages", "/v1/messages/count_tokens"] {
            let forbidden = build_local_http_error_response_with_request_path(
                "trace-pre-control",
                None,
                Some(path),
                http::StatusCode::FORBIDDEN,
                "blocked",
            )
            .expect("forbidden response should build");
            let forbidden = response_json(forbidden).await;
            assert_eq!(forbidden["type"], "error", "path: {path}");
            assert_eq!(
                forbidden["error"]["type"], "permission_error",
                "path: {path}"
            );

            let too_large = build_local_http_error_response_with_request_path(
                "trace-too-large",
                None,
                Some(path),
                http::StatusCode::PAYLOAD_TOO_LARGE,
                "too large",
            )
            .expect("payload-too-large response should build");
            let too_large = response_json(too_large).await;
            assert_eq!(too_large["type"], "error", "path: {path}");
            assert_eq!(
                too_large["error"]["type"], "request_too_large",
                "path: {path}"
            );
        }
    }
}
