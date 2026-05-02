use super::support_auth::auth_session::{
    build_auth_login_success_response, create_auth_token, decode_auth_token,
};
use super::{
    build_auth_error_response, build_auth_json_response, extract_client_device_id, http, json,
    query_param_value, resolve_authenticated_local_user, AppState, Body, Bytes,
    GatewayPublicRequestContext, IntoResponse, Json, Response,
};
use aether_oauth::core::{generate_pkce_verifier, pkce_s256, OAuthError};
use aether_oauth::identity::{
    IdentityClaims, IdentityOAuthExchangeContext, IdentityOAuthService, IdentityOAuthStartContext,
};
use axum::body::to_bytes;
use axum::http::header::{LOCATION, SET_COOKIE};
use axum::http::HeaderValue;
use url::form_urlencoded;

pub(super) async fn maybe_build_local_oauth_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    _request_body: Option<&Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("oauth") {
        return None;
    }

    match decision.route_kind.as_deref() {
        Some("list_providers")
            if request_context.request_method == http::Method::GET
                && request_context.request_path == "/api/oauth/providers" =>
        {
            Some(handle_oauth_list_providers(state).await)
        }
        Some("authorize") if request_context.request_method == http::Method::GET => {
            Some(handle_oauth_authorize(state, request_context, headers).await)
        }
        Some("callback") if request_context.request_method == http::Method::GET => {
            Some(handle_oauth_callback(state, request_context, headers).await)
        }
        Some("bindable_providers")
            if request_context.request_method == http::Method::GET
                && request_context.request_path == "/api/user/oauth/bindable-providers" =>
        {
            Some(handle_oauth_bindable_providers(state, request_context, headers).await)
        }
        Some("links")
            if request_context.request_method == http::Method::GET
                && request_context.request_path == "/api/user/oauth/links" =>
        {
            Some(handle_oauth_links(state, request_context, headers).await)
        }
        Some("bind_token") if request_context.request_method == http::Method::POST => {
            Some(handle_oauth_bind_token(state, request_context, headers).await)
        }
        Some("bind") if request_context.request_method == http::Method::GET => {
            Some(handle_oauth_bind_start(state, request_context, headers).await)
        }
        Some("unbind") if request_context.request_method == http::Method::DELETE => {
            Some(handle_oauth_unbind(state, request_context, headers).await)
        }
        _ => Some(super::build_unhandled_public_support_response(
            request_context,
        )),
    }
}

async fn handle_oauth_list_providers(state: &AppState) -> Response<Body> {
    match crate::oauth::list_enabled_identity_oauth_providers(state).await {
        Ok(providers) => Json(json!({ "providers": providers })).into_response(),
        Err(err) => build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("oauth provider lookup failed: {err:?}"),
            false,
        ),
    }
}

async fn handle_oauth_bindable_providers(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if auth.user.auth_source.eq_ignore_ascii_case("ldap") {
        return Json(json!({ "providers": [] })).into_response();
    }
    match crate::oauth::list_bindable_identity_oauth_providers(state, &auth.user.id).await {
        Ok(providers) => Json(json!({ "providers": providers })).into_response(),
        Err(err) => build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("oauth provider lookup failed: {err:?}"),
            false,
        ),
    }
}

async fn handle_oauth_links(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match crate::oauth::list_identity_oauth_links(state, &auth.user.id).await {
        Ok(links) => Json(json!({ "links": links })).into_response(),
        Err(err) => build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("oauth link lookup failed: {err:?}"),
            false,
        ),
    }
}

async fn handle_oauth_authorize(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let Some(provider_type) =
        public_oauth_provider_from_path(&request_context.request_path, "authorize")
    else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "OAuth Provider 不存在",
            false,
        );
    };
    let client_device_id = match extract_client_device_id(request_context, headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    start_identity_oauth(
        state,
        &provider_type,
        client_device_id,
        crate::oauth::IdentityOAuthStateMode::Login,
        None,
        None,
    )
    .await
}

