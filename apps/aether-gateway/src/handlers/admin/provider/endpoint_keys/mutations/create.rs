use crate::handlers::admin::provider::shared::paths::admin_provider_id_for_keys;
use crate::handlers::admin::provider::shared::payloads::AdminProviderKeyCreateRequest;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::provider_key_auth::provider_key_effective_api_formats;
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
        || decision.route_kind.as_deref() != Some("create_provider_key")
        || request_context.method() != http::Method::POST
        || !request_context
            .path()
            .starts_with("/api/admin/endpoints/providers/")
        || !request_context.path().ends_with("/keys")
    {
        return Ok(None);
    }

    let Some(provider_id) = admin_provider_id_for_keys(request_context.path()) else {
        return Ok(Some(not_found_response("Provider 不存在")));
    };
    let Some(request_body) = request_body else {
        return Ok(Some(bad_request_response("请求体不能为空")));
    };
    if !state.has_provider_catalog_data_reader() {
        return Ok(None);
    }
    let payload = match serde_json::from_slice::<AdminProviderKeyCreateRequest>(request_body) {
        Ok(payload) => payload,
        Err(_) => return Ok(Some(bad_request_response("请求体必须是合法的 JSON 对象"))),
    };
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(not_found_response(format!(
            "Provider {provider_id} 不存在"
        ))));
    };
    let record = match state
        .build_admin_create_provider_key_record(&provider, payload)
        .await
    {
        Ok(record) => record,
        Err(detail) => return Ok(Some(bad_request_response(detail))),
    };
    let Some(created) = state.create_provider_catalog_key(&record).await? else {
        return Ok(None);
    };
    let key_id = created.id.clone();
    let created = if created.auto_fetch_models {
        let summary =
            perform_model_fetch_for_key(state.as_ref(), &provider.id, &created.id).await?;
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
            .unwrap_or(created)
    } else {
        created
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let api_formats =
        provider_key_effective_api_formats(&created, &provider.provider_type, &endpoints);

    Ok(Some(
        Json(state.build_admin_provider_key_response(
            &created,
            &provider.provider_type,
            &api_formats,
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
