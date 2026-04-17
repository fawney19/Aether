use std::collections::{BTreeMap, BTreeSet};

use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::shared::{
    api_key_placeholder_display, generate_gateway_api_key_plaintext,
    masked_gateway_api_key_display, normalize_optional_api_key_concurrent_limit,
};

use super::{
    build_auth_error_response, decrypt_catalog_secret_with_fallbacks,
    encrypt_catalog_secret_with_fallbacks, format_users_me_optional_unix_secs_iso8601,
    known_capability_names, normalize_user_model_capability_settings_input,
    query_param_optional_bool, resolve_authenticated_local_user,
    user_configurable_capability_names, AppState, GatewayPublicRequestContext,
};

const USERS_ME_API_KEY_WRITE_UNAVAILABLE_DETAIL: &str = "用户 API 密钥写入暂不可用";

#[derive(Debug, Deserialize)]
struct UsersMeCreateApiKeyRequest {
    name: String,
    #[serde(default)]
    rate_limit: Option<i32>,
    #[serde(default)]
    concurrent_limit: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct UsersMeUpdateApiKeyRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    rate_limit: Option<i32>,
    #[serde(default)]
    concurrent_limit: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct UsersMePatchApiKeyRequest {
    #[serde(default)]
    is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum UsersMeApiKeyProviderValue {
    ProviderId(String),
    ProviderConfig {
        provider_id: String,
        #[serde(default)]
        priority: Option<i32>,
        #[serde(default)]
        weight: Option<f64>,
        #[serde(default)]
        enabled: Option<bool>,
    },
}

#[derive(Debug, Deserialize)]
struct UsersMeUpdateApiKeyProvidersRequest {
    #[serde(default)]
    allowed_providers: Option<Vec<UsersMeApiKeyProviderValue>>,
    #[serde(default)]
    providers: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct UsersMeUpdateApiKeyCapabilitiesRequest {
    #[serde(default)]
    force_capabilities: Option<serde_json::Value>,
    #[serde(default)]
    capabilities: Option<Vec<String>>,
}

fn users_me_api_key_path_segments(request_path: &str) -> Option<Vec<&str>> {
    let raw = request_path
        .strip_prefix("/api/users/me/api-keys/")?
        .trim()
        .trim_matches('/');
    if raw.is_empty() {
        return None;
    }
    let segments = raw
        .split('/')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!segments.is_empty()).then_some(segments)
}

fn users_me_api_key_detail_id_from_path(request_path: &str) -> Option<String> {
    let segments = users_me_api_key_path_segments(request_path)?;
    (segments.len() == 1).then(|| segments[0].to_string())
}

fn users_me_api_key_nested_id_from_path(request_path: &str, suffix: &str) -> Option<String> {
    let segments = users_me_api_key_path_segments(request_path)?;
    (segments.len() == 2 && segments[1] == suffix).then(|| segments[0].to_string())
}

pub(super) fn users_me_api_key_detail_path_matches(request_path: &str) -> bool {
    users_me_api_key_detail_id_from_path(request_path).is_some()
}

pub(super) fn users_me_api_key_providers_path_matches(request_path: &str) -> bool {
    users_me_api_key_nested_id_from_path(request_path, "providers").is_some()
}

pub(super) fn users_me_api_key_capabilities_path_matches(request_path: &str) -> bool {
    users_me_api_key_nested_id_from_path(request_path, "capabilities").is_some()
}

fn users_me_masked_api_key_display(state: &AppState, ciphertext: Option<&str>) -> String {
    let Some(ciphertext) = ciphertext.map(str::trim).filter(|value| !value.is_empty()) else {
        return api_key_placeholder_display();
    };
    let Some(full_key) = decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)
    else {
        return api_key_placeholder_display();
    };
    masked_gateway_api_key_display(Some(full_key.as_str()))
}

fn build_users_me_api_key_writer_unavailable_response() -> Response<Body> {
    build_auth_error_response(
        http::StatusCode::SERVICE_UNAVAILABLE,
        USERS_ME_API_KEY_WRITE_UNAVAILABLE_DETAIL,
        false,
    )
}

fn build_users_me_api_key_list_payload(
    state: &AppState,
    record: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
    is_locked: bool,
) -> serde_json::Value {
    json!({
        "id": record.api_key_id,
        "name": record.name,
        "key_display": users_me_masked_api_key_display(state, record.key_encrypted.as_deref()),
        "is_active": record.is_active,
        "is_locked": is_locked,
        "last_used_at": serde_json::Value::Null,
        "created_at": serde_json::Value::Null,
        "total_requests": record.total_requests,
        "total_cost_usd": record.total_cost_usd,
        "rate_limit": record.rate_limit,
        "concurrent_limit": record.concurrent_limit,
        "allowed_providers": record.allowed_providers,
        "force_capabilities": record.force_capabilities,
    })
}

fn build_users_me_api_key_detail_payload(
    state: &AppState,
    record: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
    is_locked: bool,
) -> serde_json::Value {
    json!({
        "id": record.api_key_id,
        "name": record.name,
        "key_display": users_me_masked_api_key_display(state, record.key_encrypted.as_deref()),
        "is_active": record.is_active,
        "is_locked": is_locked,
        "allowed_providers": record.allowed_providers,
        "force_capabilities": record.force_capabilities,
        "rate_limit": record.rate_limit,
        "concurrent_limit": record.concurrent_limit,
        "last_used_at": serde_json::Value::Null,
        "expires_at": format_users_me_optional_unix_secs_iso8601(record.expires_at_unix_secs),
        "created_at": serde_json::Value::Null,
    })
}

fn normalize_users_me_required_api_key_name(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("API密钥名称不能为空".to_string());
    }
    Ok(trimmed.chars().take(100).collect())
}

