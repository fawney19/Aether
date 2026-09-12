use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::shared::{
    deserialize_optional_json_patch, normalize_user_self_feature_settings_update,
};

use super::{
    auth_email_is_verified, auth_password_policy_level, base_url_from_request,
    build_auth_error_response, build_auth_json_response, build_auth_refresh_cookie_clear_header,
    consume_auth_email_registration_proof, resolve_authenticated_local_user,
    validate_auth_register_password, AppState, GatewayPublicRequestContext,
};

const USERS_ME_PROFILE_STORAGE_UNAVAILABLE_DETAIL: &str = "用户资料存储暂不可用";
const USERS_ME_CREDENTIAL_STORAGE_UNAVAILABLE_DETAIL: &str = "用户凭证存储暂不可用";

#[derive(Deserialize)]
struct UsersMeUpdateProfileRequest {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verification_token: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_json_patch")]
    feature_settings: Option<Option<serde_json::Value>>,
}

#[derive(Deserialize)]
struct UsersMeChangePasswordRequest {
    #[serde(default, alias = "current_password")]
    old_password: Option<String>,
    new_password: String,
}

fn normalize_users_me_optional_non_empty_string(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

pub(super) async fn handle_users_me_client_config_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    remote_addr: &std::net::SocketAddr,
) -> Response<Body> {
    if let Err(response) = resolve_authenticated_local_user(state, request_context, headers).await {
        return response;
    }

    let site_name = state
        .read_system_config_json_value("site_name")
        .await
        .ok()
        .flatten()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "Aether".to_string());

    Json(json!({
        "base_url": base_url_from_request(headers, request_context, remote_addr),
        "site_name": site_name,
    }))
    .into_response()
}

pub(super) async fn handle_users_me_detail_put(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "缺少请求体", false);
    };
    let payload = match serde_json::from_slice::<UsersMeUpdateProfileRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求数据验证失败",
                false,
            )
        }
    };

    let email = normalize_users_me_optional_non_empty_string(payload.email);
    let email_verification_token = payload
        .email_verification_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let username = normalize_users_me_optional_non_empty_string(payload.username);
    let email_changed = email
        .as_deref()
        .is_some_and(|value| auth.user.email.as_deref() != Some(value));
    let feature_settings = match payload.feature_settings {
        Some(value) => {
            let current = match state.read_user_feature_settings(&auth.user.id).await {
                Ok(value) => value,
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("user feature settings lookup failed: {err:?}"),
                        false,
                    )
                }
            };
            match normalize_user_self_feature_settings_update(value, current) {
                Ok(value) => Some(value),
                Err(detail) => {
                    return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
                }
            }
        }
        None => None,
    };

    if let Some(email) = email.as_deref() {
        match state
            .is_other_user_auth_email_taken(email, &auth.user.id)
            .await
        {
            Ok(true) => {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "邮箱已被使用",
                    false,
                )
            }
            Ok(false) => {}
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("user email uniqueness lookup failed: {err:?}"),
                    false,
                )
            }
        }
    }

    if email_changed {
        let Some(verification_token) = email_verification_token else {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "修改邮箱前请先完成新邮箱验证",
                false,
            );
        };
        match auth_email_is_verified(
            state,
            email.as_deref().unwrap_or_default(),
            verification_token,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "邮箱验证凭据无效或已过期，请重新验证",
                    false,
                );
            }
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("email verification lookup failed: {err:?}"),
                    false,
                );
            }
        }
    }

    if let Some(username) = username.as_deref() {
        match state
            .is_other_user_auth_username_taken(username, &auth.user.id)
            .await
        {
            Ok(true) => {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "用户名已被使用",
                    false,
                )
            }
            Ok(false) => {}
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("user username uniqueness lookup failed: {err:?}"),
                    false,
                )
            }
        }
    }

    if email_changed {
        let verification_token =
            email_verification_token.expect("email change token checked above");
        match consume_auth_email_registration_proof(
            state,
            email.as_deref().unwrap_or_default(),
            verification_token,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => {
                return build_auth_error_response(
                    http::StatusCode::CONFLICT,
                    "邮箱验证凭据已被使用，请重新验证",
                    false,
                );
            }
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("email verification consume failed: {err:?}"),
                    false,
                );
            }
        }
    }

    match state
        .update_local_auth_user_profile(
            &auth.user.id,
            email.is_some(),
            email,
            email_changed.then_some(true),
            username,
        )
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                USERS_ME_PROFILE_STORAGE_UNAVAILABLE_DETAIL,
                false,
            )
        }
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user profile update failed: {err:?}"),
                false,
            )
        }
    }

    if let Some(feature_settings) = feature_settings {
        if let Err(err) = state
            .update_user_feature_settings(&auth.user.id, feature_settings)
            .await
        {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user feature settings update failed: {err:?}"),
                false,
            );
        }
    }
    Json(json!({ "message": "个人信息更新成功" })).into_response()
}

pub(super) async fn handle_users_me_password_patch(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "缺少请求体", false);
    };
    let payload = match serde_json::from_slice::<UsersMeChangePasswordRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求数据验证失败",
                false,
            )
        }
    };

    if auth.user.auth_source.eq_ignore_ascii_case("ldap") {
        return build_auth_error_response(
            http::StatusCode::FORBIDDEN,
            "LDAP 用户不能在此修改密码",
            false,
        );
    }

    let current_password_hash = auth
        .user
        .password_hash
        .as_deref()
        .filter(|value: &&str| !value.is_empty());
    if let Some(current_password_hash) = current_password_hash {
        let Some(old_password) = payload.old_password.as_deref() else {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "请输入当前密码",
                false,
            );
        };
        let old_password_matches =
            bcrypt::verify(old_password, current_password_hash).unwrap_or(false);
        if !old_password_matches {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, "旧密码错误", false);
        }
        let new_password_matches =
            bcrypt::verify(&payload.new_password, current_password_hash).unwrap_or(false);
        if new_password_matches {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "新密码不能与当前密码相同",
                false,
            );
        }
    }

    let password_policy = match auth_password_policy_level(state).await {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("password policy lookup failed: {err:?}"),
                false,
            )
        }
    };
    if let Err(detail) = validate_auth_register_password(&payload.new_password, &password_policy) {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
    }

    let password_hash = match bcrypt::hash(&payload.new_password, bcrypt::DEFAULT_COST) {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("password hash failed: {err:?}"),
                false,
            )
        }
    };
    let updated_at = chrono::Utc::now();
    match state
        .change_local_auth_password_and_revoke_sessions(
            &auth.user.id,
            &auth.session_id,
            current_password_hash,
            password_hash,
            updated_at,
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                USERS_ME_CREDENTIAL_STORAGE_UNAVAILABLE_DETAIL,
                true,
            )
        }
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user password update failed: {err:?}"),
                false,
            )
        }
    }

    let action = if current_password_hash.is_some() {
        "修改"
    } else {
        "设置"
    };
    build_auth_json_response(
        http::StatusCode::OK,
        json!({ "message": format!("密码{action}成功") }),
        Some(build_auth_refresh_cookie_clear_header()),
    )
}