async fn handle_oauth_bind_token(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let Some(provider_type) =
        user_oauth_provider_from_path(&request_context.request_path, "bind-token")
    else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "OAuth Provider 不存在",
            false,
        );
    };
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if auth.user.auth_source.eq_ignore_ascii_case("ldap") {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "LDAP 用户不支持 OAuth 绑定",
            false,
        );
    }
    match crate::oauth::get_enabled_identity_oauth_provider_config(state, &provider_type).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::NOT_FOUND,
                "OAuth Provider 不存在或已禁用",
                false,
            )
        }
        Err(err) => return oauth_account_error_response(err),
    }
    let token = match create_auth_token(
        "oauth_bind",
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(auth.user.id)),
            ("session_id".to_string(), json!(auth.session_id)),
            ("provider_type".to_string(), json!(provider_type)),
        ]),
        chrono::Utc::now() + chrono::Duration::minutes(10),
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
    build_auth_json_response(http::StatusCode::OK, json!({ "bind_token": token }), None)
}

async fn handle_oauth_bind_start(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let Some(provider_type) = user_oauth_provider_from_path(&request_context.request_path, "bind")
    else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "OAuth Provider 不存在",
            false,
        );
    };
    let client_device_id = match extract_client_device_id(request_context, headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(bind_token) = query_param_value(
        request_context.request_query_string.as_deref(),
        "bind_token",
    ) else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "缺少绑定令牌", false);
    };
    let bind =
        match validate_bind_token(state, &provider_type, &client_device_id, &bind_token).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    start_identity_oauth(
        state,
        &provider_type,
        client_device_id,
        crate::oauth::IdentityOAuthStateMode::Bind,
        Some(bind.user_id),
        Some(bind.session_id),
    )
    .await
}

async fn handle_oauth_callback(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let Some(provider_type) =
        public_oauth_provider_from_path(&request_context.request_path, "callback")
    else {
        return redirect_oauth_error(None, "provider_unavailable");
    };
    let params = callback_params(request_context);
    if params
        .get("error")
        .is_some_and(|value| value.eq_ignore_ascii_case("access_denied"))
    {
        return redirect_oauth_error(None, "authorization_denied");
    }
    let Some(code) = params
        .get("code")
        .map(String::as_str)
        .filter(|value| !value.is_empty())
    else {
        return redirect_oauth_error(None, "invalid_callback");
    };
    let Some(nonce) = params
        .get("state")
        .map(String::as_str)
        .filter(|value| !value.is_empty())
    else {
        return redirect_oauth_error(None, "invalid_state");
    };
    let stored = match crate::oauth::consume_identity_oauth_state(state, nonce).await {
        Ok(Some(value)) => value,
        Ok(None) => return redirect_oauth_error(None, "invalid_state"),
        Err(_) => return redirect_oauth_error(None, "invalid_state"),
    };
    if stored.provider_type != provider_type {
        return redirect_oauth_error(None, "invalid_state");
    }
    let config =
        match crate::oauth::get_enabled_identity_oauth_provider_config(state, &provider_type).await
        {
            Ok(Some(value)) => value,
            Ok(None) => return redirect_oauth_error(None, "provider_disabled"),
            Err(err) => return redirect_oauth_error(None, err.code()),
        };
    let network = crate::oauth::resolve_identity_oauth_network_context(state).await;
    let exchange_ctx = IdentityOAuthExchangeContext {
        code: code.to_string(),
        state: nonce.to_string(),
        pkce_verifier: stored.pkce_verifier.clone(),
        network,
    };
    let executor = crate::oauth::GatewayOAuthHttpExecutor::from_app(state);
    let service = IdentityOAuthService::with_builtin_providers();
    let claims = match service.login(&executor, &config, &exchange_ctx).await {
        Ok(outcome) => outcome.claims,
        Err(err) => {
            return redirect_oauth_error(
                Some(&config.frontend_callback_url),
                oauth_error_code(&err),
            )
        }
    };

    match stored.mode {
        crate::oauth::IdentityOAuthStateMode::Login => {
            complete_oauth_login(
                state,
                headers,
                &config.frontend_callback_url,
                stored.client_device_id,
                claims,
            )
            .await
        }
        crate::oauth::IdentityOAuthStateMode::Bind => {
            complete_oauth_bind(state, &config.frontend_callback_url, stored, claims).await
        }
    }
}

