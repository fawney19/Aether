use super::{
    auth_access_token_expiry_hours, auth_now, auth_refresh_cookie_name, auth_user_agent,
    build_auth_error_response, build_auth_internal_error_response, build_auth_json_response,
    build_auth_refresh_cookie_clear_header, build_auth_refresh_cookie_header, extract_bearer_token,
    extract_client_device_id, extract_client_device_id_header, extract_cookie_value, http, json,
    AppState, Body, GatewayPublicRequestContext, Response, AUTH_REFRESH_TOKEN_EXPIRATION_DAYS,
};
use crate::handlers::public::support::mark_sensitive_response_no_store;
use crate::local_auth_token::LocalAuthTokenType;
use crate::GatewayUserSessionView;
use uuid::Uuid;

fn auth_token_error_is_internal(detail: &str) -> bool {
    !matches!(detail, "无效的Token" | "Token已过期") && !detail.starts_with("Token类型错误:")
}

pub(crate) fn create_auth_token(
    token_type: LocalAuthTokenType,
    payload: serde_json::Map<String, serde_json::Value>,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<String, String> {
    crate::local_auth_token::create_local_auth_token(token_type, payload, expires_at)
}

pub(crate) fn decode_auth_token(
    token: &str,
    expected_type: LocalAuthTokenType,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    crate::local_auth_token::decode_local_auth_token(token, expected_type)
}

pub(super) fn auth_token_identity_matches_user(
    payload: &serde_json::Map<String, serde_json::Value>,
    user: &aether_data::repository::users::StoredUserAuthRecord,
) -> bool {
    crate::local_auth_token::local_auth_token_identity_matches_user(payload, user)
}

pub(crate) fn build_auth_wallet_summary_payload(
    wallet: Option<&aether_data::repository::wallet::StoredWalletSnapshot>,
) -> serde_json::Value {
    let recharge_balance = wallet.map(|value| value.balance).unwrap_or(0.0);
    let gift_balance = wallet.map(|value| value.gift_balance).unwrap_or(0.0);
    let limit_mode = wallet
        .map(|value| value.limit_mode.clone())
        .unwrap_or_else(|| "finite".to_string());
    json!({
        "id": wallet.map(|value| value.id.clone()),
        "balance": recharge_balance + gift_balance,
        "recharge_balance": recharge_balance,
        "gift_balance": gift_balance,
        "refundable_balance": recharge_balance,
        "currency": wallet.map(|value| value.currency.clone()).unwrap_or_else(|| "USD".to_string()),
        "status": wallet.map(|value| value.status.clone()).unwrap_or_else(|| "active".to_string()),
        "limit_mode": limit_mode,
        "unlimited": wallet
            .map(|value| value.limit_mode.eq_ignore_ascii_case("unlimited"))
            .unwrap_or(false),
        "total_recharged": wallet.map(|value| value.total_recharged).unwrap_or(0.0),
        "total_consumed": wallet.map(|value| value.total_consumed).unwrap_or(0.0),
        "total_refunded": wallet.map(|value| value.total_refunded).unwrap_or(0.0),
        "total_adjusted": wallet.map(|value| value.total_adjusted).unwrap_or(0.0),
        "updated_at": wallet
            .and_then(|value| {
                chrono::DateTime::<chrono::Utc>::from_timestamp(value.updated_at_unix_secs as i64, 0)
            })
            .map(|value| value.to_rfc3339()),
    })
}

fn build_auth_me_payload(
    user: &aether_data::repository::users::StoredUserAuthRecord,
    wallet: Option<&aether_data::repository::wallet::StoredWalletSnapshot>,
    feature_settings: Option<serde_json::Value>,
) -> serde_json::Value {
    let billing = build_auth_wallet_summary_payload(wallet);
    let has_password = user
        .password_hash
        .as_deref()
        .is_some_and(|value| !value.is_empty());
    json!({
        "id": user.id,
        "email": user.email,
        "username": user.username,
        "role": user.role,
        "is_active": user.is_active,
        "billing": billing,
        "allowed_providers": user.allowed_providers,
        "allowed_api_formats": user.allowed_api_formats,
        "allowed_models": user.allowed_models,
        "created_at": user.created_at.map(|value| value.to_rfc3339()),
        "last_login_at": user.last_login_at.map(|value| value.to_rfc3339()),
        "auth_source": user.auth_source,
        "has_password": has_password,
        "feature_settings": feature_settings,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct AuthenticatedLocalUserContext {
    pub(crate) user: aether_data::repository::users::StoredUserAuthRecord,
    pub(crate) session_id: String,
}

pub(crate) async fn resolve_authenticated_local_user(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Result<AuthenticatedLocalUserContext, Response<Body>> {
    let Some(token) = extract_bearer_token(headers) else {
        return Err(build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "缺少用户凭证",
            false,
        ));
    };
    let claims = match decode_auth_token(&token, LocalAuthTokenType::Access) {
        Ok(value) => value,
        Err(detail) => {
            if auth_token_error_is_internal(&detail) {
                return Err(build_auth_internal_error_response(
                    "auth_access_token_decode_failed",
                    detail,
                    false,
                ));
            }
            return Err(build_auth_error_response(
                http::StatusCode::UNAUTHORIZED,
                "无效的用户令牌",
                false,
            ));
        }
    };
    let Some(user_id) = claims.get("user_id").and_then(serde_json::Value::as_str) else {
        return Err(build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "无效的用户令牌",
            false,
        ));
    };
    let Some(session_id) = claims.get("session_id").and_then(serde_json::Value::as_str) else {
        return Err(build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "登录会话已失效，请重新登录",
            false,
        ));
    };
    let user = match state.find_user_auth_by_id(user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            return Err(build_auth_error_response(
                http::StatusCode::FORBIDDEN,
                "用户不存在或已禁用",
                false,
            ))
        }
        Err(err) => {
            return Err(build_auth_internal_error_response(
                "auth_session_user_lookup_failed",
                err,
                false,
            ))
        }
    };
    if !user.is_active || user.is_deleted {
        return Err(build_auth_error_response(
            http::StatusCode::FORBIDDEN,
            "用户不存在或已禁用",
            false,
        ));
    }
    if !auth_token_identity_matches_user(&claims, &user) {
        return Err(build_auth_error_response(
            http::StatusCode::FORBIDDEN,
            "无效的用户令牌",
            false,
        ));
    }
    let client_device_id = match extract_client_device_id(request_context, headers) {
        Ok(value) => value,
        Err(response) => return Err(response),
    };
    let now = auth_now();
    let Some(session) = (match state.find_user_session(user_id, session_id).await {
        Ok(value) => value,
        Err(err) => {
            return Err(build_auth_internal_error_response(
                "auth_session_lookup_failed",
                err,
                false,
            ))
        }
    }) else {
        return Err(build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "登录会话已失效，请重新登录",
            false,
        ));
    };
    if session.is_revoked()
        || session.is_expired(now)
        || session.security_version != user.security_version
    {
        return Err(build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "登录会话已失效，请重新登录",
            false,
        ));
    }
    if session.client_device_id != client_device_id {
        return Err(build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "设备标识与登录会话不匹配",
            false,
        ));
    }
    if session.should_touch(now) {
        let _ = state
            .touch_user_session(
                user_id,
                session_id,
                now,
                None,
                auth_user_agent(headers).as_deref(),
            )
            .await;
    }
    Ok(AuthenticatedLocalUserContext {
        user,
        session_id: session.id,
    })
}

