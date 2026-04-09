use crate::handlers::admin::provider::shared::paths::admin_provider_assign_global_models_path;
use crate::handlers::admin::provider::shared::payloads::AdminBatchAssignGlobalModelsRequest;
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
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if request_context.route_family() == Some("provider_models_manage")
        && request_context.route_kind() == Some("assign_global_models")
        && request_context.method() == http::Method::POST
    {
        let Some(provider_id) = admin_provider_assign_global_models_path(request_context.path())
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(_provider) = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                )
                    .into_response(),
            ));
        };
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        let payload =
            match serde_json::from_slice::<AdminBatchAssignGlobalModelsRequest>(request_body) {
                Ok(payload) => payload,
                Err(_) => {
                    return Ok(Some(
                        (
                            http::StatusCode::BAD_REQUEST,
                            Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                        )
                            .into_response(),
                    ));
                }
            };
        let payload = match state
            .build_admin_batch_assign_global_models_payload(&provider_id, payload.global_model_ids)
            .await
        {
            Ok(payload) => payload,
            Err(detail) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": detail })),
                    )
                        .into_response(),
                ));
            }
        };
        return Ok(Some(Json(payload).into_response()));
    }

    Ok(None)
}