async fn handle_oauth_unbind(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let Some(provider_type) =
        user_oauth_provider_from_path_without_suffix(&request_context.request_path)
    else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "OAuth Provider 不存在",
            false,
        );
    };
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match crate::oauth::unbind_identity_oauth(state, &auth.user, &provider_type).await {
        Ok(true) => Json(json!({ "message": "解绑成功" })).into_response(),
        Ok(false) => {
            build_auth_error_response(http::StatusCode::NOT_FOUND, "OAuth 绑定不存在", false)
        }
        Err(err) => oauth_account_error_response(err),
    }
}

async fn start_identity_oauth(
    state: &AppState,
    provider_type: &str,
    client_device_id: String,
    mode: crate::oauth::IdentityOAuthStateMode,
    bind_user_id: Option<String>,
    bind_session_id: Option<String>,
) -> Response<Body> {
    let config = match crate::oauth::get_enabled_identity_oauth_provider_config(
        state,
        provider_type,
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::NOT_FOUND,
                "OAuth Provider 不存在或已禁用",
                false,
            )
        }
        Err(err) => return oauth_account_error_response(err),
    };
    let pkce_verifier = generate_pkce_verifier();
    let code_challenge = pkce_s256(&pkce_verifier);
    let stored = match mode {
        crate::oauth::IdentityOAuthStateMode::Login => {
            crate::oauth::StoredIdentityOAuthState::login(
                provider_type,
                client_device_id,
                Some(pkce_verifier),
            )
        }
        crate::oauth::IdentityOAuthStateMode::Bind => {
            let Some(user_id) = bind_user_id else {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "缺少绑定用户",
                    false,
                );
            };
            let Some(session_id) = bind_session_id else {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "缺少绑定会话",
                    false,
                );
            };
            crate::oauth::StoredIdentityOAuthState::bind(
                provider_type,
                client_device_id,
                Some(pkce_verifier),
                user_id,
                session_id,
            )
        }
    };
    if crate::oauth::save_identity_oauth_state(state, &stored)
        .await
        .is_err()
    {
        return build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "OAuth 状态存储不可用",
            false,
        );
    }
    let network = crate::oauth::resolve_identity_oauth_network_context(state).await;
    let start_ctx = IdentityOAuthStartContext {
        state: stored.nonce,
        code_challenge: Some(code_challenge),
        network,
    };
    let authorize = match IdentityOAuthService::with_builtin_providers().start(&config, &start_ctx)
    {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "OAuth Provider 不可用",
                false,
            )
        }
    };
    redirect_to(&authorize.authorize_url, None)
}

async fn complete_oauth_login(
    state: &AppState,
    headers: &http::HeaderMap,
    frontend_callback_url: &str,
    client_device_id: String,
    claims: IdentityClaims,
) -> Response<Body> {
    let user = match crate::oauth::resolve_identity_oauth_login_user(state, &claims).await {
        Ok(user) if user.is_active && !user.is_deleted => user,
        Ok(_) => return redirect_oauth_error(Some(frontend_callback_url), "provider_unavailable"),
        Err(err) => return redirect_oauth_error(Some(frontend_callback_url), err.code()),
    };
    let login_response =
        build_auth_login_success_response(state, headers, client_device_id, user).await;
    if login_response.status() != http::StatusCode::OK {
        return redirect_oauth_error(Some(frontend_callback_url), "provider_unavailable");
    }
    let set_cookies = login_response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let body = login_response.into_body();
    let body = match to_bytes(body, usize::MAX).await {
        Ok(value) => value,
        Err(_) => return redirect_oauth_error(Some(frontend_callback_url), "provider_unavailable"),
    };
    let payload = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(value) => value,
        Err(_) => return redirect_oauth_error(Some(frontend_callback_url), "provider_unavailable"),
    };
    let Some(access_token) = payload
        .get("access_token")
        .and_then(serde_json::Value::as_str)
    else {
        return redirect_oauth_error(Some(frontend_callback_url), "provider_unavailable");
    };
    let expires_in = payload
        .get("expires_in")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(24 * 60 * 60)
        .to_string();
    let mut response = redirect_to(
        frontend_callback_url,
        Some(RedirectParams::Fragment(vec![
            ("access_token", access_token.to_string()),
            ("token_type", "bearer".to_string()),
            ("expires_in", expires_in),
        ])),
    );
    for cookie in set_cookies {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}

