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
use crate::headers::should_skip_response_header;
use crate::plan_usage_policy::PlanUsagePolicyRejection;
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

fn apply_gateway_browser_security_headers(headers: &mut http::HeaderMap) {
    // Provider responses are API data, even when an untrusted provider labels
    // them as HTML or SVG.  Keep a direct navigation to a gateway API route
    // from becoming same-origin active content, and prevent referrer leakage
    // if a user follows a link rendered from such a response.
    headers.insert(
        http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            "default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; sandbox",
        ),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
}

pub(crate) fn build_client_response(
    upstream_response: reqwest::Response,
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
) -> Result<Response<Body>, GatewayError> {
    let status = upstream_response.status();
    let upstream_headers = collect_safe_response_headers(upstream_response.headers());
    let upstream_stream = upstream_response.bytes_stream();
    build_client_response_from_parts(
        status.as_u16(),
        &upstream_headers,
        Body::from_stream(upstream_stream),
        trace_id,
        control_decision,
    )
}

fn collect_safe_response_headers(headers: &http::HeaderMap) -> BTreeMap<String, String> {
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
            if should_skip_client_response_header(&normalized)
                || connection_declared.contains(&normalized)
            {
                return None;
            }
            value
                .to_str()
                .ok()
                .map(|value| (normalized, value.to_string()))
        })
        .collect()
}

