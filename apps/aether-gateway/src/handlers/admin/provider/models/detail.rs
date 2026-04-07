use super::payloads::build_admin_provider_model_payload;
use crate::control::GatewayControlDecision;
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::admin_provider_model_route_parts;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn maybe_handle(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    _request_body: Option<&Bytes>,
    decision: &GatewayControlDecision,
) -> Result<Option<Response<Body>>, GatewayError> {
    if decision.route_family.as_deref() == Some("provider_models_manage")
        && decision.route_kind.as_deref() == Some("get_provider_model")
        && request_context.request_method == http::Method::GET
        && request_context
            .request_path
            .starts_with("/api/admin/providers/")
        && request_context.request_path.contains("/models/")
    {
        let Some((provider_id, model_id)) =
            admin_provider_model_route_parts(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Model 不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match build_admin_provider_model_payload(state, &provider_id, &model_id).await {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Model {model_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    Ok(None)
}
