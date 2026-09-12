use super::support_auth::auth_session::build_auth_login_success_response;
use super::support_auth::local_password_login_allowed_for_user;
use super::support_auth::{auth_refresh_cookie_secure, extract_cookie_value};
use super::{
    build_auth_error_response, build_auth_json_response, extract_client_device_id, http, json,
    mark_sensitive_response_no_store, resolve_authenticated_local_user, AppState, Body, Bytes,
    GatewayPublicRequestContext, IntoResponse, Json, Response,
};
use aether_oauth::core::{generate_oauth_nonce, generate_pkce_verifier, pkce_s256, OAuthError};
use aether_oauth::identity::{
    IdentityClaims, IdentityOAuthExchangeContext, IdentityOAuthService, IdentityOAuthStartContext,
};
use axum::http::header::{LOCATION, SET_COOKIE};
use axum::http::HeaderValue;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use url::form_urlencoded;

const OAUTH_LOGIN_COOKIE_NAME_PREFIX: &str = "aether_oauth_login_";
const OAUTH_LOGIN_HOST_COOKIE_NAME_PREFIX: &str = "__Host-aether_oauth_login_";
const OAUTH_LOGIN_COOKIE_MAX_AGE_SECS: u64 = 10 * 60;
const OAUTH_LOGIN_BINDING_COMPARE_KEY: &[u8] = b"aether-oauth-login-binding-v1";

type HmacSha256 = Hmac<Sha256>;

fn browser_binding_hash(binding: &str) -> String {
    format!("{:x}", Sha256::digest(binding.as_bytes()))
}

fn oauth_login_cookie_name(state_nonce: &str) -> String {
    oauth_login_cookie_name_for_security(state_nonce, auth_refresh_cookie_secure())
}

fn oauth_login_cookie_name_for_security(state_nonce: &str, secure: bool) -> String {
    let prefix = if secure {
        OAUTH_LOGIN_HOST_COOKIE_NAME_PREFIX
    } else {
        OAUTH_LOGIN_COOKIE_NAME_PREFIX
    };
    format!("{prefix}{:x}", Sha256::digest(state_nonce.as_bytes()),)
}

fn browser_binding_matches(expected_hash: Option<&str>, binding: Option<&str>) -> bool {
    let Some(expected_hash) = expected_hash else {
        return false;
    };
    let candidate_hash = browser_binding_hash(binding.unwrap_or_default());

    // HMAC verification performs the digest comparison in constant time while still
    // handling malformed/legacy state values as a normal mismatch.
    let mut expected_mac = HmacSha256::new_from_slice(OAUTH_LOGIN_BINDING_COMPARE_KEY)
        .expect("static OAuth binding comparison key should be valid");
    expected_mac.update(expected_hash.as_bytes());
    let expected_tag = expected_mac.finalize().into_bytes();
    let mut candidate_mac = HmacSha256::new_from_slice(OAUTH_LOGIN_BINDING_COMPARE_KEY)
        .expect("static OAuth binding comparison key should be valid");
    candidate_mac.update(candidate_hash.as_bytes());
    candidate_mac.verify_slice(&expected_tag).is_ok()
}

fn build_oauth_login_cookie_header(state_nonce: &str, binding: &str) -> String {
    build_oauth_login_cookie_header_for_security(state_nonce, binding, auth_refresh_cookie_secure())
}

