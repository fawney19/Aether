use super::payloads::build_admin_provider_model_payload;
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
        && request_context.route_kind() == Some("get_provider_model")
        && request_context.method() == http::Method::GET
        && request_context.path().starts_with("/api/admin/providers/")
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