pub(crate) async fn handle_auth_me(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let wallet = state
        .read_wallet_snapshot_for_auth(&auth.user.id, "", false)
        .await
        .ok()
        .flatten();
    let feature_settings = match state.read_user_feature_settings(&auth.user.id).await {
        Ok(value) => value,
        Err(err) => {
            return build_auth_internal_error_response(
                "auth_user_feature_settings_lookup_failed",
                err,
                false,
            )
        }
    };
    mark_sensitive_response_no_store(build_auth_json_response(
        http::StatusCode::OK,
        build_auth_me_payload(&auth.user, wallet.as_ref(), feature_settings),
        None,
    ))
}

pub(super) async fn handle_auth_refresh(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if crate::headers::header_value_str(headers, http::header::CONTENT_LENGTH.as_str())
        .as_deref()
        .is_some_and(|value| value.trim() != "0")
    {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "刷新接口不接受请求体，请使用 Cookie",
            true,
        );
    }
    let cookie_name = auth_refresh_cookie_name();
    let Some(refresh_token) = extract_cookie_value(headers, &cookie_name) else {
        return build_auth_error_response(http::StatusCode::UNAUTHORIZED, "缺少刷新令牌", true);
    };
    let claims = match decode_auth_token(&refresh_token, LocalAuthTokenType::Refresh) {
        Ok(value) => value,
        Err(detail) => {
            if auth_token_error_is_internal(&detail) {
                return build_auth_internal_error_response(
                    "auth_refresh_token_decode_failed",
                    detail,
                    true,
                );
            }
            return build_auth_error_response(http::StatusCode::UNAUTHORIZED, "刷新令牌失败", true);
        }
    };
    let Some(user_id) = claims.get("user_id").and_then(serde_json::Value::as_str) else {
        return build_auth_error_response(http::StatusCode::UNAUTHORIZED, "无效的刷新令牌", true);
    };
    let Some(session_id) = claims.get("session_id").and_then(serde_json::Value::as_str) else {
        return build_auth_error_response(http::StatusCode::UNAUTHORIZED, "无效的刷新令牌", true);
    };
    let user = match state.find_user_auth_by_id(user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::UNAUTHORIZED,
                "无效的刷新令牌",
                true,
            )
        }
        Err(err) => {
            return build_auth_internal_error_response("auth_refresh_user_lookup_failed", err, true)
        }
    };
    if !user.is_active {
        return build_auth_error_response(http::StatusCode::FORBIDDEN, "用户已禁用", true);
    }
    if user.is_deleted {
        return build_auth_error_response(http::StatusCode::FORBIDDEN, "用户不存在或已禁用", true);
    }
    if !auth_token_identity_matches_user(&claims, &user) {
        return build_auth_error_response(http::StatusCode::UNAUTHORIZED, "无效的刷新令牌", true);
    }
    // Refresh is cookie-authenticated. Requiring a non-simple custom header keeps
    // cross-site forms from rotating a victim's session via a query parameter.
    let client_device_id = match extract_client_device_id_header(headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let now = auth_now();
    let Some(session) = (match state.find_user_session(user_id, session_id).await {
        Ok(value) => value,
        Err(err) => {
            return build_auth_internal_error_response(
                "auth_refresh_session_lookup_failed",
                err,
                true,
            )
        }
    }) else {
        return build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "登录会话已失效，请重新登录",
            true,
        );
    };
    if session.is_revoked()
        || session.is_expired(now)
        || session.security_version != user.security_version
    {
        return build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "登录会话已失效，请重新登录",
            true,
        );
    }
    if session.client_device_id != client_device_id {
        return build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "设备标识与登录会话不匹配",
            true,
        );
    }
    let (is_valid, is_prev) = session.verify_refresh_token(&refresh_token, now);
    if !is_valid {
        match state
            .revoke_user_session(user_id, session_id, now, "refresh_token_reused")
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return build_auth_internal_error_response(
                    "auth_refresh_replay_revoke_failed",
                    "refresh-token replay session was not revoked",
                    true,
                )
            }
            Err(err) => {
                return build_auth_internal_error_response(
                    "auth_refresh_replay_revoke_failed",
                    err,
                    true,
                )
            }
        }
        return build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "登录会话已失效，请重新登录",
            true,
        );
    }
    if is_prev {
        return build_auth_error_response(
            http::StatusCode::CONFLICT,
            "刷新令牌已轮换，请重试请求",
            false,
        );
    }

    let access_expires_at = now + chrono::Duration::hours(auth_access_token_expiry_hours());
    let access_token = match create_auth_token(
        LocalAuthTokenType::Access,
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(user.id)),
            ("role".to_string(), json!(user.role)),
            (
                "created_at".to_string(),
                json!(user.created_at.map(|value| value.to_rfc3339())),
            ),
            ("session_id".to_string(), json!(session.id)),
        ]),
        access_expires_at,
    ) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_internal_error_response(
                "auth_refresh_access_token_create_failed",
                detail,
                true,
            )
        }
    };

    let new_refresh_token = match create_auth_token(
        LocalAuthTokenType::Refresh,
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(user.id)),
            (
                "created_at".to_string(),
                json!(user.created_at.map(|value| value.to_rfc3339())),
            ),
            ("session_id".to_string(), json!(session.id)),
            ("jti".to_string(), json!(uuid::Uuid::new_v4().to_string())),
        ]),
        now + chrono::Duration::days(AUTH_REFRESH_TOKEN_EXPIRATION_DAYS),
    ) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_internal_error_response(
                "auth_refresh_token_create_failed",
                detail,
                true,
            )
        }
    };
    let rotated = state
        .rotate_user_session_refresh_token(
            user_id,
            session_id,
            &session.refresh_token_hash,
            &GatewayUserSessionView::hash_refresh_token(&new_refresh_token),
            now,
            now + chrono::Duration::days(AUTH_REFRESH_TOKEN_EXPIRATION_DAYS),
            None,
            auth_user_agent(headers).as_deref(),
        )
        .await;
    match rotated {
        Ok(true) => {}
        Ok(false) => {
            return build_auth_error_response(
                http::StatusCode::CONFLICT,
                "刷新令牌已轮换，请重试请求",
                false,
            )
        }
        Err(err) => {
            return build_auth_internal_error_response(
                "auth_refresh_token_rotation_failed",
                err,
                true,
            )
        }
    }
    let set_cookie = Some(build_auth_refresh_cookie_header(&new_refresh_token));

    build_auth_json_response(
        http::StatusCode::OK,
        json!({
            "access_token": access_token,
            "token_type": "bearer",
            "expires_in": auth_access_token_expiry_hours() * 60 * 60,
        }),
        set_cookie,
    )
}

