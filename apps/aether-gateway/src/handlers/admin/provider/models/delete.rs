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
        && decision.route_kind.as_deref() == Some("delete_provider_model")
        && request_context.request_method == http::Method::DELETE
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
        let Some(existing) = state
            .get_admin_provider_model(&provider_id, &model_id)
            .await?
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Model {model_id} 不存在") })),
                )
                    .into_response(),
            ));
        };
        if !state
            .delete_admin_provider_model(&provider_id, &model_id)
            .await?
        {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Model {model_id} 不存在") })),
                )
                    .into_response(),
            ));
        }
        return Ok(Some(
            Json(json!({
                "message": format!("Model '{}' deleted successfully", existing.provider_model_name),
            }))
            .into_response(),
        ));
    }

    Ok(None)
}
