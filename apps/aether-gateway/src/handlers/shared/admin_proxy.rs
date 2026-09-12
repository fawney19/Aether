use crate::audit::attach_admin_audit_event;
use crate::control::GatewayPublicRequestContext;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(crate) fn build_unhandled_admin_proxy_response(
    request_context: &GatewayPublicRequestContext,
) -> Response<Body> {
    let decision = request_context.control_decision.as_ref();
    (
        http::StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "detail": "admin proxy route not implemented in rust frontdoor",
            "route_family": decision.and_then(|value| value.route_family.as_deref()),
            "route_kind": decision.and_then(|value| value.route_kind.as_deref()),
            "request_path": request_context.request_path,
        })),
    )
        .into_response()
}

pub(crate) fn build_admin_proxy_auth_required_response(
    request_context: &GatewayPublicRequestContext,
) -> Response<Body> {
    let decision = request_context.control_decision.as_ref();
    (
        http::StatusCode::UNAUTHORIZED,
        Json(json!({
            "detail": "admin authentication required",
            "route_family": decision.and_then(|value| value.route_family.as_deref()),
            "route_kind": decision.and_then(|value| value.route_kind.as_deref()),
            "request_path": request_context.request_path,
        })),
    )
        .into_response()
}

pub(crate) fn attach_admin_audit_response(
    mut response: Response<Body>,
    event_name: &'static str,
    action: &'static str,
    target_type: &'static str,
    target_id: &str,
) -> Response<Body> {
    attach_admin_audit_event(&mut response, event_name, action, target_type, target_id);
    response
}

pub(crate) fn mark_sensitive_admin_response_no_store(
    mut response: Response<Body>,
) -> Response<Body> {
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        http::header::PRAGMA,
        http::HeaderValue::from_static("no-cache"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::mark_sensitive_admin_response_no_store;
    use axum::{http, response::IntoResponse, Json};

    #[test]
    fn plaintext_admin_secret_responses_are_never_cacheable() {
        let response = mark_sensitive_admin_response_no_store(
            Json(serde_json::json!({ "key": "secret" })).into_response(),
        );

        assert_eq!(
            response.headers().get(http::header::CACHE_CONTROL),
            Some(&http::HeaderValue::from_static("no-store"))
        );
        assert_eq!(
            response.headers().get(http::header::PRAGMA),
            Some(&http::HeaderValue::from_static("no-cache"))
        );
    }
}
