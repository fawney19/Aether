use super::super::super::{
    build_admin_users_bad_request_response, build_admin_users_data_unavailable_response,
    build_admin_users_read_only_response, AdminCreateUserApiKeyRequest,
};
use super::super::helpers::{
    attach_audit_response, default_admin_user_api_key_name, format_optional_unix_secs_iso8601,
    generate_admin_user_api_key_plaintext, hash_admin_user_api_key, masked_user_api_key_display,
    normalize_admin_api_key_providers, normalize_admin_optional_api_key_name,
};
use super::super::paths::admin_user_id_from_api_keys_path;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::shared::normalize_optional_api_key_concurrent_limit;
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(crate) async fn build_admin_create_user_api_key_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_api_key_writer() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法创建用户 API Key",
        ));
    }

    let Some(user_id) = admin_user_id_from_api_keys_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 user_id"));
    };
    if state.find_user_auth_by_id(&user_id).await?.is_none() {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    }

    let Some(request_body) = request_body else {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "请求数据验证失败" })),
        )
            .into_response());
    };
    let payload = match serde_json::from_slice::<AdminCreateUserApiKeyRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "请求数据验证失败" })),
            )
                .into_response());
        }
    };
    if payload.allowed_api_formats.is_some()
        || payload.allowed_models.is_some()
        || payload.expire_days.is_some()
        || payload.expires_at.is_some()
        || payload.initial_balance_usd.is_some()
        || payload.unlimited_balance.unwrap_or(false)
        || payload.is_standalone.unwrap_or(false)
        || payload.auto_delete_on_expiry.unwrap_or(false)
    {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "当前仅支持 name、rate_limit、concurrent_limit、allowed_providers 字段" })),
        )
            .into_response());
    }

    let name = match normalize_admin_optional_api_key_name(payload.name) {
        Ok(Some(value)) => value,
        Ok(None) => default_admin_user_api_key_name(),
        Err(detail) => {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": detail })),
            )
                .into_response());
        }
    };
    let allowed_providers = match normalize_admin_api_key_providers(payload.allowed_providers) {
        Ok(value) => value,
        Err(detail) => {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": detail })),
            )
                .into_response());
        }
    };
    let rate_limit = payload.rate_limit.unwrap_or(0);
    if rate_limit < 0 {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "rate_limit 必须大于等于 0" })),
        )
            .into_response());
    }
    let concurrent_limit =
        match normalize_optional_api_key_concurrent_limit(payload.concurrent_limit) {
            Ok(value) => value,
            Err(detail) => {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": detail })),
                )
                    .into_response());
            }
        };

    let plaintext_key = generate_admin_user_api_key_plaintext();
    let Some(key_encrypted) = state.encrypt_catalog_secret_with_fallbacks(&plaintext_key) else {
        return Ok((
            http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": "API密钥加密失败" })),
        )
            .into_response());
    };

    let Some(created) = state
        .create_user_api_key(aether_data::repository::auth::CreateUserApiKeyRecord {
            user_id: user_id.clone(),
            api_key_id: uuid::Uuid::new_v4().to_string(),
            key_hash: hash_admin_user_api_key(&plaintext_key),
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
        })
        .await?
    else {
        return Ok(build_admin_users_data_unavailable_response());
    };

    let created = if allowed_providers.is_some() {
        match state
            .set_user_api_key_allowed_providers(&user_id, &created.api_key_id, allowed_providers)
            .await?
        {
            Some(updated) => updated,
            None => created,
        }
    } else {
        created
    };

    Ok(attach_audit_response(
        Json(json!({
            "id": created.api_key_id,
            "key": plaintext_key,
            "name": created.name,
            "key_display": masked_user_api_key_display(state, created.key_encrypted.as_deref()),
            "rate_limit": created.rate_limit,
            "concurrent_limit": created.concurrent_limit,
            "expires_at": format_optional_unix_secs_iso8601(created.expires_at_unix_secs),
            "created_at": chrono::Utc::now().to_rfc3339(),
            "message": "API Key创建成功，请妥善保存完整密钥",
        }))
        .into_response(),
        "admin_user_api_key_created",
        "create_user_api_key",
        "user_api_key",
        &created.api_key_id,
    ))
}
