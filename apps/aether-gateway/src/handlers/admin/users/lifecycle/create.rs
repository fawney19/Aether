use super::super::{
    admin_default_user_initial_gift, build_admin_users_read_only_response,
    disabled_user_policy_detail, disabled_user_policy_field, normalize_admin_feature_settings,
    normalize_admin_optional_user_email, normalize_admin_user_group_ids, normalize_admin_user_role,
    normalize_admin_username, validate_admin_user_password, AdminCreateUserRequest,
};
use super::support::{admin_user_password_policy, build_admin_user_payload_with_groups};
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

pub(in super::super) async fn build_admin_create_user_response(
    state: &AdminAppState<'_>,
    _request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_user_write_capability() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法创建用户",
        ));
    }
    if !state.has_auth_wallet_write_capability() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法初始化用户钱包",
        ));
    }
    let Some(request_body) = request_body else {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "请求数据验证失败" })),
        )
            .into_response());
    };
    let raw_payload = match serde_json::from_slice::<Value>(request_body) {
        Ok(Value::Object(map)) => map,
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
    let payload = match serde_json::from_value::<AdminCreateUserRequest>(Value::Object(raw_payload))
    {
        Ok(value) => value,
        Err(_) => {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "请求数据验证失败" })),
            )
                .into_response())
        }
    };
    let feature_settings = match normalize_admin_feature_settings(payload.feature_settings) {
        Ok(value) => value,
        Err(detail) => {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": detail })),
            )
                .into_response())
        }
    };

    let email = match normalize_admin_optional_user_email(payload.email.as_deref()) {
        Ok(value) => value,
        Err(detail) => {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": detail })),
            )
                .into_response())
        }
    };
    let username = match normalize_admin_username(&payload.username) {
        Ok(value) => value,
        Err(detail) => {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": detail })),
            )
                .into_response())
        }
    };
    let role = match normalize_admin_user_role(payload.role.as_deref()) {
        Ok(value) => value,
        Err(detail) => {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": detail })),
            )
                .into_response())
        }
    };
    let password_policy = admin_user_password_policy(state).await?;
    if let Err(detail) = validate_admin_user_password(&payload.password, &password_policy) {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": detail })),
        )
            .into_response());
    }
    if payload
        .initial_gift_usd
        .is_some_and(|value| !value.is_finite() || !(0.0..=10000.0).contains(&value))
    {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "初始赠款必须在 0-10000 范围内" })),
        )
            .into_response());
    }
    let requested_group_ids = normalize_admin_user_group_ids(payload.group_ids);
    let group_ids = state
        .include_default_user_group_ids_for_role(&requested_group_ids, &role)
        .await?;
    let groups = if group_ids.is_empty() {
        Vec::new()
    } else {
        let groups = state.list_user_groups_by_ids(&group_ids).await?;
        if groups.len() != group_ids.len() {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "用户分组不存在" })),
            )
                .into_response());
        }
        groups
    };

    if let Some(email) = email.as_deref() {
        if state.find_user_auth_by_identifier(email).await?.is_some() {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": format!("邮箱已存在: {email}") })),
            )
                .into_response());
        }
    }
    if state
        .find_user_auth_by_identifier(&username)
        .await?
        .is_some()
    {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": format!("用户名已存在: {username}") })),
        )
            .into_response());
    }

    let password_hash = match bcrypt::hash(&payload.password, bcrypt::DEFAULT_COST) {
        Ok(value) => value,
        Err(_) => {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "密码长度不能超过72字节" })),
            )
                .into_response())
        }
    };
    let initial_gift_usd = if payload.unlimited {
        0.0
    } else if let Some(value) = payload.initial_gift_usd {
        value
    } else {
        admin_default_user_initial_gift(
            state
                .read_system_config_json_value("default_user_initial_gift_usd")
                .await?
                .as_ref(),
        )
    };

    let Some(user) = state
        .create_local_auth_user_with_settings(
            email,
            false,
            username,
            password_hash,
            role,
            None,
            None,
            None,
            None,
        )
        .await?
    else {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法创建用户",
        ));
    };

    if state
        .initialize_auth_user_wallet(&user.id, initial_gift_usd, payload.unlimited)
        .await?
        .is_none()
    {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法初始化用户钱包",
        ));
    }
    if !group_ids.is_empty() {
        state
            .replace_user_groups_for_user(&user.id, &group_ids)
            .await?;
    }
    let feature_settings = if feature_settings.is_some() {
        state
            .update_user_feature_settings(&user.id, feature_settings.clone())
            .await?
            .or(feature_settings)
    } else {
        None
    };

    let mut payload =
        build_admin_user_payload_with_groups(&user, None, None, payload.unlimited, &groups);
    payload["feature_settings"] = feature_settings.unwrap_or(Value::Null);

    Ok(attach_admin_audit_response(
        Json(payload).into_response(),
        "admin_user_created",
        "create_user",
        "user",
        &user.id,
    ))
}
