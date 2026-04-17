use super::shared::{
    admin_api_key_total_tokens_by_ids, admin_api_keys_id_from_path, admin_api_keys_operator_id,
    build_admin_api_key_detail_payload, build_admin_api_keys_bad_request_response,
    build_admin_api_keys_data_unavailable_response, build_admin_api_keys_not_found_response,
    AdminStandaloneApiKeyCreateRequest, AdminStandaloneApiKeyToggleRequest,
    AdminStandaloneApiKeyUpdatePatch,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::handlers::admin::users::{
    default_admin_user_api_key_name, format_optional_unix_secs_iso8601,
    generate_admin_user_api_key_plaintext, hash_admin_user_api_key, masked_user_api_key_display,
    normalize_admin_optional_api_key_name, normalize_admin_user_api_formats,
    normalize_admin_user_string_list,
};
use crate::handlers::shared::normalize_optional_api_key_concurrent_limit;
use crate::GatewayError;
use aether_admin::system::serialize_admin_system_users_export_wallet;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

fn parse_standalone_api_key_expires_at(value: Option<&str>) -> Result<Option<u64>, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if let Ok(date) = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let Some(expires_at) = date.and_hms_opt(23, 59, 59) else {
            return Err("expires_at 超出有效时间范围".to_string());
        };
        return u64::try_from(expires_at.and_utc().timestamp())
            .map(Some)
            .map_err(|_| "expires_at 超出有效时间范围".to_string());
    }
    let parsed = chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|_| "expires_at 必须是 YYYY-MM-DD 或 RFC3339 时间".to_string())?;
    u64::try_from(parsed.timestamp())
        .map(Some)
        .map_err(|_| "expires_at 超出有效时间范围".to_string())
}

fn normalize_standalone_initial_balance(
    initial_balance_usd: Option<f64>,
    unlimited_balance: Option<bool>,
) -> Result<(f64, bool), String> {
    let unlimited = unlimited_balance.unwrap_or(initial_balance_usd.is_none());
    if unlimited {
        return Ok((0.0, true));
    }
    let Some(initial_balance_usd) = initial_balance_usd else {
        return Err("initial_balance_usd 必须大于 0".to_string());
    };
    if !initial_balance_usd.is_finite() || initial_balance_usd <= 0.0 {
        return Err("initial_balance_usd 必须大于 0".to_string());
    }
    Ok((initial_balance_usd, false))
}

