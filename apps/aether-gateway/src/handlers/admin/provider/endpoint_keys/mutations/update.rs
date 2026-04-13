use crate::handlers::admin::provider::shared::paths::admin_update_key_id;
use crate::handlers::admin::provider::shared::payloads::AdminProviderKeyUpdatePatch;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::{model_fetch::perform_model_fetch_for_key, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn maybe_handle(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("endpoints_manage")
        || decision.route_kind.as_deref() != Some("update_key")
        || request_context.method() != http::Method::PUT
        || !request_context
            .path()
            .starts_with("/api/admin/endpoints/keys/")
    {
        return Ok(None);
    }

    let Some(key_id) = admin_update_key_id(request_context.path()) else {
        return Ok(Some(not_found_response("Key 不存在")));
    };
    let Some(request_body) = request_body else {
        return Ok(Some(bad_request_response("请求体不能为空")));
    };
    if !state.has_provider_catalog_data_reader() {
        return Ok(None);
    }

    let raw_value = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(value) => value,
        Err(_) => return Ok(Some(bad_request_response("请求体必须是合法的 JSON 对象"))),
    };
    let Some(raw_payload) = raw_value.as_object().cloned() else {
        return Ok(Some(bad_request_response("请求体必须是合法的 JSON 对象")));
    };
    let patch = match AdminProviderKeyUpdatePatch::from_object(raw_payload) {
        Ok(patch) => patch,
        Err(_) => return Ok(Some(bad_request_response("请求体必须是合法的 JSON 对象"))),
    };

    let Some(existing_key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(not_found_response(format!("Key {key_id} 不存在"))));
    };
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&existing_key.provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(not_found_response(format!(
            "Provider {} 不存在",
            existing_key.provider_id
        ))));
    };

    let updated_record = match state
        .build_admin_update_provider_key_record(&provider, &existing_key, patch)
        .await
    {
        Ok(record) => record,
        Err(detail) => return Ok(Some(bad_request_response(detail))),
    };
    let Some(updated) = state.update_provider_catalog_key(&updated_record).await? else {
        return Ok(None);
    };
    let should_overwrite_allowed_models_immediately =
        !existing_key.auto_fetch_models && updated.auto_fetch_models;
    let updated = if should_overwrite_allowed_models_immediately {
        let summary =
            perform_model_fetch_for_key(state.as_ref(), &provider.id, &updated.id).await?;
        if summary.succeeded == 0 {
            let detail = state
                .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
                .await?
                .into_iter()
                .next()
                .and_then(|key| key.last_models_fetch_error)
                .unwrap_or_else(|| "未获取到可用上游模型".to_string());
            return Err(GatewayError::Internal(format!(
                "开启自动获取模型后同步上游模型失败: {detail}"
            )));
        }

        state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
            .await?
            .into_iter()
            .next()
            .unwrap_or(updated)
    } else {
        updated
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    Ok(Some(
        Json(state.build_admin_provider_key_response(
            &updated,
            &provider.provider_type,
            now_unix_secs,
        ))
        .into_response(),
    ))
}

fn bad_request_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

fn not_found_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}