fn build_oauth_login_cookie_header_for_security(
    state_nonce: &str,
    binding: &str,
    secure: bool,
) -> String {
    let path = if secure { "/" } else { "/api/oauth" };
    let mut cookie = format!(
        "{}={}; Path={path}; HttpOnly; SameSite=Lax; Max-Age={}",
        oauth_login_cookie_name_for_security(state_nonce, secure),
        binding,
        OAUTH_LOGIN_COOKIE_MAX_AGE_SECS,
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

fn build_oauth_login_cookie_clear_header(state_nonce: &str) -> String {
    build_oauth_login_cookie_clear_header_for_security(state_nonce, auth_refresh_cookie_secure())
}

fn build_oauth_login_cookie_clear_header_for_security(state_nonce: &str, secure: bool) -> String {
    let path = if secure { "/" } else { "/api/oauth" };
    let mut cookie = format!(
        "{}=; Path={path}; HttpOnly; SameSite=Lax; Max-Age=0",
        oauth_login_cookie_name_for_security(state_nonce, secure),
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

fn append_oauth_login_cookie(
    mut response: Response<Body>,
    state_nonce: &str,
    binding: &str,
) -> Response<Body> {
    response.headers_mut().append(
        SET_COOKIE,
        HeaderValue::from_str(&build_oauth_login_cookie_header(state_nonce, binding))
            .expect("OAuth login cookie header should be valid"),
    );
    mark_sensitive_response_no_store(response)
}

fn append_oauth_login_cookie_clear(
    mut response: Response<Body>,
    state_nonce: &str,
) -> Response<Body> {
    response.headers_mut().append(
        SET_COOKIE,
        HeaderValue::from_str(&build_oauth_login_cookie_clear_header(state_nonce))
            .expect("OAuth login cookie clear header should be valid"),
    );
    mark_sensitive_response_no_store(response)
}

fn redirect_oauth_error_for_mode(
    frontend_callback_url: Option<&str>,
    code: &str,
    login_cookie_state_nonce: Option<&str>,
) -> Response<Body> {
    let response = redirect_oauth_error(frontend_callback_url, code);
    if let Some(state_nonce) = login_cookie_state_nonce {
        append_oauth_login_cookie_clear(response, state_nonce)
    } else {
        response
    }
}

pub(super) async fn maybe_build_local_oauth_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    client_ip: std::net::IpAddr,
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
            Some(handle_oauth_callback(state, request_context, headers, client_ip).await)
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
        return mark_sensitive_response_no_store(Json(json!({ "providers": [] })).into_response());
    }
    match crate::oauth::list_bindable_identity_oauth_providers(state, &auth.user.id).await {
        Ok(providers) => mark_sensitive_response_no_store(
            Json(json!({ "providers": providers })).into_response(),
        ),
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
        Ok(links) => {
            mark_sensitive_response_no_store(Json(json!({ "links": links })).into_response())
        }
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
    let client_device_id = match extract_client_device_id(request_context, headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let user_id = auth.user.id.clone();
    let session_id = auth.session_id.clone();
    let (authorize_url, browser_cookie) = match build_identity_oauth_authorize_url(
        state,
        &provider_type,
        client_device_id,
        crate::oauth::IdentityOAuthStateMode::Bind,
        Some(user_id),
        Some(session_id),
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    // The response contains only the provider authorization URL. The binding
    // capability remains server-side in the one-time OAuth state record.
    let response = build_auth_json_response(
        http::StatusCode::OK,
        json!({ "authorize_url": authorize_url }),
        None,
    );
    let (state_nonce, binding) = browser_cookie;
    append_oauth_login_cookie(response, &state_nonce, &binding)
}

async fn handle_oauth_bind_start(
    _state: &AppState,
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
    // Bind state is created by the authenticated POST /bind-token endpoint.
    // A browser navigation must never carry a bearer binding token in its URL.
    // The state record is selected by the provider authorization URL returned by
    // that endpoint, so this legacy GET endpoint is intentionally unavailable.
    let _ = headers;
    build_auth_error_response(
        http::StatusCode::GONE,
        "OAuth 绑定流程已更新，请重新发起绑定",
        false,
    )
}

async fn handle_oauth_callback(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    client_ip: std::net::IpAddr,
) -> Response<Body> {
    let Some(provider_type) =
        public_oauth_provider_from_path(&request_context.request_path, "callback")
    else {
        return redirect_oauth_error(None, "provider_unavailable");
    };
    let params = match callback_params(request_context.request_query_string.as_deref()) {
        Ok(params) => params,
        Err(CallbackParamsError::DuplicateState) => {
            return redirect_oauth_error(None, "invalid_state")
        }
        Err(CallbackParamsError::DuplicateCallbackParameter) => {
            return redirect_oauth_error(None, "invalid_callback")
        }
    };
    let Some(nonce) = params
        .get("state")
        .map(String::as_str)
        .filter(|value| !value.is_empty())
    else {
        return redirect_oauth_error(None, "invalid_state");
    };
    let preview = match crate::oauth::load_identity_oauth_state(state, nonce).await {
        Ok(Some(value)) => value,
        Ok(None) => return redirect_oauth_error(None, "invalid_state"),
        Err(_) => return redirect_oauth_error(None, "invalid_state"),
    };
    let browser_cookie_state_nonce = Some(preview.nonce.as_str());
    let login_cookie_name = oauth_login_cookie_name(&preview.nonce);
    let browser_binding_matches = browser_binding_matches(
        preview.browser_binding_hash.as_deref(),
        extract_cookie_value(headers, &login_cookie_name).as_deref(),
    );
    // Both login and account binding states carry authorization authority. A
    // callback must prove it originated in the browser that started the flow.
    if !browser_binding_matches {
        return redirect_oauth_error_for_mode(None, "invalid_state", browser_cookie_state_nonce);
    }
    if preview.provider_type != provider_type {
        return redirect_oauth_error_for_mode(None, "invalid_state", browser_cookie_state_nonce);
    }
    let stored = match crate::oauth::consume_identity_oauth_state(state, nonce).await {
        Ok(Some(value)) if value == preview => value,
        Ok(Some(_)) | Ok(None) | Err(_) => {
            return redirect_oauth_error_for_mode(None, "invalid_state", browser_cookie_state_nonce)
        }
    };
    let browser_cookie_state_nonce = Some(stored.nonce.as_str());
    if params
        .get("error")
        .is_some_and(|value| value.eq_ignore_ascii_case("access_denied"))
    {
        return redirect_oauth_error_for_mode(
            None,
            "authorization_denied",
            browser_cookie_state_nonce,
        );
    }
    let Some(code) = params
        .get("code")
        .map(String::as_str)
        .filter(|value| !value.is_empty())
    else {
        return redirect_oauth_error_for_mode(None, "invalid_callback", browser_cookie_state_nonce);
    };
    let config =
        match crate::oauth::get_enabled_identity_oauth_provider_config(state, &provider_type).await
        {
            Ok(Some(value)) => value,
            Ok(None) => {
                return redirect_oauth_error_for_mode(
                    None,
                    "provider_disabled",
                    browser_cookie_state_nonce,
                )
            }
            Err(err) => {
                return redirect_oauth_error_for_mode(None, err.code(), browser_cookie_state_nonce)
            }
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
            return redirect_oauth_error_for_mode(
                Some(&config.frontend_callback_url),
                oauth_error_code(&err),
                browser_cookie_state_nonce,
            )
        }
    };

    match stored.mode {
        crate::oauth::IdentityOAuthStateMode::Login => {
            let response = complete_oauth_login(
                state,
                headers,
                client_ip,
                &config.frontend_callback_url,
                stored.client_device_id,
                claims,
            )
            .await;
            append_oauth_login_cookie_clear(response, &stored.nonce)
        }
        crate::oauth::IdentityOAuthStateMode::Bind => {
            let state_nonce = stored.nonce.clone();
            let response =
                complete_oauth_bind(state, &config.frontend_callback_url, stored, claims).await;
            append_oauth_login_cookie_clear(response, &state_nonce)
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
    let local_password_login_allowed =
        match local_password_login_allowed_for_user(state, Some(&auth.user)).await {
            Ok(allowed) => allowed,
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("auth login policy lookup failed: {err:?}"),
                    false,
                )
            }
        };
    match crate::oauth::unbind_identity_oauth(
        state,
        &auth.user,
        &provider_type,
        local_password_login_allowed,
    )
    .await
    {
        Ok(true) => Json(json!({ "message": "解绑成功" })).into_response(),
        Ok(false) => {
            build_auth_error_response(http::StatusCode::NOT_FOUND, "OAuth 绑定不存在", false)
        }
        Err(err) => oauth_account_error_response(err),
    }
}

async fn build_identity_oauth_authorize_url(
    state: &AppState,
    provider_type: &str,
    client_device_id: String,
    mode: crate::oauth::IdentityOAuthStateMode,
    bind_user_id: Option<String>,
    bind_session_id: Option<String>,
) -> Result<(String, (String, String)), Response<Body>> {
    let config = match crate::oauth::get_enabled_identity_oauth_provider_config(
        state,
        provider_type,
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => {
            return Err(build_auth_error_response(
                http::StatusCode::NOT_FOUND,
                "OAuth Provider 不存在或已禁用",
                false,
            ))
        }
        Err(err) => return Err(oauth_account_error_response(err)),
    };
    let pkce_verifier = generate_pkce_verifier();
    let code_challenge = pkce_s256(&pkce_verifier);
    let browser_binding = generate_oauth_nonce();
    let stored = match mode {
        crate::oauth::IdentityOAuthStateMode::Login => {
            crate::oauth::StoredIdentityOAuthState::login(
                provider_type,
                client_device_id,
                Some(pkce_verifier),
                Some(browser_binding_hash(&browser_binding)),
            )
        }
        crate::oauth::IdentityOAuthStateMode::Bind => {
            let Some(user_id) = bind_user_id else {
                return Err(build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "缺少绑定用户",
                    false,
                ));
            };
            let Some(session_id) = bind_session_id else {
                return Err(build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "缺少绑定会话",
                    false,
                ));
            };
            crate::oauth::StoredIdentityOAuthState::bind(
                provider_type,
                client_device_id,
                Some(pkce_verifier),
                browser_binding_hash(&browser_binding),
                user_id,
                session_id,
            )
        }
    };
    if crate::oauth::save_identity_oauth_state(state, &stored)
        .await
        .is_err()
    {
        return Err(build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "OAuth 状态存储不可用",
            false,
        ));
    }
    let network = crate::oauth::resolve_identity_oauth_network_context(state).await;
    let state_nonce = stored.nonce.clone();
    let start_ctx = IdentityOAuthStartContext {
        state: stored.nonce,
        code_challenge: Some(code_challenge),
        network,
    };
    let authorize = match IdentityOAuthService::with_builtin_providers().start(&config, &start_ctx)
    {
        Ok(value) => value,
        Err(_) => {
            return Err(build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "OAuth Provider 不可用",
                false,
            ))
        }
    };
    Ok((authorize.authorize_url, (state_nonce, browser_binding)))
}