fn generate_users_me_api_key_plaintext() -> String {
    generate_gateway_api_key_plaintext()
}

fn hash_users_me_api_key(value: &str) -> String {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn normalize_users_me_api_key_providers(
    payload: UsersMeUpdateApiKeyProvidersRequest,
) -> Result<Option<Vec<String>>, String> {
    let values = if let Some(values) = payload.allowed_providers {
        values
            .into_iter()
            .map(|value| match value {
                UsersMeApiKeyProviderValue::ProviderId(provider_id) => provider_id,
                UsersMeApiKeyProviderValue::ProviderConfig { provider_id, .. } => provider_id,
            })
            .collect::<Vec<_>>()
    } else if let Some(values) = payload.providers {
        values
    } else {
        return Ok(None);
    };

    let mut normalized = Vec::new();
    let mut seen = BTreeSet::new();
    for value in values {
        let provider_id = value.trim();
        if provider_id.is_empty() {
            return Err("提供商ID不能为空".to_string());
        }
        if seen.insert(provider_id.to_string()) {
            normalized.push(provider_id.to_string());
        }
    }
    Ok(Some(normalized))
}

fn normalize_users_me_api_key_force_capabilities(
    payload: UsersMeUpdateApiKeyCapabilitiesRequest,
) -> Result<Option<serde_json::Value>, String> {
    if let Some(capabilities) = payload.capabilities {
        let mut map = serde_json::Map::new();
        for capability in capabilities {
            let capability = capability.trim();
            if capability.is_empty() {
                return Err("能力名称不能为空".to_string());
            }
            map.insert(capability.to_string(), serde_json::Value::Bool(true));
        }
        return validate_users_me_force_capabilities(Some(serde_json::Value::Object(map)));
    }
    validate_users_me_force_capabilities(payload.force_capabilities)
}

fn validate_users_me_force_capabilities(
    value: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let Some(value) = normalize_user_model_capability_settings_input(value) else {
        return Ok(None);
    };
    let Some(map) = value.as_object() else {
        return Err("force_capabilities 必须是对象类型".to_string());
    };

    let user_configurable = user_configurable_capability_names();
    let known_capabilities = known_capability_names();
    for (capability_name, capability_value) in map {
        if !known_capabilities.contains(capability_name.as_str()) {
            return Err(format!("未知的能力类型: {capability_name}"));
        }
        if !user_configurable.contains(capability_name.as_str()) {
            return Err(format!("能力 {capability_name} 不支持用户配置"));
        }
        if !capability_value.is_boolean() {
            return Err(format!("能力 {capability_name} 的值必须是布尔类型"));
        }
    }

    Ok(Some(value))
}

pub(super) async fn handle_users_me_api_keys_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut records = match state
        .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&auth.user.id))
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user api key lookup failed: {err:?}"),
                false,
            )
        }
    };
    records.retain(|record| !record.is_standalone);
    records.sort_by(|left, right| left.api_key_id.cmp(&right.api_key_id));

    let snapshot_ids = records
        .iter()
        .map(|record| record.api_key_id.clone())
        .collect::<Vec<_>>();
    let snapshot_by_id = match state
        .data
        .list_auth_api_key_snapshots_by_ids(&snapshot_ids)
        .await
    {
        Ok(value) => value
            .into_iter()
            .map(|snapshot| (snapshot.api_key_id.clone(), snapshot))
            .collect::<BTreeMap<_, _>>(),
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user api key snapshot lookup failed: {err:?}"),
                false,
            )
        }
    };

    Json(
        records
            .iter()
            .map(|record| {
                let is_locked = snapshot_by_id
                    .get(&record.api_key_id)
                    .map(|snapshot| snapshot.api_key_is_locked)
                    .unwrap_or(false);
                build_users_me_api_key_list_payload(state, record, is_locked)
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

pub(super) async fn handle_users_me_api_key_detail_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(api_key_id) = users_me_api_key_detail_id_from_path(&request_context.request_path)
    else {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "API密钥不存在", false);
    };
    let include_key = query_param_optional_bool(
        request_context.request_query_string.as_deref(),
        "include_key",
    )
    .unwrap_or(false);

    let records = match state
        .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&auth.user.id))
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user api key lookup failed: {err:?}"),
                false,
            )
        }
    };
    let Some(record) = records
        .into_iter()
        .find(|record| !record.is_standalone && record.api_key_id == api_key_id)
    else {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "API密钥不存在", false);
    };

    if include_key {
        let Some(ciphertext) = record.key_encrypted.as_deref().map(str::trim) else {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "该密钥没有存储完整密钥信息",
                false,
            );
        };
        if ciphertext.is_empty() {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "该密钥没有存储完整密钥信息",
                false,
            );
        }
        let Some(full_key) =
            decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)
        else {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                "解密密钥失败",
                false,
            );
        };
        return Json(json!({ "key": full_key })).into_response();
    }

    let snapshot_ids = vec![api_key_id.clone()];
    let is_locked = match state
        .data
        .list_auth_api_key_snapshots_by_ids(&snapshot_ids)
        .await
    {
        Ok(value) => value
            .into_iter()
            .find(|snapshot| snapshot.api_key_id == api_key_id)
            .map(|snapshot| snapshot.api_key_is_locked)
            .unwrap_or(false),
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user api key snapshot lookup failed: {err:?}"),
                false,
            )
        }
    };

    Json(build_users_me_api_key_detail_payload(
        state, &record, is_locked,
    ))
    .into_response()
}