pub(super) async fn build_admin_create_api_key_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_api_key_writer() || !state.has_auth_wallet_write_capability() {
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
    if payload.expire_days.is_some() {
        return Ok(build_admin_api_keys_bad_request_response(
            "expire_days 暂不支持，请改用 expires_at",
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
    if payload.rate_limit.is_some_and(|value| value < 0) {
        return Ok(build_admin_api_keys_bad_request_response(
            "rate_limit 必须大于等于 0",
        ));
    }
    let concurrent_limit =
        match normalize_optional_api_key_concurrent_limit(payload.concurrent_limit) {
            Ok(value) => value,
            Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
        };
    let (initial_balance_usd, unlimited_balance) = match normalize_standalone_initial_balance(
        payload.initial_balance_usd,
        payload.unlimited_balance,
    ) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
    };
    let expires_at_unix_secs =
        match parse_standalone_api_key_expires_at(payload.expires_at.as_deref()) {
            Ok(value) => value,
            Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
        };
    let auto_delete_on_expiry = payload.auto_delete_on_expiry.unwrap_or(false);
    if auto_delete_on_expiry && expires_at_unix_secs.is_none() {
        return Ok(build_admin_api_keys_bad_request_response(
            "设置 auto_delete_on_expiry 前必须提供 expires_at",
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
                rate_limit: payload.rate_limit,
                concurrent_limit,
                force_capabilities: None,
                is_active: true,
                expires_at_unix_secs,
                auto_delete_on_expiry,
                total_requests: 0,
                total_cost_usd: 0.0,
            },
        )
        .await?
    else {
        return Ok(build_admin_api_keys_data_unavailable_response());
    };
    let wallet = match state
        .initialize_auth_api_key_wallet(&created.api_key_id, initial_balance_usd, unlimited_balance)
        .await?
    {
        Some(wallet) => wallet,
        None => return Ok(build_admin_api_keys_data_unavailable_response()),
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
            "concurrent_limit": created.concurrent_limit,
            "allowed_providers": created.allowed_providers,
            "allowed_api_formats": created.allowed_api_formats,
            "allowed_models": created.allowed_models,
            "expires_at": format_optional_unix_secs_iso8601(created.expires_at_unix_secs),
            "auto_delete_on_expiry": created.auto_delete_on_expiry,
            "wallet": serialize_admin_system_users_export_wallet(Some(&wallet)),
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
    let patch = match AdminStandaloneApiKeyUpdatePatch::from_object(raw_payload) {
        Ok(value) => value,
        Err(_) => {
            return Ok(build_admin_api_keys_bad_request_response(
                "请求数据验证失败",
            ));
        }
    };
    let null_unlimited_balance =
        patch.contains("unlimited_balance") && patch.is_null("unlimited_balance");
    let null_auto_delete_on_expiry =
        patch.contains("auto_delete_on_expiry") && patch.is_null("auto_delete_on_expiry");
    let (field_presence, payload) = patch.into_parts();
    if null_unlimited_balance {
        return Ok(build_admin_api_keys_bad_request_response(
            "unlimited_balance 必须是布尔值",
        ));
    }
    if null_auto_delete_on_expiry {
        return Ok(build_admin_api_keys_bad_request_response(
            "auto_delete_on_expiry 必须是布尔值",
        ));
    }
    if payload.expire_days.is_some() {
        return Ok(build_admin_api_keys_bad_request_response(
            "expire_days 暂不支持，请改用 expires_at",
        ));
    }

    let Some(existing) = state
        .find_auth_api_key_export_standalone_record_by_id(&api_key_id)
        .await?
    else {
        return Ok(build_admin_api_keys_not_found_response());
    };

    let name = match normalize_admin_optional_api_key_name(payload.name) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
    };
    if payload.rate_limit.is_some_and(|value| value < 0) {
        return Ok(build_admin_api_keys_bad_request_response(
            "rate_limit 必须大于等于 0",
        ));
    }
    let concurrent_limit =
        match normalize_optional_api_key_concurrent_limit(payload.concurrent_limit) {
            Ok(value) => value,
            Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
        };
    let allowed_providers = if field_presence.contains("allowed_providers") {
        match normalize_admin_user_string_list(payload.allowed_providers, "allowed_providers") {
            Ok(value) => Some(value),
            Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
        }
    } else {
        None
    };
    let allowed_api_formats = if field_presence.contains("allowed_api_formats") {
        match normalize_admin_user_api_formats(payload.allowed_api_formats) {
            Ok(value) => Some(value),
            Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
        }
    } else {
        None
    };
    let allowed_models = if field_presence.contains("allowed_models") {
        match normalize_admin_user_string_list(payload.allowed_models, "allowed_models") {
            Ok(value) => Some(value),
            Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
        }
    } else {
        None
    };
    let effective_expires_at_unix_secs = if field_presence.contains("expires_at") {
        match parse_standalone_api_key_expires_at(payload.expires_at.as_deref()) {
            Ok(value) => value,
            Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
        }
    } else {
        existing.expires_at_unix_secs
    };
    let effective_auto_delete_on_expiry = if field_presence.contains("auto_delete_on_expiry") {
        payload.auto_delete_on_expiry.unwrap_or(false)
    } else {
        existing.auto_delete_on_expiry
    };
    if effective_auto_delete_on_expiry && effective_expires_at_unix_secs.is_none() {
        return Ok(build_admin_api_keys_bad_request_response(
            "设置 auto_delete_on_expiry 前必须提供 expires_at",
        ));
    }

    let mut wallet = state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::ApiKeyId(
            &api_key_id,
        ))
        .await?;
    if field_presence.contains("unlimited_balance") {
        if !state.has_auth_wallet_write_capability() {
            return Ok(build_admin_api_keys_data_unavailable_response());
        }
        let desired_unlimited = payload.unlimited_balance.unwrap_or(false);
        let desired_limit_mode = if desired_unlimited {
            "unlimited"
        } else {
            "finite"
        };
        wallet = match wallet {
            Some(existing_wallet)
                if existing_wallet
                    .limit_mode
                    .eq_ignore_ascii_case(desired_limit_mode) =>
            {
                Some(existing_wallet)
            }
            Some(_) => {
                state
                    .update_auth_api_key_wallet_limit_mode(&api_key_id, desired_limit_mode)
                    .await?
            }
            None => {
                state
                    .initialize_auth_api_key_wallet(&api_key_id, 0.0, desired_unlimited)
                    .await?
            }
        };
    }

    let Some(updated) = state
        .update_standalone_api_key_basic(
            aether_data::repository::auth::UpdateStandaloneApiKeyBasicRecord {
                api_key_id: api_key_id.clone(),
                name,
                rate_limit_present: field_presence.contains("rate_limit"),
                rate_limit: payload.rate_limit,
                concurrent_limit_present: field_presence.contains("concurrent_limit"),
                concurrent_limit,
                allowed_providers,
                allowed_api_formats,
                allowed_models,
                expires_at_present: field_presence.contains("expires_at"),
                expires_at_unix_secs: if field_presence.contains("expires_at") {
                    effective_expires_at_unix_secs
                } else {
                    None
                },
                auto_delete_on_expiry_present: field_presence.contains("auto_delete_on_expiry"),
                auto_delete_on_expiry: effective_auto_delete_on_expiry,
            },
        )
        .await?
    else {
        return Ok(build_admin_api_keys_data_unavailable_response());
    };

    if wallet.is_none() {
        wallet = state
            .find_wallet(aether_data::repository::wallet::WalletLookupKey::ApiKeyId(
                &api_key_id,
            ))
            .await?;
    }
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