async fn complete_oauth_bind(
    state: &AppState,
    frontend_callback_url: &str,
    stored: crate::oauth::StoredIdentityOAuthState,
    claims: IdentityClaims,
) -> Response<Body> {
    let Some(user_id) = stored.bind_user_id.as_deref() else {
        return redirect_oauth_error(Some(frontend_callback_url), "invalid_state");
    };
    let Some(session_id) = stored.bind_session_id.as_deref() else {
        return redirect_oauth_error(Some(frontend_callback_url), "invalid_state");
    };
    let user = match state.find_user_auth_by_id(user_id).await {
        Ok(Some(user)) if user.is_active && !user.is_deleted => user,
        _ => return redirect_oauth_error(Some(frontend_callback_url), "invalid_state"),
    };
    let session = match state.find_user_session(user_id, session_id).await {
        Ok(Some(session)) => session,
        _ => return redirect_oauth_error(Some(frontend_callback_url), "invalid_state"),
    };
    let now = chrono::Utc::now();
    if session.is_revoked()
        || session.is_expired(now)
        || session.client_device_id != stored.client_device_id
    {
        return redirect_oauth_error(Some(frontend_callback_url), "invalid_state");
    }
    if let Err(err) = crate::oauth::bind_identity_oauth_to_user(state, &user, &claims).await {
        return redirect_oauth_error(Some(frontend_callback_url), err.code());
    }
    redirect_to(
        frontend_callback_url,
        Some(RedirectParams::Query(vec![(
            "oauth_bound",
            claims.provider_type,
        )])),
    )
}

#[derive(Debug, Clone)]
struct ValidatedBindToken {
    user_id: String,
    session_id: String,
}

async fn validate_bind_token(
    state: &AppState,
    provider_type: &str,
    client_device_id: &str,
    bind_token: &str,
) -> Result<ValidatedBindToken, Response<Body>> {
    let payload = decode_auth_token(bind_token, "oauth_bind").map_err(|detail| {
        build_auth_error_response(http::StatusCode::UNAUTHORIZED, detail, false)
    })?;
    let token_provider = payload
        .get("provider_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if token_provider != provider_type {
        return Err(build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "绑定令牌不匹配",
            false,
        ));
    }
    let Some(user_id) = payload.get("user_id").and_then(serde_json::Value::as_str) else {
        return Err(build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "绑定令牌无效",
            false,
        ));
    };
    let Some(session_id) = payload
        .get("session_id")
        .and_then(serde_json::Value::as_str)
    else {
        return Err(build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "绑定令牌无效",
            false,
        ));
    };
    let session = state
        .find_user_session(user_id, session_id)
        .await
        .map_err(|err| {
            build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("auth session lookup failed: {err:?}"),
                false,
            )
        })?
        .ok_or_else(|| {
            build_auth_error_response(http::StatusCode::UNAUTHORIZED, "绑定会话已失效", false)
        })?;
    if session.is_revoked()
        || session.is_expired(chrono::Utc::now())
        || session.client_device_id != client_device_id
    {
        return Err(build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "绑定会话已失效",
            false,
        ));
    }
    Ok(ValidatedBindToken {
        user_id: user_id.to_string(),
        session_id: session_id.to_string(),
    })
}