async fn resolve_users_me_api_key_snapshot_by_id(
    state: &AppState,
    user_id: &str,
    api_key_id: &str,
) -> Result<crate::data::auth::GatewayAuthApiKeySnapshot, Response<Body>> {
    let snapshot = match state
        .data
        .read_auth_api_key_snapshot(
            user_id,
            api_key_id,
            chrono::Utc::now().timestamp().max(0) as u64,
        )
        .await
    {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            return Err(build_auth_error_response(
                http::StatusCode::NOT_FOUND,
                "API密钥不存在",
                false,
            ))
        }
        Err(err) => {
            return Err(build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user api key snapshot lookup failed: {err:?}"),
                false,
            ))
        }
    };
    if snapshot.api_key_is_standalone {
        return Err(build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "API密钥不存在",
            false,
        ));
    }
    Ok(snapshot)
}

fn ensure_users_me_api_key_mutable(
    snapshot: &crate::data::auth::GatewayAuthApiKeySnapshot,
) -> Result<(), Response<Body>> {
    if snapshot.api_key_is_locked {
        return Err(build_auth_error_response(
            http::StatusCode::FORBIDDEN,
            "该密钥已被管理员锁定，无法修改",
            false,
        ));
    }
    Ok(())
}

pub(super) async fn handle_users_me_api_key_create(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    if !state.data.has_auth_api_key_writer() {
        return build_users_me_api_key_writer_unavailable_response();
    }
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "请求数据验证失败", false);
    };
    let payload = match serde_json::from_slice::<UsersMeCreateApiKeyRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求数据验证失败",
                false,
            )
        }
    };
    let name = match normalize_users_me_required_api_key_name(&payload.name) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    let rate_limit = payload.rate_limit.unwrap_or(0);
    if rate_limit < 0 {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "rate_limit 必须大于等于 0",
            false,
        );
    }
    let concurrent_limit =
        match normalize_optional_api_key_concurrent_limit(payload.concurrent_limit) {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
            }
        };

    let plaintext_key = generate_users_me_api_key_plaintext();
    let Some(key_encrypted) = encrypt_catalog_secret_with_fallbacks(state, &plaintext_key) else {
        return build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            "API密钥加密失败",
            false,
        );
    };
    let record = aether_data::repository::auth::CreateUserApiKeyRecord {
        user_id: auth.user.id.clone(),
        api_key_id: uuid::Uuid::new_v4().to_string(),
        key_hash: hash_users_me_api_key(&plaintext_key),
        key_encrypted: Some(key_encrypted),
        name: Some(name.clone()),
        allowed_providers: None,
        allowed_api_formats: None,
        allowed_models: None,
        rate_limit,
        concurrent_limit,
        force_capabilities: None,
        is_active: true,
        expires_at_unix_secs: None,
        auto_delete_on_expiry: false,
        total_requests: 0,
        total_cost_usd: 0.0,
    };
    let Some(created) = (match state.create_user_api_key(record).await {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user api key create failed: {err:?}"),
                false,
            )
        }
    }) else {
        return build_users_me_api_key_writer_unavailable_response();
    };

    Json(json!({
        "id": created.api_key_id,
        "name": created.name,
        "key": plaintext_key,
        "key_display": users_me_masked_api_key_display(state, created.key_encrypted.as_deref()),
        "rate_limit": created.rate_limit,
        "concurrent_limit": created.concurrent_limit,
        "message": "API密钥创建成功",
    }))
    .into_response()
}

