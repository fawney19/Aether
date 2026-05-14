use super::super::{
    build_admin_users_bad_request_response, build_admin_users_data_unavailable_response,
    build_admin_users_read_only_response, disabled_user_policy_detail, disabled_user_policy_field,
    normalize_admin_feature_settings, normalize_admin_optional_user_email,
    normalize_admin_user_group_ids, normalize_admin_user_role, normalize_admin_username,
    validate_admin_user_password, AdminUpdateUserPatch,
};
use super::support::{
    admin_user_id_from_detail_path, admin_user_password_policy,
    build_admin_user_payload_with_groups, find_admin_export_user,
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
use serde_json::{json, Value};

pub(in super::super) async fn build_admin_update_user_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(user_id) = admin_user_id_from_detail_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 user_id"));
    };
    let Some(existing_user) = state.find_user_auth_by_id(&user_id).await? else {
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
    if let Some(field) = disabled_user_policy_field(&raw_payload) {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": disabled_user_policy_detail(field) })),
        )
            .into_response());
    }
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
    let feature_settings = if field_presence.contains("feature_settings") {
        match normalize_admin_feature_settings(payload.feature_settings.flatten()) {
            Ok(value) => Some(value),
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
    let effective_role = role.as_deref().unwrap_or(existing_user.role.as_str());
    let group_ids = if field_presence.contains("group_ids") {
        let requested_group_ids = normalize_admin_user_group_ids(payload.group_ids);
        Some(
            state
                .include_default_user_group_ids_for_role(&requested_group_ids, effective_role)
                .await?,
        )
    } else if role.is_some() {
        let requested_group_ids = state
            .list_user_groups_for_user(&user_id)
            .await?
            .into_iter()
            .map(|group| group.id)
            .collect::<Vec<_>>();
        Some(
            state
                .include_default_user_group_ids_for_role(&requested_group_ids, effective_role)
                .await?,
        )
    } else {
        None
    };
    if let Some(group_ids) = group_ids.as_ref() {
        if !group_ids.is_empty() {
            let groups = state.list_user_groups_by_ids(group_ids).await?;
            if groups.len() != group_ids.len() {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "用户分组不存在" })),
                )
                    .into_response());
            }
        }
    }
    let needs_auth_user_write = email.is_some()
        || username.is_some()
        || payload.password.is_some()
        || role.is_some()
        || payload.is_active.is_some()
        || group_ids.is_some()
        || feature_settings.is_some();
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
    if let Some(group_ids) = group_ids.as_ref() {
        state
            .replace_user_groups_for_user(&user_id, group_ids)
            .await?;
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

    if role.is_some() || payload.is_active.is_some() {
        if state
            .update_local_auth_user_admin_fields(
                &user_id,
                role,
                false,
                None,
                false,
                None,
                false,
                None,
                false,
                None,
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
    if let Some(feature_settings) = feature_settings {
        state
            .update_user_feature_settings(&user_id, feature_settings)
            .await?;
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
    let groups = state.list_user_groups_for_user(&user_id).await?;
    let rate_limit = export_row.as_ref().and_then(|row| row.rate_limit);

    let mut payload = build_admin_user_payload_with_groups(
        &user,
        rate_limit,
        export_row.as_ref().map(|row| row.rate_limit_mode.as_str()),
        unlimited,
        &groups,
    );
    payload["feature_settings"] = export_row
        .as_ref()
        .and_then(|row| row.feature_settings.clone())
        .unwrap_or(Value::Null);

    Ok(attach_admin_audit_response(
        Json(payload).into_response(),
        "admin_user_updated",
        "update_user",
        "user",
        &user_id,
    ))
}
