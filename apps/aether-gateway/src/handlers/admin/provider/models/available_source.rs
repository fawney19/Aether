use crate::handlers::admin::provider::shared::paths::admin_provider_available_source_models_path;
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
        && request_context.route_kind() == Some("available_source_models")
        && request_context.method() == http::Method::GET
    {
        let Some(provider_id) = admin_provider_available_source_models_path(request_context.path())
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match state
                .build_admin_provider_available_source_models_payload(&provider_id)
                .await
            {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    Ok(None)
}