pub(super) async fn handle_users_me_api_key_update(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    if !state.data.has_auth_api_key_writer() {
        return build_users_me_api_key_writer_unavailable_response();
    }
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(api_key_id) = users_me_api_key_detail_id_from_path(&request_context.request_path)
    else {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "API密钥不存在", false);
    };
    let snapshot =
        match resolve_users_me_api_key_snapshot_by_id(state, &auth.user.id, &api_key_id).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(response) = ensure_users_me_api_key_mutable(&snapshot) {
        return response;
    }
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "请求数据验证失败", false);
    };
    let payload = match serde_json::from_slice::<UsersMeUpdateApiKeyRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求数据验证失败",
                false,
            )
        }
    };
    let name = match payload.name {
        Some(value) => match normalize_users_me_required_api_key_name(&value) {
            Ok(value) => Some(value),
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
            }
        },
        None => None,
    };
    let rate_limit = payload.rate_limit;
    if rate_limit.is_some_and(|value| value < 0) {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "rate_limit 必须大于等于 0",
            false,
        );
    }
    let concurrent_limit =
        match normalize_optional_api_key_concurrent_limit(payload.concurrent_limit) {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
            }
        };

    let Some(updated) = (match state
        .update_user_api_key_basic(aether_data::repository::auth::UpdateUserApiKeyBasicRecord {
            user_id: auth.user.id.clone(),
            api_key_id: snapshot.api_key_id.clone(),
            name,
            rate_limit,
            concurrent_limit,
        })
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user api key update failed: {err:?}"),
                false,
            )
        }
    }) else {
        return build_users_me_api_key_writer_unavailable_response();
    };

    let mut payload =
        build_users_me_api_key_detail_payload(state, &updated, snapshot.api_key_is_locked);
    payload["message"] = json!("API密钥已更新");
    Json(payload).into_response()
}