pub(crate) async fn build_auth_login_success_response(
    state: &AppState,
    headers: &http::HeaderMap,
    client_ip: std::net::IpAddr,
    client_device_id: String,
    user: aether_data::repository::users::StoredUserAuthRecord,
    expected_password_hash: Option<&str>,
) -> Response<Body> {
    let now = auth_now();
    if expected_password_hash.is_none() {
        if let Err(err) = state.touch_auth_user_last_login(&user.id, now).await {
            return build_auth_internal_error_response("auth_last_login_update_failed", err, false);
        }
    }

    let session_id = Uuid::new_v4().to_string();
    let access_expires_at = now + chrono::Duration::hours(auth_access_token_expiry_hours());
    let refresh_expires_at = now + chrono::Duration::days(AUTH_REFRESH_TOKEN_EXPIRATION_DAYS);
    let access_token = match create_auth_token(
        LocalAuthTokenType::Access,
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(user.id.clone())),
            ("role".to_string(), json!(user.role.clone())),
            (
                "created_at".to_string(),
                json!(user.created_at.map(|value| value.to_rfc3339())),
            ),
            ("session_id".to_string(), json!(session_id.clone())),
        ]),
        access_expires_at,
    ) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_internal_error_response(
                "auth_login_access_token_create_failed",
                detail,
                false,
            )
        }
    };
    let refresh_token = match create_auth_token(
        LocalAuthTokenType::Refresh,
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(user.id.clone())),
            (
                "created_at".to_string(),
                json!(user.created_at.map(|value| value.to_rfc3339())),
            ),
            ("session_id".to_string(), json!(session_id.clone())),
            ("jti".to_string(), json!(Uuid::new_v4().to_string())),
        ]),
        refresh_expires_at,
    ) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_internal_error_response(
                "auth_login_refresh_token_create_failed",
                detail,
                false,
            )
        }
    };
    let session = match GatewayUserSessionView::new(
        session_id,
        user.id.clone(),
        client_device_id,
        None,
        GatewayUserSessionView::hash_refresh_token(&refresh_token),
        None,
        None,
        Some(now),
        Some(refresh_expires_at),
        None,
        None,
        Some(client_ip.to_string()),
        auth_user_agent(headers),
        Some(now),
        Some(now),
    )
    .and_then(|session| session.with_security_version(user.security_version))
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_internal_error_response(
                "auth_login_session_build_failed",
                err,
                false,
            )
        }
    };
    let created_result = match expected_password_hash {
        Some(expected_password_hash) => {
            state
                .create_user_session_if_password_matches(session, expected_password_hash)
                .await
        }
        None => state.create_user_session(session).await,
    };
    let created = match created_result {
        Ok(Some(session)) => session,
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::UNAUTHORIZED,
                "邮箱或密码错误",
                false,
            );
        }
        Err(err) => {
            return build_auth_internal_error_response(
                "auth_login_session_create_failed",
                err,
                false,
            )
        }
    };

    build_auth_json_response(
        http::StatusCode::OK,
        json!({
            "access_token": access_token,
            "token_type": "bearer",
            "expires_in": auth_access_token_expiry_hours() * 60 * 60,
            "user_id": user.id,
            "email": user.email,
            "username": user.username,
            "role": user.role,
            "session_id": created.id,
        }),
        Some(build_auth_refresh_cookie_header(&refresh_token)),
    )
}