fn callback_params(
    request_context: &GatewayPublicRequestContext,
) -> std::collections::BTreeMap<String, String> {
    request_context
        .request_query_string
        .as_deref()
        .map(|query| {
            form_urlencoded::parse(query.as_bytes())
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect()
        })
        .unwrap_or_default()
}

fn public_oauth_provider_from_path(path: &str, suffix: &str) -> Option<String> {
    path.strip_prefix("/api/oauth/")?
        .strip_suffix(&format!("/{suffix}"))?
        .split('/')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn user_oauth_provider_from_path(path: &str, suffix: &str) -> Option<String> {
    path.strip_prefix("/api/user/oauth/")?
        .strip_suffix(&format!("/{suffix}"))?
        .split('/')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn user_oauth_provider_from_path_without_suffix(path: &str) -> Option<String> {
    let provider_type = path.strip_prefix("/api/user/oauth/")?;
    (!provider_type.is_empty() && !provider_type.contains('/'))
        .then(|| provider_type.trim().to_ascii_lowercase())
}

fn oauth_error_code(error: &OAuthError) -> &'static str {
    match error {
        OAuthError::InvalidState => "invalid_state",
        OAuthError::UnsupportedProvider(_) | OAuthError::InvalidRequest(_) => {
            "provider_unavailable"
        }
        OAuthError::HttpStatus { .. }
        | OAuthError::InvalidResponse(_)
        | OAuthError::Transport(_) => "token_exchange_failed",
        OAuthError::Storage(_) | OAuthError::EncryptionUnavailable => "provider_unavailable",
    }
}

fn oauth_account_error_response(error: crate::oauth::IdentityOAuthAccountError) -> Response<Body> {
    let status = match error {
        crate::oauth::IdentityOAuthAccountError::ProviderUnavailable
        | crate::oauth::IdentityOAuthAccountError::Storage(_) => {
            http::StatusCode::SERVICE_UNAVAILABLE
        }
        crate::oauth::IdentityOAuthAccountError::OAuthAlreadyBound
        | crate::oauth::IdentityOAuthAccountError::AlreadyBoundProvider
        | crate::oauth::IdentityOAuthAccountError::LastOAuthBinding
        | crate::oauth::IdentityOAuthAccountError::LastLoginMethod => http::StatusCode::CONFLICT,
        _ => http::StatusCode::BAD_REQUEST,
    };
    build_auth_error_response(status, error.detail(), false)
}

enum RedirectParams {
    Query(Vec<(&'static str, String)>),
    Fragment(Vec<(&'static str, String)>),
}

fn redirect_oauth_error(frontend_callback_url: Option<&str>, code: &str) -> Response<Body> {
    redirect_to(
        frontend_callback_url.unwrap_or("/auth/callback"),
        Some(RedirectParams::Query(vec![(
            "error_code",
            code.to_string(),
        )])),
    )
}

fn redirect_to(target: &str, params: Option<RedirectParams>) -> Response<Body> {
    let location = build_redirect_location(target, params);
    let mut response = Response::new(Body::empty());
    *response.status_mut() = http::StatusCode::FOUND;
    if let Ok(value) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(LOCATION, value);
    }
    response
}

fn build_redirect_location(target: &str, params: Option<RedirectParams>) -> String {
    let Ok(mut url) = url::Url::parse(target) else {
        return target.to_string();
    };
    match params {
        Some(RedirectParams::Query(items)) => {
            {
                let mut query = url.query_pairs_mut();
                for (key, value) in items {
                    query.append_pair(key, &value);
                }
            }
            url.to_string()
        }
        Some(RedirectParams::Fragment(items)) => {
            let mut serializer = form_urlencoded::Serializer::new(String::new());
            for (key, value) in items {
                serializer.append_pair(key, &value);
            }
            url.set_fragment(Some(&serializer.finish()));
            url.to_string()
        }
        None => url.to_string(),
    }
}