pub(super) async fn handle_users_me_api_key_patch(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    if !state.data.has_auth_api_key_writer() {
        return build_users_me_api_key_writer_unavailable_response();
    }
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(api_key_id) = users_me_api_key_detail_id_from_path(&request_context.request_path)
    else {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "API密钥不存在", false);
    };
    let snapshot =
        match resolve_users_me_api_key_snapshot_by_id(state, &auth.user.id, &api_key_id).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(response) = ensure_users_me_api_key_mutable(&snapshot) {
        return response;
    }
    let desired_is_active = if let Some(request_body) = request_body {
        match serde_json::from_slice::<UsersMePatchApiKeyRequest>(request_body) {
            Ok(value) => value.is_active.unwrap_or(!snapshot.api_key_is_active),
            Err(_) => {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "请求数据验证失败",
                    false,
                )
            }
        }
    } else {
        !snapshot.api_key_is_active
    };

    let Some(updated) = (match state
        .set_user_api_key_active(&auth.user.id, &snapshot.api_key_id, desired_is_active)
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user api key toggle failed: {err:?}"),
                false,
            )
        }
    }) else {
        return build_users_me_api_key_writer_unavailable_response();
    };

    Json(json!({
        "id": updated.api_key_id,
        "is_active": updated.is_active,
        "message": format!("API密钥已{}", if updated.is_active { "启用" } else { "禁用" }),
    }))
    .into_response()
}

pub(super) async fn handle_users_me_api_key_delete(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.data.has_auth_api_key_writer() {
        return build_users_me_api_key_writer_unavailable_response();
    }
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(api_key_id) = users_me_api_key_detail_id_from_path(&request_context.request_path)
    else {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "API密钥不存在", false);
    };
    let snapshot =
        match resolve_users_me_api_key_snapshot_by_id(state, &auth.user.id, &api_key_id).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(response) = ensure_users_me_api_key_mutable(&snapshot) {
        return response;
    }

    match state
        .delete_user_api_key(&auth.user.id, &snapshot.api_key_id)
        .await
    {
        Ok(true) => Json(json!({ "message": "API密钥已删除" })).into_response(),
        Ok(false) => build_auth_error_response(http::StatusCode::NOT_FOUND, "API密钥不存在", false),
        Err(err) => build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("user api key delete failed: {err:?}"),
            false,
        ),
    }
}

