use super::payloads::{admin_provider_model_name_exists, build_admin_provider_model_response};
use super::write::build_admin_provider_model_create_record;
use crate::control::GatewayControlDecision;
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::admin_provider_models_batch_path;
use crate::handlers::admin::provider::shared::payloads::AdminProviderModelCreateRequest;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn maybe_handle(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
    decision: &GatewayControlDecision,
) -> Result<Option<Response<Body>>, GatewayError> {
    if decision.route_family.as_deref() == Some("provider_models_manage")
        && decision.route_kind.as_deref() == Some("batch_create_provider_models")
        && request_context.request_method == http::Method::POST
        && request_context.request_path.ends_with("/models/batch")
    {
        let Some(provider_id) = admin_provider_models_batch_path(&request_context.request_path)
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
        let payloads =
            match serde_json::from_slice::<Vec<AdminProviderModelCreateRequest>>(request_body) {
                Ok(payloads) => payloads,
                Err(_) => {
                    return Ok(Some(
                        (
                            http::StatusCode::BAD_REQUEST,
                            Json(json!({ "detail": "请求体必须是合法的 JSON 数组" })),
                        )
                            .into_response(),
                    ));
                }
            };
        let mut created = Vec::new();
        let mut seen = BTreeSet::new();
        for payload in payloads {
            let normalized_name = payload.provider_model_name.trim().to_string();
            if normalized_name.is_empty() {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "provider_model_name 不能为空" })),
                    )
                        .into_response(),
                ));
            }
            if !seen.insert(normalized_name.clone()) {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": format!("批量请求中包含重复模型 {normalized_name}") })),
                    )
                        .into_response(),
                ));
            }
            if admin_provider_model_name_exists(state, &provider_id, &normalized_name, None).await?
            {
                continue;
            }
            let record = match build_admin_provider_model_create_record(
                state,
                &provider_id,
                payload,
            )
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
            let Some(model) = state.create_admin_provider_model(&record).await? else {
                return Ok(Some(
                    (
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "detail": "批量创建模型失败" })),
                    )
                        .into_response(),
                ));
            };
            created.push(model);
        }
        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        return Ok(Some(
            Json(serde_json::Value::Array(
                created
                    .iter()
                    .map(|model| build_admin_provider_model_response(model, now_unix_secs))
                    .collect(),
            ))
            .into_response(),
        ));
    }

    Ok(None)
}