async fn try_auth_logout_with_access_token(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Option<Response<Body>> {
    let token = extract_bearer_token(headers)?;
    let claims = decode_auth_token(&token, LocalAuthTokenType::Access).ok()?;
    let user_id = claims.get("user_id").and_then(serde_json::Value::as_str)?;
    let session_id = claims
        .get("session_id")
        .and_then(serde_json::Value::as_str)?;
    let user = match state.find_user_auth_by_id(user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => return None,
        Err(err) => {
            return Some(build_auth_internal_error_response(
                "auth_logout_user_lookup_failed",
                err,
                true,
            ))
        }
    };
    if !user.is_active || user.is_deleted || !auth_token_identity_matches_user(&claims, &user) {
        return None;
    }
    let client_device_id = extract_client_device_id(request_context, headers).ok()?;
    let now = auth_now();
    let session = match state.find_user_session(user_id, session_id).await {
        Ok(Some(session)) => session,
        Ok(None) => return None,
        Err(err) => {
            return Some(build_auth_internal_error_response(
                "auth_logout_session_lookup_failed",
                err,
                true,
            ))
        }
    };
    if session.is_revoked()
        || session.is_expired(now)
        || session.security_version != user.security_version
        || session.client_device_id != client_device_id
    {
        return None;
    }
    match state
        .revoke_user_session(user_id, session_id, now, "user_logout")
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return Some(build_auth_internal_error_response(
                "auth_logout_session_revoke_failed",
                "auth session was not revoked",
                true,
            ))
        }
        Err(err) => {
            return Some(build_auth_internal_error_response(
                "auth_logout_session_revoke_failed",
                err,
                true,
            ))
        }
    }
    Some(build_auth_json_response(
        http::StatusCode::OK,
        json!({ "message": "登出成功", "success": true }),
        Some(build_auth_refresh_cookie_clear_header()),
    ))
}