pub(super) async fn handle_users_me_api_key_providers_put(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    if !state.data.has_auth_api_key_writer() {
        return build_users_me_api_key_writer_unavailable_response();
    }
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(api_key_id) =
        users_me_api_key_nested_id_from_path(&request_context.request_path, "providers")
    else {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "API密钥不存在", false);
    };
    let snapshot =
        match resolve_users_me_api_key_snapshot_by_id(state, &auth.user.id, &api_key_id).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(response) = ensure_users_me_api_key_mutable(&snapshot) {
        return response;
    }
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "请求数据验证失败", false);
    };
    let payload = match serde_json::from_slice::<UsersMeUpdateApiKeyProvidersRequest>(request_body)
    {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求数据验证失败",
                false,
            )
        }
    };
    let allowed_providers = match normalize_users_me_api_key_providers(payload) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };

    let allowed_providers = if let Some(providers) = allowed_providers {
        if state.has_provider_catalog_data_reader() {
            let catalog_providers = match state.list_provider_catalog_providers(true).await {
                Ok(value) => value,
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("provider validation failed: {err:?}"),
                        false,
                    )
                }
            };
            let mut by_key = BTreeMap::new();
            for provider in catalog_providers {
                by_key.insert(provider.id.to_ascii_lowercase(), provider.id.clone());
                by_key.insert(provider.name.to_ascii_lowercase(), provider.id);
            }
            let mut invalid = Vec::new();
            let mut normalized = Vec::new();
            for provider_id in providers {
                let key = provider_id.trim().to_ascii_lowercase();
                if let Some(mapped) = by_key.get(&key) {
                    if !normalized.iter().any(|value| value == mapped) {
                        normalized.push(mapped.clone());
                    }
                } else {
                    invalid.push(provider_id);
                }
            }
            if !invalid.is_empty() {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    format!("无效的提供商ID: {}", invalid.join(", ")),
                    false,
                );
            }
            Some(normalized)
        } else {
            let mut invalid = Vec::new();
            for provider_id in &providers {
                match state.find_active_provider_name(provider_id).await {
                    Ok(Some(_)) => {}
                    Ok(None) => invalid.push(provider_id.clone()),
                    Err(err) => {
                        return build_auth_error_response(
                            http::StatusCode::INTERNAL_SERVER_ERROR,
                            format!("provider validation failed: {err:?}"),
                            false,
                        )
                    }
                }
            }
            if !invalid.is_empty() {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    format!("无效的提供商ID: {}", invalid.join(", ")),
                    false,
                );
            }
            Some(providers)
        }
    } else {
        None
    };

    let Some(updated) = (match state
        .set_user_api_key_allowed_providers(&auth.user.id, &snapshot.api_key_id, allowed_providers)
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user api key providers update failed: {err:?}"),
                false,
            )
        }
    }) else {
        return build_users_me_api_key_writer_unavailable_response();
    };

    Json(json!({
        "message": "API密钥可用提供商已更新",
        "allowed_providers": updated.allowed_providers,
    }))
    .into_response()
}

pub(super) async fn handle_users_me_api_key_capabilities_put(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    if !state.data.has_auth_api_key_writer() {
        return build_users_me_api_key_writer_unavailable_response();
    }
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(api_key_id) =
        users_me_api_key_nested_id_from_path(&request_context.request_path, "capabilities")
    else {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "API密钥不存在", false);
    };
    let snapshot =
        match resolve_users_me_api_key_snapshot_by_id(state, &auth.user.id, &api_key_id).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(response) = ensure_users_me_api_key_mutable(&snapshot) {
        return response;
    }
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "请求数据验证失败", false);
    };
    let payload =
        match serde_json::from_slice::<UsersMeUpdateApiKeyCapabilitiesRequest>(request_body) {
            Ok(value) => value,
            Err(_) => {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "请求数据验证失败",
                    false,
                )
            }
        };
    let force_capabilities = match normalize_users_me_api_key_force_capabilities(payload) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };

    let Some(updated) = (match state
        .set_user_api_key_force_capabilities(
            &auth.user.id,
            &snapshot.api_key_id,
            force_capabilities,
        )
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user api key capabilities update failed: {err:?}"),
                false,
            )
        }
    }) else {
        return build_users_me_api_key_writer_unavailable_response();
    };

    Json(json!({
        "message": "API密钥能力配置已更新",
        "force_capabilities": updated.force_capabilities.unwrap_or(serde_json::Value::Null),
    }))
    .into_response()
}
