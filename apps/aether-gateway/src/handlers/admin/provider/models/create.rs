use super::payloads::build_admin_provider_model_response;
use super::write::build_admin_provider_model_create_record;
use crate::control::GatewayControlDecision;
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::admin_provider_id_for_models_list;
use crate::handlers::admin::provider::shared::payloads::AdminProviderModelCreateRequest;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn maybe_handle(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
    decision: &GatewayControlDecision,
) -> Result<Option<Response<Body>>, GatewayError> {
    if decision.route_family.as_deref() == Some("provider_models_manage")
        && decision.route_kind.as_deref() == Some("create_provider_model")
        && request_context.request_method == http::Method::POST
        && request_context.request_path.ends_with("/models")
    {
        let Some(provider_id) = admin_provider_id_for_models_list(&request_context.request_path)
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
        let payload = match serde_json::from_slice::<AdminProviderModelCreateRequest>(request_body)
        {
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
        let record =
            match build_admin_provider_model_create_record(state, &provider_id, payload).await {
                Ok(record) => record,
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
        return Ok(Some(
            match state.create_admin_provider_model(&record).await? {
                Some(created) => {
                    let now_unix_secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|duration| duration.as_secs())
                        .unwrap_or(0);
                    Json(build_admin_provider_model_response(&created, now_unix_secs))
                        .into_response()
                }
                None => (
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "detail": "创建模型失败" })),
                )
                    .into_response(),
            },
        ));
    }

    Ok(None)
}
