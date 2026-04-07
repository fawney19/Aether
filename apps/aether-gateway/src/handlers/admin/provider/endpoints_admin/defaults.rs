use super::extractors::admin_default_body_rules_api_format;
use crate::api::ai::admin_default_body_rules_for_signature;
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::shared::query_param_value;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn maybe_handle(
    _state: &AppState,
    request_context: &GatewayPublicRequestContext,
    _request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("endpoints_manage")
        || decision.route_kind.as_deref() != Some("default_body_rules")
        || !request_context
            .request_path
            .starts_with("/api/admin/endpoints/defaults/")
        || !request_context.request_path.ends_with("/body-rules")
    {
        return Ok(None);
    }

    let Some(api_format) = admin_default_body_rules_api_format(&request_context.request_path)
    else {
        return Ok(Some(
            (
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "无效的 api_format" })),
            )
                .into_response(),
        ));
    };
    let provider_type = query_param_value(
        request_context.request_query_string.as_deref(),
        "provider_type",
    );

    Ok(Some(
        match admin_default_body_rules_for_signature(&api_format, provider_type.as_deref()) {
            Some((normalized_api_format, body_rules)) => Json(json!({
                "api_format": normalized_api_format,
                "body_rules": body_rules,
            }))
            .into_response(),
            None => (
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": format!("无效的 api_format: {api_format}") })),
            )
                .into_response(),
        },
    ))
}
