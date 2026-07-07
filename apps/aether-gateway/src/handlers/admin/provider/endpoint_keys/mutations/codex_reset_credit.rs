use crate::handlers::admin::provider::oauth::quota::codex::consume_codex_reset_credit_locally;
use crate::handlers::admin::provider::oauth::quota::shared::{
    provider_quota_refresh_endpoint_for_provider, provider_quota_refresh_missing_endpoint_message,
};
use crate::handlers::admin::provider::shared::paths::admin_codex_reset_credit_consume_key_id;
use crate::handlers::admin::provider::shared::payloads::AdminCodexResetCreditConsumeRequest;
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
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("endpoints_manage")
        || decision.route_kind.as_deref() != Some("codex_reset_credit_consume")
        || request_context.method() != http::Method::POST
        || !request_context
            .path()
            .starts_with("/api/admin/endpoints/keys/")
        || !request_context
            .path()
            .ends_with("/codex-reset-credit/consume")
    {
        return Ok(None);
    }

    let Some(key_id) = admin_codex_reset_credit_consume_key_id(request_context.path()) else {
        return Ok(Some(not_found_response("Key 不存在")));
    };
    let payload = match request_body.filter(|body| !body.is_empty()) {
        Some(request_body) => {
            match serde_json::from_slice::<AdminCodexResetCreditConsumeRequest>(request_body) {
                Ok(payload) => payload,
                Err(_) => {
                    return Ok(Some(bad_request_response("请求体必须是合法的 JSON 对象")));
                }
            }
        }
        None => {
            return Ok(Some(bad_request_response("请求体必须包含 idempotency_key")));
        }
    };
    let idempotency_key = payload.idempotency_key.trim().to_string();
    if idempotency_key.is_empty() {
        return Ok(Some(bad_request_response("idempotency_key 不能为空")));
    }

    let Some(key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(not_found_response(format!("Key {key_id} 不存在"))));
    };
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&key.provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(not_found_response(format!(
            "Provider {} 不存在",
            key.provider_id
        ))));
    };
    let normalized_provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if normalized_provider_type != "codex" {
        return Ok(Some(bad_request_response(
            "仅 Codex Provider 支持使用重置机会",
        )));
    }

    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let Some(endpoint) =
        provider_quota_refresh_endpoint_for_provider(&normalized_provider_type, &endpoints, true)
    else {
        return Ok(Some(bad_request_response(
            provider_quota_refresh_missing_endpoint_message(&normalized_provider_type),
        )));
    };

    let (status, payload) =
        consume_codex_reset_credit_locally(state, &provider, &endpoint, key, &idempotency_key)
            .await?;
    Ok(Some((status, Json(payload)).into_response()))
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
