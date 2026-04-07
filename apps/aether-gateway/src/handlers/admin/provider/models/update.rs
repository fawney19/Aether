use super::payloads::build_admin_provider_model_response;
use super::write::build_admin_provider_model_update_record;
use crate::control::GatewayControlDecision;
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::admin_provider_model_route_parts;
use crate::handlers::admin::provider::shared::payloads::AdminProviderModelUpdateRequest;
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
        && decision.route_kind.as_deref() == Some("update_provider_model")
        && request_context.request_method == http::Method::PATCH
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
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        let raw_value = match serde_json::from_slice::<serde_json::Value>(request_body) {
            Ok(value) => value,
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
        let Some(raw_payload) = raw_value.as_object().cloned() else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                )
                    .into_response(),
            ));
        };
        let payload = match serde_json::from_value::<AdminProviderModelUpdateRequest>(raw_value) {
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
            match build_admin_provider_model_update_record(state, &existing, &raw_payload, payload)
                .await
            {
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
            match state.update_admin_provider_model(&record).await? {
                Some(updated) => {
                    let now_unix_secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|duration| duration.as_secs())
                        .unwrap_or(0);
                    Json(build_admin_provider_model_response(&updated, now_unix_secs))
                        .into_response()
                }
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
