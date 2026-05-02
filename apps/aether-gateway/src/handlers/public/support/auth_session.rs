use super::{
    auth_access_token_expiry_hours, auth_client_ip, auth_jwt_secret, auth_now,
    auth_refresh_cookie_name, auth_user_agent, build_auth_error_response, build_auth_json_response,
    build_auth_refresh_cookie_clear_header, build_auth_refresh_cookie_header, extract_bearer_token,
    extract_client_device_id, extract_cookie_value, http, json, AppState, Body,
    GatewayPublicRequestContext, Response, AUTH_REFRESH_TOKEN_EXPIRATION_DAYS,
};
use crate::GatewayUserSessionView;
use uuid::Uuid;

fn base64url_encode(bytes: &[u8]) -> String {
    use base64::Engine;

    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn base64url_decode(value: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;

    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| "无效的Token".to_string())
}

pub(crate) fn create_auth_token(
    token_type: &str,
    mut payload: serde_json::Map<String, serde_json::Value>,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<String, String> {
    use hmac::Mac;

    let secret = auth_jwt_secret()?;
    let header = serde_json::json!({ "alg": "HS256", "typ": "JWT" });
    payload.insert("exp".to_string(), json!(expires_at.timestamp()));
    payload.insert("type".to_string(), json!(token_type));
    let header_segment = base64url_encode(
        serde_json::to_vec(&header)
            .map_err(|_| "无法序列化JWT header".to_string())?
            .as_slice(),
    );
    let payload_segment = base64url_encode(
        serde_json::to_vec(&payload)
            .map_err(|_| "无法序列化JWT payload".to_string())?
            .as_slice(),
    );
    let signing_input = format!("{header_segment}.{payload_segment}");
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| "JWT secret 无效".to_string())?;
    mac.update(signing_input.as_bytes());
    let signature = mac.finalize().into_bytes();
    Ok(format!(
        "{header_segment}.{payload_segment}.{}",
        base64url_encode(signature.as_slice())
    ))
}

pub(crate) fn decode_auth_token(
    token: &str,
    expected_type: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    use hmac::Mac;

    let secret = auth_jwt_secret()?;
    let mut parts = token.split('.');
    let Some(header_segment) = parts.next() else {
        return Err("无效的Token".to_string());
    };
    let Some(payload_segment) = parts.next() else {
        return Err("无效的Token".to_string());
    };
    let Some(signature_segment) = parts.next() else {
        return Err("无效的Token".to_string());
    };
    if parts.next().is_some() {
        return Err("无效的Token".to_string());
    }

    let signing_input = format!("{header_segment}.{payload_segment}");
    let signature = base64url_decode(signature_segment)?;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| "JWT secret 无效".to_string())?;
    mac.update(signing_input.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| "无效的Token".to_string())?;

    let payload_bytes = base64url_decode(payload_segment)?;
    let payload = serde_json::from_slice::<serde_json::Value>(&payload_bytes)
        .map_err(|_| "无效的Token".to_string())?;
    let payload = payload
        .as_object()
        .cloned()
        .ok_or_else(|| "无效的Token".to_string())?;
    let actual_type = payload
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if actual_type != expected_type {
        return Err(format!(
            "Token类型错误: 期望 {expected_type}, 实际 {actual_type}"
        ));
    }
    let exp = payload
        .get("exp")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| "无效的Token".to_string())?;
    if exp <= auth_now().timestamp() {
        return Err("Token已过期".to_string());
    }
    Ok(payload)
}

