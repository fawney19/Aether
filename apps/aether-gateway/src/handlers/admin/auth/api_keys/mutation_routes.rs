use super::shared::{
    admin_api_key_total_tokens_by_ids, admin_api_keys_id_from_path, admin_api_keys_operator_id,
    build_admin_api_key_detail_payload, build_admin_api_keys_bad_request_response,
    build_admin_api_keys_data_unavailable_response, build_admin_api_keys_not_found_response,
    AdminStandaloneApiKeyCreateRequest, AdminStandaloneApiKeyFieldPresence,
    AdminStandaloneApiKeyToggleRequest, AdminStandaloneApiKeyUpdateRequest,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::handlers::admin::users::{
    default_admin_user_api_key_name, format_optional_unix_secs_iso8601,
    generate_admin_user_api_key_plaintext, hash_admin_user_api_key, masked_user_api_key_display,
    normalize_admin_optional_api_key_name, normalize_admin_user_api_formats,
    normalize_admin_user_string_list,
};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn build_admin_create_api_key_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_api_key_writer() {
        return Ok(build_admin_api_keys_data_unavailable_response());
    }

    let Some(operator_id) = admin_api_keys_operator_id(request_context) else {
        return Ok(build_admin_api_keys_data_unavailable_response());
    };
    let Some(request_body) = request_body else {
        return Ok(build_admin_api_keys_bad_request_response(
            "请求数据验证失败",
        ));
    };
    let payload = match serde_json::from_slice::<AdminStandaloneApiKeyCreateRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return Ok(build_admin_api_keys_bad_request_response(
                "请求数据验证失败",
            ));
        }
    };
    if payload.initial_balance_usd.is_some()
        || payload.unlimited_balance.is_some()
        || payload.expire_days.is_some()
        || payload.expires_at.is_some()
        || payload.auto_delete_on_expiry.is_some()
    {
        return Ok(build_admin_api_keys_bad_request_response(
            "当前仅支持 name、rate_limit、allowed_providers、allowed_api_formats、allowed_models 字段",
        ));
    }

    let name = match normalize_admin_optional_api_key_name(payload.name) {
        Ok(Some(value)) => value,
        Ok(None) => default_admin_user_api_key_name(),
        Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
    };
    let allowed_providers =
        match normalize_admin_user_string_list(payload.allowed_providers, "allowed_providers") {
            Ok(value) => value,
            Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
        };
    let allowed_api_formats = match normalize_admin_user_api_formats(payload.allowed_api_formats) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
    };
    let allowed_models =
        match normalize_admin_user_string_list(payload.allowed_models, "allowed_models") {
            Ok(value) => value,
            Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
        };
    let rate_limit = payload.rate_limit.unwrap_or(0);
    if rate_limit < 0 {
        return Ok(build_admin_api_keys_bad_request_response(
            "rate_limit 必须大于等于 0",
        ));
    }

    let plaintext_key = generate_admin_user_api_key_plaintext();
    let Some(key_encrypted) = state.encrypt_catalog_secret_with_fallbacks(&plaintext_key) else {
        return Ok((
            http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": "API密钥加密失败" })),
        )
            .into_response());
    };

    let Some(created) = state
        .create_standalone_api_key(
            aether_data::repository::auth::CreateStandaloneApiKeyRecord {
                user_id: operator_id,
                api_key_id: uuid::Uuid::new_v4().to_string(),
                key_hash: hash_admin_user_api_key(&plaintext_key),
                key_encrypted: Some(key_encrypted),
                name: Some(name),
                allowed_providers,
                allowed_api_formats,
                allowed_models,
                rate_limit,
                concurrent_limit: 5,
            },
        )
        .await?
    else {
        return Ok(build_admin_api_keys_data_unavailable_response());
    };

    Ok(attach_admin_audit_response(
        Json(json!({
            "id": created.api_key_id,
            "key": plaintext_key,
            "name": created.name,
            "key_display": masked_user_api_key_display(state, created.key_encrypted.as_deref()),
            "is_standalone": true,
            "is_active": created.is_active,
            "rate_limit": created.rate_limit,
            "allowed_providers": created.allowed_providers,
            "allowed_api_formats": created.allowed_api_formats,
            "allowed_models": created.allowed_models,
            "expires_at": format_optional_unix_secs_iso8601(created.expires_at_unix_secs),
            "wallet": serde_json::Value::Null,
            "message": "独立余额Key创建成功，请妥善保存完整密钥，后续将无法查看",
        }))
        .into_response(),
        "admin_standalone_api_key_created",
        "create_standalone_api_key",
        "api_key",
        &created.api_key_id,
    ))
}