async fn try_auth_logout_with_refresh_cookie(
    state: &AppState,
    _request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Option<Response<Body>> {
    let refresh_token = extract_cookie_value(headers, &auth_refresh_cookie_name())?;
    let claims = decode_auth_token(&refresh_token, LocalAuthTokenType::Refresh).ok()?;
    let user_id = claims.get("user_id").and_then(serde_json::Value::as_str)?;
    let session_id = claims
        .get("session_id")
        .and_then(serde_json::Value::as_str)?;
    let user = match state.find_user_auth_by_id(user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => return None,
        Err(err) => {
            return Some(build_auth_internal_error_response(
                "auth_logout_refresh_user_lookup_failed",
                err,
                true,
            ))
        }
    };
    if !user.is_active || user.is_deleted || !auth_token_identity_matches_user(&claims, &user) {
        return None;
    }
    // The cookie fallback must not accept the device binding from the URL: a
    // cross-site form can submit query parameters but cannot set this header.
    let client_device_id = match extract_client_device_id_header(headers) {
        Ok(value) => value,
        Err(response) => return Some(response),
    };
    let now = auth_now();
    let session = match state.find_user_session(user_id, session_id).await {
        Ok(Some(session)) => session,
        Ok(None) => return None,
        Err(err) => {
            return Some(build_auth_internal_error_response(
                "auth_logout_refresh_session_lookup_failed",
                err,
                true,
            ))
        }
    };
    if session.is_revoked()
        || session.is_expired(now)
        || session.security_version != user.security_version
        || session.client_device_id != client_device_id
    {
        return None;
    }
    let (refresh_token_is_valid, _) = session.verify_refresh_token(&refresh_token, now);
    if !refresh_token_is_valid {
        return None;
    }
    match state
        .revoke_user_session(user_id, session_id, now, "user_logout")
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return Some(build_auth_internal_error_response(
                "auth_logout_refresh_session_revoke_failed",
                "auth session was not revoked",
                true,
            ))
        }
        Err(err) => {
            return Some(build_auth_internal_error_response(
                "auth_logout_refresh_session_revoke_failed",
                err,
                true,
            ))
        }
    }
    Some(build_auth_json_response(
        http::StatusCode::OK,
        json!({ "message": "登出成功", "success": true }),
        Some(build_auth_refresh_cookie_clear_header()),
    ))
}

pub(super) async fn handle_auth_logout(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if let Some(response) = try_auth_logout_with_access_token(state, request_context, headers).await
    {
        return response;
    }
    if let Some(response) =
        try_auth_logout_with_refresh_cookie(state, request_context, headers).await
    {
        return response;
    }
    build_auth_error_response(http::StatusCode::UNAUTHORIZED, "缺少认证令牌", true)
}