pub(super) fn auth_token_identity_matches_user(
    payload: &serde_json::Map<String, serde_json::Value>,
    user: &aether_data::repository::users::StoredUserAuthRecord,
) -> bool {
    if let Some(token_email) = payload.get("email").and_then(serde_json::Value::as_str) {
        if user
            .email
            .as_deref()
            .is_some_and(|email| email != token_email)
        {
            return false;
        }
    }

    let Some(token_created_at) = payload
        .get("created_at")
        .and_then(serde_json::Value::as_str)
    else {
        return true;
    };
    let Some(user_created_at) = user.created_at else {
        return true;
    };
    let Ok(token_created_at) = chrono::DateTime::parse_from_rfc3339(token_created_at) else {
        return false;
    };
    let token_created_at = token_created_at.with_timezone(&chrono::Utc);
    (user_created_at - token_created_at).num_seconds().abs() <= 1
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
    let claims = match decode_auth_token(&token, "access") {
        Ok(value) => value,
        Err(detail) => {
            return Err(build_auth_error_response(
                http::StatusCode::UNAUTHORIZED,
                detail,
                false,
            ))
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
            return Err(build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("auth user lookup failed: {err:?}"),
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
            return Err(build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("auth session lookup failed: {err:?}"),
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
    if session.is_revoked() || session.is_expired(now) {
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
    build_auth_json_response(
        http::StatusCode::OK,
        build_auth_me_payload(&auth.user, wallet.as_ref()),
        None,
    )
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
    let claims = match decode_auth_token(&refresh_token, "refresh") {
        Ok(value) => value,
        Err(detail) => {
            let detail = if detail == "Token已过期" || detail == "无效的Token" {
                "刷新令牌失败".to_string()
            } else {
                detail
            };
            return build_auth_error_response(http::StatusCode::UNAUTHORIZED, detail, true);
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
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("auth user lookup failed: {err:?}"),
                true,
            )
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
    let client_device_id = match extract_client_device_id(request_context, headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let now = auth_now();
    let Some(session) = (match state.find_user_session(user_id, session_id).await {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("auth session lookup failed: {err:?}"),
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
    if session.is_revoked() || session.is_expired(now) {
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
        let _ = state
            .revoke_user_session(user_id, session_id, now, "refresh_token_reused")
            .await;
        return build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "登录会话已失效，请重新登录",
            true,
        );
    }

    let access_expires_at = now + chrono::Duration::hours(auth_access_token_expiry_hours());
    let access_token = match create_auth_token(
        "access",
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
            return build_auth_error_response(http::StatusCode::INTERNAL_SERVER_ERROR, detail, true)
        }
    };

    let mut set_cookie = None;
    if !is_prev {
        let new_refresh_token = match create_auth_token(
            "refresh",
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
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
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
        if rotated.ok() != Some(true) {
            return build_auth_error_response(http::StatusCode::UNAUTHORIZED, "刷新令牌失败", true);
        }
        set_cookie = Some(build_auth_refresh_cookie_header(&new_refresh_token));
    }

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
    client_device_id: String,
    user: aether_data::repository::users::StoredUserAuthRecord,
) -> Response<Body> {
    let now = auth_now();
    if let Err(err) = state.touch_auth_user_last_login(&user.id, now).await {
        return build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("auth last login update failed: {err:?}"),
            false,
        );
    }

    let session_id = Uuid::new_v4().to_string();
    let access_expires_at = now + chrono::Duration::hours(auth_access_token_expiry_hours());
    let refresh_expires_at = now + chrono::Duration::days(AUTH_REFRESH_TOKEN_EXPIRATION_DAYS);
    let access_token = match create_auth_token(
        "access",
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
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                detail,
                false,
            )
        }
    };
    let refresh_token = match create_auth_token(
        "refresh",
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
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
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
        auth_client_ip(headers),
        auth_user_agent(headers),
        Some(now),
        Some(now),
    ) {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("auth session build failed: {err:?}"),
                false,
            )
        }
    };
    let created = match state.create_user_session(session).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                "auth session backend unavailable",
                false,
            )
        }
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("auth session create failed: {err:?}"),
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
    let claims = decode_auth_token(&token, "access").ok()?;
    let user_id = claims.get("user_id").and_then(serde_json::Value::as_str)?;
    let session_id = claims
        .get("session_id")
        .and_then(serde_json::Value::as_str)?;
    let user = state.find_user_auth_by_id(user_id).await.ok().flatten()?;
    if !user.is_active || user.is_deleted || !auth_token_identity_matches_user(&claims, &user) {
        return None;
    }
    let client_device_id = extract_client_device_id(request_context, headers).ok()?;
    let now = auth_now();
    let session = state
        .find_user_session(user_id, session_id)
        .await
        .ok()
        .flatten()?;
    if session.is_revoked()
        || session.is_expired(now)
        || session.client_device_id != client_device_id
    {
        return None;
    }
    let _ = state
        .revoke_user_session(user_id, session_id, now, "user_logout")
        .await;
    Some(build_auth_json_response(
        http::StatusCode::OK,
        json!({ "message": "登出成功", "success": true }),
        Some(build_auth_refresh_cookie_clear_header()),
    ))
}

async fn try_auth_logout_with_refresh_cookie(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Option<Response<Body>> {
    let refresh_token = extract_cookie_value(headers, &auth_refresh_cookie_name())?;
    let claims = decode_auth_token(&refresh_token, "refresh").ok()?;
    let user_id = claims.get("user_id").and_then(serde_json::Value::as_str)?;
    let session_id = claims
        .get("session_id")
        .and_then(serde_json::Value::as_str)?;
    let client_device_id = extract_client_device_id(request_context, headers).ok()?;
    let now = auth_now();
    if let Some(session) = state
        .find_user_session(user_id, session_id)
        .await
        .ok()
        .flatten()
    {
        if !session.is_revoked()
            && !session.is_expired(now)
            && session.client_device_id == client_device_id
        {
            let _ = state
                .revoke_user_session(user_id, session_id, now, "user_logout")
                .await;
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