async fn start_identity_oauth(
    state: &AppState,
    provider_type: &str,
    client_device_id: String,
    mode: crate::oauth::IdentityOAuthStateMode,
    bind_user_id: Option<String>,
    bind_session_id: Option<String>,
) -> Response<Body> {
    let (authorize_url, browser_cookie) = match build_identity_oauth_authorize_url(
        state,
        provider_type,
        client_device_id,
        mode,
        bind_user_id,
        bind_session_id,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let response = redirect_to(&authorize_url, None);
    let (state_nonce, binding) = browser_cookie;
    append_oauth_login_cookie(response, &state_nonce, &binding)
}

async fn complete_oauth_login(
    state: &AppState,
    headers: &http::HeaderMap,
    client_ip: std::net::IpAddr,
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
        build_auth_login_success_response(state, headers, client_ip, client_device_id, user, None)
            .await;
    if login_response.status() != http::StatusCode::OK {
        return redirect_oauth_error(Some(frontend_callback_url), "provider_unavailable");
    }
    redirect_oauth_login_success(frontend_callback_url, &login_response)
}

fn redirect_oauth_login_success(
    frontend_callback_url: &str,
    login_response: &Response<Body>,
) -> Response<Body> {
    let set_cookies = login_response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    // The callback only carries the HttpOnly refresh cookie. The frontend
    // exchanges that cookie for an in-memory access token after navigation, so
    // bearer credentials never enter the URL, browser history, or referrer.
    let mut response = redirect_to(frontend_callback_url, None);
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
        || session.security_version != user.security_version
        || session.client_device_id != stored.client_device_id
    {
        return redirect_oauth_error(Some(frontend_callback_url), "invalid_state");
    }
    let session_expectation =
        match aether_data::repository::users::BindUserOAuthLinkSessionExpectation::new(
            session.id,
            stored.client_device_id,
            user.security_version,
            now,
        ) {
            Ok(expectation) => expectation,
            Err(_) => return redirect_oauth_error(Some(frontend_callback_url), "invalid_state"),
        };
    if let Err(err) =
        crate::oauth::bind_identity_oauth_to_user(state, &user, &claims, &session_expectation).await
    {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_browser_binding_uses_a_hash_and_constant_time_match() {
        let binding = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let hash = browser_binding_hash(binding);
        assert_ne!(hash, binding);
        assert_eq!(hash.len(), 64);
        assert!(browser_binding_matches(Some(&hash), Some(binding)));
        assert!(!browser_binding_matches(Some(&hash), Some("wrong-binding")));
        assert!(!browser_binding_matches(None, Some(binding)));
        assert!(!browser_binding_matches(Some("malformed"), Some(binding)));
    }

    #[test]
    fn secure_login_cookie_uses_host_prefix_and_required_scope() {
        let state_nonce = "state-nonce";
        let binding = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let cookie_name = oauth_login_cookie_name_for_security(state_nonce, true);
        let set_cookie = build_oauth_login_cookie_header_for_security(state_nonce, binding, true);
        assert!(set_cookie.starts_with(&format!("{cookie_name}=")));
        assert!(cookie_name
            .strip_prefix(OAUTH_LOGIN_HOST_COOKIE_NAME_PREFIX)
            .is_some_and(
                |suffix| suffix.len() == 64 && suffix.chars().all(|ch| ch.is_ascii_hexdigit())
            ));
        assert!(set_cookie.contains("Path=/"));
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("SameSite=Lax"));
        assert!(set_cookie.contains("Max-Age=600"));
        assert!(set_cookie.contains("Secure"));
        assert!(!set_cookie.contains("Domain="));

        let clear_cookie = build_oauth_login_cookie_clear_header_for_security(state_nonce, true);
        assert!(clear_cookie.starts_with(&format!("{cookie_name}=;")));
        assert!(clear_cookie.contains("Path=/"));
        assert!(clear_cookie.contains("Max-Age=0"));
        assert!(clear_cookie.contains("Secure"));
        assert!(!clear_cookie.contains("Domain="));
        assert!(!clear_cookie.contains("aether_refresh_token"));
    }

    #[test]
    fn insecure_local_login_cookie_keeps_legacy_name_and_narrow_path() {
        let state_nonce = "state-nonce";
        let binding = "local-binding";
        let cookie_name = oauth_login_cookie_name_for_security(state_nonce, false);
        let set_cookie = build_oauth_login_cookie_header_for_security(state_nonce, binding, false);

        assert!(cookie_name.starts_with(OAUTH_LOGIN_COOKIE_NAME_PREFIX));
        assert!(!cookie_name.starts_with(OAUTH_LOGIN_HOST_COOKIE_NAME_PREFIX));
        assert!(set_cookie.starts_with(&format!("{cookie_name}=")));
        assert!(set_cookie.contains("Path=/api/oauth"));
        assert!(!set_cookie.contains("Secure"));
        assert!(!set_cookie.contains("Domain="));

        let clear_cookie = build_oauth_login_cookie_clear_header_for_security(state_nonce, false);
        assert!(clear_cookie.starts_with(&format!("{cookie_name}=;")));
        assert!(clear_cookie.contains("Path=/api/oauth"));
        assert!(!clear_cookie.contains("Secure"));
        assert!(!clear_cookie.contains("Domain="));
    }

    #[test]
    fn login_cookie_names_are_distinct_per_state_nonce() {
        let first = oauth_login_cookie_name_for_security("first-state", true);
        let second = oauth_login_cookie_name_for_security("second-state", true);

        assert_ne!(first, second);
        assert_ne!(
            build_oauth_login_cookie_clear_header_for_security("first-state", true),
            build_oauth_login_cookie_clear_header_for_security("second-state", true)
        );
    }

    #[test]
    fn oauth_provider_paths_require_exactly_one_provider_segment() {
        assert_eq!(
            public_oauth_provider_from_path("/api/oauth/LinuxDo/authorize", "authorize"),
            Some("linuxdo".to_string())
        );
        assert_eq!(
            user_oauth_provider_from_path("/api/user/oauth/LinuxDo/bind-token", "bind-token"),
            Some("linuxdo".to_string())
        );
        assert_eq!(
            user_oauth_provider_from_path_without_suffix("/api/user/oauth/LinuxDo"),
            Some("linuxdo".to_string())
        );

        for path in [
            "/api/oauth/linuxdo/extra/authorize",
            "/api/oauth//authorize",
            "/api/oauth/linuxdo//authorize",
        ] {
            assert_eq!(public_oauth_provider_from_path(path, "authorize"), None);
        }
        for path in [
            "/api/user/oauth/linuxdo/extra/bind-token",
            "/api/user/oauth//bind-token",
            "/api/user/oauth/linuxdo//bind-token",
        ] {
            assert_eq!(user_oauth_provider_from_path(path, "bind-token"), None);
        }
        for path in [
            "/api/user/oauth/",
            "/api/user/oauth/   ",
            "/api/user/oauth/linuxdo/extra",
        ] {
            assert_eq!(user_oauth_provider_from_path_without_suffix(path), None);
        }
    }

    #[test]
    fn oauth_callback_rejects_duplicate_security_parameters() {
        assert_eq!(
            callback_params(Some("state=first&state=second")),
            Err(CallbackParamsError::DuplicateState)
        );
        assert_eq!(
            callback_params(Some("state=first&st%61te=second")),
            Err(CallbackParamsError::DuplicateState)
        );
        for query in [
            "state=state&code=first&code=second",
            "state=state&error=first&error=second",
        ] {
            assert_eq!(
                callback_params(Some(query)),
                Err(CallbackParamsError::DuplicateCallbackParameter)
            );
        }

        let params = callback_params(Some("state=state&code=code"))
            .expect("unique callback parameters should parse");
        assert_eq!(params.get("state").map(String::as_str), Some("state"));
        assert_eq!(params.get("code").map(String::as_str), Some("code"));
    }

    #[test]
    fn redirect_location_appends_parameters_to_relative_targets() {
        let location = build_redirect_location(
            "/auth/callback",
            Some(RedirectParams::Query(vec![(
                "error_code",
                "invalid_state".to_string(),
            )])),
        );
        assert_eq!(location, "/auth/callback?error_code=invalid_state");

        let location = build_redirect_location(
            "/auth/callback?existing=1#old",
            Some(RedirectParams::Query(vec![(
                "error_code",
                "provider unavailable".to_string(),
            )])),
        );
        assert_eq!(
            location,
            "/auth/callback?existing=1&error_code=provider+unavailable#old"
        );
    }

    #[test]
    fn oauth_login_success_redirect_keeps_cookie_but_carries_no_access_token() {
        let login_response = Response::builder()
            .status(http::StatusCode::OK)
            .header(
                SET_COOKIE,
                "aether_refresh_token=refresh-secret; Path=/api/auth; HttpOnly",
            )
            .body(Body::from(
                r#"{"access_token":"must-never-enter-callback-url"}"#,
            ))
            .expect("login response should build");
        let response = redirect_oauth_login_success(
            "https://frontend.example/auth/callback?source=oauth",
            &login_response,
        );
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("OAuth login redirect should have a location");

        assert_eq!(
            location,
            "https://frontend.example/auth/callback?source=oauth"
        );
        assert!(!location.contains("access_token"));
        assert!(!location.contains("must-never-enter-callback-url"));
        assert!(!location.contains('#'));
        let set_cookie = response
            .headers()
            .get(SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("refresh cookie should be preserved");
        assert!(set_cookie.starts_with("aether_refresh_token=refresh-secret"));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallbackParamsError {
    DuplicateState,
    DuplicateCallbackParameter,
}

fn callback_params(
    query: Option<&str>,
) -> Result<std::collections::BTreeMap<String, String>, CallbackParamsError> {
    let mut params = std::collections::BTreeMap::new();
    for (key, value) in query
        .into_iter()
        .flat_map(|query| form_urlencoded::parse(query.as_bytes()))
    {
        let key = key.into_owned();
        if matches!(key.as_str(), "state" | "code" | "error") && params.contains_key(&key) {
            return Err(if key == "state" {
                CallbackParamsError::DuplicateState
            } else {
                CallbackParamsError::DuplicateCallbackParameter
            });
        }
        params.insert(key, value.into_owned());
    }
    Ok(params)
}

fn public_oauth_provider_from_path(path: &str, suffix: &str) -> Option<String> {
    oauth_provider_from_path(path, "/api/oauth/", suffix)
}

fn user_oauth_provider_from_path(path: &str, suffix: &str) -> Option<String> {
    oauth_provider_from_path(path, "/api/user/oauth/", suffix)
}

fn oauth_provider_from_path(path: &str, prefix: &str, suffix: &str) -> Option<String> {
    let provider_type = path
        .strip_prefix(prefix)?
        .strip_suffix(&format!("/{suffix}"))?
        .trim();
    (!provider_type.is_empty() && !provider_type.contains('/'))
        .then(|| provider_type.to_ascii_lowercase())
}

fn user_oauth_provider_from_path_without_suffix(path: &str) -> Option<String> {
    let provider_type = path.strip_prefix("/api/user/oauth/")?.trim();
    (!provider_type.is_empty() && !provider_type.contains('/'))
        .then(|| provider_type.to_ascii_lowercase())
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
    let relative_target =
        url::Url::parse(target).is_err() && target.starts_with('/') && !target.starts_with("//");
    let parsed_target = url::Url::parse(target).or_else(|_| {
        if relative_target {
            url::Url::parse("http://aether.invalid").and_then(|base| base.join(target))
        } else {
            Err(url::ParseError::RelativeUrlWithoutBase)
        }
    });
    let Ok(mut url) = parsed_target else {
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
            if relative_target {
                relative_url_string(&url)
            } else {
                url.to_string()
            }
        }
        None if relative_target => relative_url_string(&url),
        None => url.to_string(),
    }
}

fn relative_url_string(url: &url::Url) -> String {
    let mut target = url.path().to_string();
    if let Some(query) = url.query() {
        target.push('?');
        target.push_str(query);
    }
    if let Some(fragment) = url.fragment() {
        target.push('#');
        target.push_str(fragment);
    }
    target
}
