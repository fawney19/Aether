use crate::handlers::admin::provider::shared::paths::admin_provider_model_route_parts;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn maybe_handle(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    _request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if request_context.route_family() == Some("provider_models_manage")
        && request_context.route_kind() == Some("delete_provider_model")
        && request_context.method() == http::Method::DELETE
        && request_context.path().contains("/models/")
    {
        let Some((provider_id, model_id)) =
            admin_provider_model_route_parts(request_context.path())
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