pub(super) async fn build_admin_update_api_key_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_api_key_writer() {
        return Ok(build_admin_api_keys_data_unavailable_response());
    }

    let Some(api_key_id) = admin_api_keys_id_from_path(request_context.path()) else {
        return Ok(build_admin_api_keys_data_unavailable_response());
    };
    let Some(request_body) = request_body else {
        return Ok(build_admin_api_keys_bad_request_response(
            "请求数据验证失败",
        ));
    };
    let raw_payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => {
            return Ok(build_admin_api_keys_bad_request_response(
                "请求数据验证失败",
            ));
        }
    };
    let field_presence = AdminStandaloneApiKeyFieldPresence {
        allowed_providers: raw_payload.contains_key("allowed_providers"),
        allowed_api_formats: raw_payload.contains_key("allowed_api_formats"),
        allowed_models: raw_payload.contains_key("allowed_models"),
    };
    let payload = match serde_json::from_value::<AdminStandaloneApiKeyUpdateRequest>(
        serde_json::Value::Object(raw_payload),
    ) {
        Ok(value) => value,
        Err(_) => {
            return Ok(build_admin_api_keys_bad_request_response(
                "请求数据验证失败",
            ));
        }
    };
    if payload.initial_balance_usd.is_some()
        || payload.unlimited_balance.is_some()
        || payload.expire_days.is_some()
        || payload.expires_at.is_some()
        || payload.auto_delete_on_expiry.is_some()
    {
        return Ok(build_admin_api_keys_bad_request_response(
            "当前仅支持 name、rate_limit、allowed_providers、allowed_api_formats、allowed_models 字段",
        ));
    }

    let name = match normalize_admin_optional_api_key_name(payload.name) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
    };
    if payload.rate_limit.is_some_and(|value| value < 0) {
        return Ok(build_admin_api_keys_bad_request_response(
            "rate_limit 必须大于等于 0",
        ));
    }
    let allowed_providers = if field_presence.allowed_providers {
        match normalize_admin_user_string_list(payload.allowed_providers, "allowed_providers") {
            Ok(value) => Some(value),
            Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
        }
    } else {
        None
    };
    let allowed_api_formats = if field_presence.allowed_api_formats {
        match normalize_admin_user_api_formats(payload.allowed_api_formats) {
            Ok(value) => Some(value),
            Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
        }
    } else {
        None
    };
    let allowed_models = if field_presence.allowed_models {
        match normalize_admin_user_string_list(payload.allowed_models, "allowed_models") {
            Ok(value) => Some(value),
            Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
        }
    } else {
        None
    };

    let Some(updated) = state
        .update_standalone_api_key_basic(
            aether_data::repository::auth::UpdateStandaloneApiKeyBasicRecord {
                api_key_id: api_key_id.clone(),
                name,
                rate_limit: payload.rate_limit,
                allowed_providers,
                allowed_api_formats,
                allowed_models,
            },
        )
        .await?
    else {
        return Ok(build_admin_api_keys_not_found_response());
    };

    let wallet = state
        .list_wallet_snapshots_by_api_key_ids(std::slice::from_ref(&api_key_id))
        .await?
        .into_iter()
        .find(|wallet| wallet.api_key_id.as_deref() == Some(api_key_id.as_str()));
    let total_tokens_by_api_key_id =
        admin_api_key_total_tokens_by_ids(state, std::slice::from_ref(&api_key_id)).await?;
    let total_tokens = total_tokens_by_api_key_id
        .get(&api_key_id)
        .copied()
        .unwrap_or(0);
    let mut payload =
        build_admin_api_key_detail_payload(state, &updated, total_tokens, wallet.as_ref());
    payload["message"] = json!("API密钥已更新");
    Ok(attach_admin_audit_response(
        Json(payload).into_response(),
        "admin_standalone_api_key_updated",
        "update_standalone_api_key",
        "api_key",
        &api_key_id,
    ))
}

pub(super) async fn build_admin_toggle_api_key_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_api_key_writer() {
        return Ok(build_admin_api_keys_data_unavailable_response());
    }

    let Some(api_key_id) = admin_api_keys_id_from_path(request_context.path()) else {
        return Ok(build_admin_api_keys_data_unavailable_response());
    };

    let requested_active = match request_body {
        None => None,
        Some(request_body) if request_body.is_empty() => None,
        Some(request_body) => {
            match serde_json::from_slice::<AdminStandaloneApiKeyToggleRequest>(request_body) {
                Ok(value) => value.is_active,
                Err(_) => {
                    return Ok(build_admin_api_keys_bad_request_response(
                        "请求数据验证失败",
                    ));
                }
            }
        }
    };

    let Some(snapshot) = state
        .list_auth_api_key_snapshots_by_ids(std::slice::from_ref(&api_key_id))
        .await?
        .into_iter()
        .find(|snapshot| snapshot.api_key_id == api_key_id)
    else {
        return Ok(build_admin_api_keys_not_found_response());
    };
    if !snapshot.api_key_is_standalone {
        return Ok(build_admin_api_keys_bad_request_response("仅支持独立密钥"));
    }

    let is_active = requested_active.unwrap_or(!snapshot.api_key_is_active);
    let Some(updated) = state
        .set_standalone_api_key_active(&api_key_id, is_active)
        .await?
    else {
        return Ok(build_admin_api_keys_not_found_response());
    };

    Ok(attach_admin_audit_response(
        Json(json!({
            "id": updated.api_key_id,
            "is_active": updated.is_active,
            "message": if updated.is_active { "API密钥已启用" } else { "API密钥已禁用" },
        }))
        .into_response(),
        "admin_standalone_api_key_toggled",
        "toggle_standalone_api_key",
        "api_key",
        &api_key_id,
    ))
}

pub(super) async fn build_admin_delete_api_key_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_api_key_writer() {
        return Ok(build_admin_api_keys_data_unavailable_response());
    }

    let Some(api_key_id) = admin_api_keys_id_from_path(request_context.path()) else {
        return Ok(build_admin_api_keys_data_unavailable_response());
    };

    match state.delete_standalone_api_key(&api_key_id).await? {
        true => Ok(attach_admin_audit_response(
            Json(json!({ "message": "API密钥已删除" })).into_response(),
            "admin_standalone_api_key_deleted",
            "delete_standalone_api_key",
            "api_key",
            &api_key_id,
        )),
        false => Ok(build_admin_api_keys_not_found_response()),
    }
}