fn should_skip_client_response_header(name: &str) -> bool {
    should_skip_response_header(name)
        // A provider Location is relative to the provider, not to the gateway.
        // Forwarding it lets redirect-following clients bypass the gateway and
        // can disclose their gateway Authorization header to another origin.
        // Keep Location available inside execution reports, but never expose
        // it on the client-facing response boundary.
        || name.eq_ignore_ascii_case(http::header::LOCATION.as_str())
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

    let connection_declared = aether_http::connection_declared_header_names(
        upstream_headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case(http::header::CONNECTION.as_str()))
            .map(|(_, value)| value.as_str()),
    );

    for (name, value) in upstream_headers {
        if should_skip_client_response_header(name.as_str())
            || connection_declared.contains(&name.to_ascii_lowercase())
        {
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
    apply_gateway_browser_security_headers(response.headers_mut());
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

pub(crate) fn build_local_plan_usage_limited_response(
    trace_id: &str,
    control_decision: Option<&GatewayControlDecision>,
    rejection: &PlanUsagePolicyRejection,
) -> Result<Response<Body>, GatewayError> {
    let message = "套餐使用限制已达到上限，请稍后重试";
    let fallback_payload = json!({
        "error": {
            "type": "plan_usage_limit_exceeded",
            "message": message,
            "details": {
                "metric": rejection.metric,
                "window": rejection.window,
                "limit": rejection.limit,
                "retry_after": rejection.retry_after,
            }
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
        ("X-RateLimit-Scope".to_string(), "plan".to_string()),
        (
            "X-RateLimit-Metric".to_string(),
            rejection.metric.to_string(),
        ),
        (
            "X-RateLimit-Window".to_string(),
            rejection.window.to_string(),
        ),
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
        build_client_response, build_client_response_from_parts,
        build_client_response_from_parts_with_mutator, build_local_auth_rejection_response,
        build_local_http_error_response_with_request_path, build_local_overloaded_response,
        build_local_plan_usage_limited_response, build_local_user_rpm_limited_response,
    };
    use crate::control::{GatewayControlDecision, GatewayLocalAuthRejection};
    use crate::plan_usage_policy::PlanUsagePolicyRejection;
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

    #[test]
    fn upstream_security_headers_are_stripped_before_gateway_headers_are_added() {
        let response = build_client_response_from_parts_with_mutator(
            200,
            &BTreeMap::from([
                ("set-cookie".to_string(), "session=attacker".to_string()),
                (
                    "x-aether-gateway".to_string(),
                    "attacker-gateway".to_string(),
                ),
                (
                    "x-aether-control-action".to_string(),
                    "attacker-action".to_string(),
                ),
                (
                    "x-aether-future-control".to_string(),
                    "attacker-future".to_string(),
                ),
                (
                    "x-accel-redirect".to_string(),
                    "/internal/private-file".to_string(),
                ),
                ("x-sendfile".to_string(), "/etc/passwd".to_string()),
                (
                    "x-reproxy-url".to_string(),
                    "http://127.0.0.1:9000/private".to_string(),
                ),
                (
                    "access-control-allow-origin".to_string(),
                    "https://attacker.example".to_string(),
                ),
                (
                    "access-control-allow-credentials".to_string(),
                    "true".to_string(),
                ),
                ("content-length".to_string(), "999999".to_string()),
                (
                    "content-security-policy".to_string(),
                    "default-src * 'unsafe-inline' 'unsafe-eval'".to_string(),
                ),
                (
                    "content-security-policy-report-only".to_string(),
                    "default-src 'none'; report-uri https://attacker.example/csp".to_string(),
                ),
                (
                    "reporting-endpoints".to_string(),
                    "attacker=\"https://attacker.example/reports\"".to_string(),
                ),
                ("report-to".to_string(), "attacker".to_string()),
                (
                    "nel".to_string(),
                    "{\"report_to\":\"attacker\"}".to_string(),
                ),
                (
                    "refresh".to_string(),
                    "0; url=https://attacker.example".to_string(),
                ),
                ("referrer-policy".to_string(), "unsafe-url".to_string()),
                ("x-content-type-options".to_string(), "invalid".to_string()),
                (
                    "location".to_string(),
                    "https://provider.example/direct".to_string(),
                ),
                ("x-upstream-visible".to_string(), "ok".to_string()),
            ]),
            Body::empty(),
            "trace-upstream-header-filter",
            None,
            |headers| {
                headers.insert(
                    http::HeaderName::from_static("x-aether-control-action"),
                    http::HeaderValue::from_static("gateway-action"),
                );
                Ok(())
            },
        )
        .expect("response should build");

        assert!(response.headers().get(http::header::SET_COOKIE).is_none());
        assert!(response.headers().get("x-aether-future-control").is_none());
        assert!(response.headers().get("x-accel-redirect").is_none());
        assert!(response.headers().get("x-sendfile").is_none());
        assert!(response.headers().get("x-reproxy-url").is_none());
        assert!(response
            .headers()
            .get("access-control-allow-origin")
            .is_none());
        assert!(response
            .headers()
            .get("access-control-allow-credentials")
            .is_none());
        assert!(response
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .is_none());
        assert!(response
            .headers()
            .get("content-security-policy-report-only")
            .is_none());
        assert!(response.headers().get("reporting-endpoints").is_none());
        assert!(response.headers().get("report-to").is_none());
        assert!(response.headers().get("nel").is_none());
        assert!(response.headers().get("refresh").is_none());
        assert!(response.headers().get(http::header::LOCATION).is_none());
        assert_eq!(
            response.headers()["content-security-policy"],
            "default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; sandbox"
        );
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        assert_eq!(
            response.headers()[http::header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert_eq!(response.headers()["x-aether-gateway"], "rust-phase3b");
        assert_eq!(
            response.headers()["x-aether-control-action"],
            "gateway-action"
        );
        assert_eq!(response.headers()["x-upstream-visible"], "ok");
    }

    #[tokio::test]
    async fn raw_response_collector_honors_all_connection_header_lines() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("connection");
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.expect("request read");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nConnection: x-first-hop\r\nConnection: x-second-hop\r\nX-First-Hop: first-secret\r\nX-Second-Hop: second-secret\r\nContent-Length: 2\r\n\r\nok",
                )
                .await
                .expect("response write");
        });
        let upstream = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client")
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("upstream response");

        let response = build_client_response(upstream, "trace-connection-lines", None)
            .expect("client response");
        server.await.expect("server");

        assert!(response.headers().get("connection").is_none());
        assert!(response.headers().get("x-first-hop").is_none());
        assert!(response.headers().get("x-second-hop").is_none());
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

    #[tokio::test]
    async fn plan_usage_rejection_exposes_machine_readable_limit_headers() {
        let response = build_local_plan_usage_limited_response(
            "trace-plan-limit",
            None,
            &PlanUsagePolicyRejection {
                metric: "request_count",
                limit: 100.0,
                retry_after: 42,
                window: "calendar_week",
            },
        )
        .expect("response");
        assert_eq!(response.status(), http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "42");
        assert_eq!(response.headers()["x-ratelimit-scope"], "plan");
        assert_eq!(response.headers()["x-ratelimit-window"], "calendar_week");
        let payload = response_json(response).await;
        assert_eq!(payload["error"]["type"], "plan_usage_limit_exceeded");
    }
}
