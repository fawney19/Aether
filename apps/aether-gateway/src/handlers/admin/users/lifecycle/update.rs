use super::super::{
    build_admin_users_bad_request_response, build_admin_users_data_unavailable_response,
    build_admin_users_read_only_response, normalize_admin_optional_user_email,
    normalize_admin_user_api_formats, normalize_admin_user_role, normalize_admin_user_string_list,
    normalize_admin_username, validate_admin_user_password, AdminUpdateUserPatch,
};
use super::support::{
    admin_user_id_from_detail_path, admin_user_password_policy, build_admin_user_payload,
    find_admin_export_user,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(in super::super) async fn build_admin_update_user_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(user_id) = admin_user_id_from_detail_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 user_id"));
    };
    let Some(_existing_user) = state.find_user_auth_by_id(&user_id).await? else {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    };
    let Some(request_body) = request_body else {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "请求数据验证失败" })),
        )
            .into_response());
    };
    let raw_payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "请求数据验证失败" })),
            )
                .into_response())
        }
    };
    let patch = match AdminUpdateUserPatch::from_object(raw_payload.clone()) {
        Ok(value) => value,
        Err(_) => {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "请求数据验证失败" })),
            )
                .into_response())
        }
    };
    let (field_presence, payload) = patch.into_parts();

    let email = match payload.email.as_deref() {
        Some(value) => match normalize_admin_optional_user_email(Some(value)) {
            Ok(value) => value,
            Err(detail) => {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": detail })),
                )
                    .into_response())
            }
        },
        None => None,
    };
    if let Some(email) = email.as_deref() {
        if state
            .is_other_user_auth_email_taken(email, &user_id)
            .await?
        {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": format!("邮箱已存在: {email}") })),
            )
                .into_response());
        }
    }

    let username = match payload.username.as_deref() {
        Some(value) => match normalize_admin_username(value) {
            Ok(value) => Some(value),
            Err(detail) => {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": detail })),
                )
                    .into_response())
            }
        },
        None => None,
    };
    if let Some(username) = username.as_deref() {
        if state
            .is_other_user_auth_username_taken(username, &user_id)
            .await?
        {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": format!("用户名已存在: {username}") })),
            )
                .into_response());
        }
    }

    let role = match payload.role.as_deref() {
        Some(value) => match normalize_admin_user_role(Some(value)) {
            Ok(value) => Some(value),
            Err(detail) => {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": detail })),
                )
                    .into_response())
            }
        },
        None => None,
    };
    if payload.rate_limit.is_some_and(|value| value < 0) {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "rate_limit 必须大于等于 0" })),
        )
            .into_response());
    }
    let allowed_providers = if field_presence.contains("allowed_providers") {
        match normalize_admin_user_string_list(payload.allowed_providers, "allowed_providers") {
            Ok(value) => value,
            Err(detail) => {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": detail })),
                )
                    .into_response())
            }
        }
    } else {
        None
    };
    let allowed_api_formats = if field_presence.contains("allowed_api_formats") {
        match normalize_admin_user_api_formats(payload.allowed_api_formats) {
            Ok(value) => value,
            Err(detail) => {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": detail })),
                )
                    .into_response())
            }
        }
    } else {
        None
    };
    let allowed_models = if field_presence.contains("allowed_models") {
        match normalize_admin_user_string_list(payload.allowed_models, "allowed_models") {
            Ok(value) => value,
            Err(detail) => {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": detail })),
                )
                    .into_response())
            }
        }
    } else {
        None
    };
    let needs_auth_user_write = email.is_some()
        || username.is_some()
        || payload.password.is_some()
        || role.is_some()
        || field_presence.contains("allowed_providers")
        || field_presence.contains("allowed_api_formats")
        || field_presence.contains("allowed_models")
        || payload.rate_limit.is_some()
        || payload.is_active.is_some();
    if needs_auth_user_write && !state.has_auth_user_write_capability() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法更新用户",
        ));
    }
    if payload.unlimited.is_some() && !state.has_auth_wallet_write_capability() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法更新用户钱包",
        ));
    }

    if email.is_some() || username.is_some() {
        if state
            .update_local_auth_user_profile(&user_id, email.clone(), username.clone())
            .await?
            .is_none()
        {
            return Ok((
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "用户不存在" })),
            )
                .into_response());
        }
    }

    if let Some(password) = payload.password.as_deref() {
        let password_policy = admin_user_password_policy(state).await?;
        if let Err(detail) = validate_admin_user_password(password, &password_policy) {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": detail })),
            )
                .into_response());
        }
        let password_hash = match bcrypt::hash(password, bcrypt::DEFAULT_COST) {
            Ok(value) => value,
            Err(_) => {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "密码长度不能超过72字节" })),
                )
                    .into_response())
            }
        };
        if state
            .update_local_auth_user_password_hash(&user_id, password_hash, chrono::Utc::now())
            .await?
            .is_none()
        {
            return Ok((
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "用户不存在" })),
            )
                .into_response());
        }
    }

    if role.is_some()
        || field_presence.contains("allowed_providers")
        || field_presence.contains("allowed_api_formats")
        || field_presence.contains("allowed_models")
        || payload.rate_limit.is_some()
        || payload.is_active.is_some()
    {
        if state
            .update_local_auth_user_admin_fields(
                &user_id,
                role,
                field_presence.contains("allowed_providers"),
                allowed_providers,
                field_presence.contains("allowed_api_formats"),
                allowed_api_formats,
                field_presence.contains("allowed_models"),
                allowed_models,
                payload.rate_limit,
                payload.is_active,
            )
            .await?
            .is_none()
        {
            return Ok((
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "用户不存在" })),
            )
                .into_response());
        }
    }

    if let Some(unlimited) = payload.unlimited {
        match state
            .find_wallet(aether_data::repository::wallet::WalletLookupKey::UserId(
                &user_id,
            ))
            .await?
        {
            Some(wallet) => {
                let desired_limit_mode = if unlimited { "unlimited" } else { "finite" };
                if !wallet.limit_mode.eq_ignore_ascii_case(desired_limit_mode) {
                    if state
                        .update_auth_user_wallet_limit_mode(&user_id, desired_limit_mode)
                        .await?
                        .is_none()
                    {
                        return Ok(build_admin_users_data_unavailable_response());
                    }
                }
            }
            None => {
                if state
                    .initialize_auth_user_wallet(&user_id, 0.0, unlimited)
                    .await?
                    .is_none()
                {
                    return Ok(build_admin_users_data_unavailable_response());
                }
            }
        }
    }

    let Some(user) = state.find_user_auth_by_id(&user_id).await? else {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    };
    let wallet = state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::UserId(
            &user_id,
        ))
        .await?;
    let unlimited = wallet
        .as_ref()
        .is_some_and(|wallet| wallet.limit_mode.eq_ignore_ascii_case("unlimited"));
    let export_row = find_admin_export_user(state, &user_id).await?;
    let rate_limit = export_row
        .as_ref()
        .and_then(|row| row.rate_limit)
        .or(payload.rate_limit);

    Ok(attach_admin_audit_response(
        Json(build_admin_user_payload(&user, rate_limit, unlimited)).into_response(),
        "admin_user_updated",
        "update_user",
        "user",
        &user_id,
    ))
}
