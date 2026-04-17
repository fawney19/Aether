use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{query_param_value, AdminTypedObjectPatch};
use crate::handlers::admin::users::{
    format_optional_unix_secs_iso8601, masked_user_api_key_display,
};
use crate::GatewayError;
use aether_admin::system::serialize_admin_system_users_export_wallet;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

const ADMIN_API_KEYS_DATA_UNAVAILABLE_DETAIL: &str = "Admin standalone API key data unavailable";

#[derive(Debug, Default, serde::Deserialize)]
pub(super) struct AdminStandaloneApiKeyCreateRequest {
    pub(super) name: Option<String>,
    pub(super) allowed_providers: Option<Vec<String>>,
    pub(super) allowed_api_formats: Option<Vec<String>>,
    pub(super) allowed_models: Option<Vec<String>>,
    pub(super) rate_limit: Option<i32>,
    pub(super) concurrent_limit: Option<i32>,
    pub(super) initial_balance_usd: Option<f64>,
    pub(super) unlimited_balance: Option<bool>,
    pub(super) expire_days: Option<i32>,
    pub(super) expires_at: Option<String>,
    pub(super) auto_delete_on_expiry: Option<bool>,
}

#[derive(Debug, Default, serde::Deserialize)]
pub(super) struct AdminStandaloneApiKeyUpdateRequest {
    pub(super) name: Option<String>,
    pub(super) allowed_providers: Option<Vec<String>>,
    pub(super) allowed_api_formats: Option<Vec<String>>,
    pub(super) allowed_models: Option<Vec<String>>,
    pub(super) rate_limit: Option<i32>,
    pub(super) concurrent_limit: Option<i32>,
    pub(super) initial_balance_usd: Option<f64>,
    pub(super) unlimited_balance: Option<bool>,
    pub(super) expire_days: Option<i32>,
    pub(super) expires_at: Option<String>,
    pub(super) auto_delete_on_expiry: Option<bool>,
}

pub(super) type AdminStandaloneApiKeyUpdatePatch =
    AdminTypedObjectPatch<AdminStandaloneApiKeyUpdateRequest>;

#[derive(Debug, Default, serde::Deserialize)]
pub(super) struct AdminStandaloneApiKeyToggleRequest {
    pub(super) is_active: Option<bool>,
}

pub(super) fn build_admin_api_keys_data_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": ADMIN_API_KEYS_DATA_UNAVAILABLE_DETAIL })),
    )
        .into_response()
}

pub(super) fn build_admin_api_keys_bad_request_response(
    detail: impl Into<String>,
) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

pub(super) fn build_admin_api_keys_not_found_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "API密钥不存在" })),
    )
        .into_response()
}

pub(super) fn admin_api_keys_id_from_path(request_path: &str) -> Option<String> {
    let value = request_path
        .strip_prefix("/api/admin/api-keys/")?
        .trim()
        .trim_matches('/')
        .to_string();
    if value.is_empty() || value.contains('/') {
        None
    } else {
        Some(value)
    }
}

pub(super) fn admin_api_keys_operator_id(
    request_context: &AdminRequestContext<'_>,
) -> Option<String> {
    request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.clone())
}

pub(super) fn admin_api_keys_parse_skip(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "skip") {
        None => Ok(0),
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| "skip must be a non-negative integer".to_string()),
    }
}

pub(super) fn admin_api_keys_parse_limit(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "limit") {
        None => Ok(100),
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| "limit must be a positive integer".to_string())?;
            if parsed == 0 || parsed > 500 {
                return Err("limit must be between 1 and 500".to_string());
            }
            Ok(parsed)
        }
    }
}

fn masked_admin_api_key_display(state: &AdminAppState<'_>, ciphertext: Option<&str>) -> String {
    masked_user_api_key_display(state, ciphertext)
}

pub(super) fn build_admin_api_key_list_item_payload(
    state: &AdminAppState<'_>,
    record: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
    total_tokens: Option<u64>,
    wallet: Option<&aether_data::repository::wallet::StoredWalletSnapshot>,
) -> serde_json::Value {
    json!({
        "id": record.api_key_id,
        "user_id": record.user_id,
        "name": record.name,
        "key_display": masked_admin_api_key_display(state, record.key_encrypted.as_deref()),
        "is_active": record.is_active,
        "is_standalone": true,
        "total_requests": record.total_requests,
        "total_tokens": total_tokens,
        "total_cost_usd": record.total_cost_usd,
        "rate_limit": record.rate_limit,
        "concurrent_limit": record.concurrent_limit,
        "allowed_providers": record.allowed_providers,
        "allowed_api_formats": record.allowed_api_formats,
        "allowed_models": record.allowed_models,
        "last_used_at": serde_json::Value::Null,
        "expires_at": format_optional_unix_secs_iso8601(record.expires_at_unix_secs),
        "created_at": serde_json::Value::Null,
        "updated_at": serde_json::Value::Null,
        "auto_delete_on_expiry": record.auto_delete_on_expiry,
        "wallet": serialize_admin_system_users_export_wallet(wallet),
    })
}

pub(super) fn build_admin_api_key_detail_payload(
    state: &AdminAppState<'_>,
    record: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
    total_tokens: u64,
    wallet: Option<&aether_data::repository::wallet::StoredWalletSnapshot>,
) -> serde_json::Value {
    json!({
        "id": record.api_key_id,
        "user_id": record.user_id,
        "name": record.name,
        "key_display": masked_admin_api_key_display(state, record.key_encrypted.as_deref()),
        "is_active": record.is_active,
        "is_standalone": true,
        "total_requests": record.total_requests,
        "total_tokens": total_tokens,
        "total_cost_usd": record.total_cost_usd,
        "rate_limit": record.rate_limit,
        "concurrent_limit": record.concurrent_limit,
        "allowed_providers": record.allowed_providers,
        "allowed_api_formats": record.allowed_api_formats,
        "allowed_models": record.allowed_models,
        "last_used_at": serde_json::Value::Null,
        "expires_at": format_optional_unix_secs_iso8601(record.expires_at_unix_secs),
        "created_at": serde_json::Value::Null,
        "updated_at": serde_json::Value::Null,
        "auto_delete_on_expiry": record.auto_delete_on_expiry,
        "wallet": serialize_admin_system_users_export_wallet(wallet),
    })
}

pub(super) async fn admin_api_key_total_tokens_by_ids(
    state: &AdminAppState<'_>,
    api_key_ids: &[String],
) -> Result<std::collections::BTreeMap<String, u64>, GatewayError> {
    if api_key_ids.is_empty() || !state.has_usage_data_reader() {
        return Ok(std::collections::BTreeMap::new());
    }

    state
        .summarize_usage_total_tokens_by_api_key_ids(api_key_ids)
        .await
}
