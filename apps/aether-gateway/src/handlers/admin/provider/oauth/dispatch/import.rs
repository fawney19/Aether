use super::super::duplicates::{
    acquire_codex_oauth_account_locks, find_duplicate_provider_oauth_key,
    release_codex_oauth_account_locks,
};
use super::super::errors::build_internal_control_error_response;
use super::super::provisioning::{
    build_provider_oauth_auth_config_from_token_payload, create_provider_oauth_catalog_key,
    provider_oauth_active_api_formats, provider_oauth_key_proxy_value,
    update_existing_provider_oauth_catalog_key,
};
use super::super::runtime::{
    resolve_provider_oauth_runtime_endpoints,
    spawn_provider_oauth_account_state_refresh_after_update,
};
use super::super::state::{
    admin_provider_oauth_template, build_admin_provider_oauth_backend_unavailable_response,
    enrich_admin_provider_oauth_auth_config, exchange_admin_provider_oauth_refresh_token,
    is_fixed_provider_type_for_provider_oauth, json_u64_value,
};
use super::helpers::admin_provider_oauth_key_name_from_auth_config;
use super::token_import::{
    build_provider_access_token_import_auth_config, decode_access_token_expires_at,
    flatten_claude_code_credentials_payload, is_claude_session_key,
    normalize_provider_import_tokens, normalize_provider_oauth_import_headers_from_object,
    provider_oauth_import_authorization_bearer_token_from_object,
    provider_type_supports_access_token_import, validate_claude_access_token_import,
};
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_import_provider_id;
use crate::handlers::admin::request::{
    AdminAppState, AdminProviderOAuthTemplate, AdminRequestContext,
};
use crate::GatewayError;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use aether_oauth::provider::{
    ProviderOAuthImportInput, ProviderOAuthService, ProviderOAuthTransportContext,
};
use aether_oauth::{core::OAuthError, network::OAuthNetworkContext};
use aether_runtime_state::RuntimeLockLease;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use uuid::Uuid;

const CODEX_AGENT_IDENTITY_ENROLLMENT_LOCK_TTL: Duration = Duration::from_secs(180);
const CODEX_AGENT_IDENTITY_ENROLLMENT_LOCK_RENEW_INTERVAL: Duration = Duration::from_secs(60);

struct AdminProviderOAuthSingleImportTokens {
    access_token: String,
    auth_config: serde_json::Map<String, serde_json::Value>,
    expires_at: Option<u64>,
}

struct CodexAgentIdentityEnrollment {
    leases: Vec<RuntimeLockLease>,
    duplicate: Option<StoredProviderCatalogKey>,
    lease_lost: Arc<AtomicBool>,
    heartbeat: tokio::task::JoinHandle<()>,
}

impl Drop for CodexAgentIdentityEnrollment {
    fn drop(&mut self) {
        self.heartbeat.abort();
    }
}

fn sanitize_windsurf_import_error(error: &OAuthError) -> String {
    match error {
        OAuthError::InvalidRequest(_) => "Windsurf 凭据验证失败: 请求参数无效".to_string(),
        OAuthError::HttpStatus { status_code, .. } => {
            format!("Windsurf 凭据验证失败: HTTP {status_code}")
        }
        OAuthError::InvalidResponse(detail) => sanitize_windsurf_invalid_response_detail(detail)
            .unwrap_or_else(|| "Windsurf 凭据验证失败".to_string()),
        _ => "Windsurf 凭据验证失败".to_string(),
    }
}

fn sanitize_windsurf_invalid_response_detail(detail: &str) -> Option<String> {
    let detail = detail.trim();
    if detail.eq_ignore_ascii_case("Auth1 response is not json") {
        return Some("Windsurf 凭据验证失败: Auth1 响应无法解析".to_string());
    }
    if detail.eq_ignore_ascii_case("Auth1 response missing token") {
        return Some("Windsurf 凭据验证失败: Auth1 响应缺少 token".to_string());
    }
    if detail.contains("WindsurfPostAuth response missing sessionToken")
        || detail.contains("WindsurfPostAuth response is not json")
        || (detail.contains("WindsurfPostAuth failed") && detail.contains("missing sessionToken"))
    {
        return Some("Windsurf 凭据验证失败: PostAuth 未返回 sessionToken".to_string());
    }
    if detail.contains("WindsurfPostAuth failed") {
        return Some("Windsurf 凭据验证失败: PostAuth 失败".to_string());
    }
    None
}

fn import_payload_has_windsurf_credentials(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    import_payload_string(payload, "api_key", "apiKey").is_some()
        || import_payload_string_any(payload, &["token", "auth_token", "authToken"]).is_some()
        || import_payload_string(payload, "refresh_token", "refreshToken").is_some()
        || import_payload_string(payload, "access_token", "accessToken").is_some()
        || (import_payload_string_any(payload, &["email"]).is_some()
            && import_payload_string_any(payload, &["password"]).is_some())
}

fn import_payload_string(
    payload: &serde_json::Map<String, serde_json::Value>,
    snake_case: &str,
    camel_case: &str,
) -> Option<String> {
    import_payload_string_any(payload, &[snake_case, camel_case])
}

fn import_payload_string_any(
    payload: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<String> {
    keys.iter()
        .find_map(|key| payload.get(*key))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn import_payload_bool(payload: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    payload
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn codex_agent_identity_access_token_input(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    import_payload_string_any(payload, &["access_token", "accessToken"])
}

fn import_payload_requests_agent_identity(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    import_payload_bool(payload, "create_agent_identity")
}

fn import_payload_requests_legacy_agent_identity(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    import_payload_bool(payload, "create_agent_identity_from_session_token")
}

fn codex_access_token_identity_hints(
    access_token: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, &'static str> {
    let mut hints = serde_json::Map::new();
    enrich_admin_provider_oauth_auth_config(
        "codex",
        &mut hints,
        &json!({ "access_token": access_token }),
    );
    let account_id = hints
        .get("account_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let user_id = hints
        .get("user_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if account_id.is_none() || user_id.is_none() {
        return Err("ChatGPT Access Token 缺少账号身份字段");
    }
    Ok(hints)
}

async fn prepare_codex_agent_identity_enrollment(
    state: &AdminAppState<'_>,
    provider_id: &str,
    identity_hints: &serde_json::Map<String, serde_json::Value>,
) -> Result<CodexAgentIdentityEnrollment, Response<Body>> {
    let lock_keys =
        crate::handlers::admin::provider::oauth::duplicates::codex_agent_identity_account_lock_keys(
            provider_id,
            identity_hints,
        );
    if lock_keys.is_empty() {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "ChatGPT Access Token 缺少账号身份字段",
        ));
    }
    let owner = format!(
        "aether-gateway-agent-identity-enrollment-{}",
        Uuid::new_v4()
    );
    let mut leases = Vec::with_capacity(lock_keys.len());
    for lock_key in lock_keys {
        match state
            .runtime_state()
            .lock_try_acquire(
                lock_key.as_str(),
                owner.as_str(),
                CODEX_AGENT_IDENTITY_ENROLLMENT_LOCK_TTL,
            )
            .await
        {
            Ok(Some(lease)) => leases.push(lease),
            Ok(None) => {
                release_codex_agent_identity_leases(state, leases).await;
                return Err(build_internal_control_error_response(
                    http::StatusCode::CONFLICT,
                    "该 ChatGPT 账号正在创建 Agent Identity，请稍后重试",
                ));
            }
            Err(error) => {
                tracing::warn!(
                    provider_id = %provider_id,
                    error = ?error,
                    "gateway Agent Identity enrollment lock unavailable"
                );
                release_codex_agent_identity_leases(state, leases).await;
                return Err(build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "Agent Identity 创建锁暂不可用，请稍后重试",
                ));
            }
        }
    }

    let lease_lost = Arc::new(AtomicBool::new(false));
    let heartbeat = spawn_codex_agent_identity_enrollment_heartbeat(
        state.cloned_app(),
        leases.clone(),
        Arc::clone(&lease_lost),
    );

    let duplicate = match state
        .find_duplicate_provider_oauth_key(provider_id, identity_hints, None)
        .await
    {
        Ok(duplicate) => duplicate,
        Err(detail) => {
            heartbeat.abort();
            release_codex_agent_identity_leases(state, leases).await;
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
    };
    Ok(CodexAgentIdentityEnrollment {
        leases,
        duplicate,
        lease_lost,
        heartbeat,
    })
}

fn spawn_codex_agent_identity_enrollment_heartbeat(
    app: crate::AppState,
    leases: Vec<RuntimeLockLease>,
    lease_lost: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut renew_timer =
            tokio::time::interval(CODEX_AGENT_IDENTITY_ENROLLMENT_LOCK_RENEW_INTERVAL);
        renew_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        renew_timer.tick().await;
        loop {
            renew_timer.tick().await;
            for lease in &leases {
                match app
                    .runtime_state()
                    .lock_renew(lease, CODEX_AGENT_IDENTITY_ENROLLMENT_LOCK_TTL)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        lease_lost.store(true, Ordering::Release);
                        tracing::error!(
                            lock_key = %lease.key,
                            "gateway Agent Identity enrollment lock was lost"
                        );
                        return;
                    }
                    Err(error) => {
                        lease_lost.store(true, Ordering::Release);
                        tracing::error!(
                            lock_key = %lease.key,
                            error = ?error,
                            "gateway Agent Identity enrollment lock renewal failed"
                        );
                        return;
                    }
                }
            }
        }
    })
}

async fn release_codex_agent_identity_leases(
    state: &AdminAppState<'_>,
    leases: Vec<RuntimeLockLease>,
) {
    for lease in leases {
        if let Err(error) = state.runtime_state().lock_release(&lease).await {
            tracing::warn!(
                lock_key = %lease.key,
                error = ?error,
                "gateway Agent Identity enrollment lock release failed"
            );
        }
    }
}

async fn release_codex_agent_identity_enrollment(
    state: &AdminAppState<'_>,
    enrollment: Option<CodexAgentIdentityEnrollment>,
) {
    let Some(mut enrollment) = enrollment else {
        return;
    };
    enrollment.heartbeat.abort();
    release_codex_agent_identity_leases(state, std::mem::take(&mut enrollment.leases)).await;
}

async fn resolve_admin_provider_oauth_codex_access_token_agent_identity_import(
    state: &AdminAppState<'_>,
    access_token: &str,
    identity_hints: serde_json::Map<String, serde_json::Value>,
    request_proxy: Option<ProxySnapshot>,
) -> Result<AdminProviderOAuthSingleImportTokens, Response<Body>> {
    let executor = crate::oauth::GatewayOAuthHttpExecutor::new(*state);
    let is_fedramp_account = identity_hints
        .get("is_fedramp")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let mut auth_config =
        aether_provider_transport::register_codex_agent_identity_from_access_token(
            &executor,
            access_token,
            aether_provider_transport::CodexAgentIdentityRegistrationOptions { is_fedramp_account },
            OAuthNetworkContext::provider_operation(request_proxy),
        )
        .await
        .map_err(|error| {
            build_internal_control_error_response(http::StatusCode::BAD_REQUEST, error.to_string())
        })?;
    for key in [
        "access_token",
        "accessToken",
        "refresh_token",
        "refreshToken",
        "id_token",
        "idToken",
    ] {
        auth_config.remove(key);
    }
    for (key, value) in identity_hints {
        auth_config.entry(key).or_insert(value);
    }
    Ok(AdminProviderOAuthSingleImportTokens {
        // Agent Identity signs every request and never uses the encrypted OAuth token column.
        access_token: "__placeholder__".to_string(),
        auth_config,
        expires_at: None,
    })
}

fn import_payload_project_id_any(
    payload: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<String> {
    keys.iter().find_map(|key| {
        let value = payload.get(*key)?;
        if let Some(string) = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(string.to_string());
        }
        value
            .as_object()
            .and_then(|object| {
                object
                    .get("id")
                    .or_else(|| object.get("project_id"))
                    .or_else(|| object.get("projectId"))
            })
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn import_payload_u64_any(
    payload: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<u64> {
    keys.iter().find_map(|key| {
        let value = payload.get(*key)?;
        json_u64_value(Some(value)).or_else(|| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .and_then(|value| u64::try_from(value.timestamp()).ok())
        })
    })
}

fn import_payload_export_access_token(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    if import_payload_string(payload, "refresh_token", "refreshToken").is_some() {
        return None;
    }
    let access_token = import_payload_string(payload, "access_token", "accessToken")?;
    let header_bearer = provider_oauth_import_authorization_bearer_token_from_object(payload);
    if header_bearer.as_deref() == Some(access_token.as_str()) {
        return None;
    }
    Some(access_token)
}

fn apply_single_import_hints(
    provider_type: &str,
    payload: &serde_json::Map<String, serde_json::Value>,
    auth_config: &mut serde_json::Map<String, serde_json::Value>,
) {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    if provider_type == "antigravity" {
        if let Some(project_id) = import_payload_project_id_any(
            payload,
            &[
                "project_id",
                "projectId",
                "cloudaicompanionProject",
                "cloudAiCompanionProject",
            ],
        ) {
            auth_config
                .entry("project_id".to_string())
                .or_insert_with(|| json!(project_id));
        }
        for (target, keys) in [
            // Antigravity 的 token 响应不带账号邮箱，只能从导入 JSON 里取，
            // 否则账号名会退化成 antigravity_<timestamp>。
            ("email", &["email", "oauth_email"][..]),
            (
                "client_version",
                &[
                    "client_version",
                    "clientVersion",
                    "antigravityClientVersion",
                ][..],
            ),
            (
                "session_id",
                &[
                    "session_id",
                    "sessionId",
                    "vscode_session_id",
                    "vscodeSessionId",
                ][..],
            ),
            ("user_agent", &["user_agent", "userAgent"][..]),
        ] {
            let Some(value) = import_payload_string_any(payload, keys) else {
                continue;
            };
            auth_config
                .entry(target.to_string())
                .or_insert_with(|| json!(value));
        }
        return;
    }
    if provider_type == "claude_code" {
        for (target, keys) in [
            (
                "org_uuid",
                &["org_uuid", "organization_uuid", "organizationUuid"][..],
            ),
            (
                "subscription_type",
                &["subscription_type", "subscriptionType"][..],
            ),
            ("rate_limit_tier", &["rate_limit_tier", "rateLimitTier"][..]),
        ] {
            if let Some(value) = import_payload_string_any(payload, keys) {
                auth_config
                    .entry(target.to_string())
                    .or_insert_with(|| json!(value));
            }
        }
        if let Some(scopes) = payload.get("scopes").cloned() {
            auth_config.entry("scopes".to_string()).or_insert(scopes);
        }
        return;
    }
    if !matches!(provider_type.as_str(), "codex" | "chatgpt_web" | "grok") {
        return;
    }

    for (target, keys) in [
        ("email", &["email", "oauth_email"][..]),
        (
            "account_id",
            &[
                "account_id",
                "accountId",
                "chatgpt_account_id",
                "chatgptAccountId",
            ][..],
        ),
        (
            "account_user_id",
            &[
                "account_user_id",
                "accountUserId",
                "chatgpt_account_user_id",
                "chatgptAccountUserId",
            ][..],
        ),
        (
            "plan_type",
            &[
                "plan_type",
                "planType",
                "chatgpt_plan_type",
                "chatgptPlanType",
            ][..],
        ),
        (
            "user_id",
            &["user_id", "userId", "chatgpt_user_id", "chatgptUserId"][..],
        ),
        ("account_name", &["account_name", "accountName"][..]),
        ("sso_rw_token", &["sso_rw_token", "ssoRwToken"][..]),
        (
            "cf_cookies",
            &["cf_cookies", "cfCookies", "cookie", "cookieHeader"][..],
        ),
        ("cf_clearance", &["cf_clearance", "cfClearance"][..]),
        ("user_agent", &["user_agent", "userAgent"][..]),
        (
            "browser_profile",
            &[
                "browser_profile",
                "browserProfile",
                "browser",
                "impersonate",
            ][..],
        ),
        ("pool_tier", &["pool_tier", "poolTier", "tier"][..]),
    ] {
        let Some(value) = import_payload_string_any(payload, keys) else {
            continue;
        };
        auth_config.entry(target.to_string()).or_insert_with(|| {
            if target == "plan_type" || target == "pool_tier" {
                json!(value.to_ascii_lowercase())
            } else {
                json!(value)
            }
        });
    }

    if let Some(access_token) = import_payload_export_access_token(payload) {
        auth_config
            .entry("access_token".to_string())
            .or_insert_with(|| json!(access_token));
    }

    if let Some(request_headers) = normalize_provider_oauth_import_headers_from_object(payload) {
        auth_config
            .entry("headers".to_string())
            .or_insert_with(|| json!(request_headers));
    }
}

async fn resolve_admin_provider_oauth_single_import_tokens(
    state: &AdminAppState<'_>,
    template: Option<AdminProviderOAuthTemplate>,
    provider_type: &str,
    refresh_token: Option<&str>,
    access_token: Option<&str>,
    imported_expires_at: Option<u64>,
    request_proxy: Option<ProxySnapshot>,
) -> Result<AdminProviderOAuthSingleImportTokens, Response<Body>> {
    if let Some(refresh_token) = refresh_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let Some(template) = template else {
            if provider_type_supports_access_token_import(provider_type) {
                if let Some(access_token) = access_token
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    let (auth_config, expires_at) = build_provider_access_token_import_auth_config(
                        provider_type,
                        access_token,
                        Some(refresh_token),
                        imported_expires_at,
                        Some("Provider 不支持 Refresh Token 交换，已回退为 Session Token 导入"),
                    );
                    return Ok(AdminProviderOAuthSingleImportTokens {
                        access_token: access_token.to_string(),
                        auth_config,
                        expires_at,
                    });
                }
            }
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "该 Provider 不支持 Refresh Token 导入，请提供 sso_token 或 access_token",
            ));
        };

        let token_payload = match state
            .exchange_admin_provider_oauth_refresh_token(
                template,
                refresh_token,
                request_proxy.clone(),
            )
            .await
        {
            Ok(payload) => payload,
            Err(response) => {
                if !provider_type.eq_ignore_ascii_case("claude_code")
                    && provider_type_supports_access_token_import(provider_type)
                {
                    if let Some(access_token) = access_token
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        let (auth_config, expires_at) =
                            build_provider_access_token_import_auth_config(
                                provider_type,
                                access_token,
                                Some(refresh_token),
                                imported_expires_at,
                                Some("Refresh Token 验证失败，已回退为 Access Token 导入"),
                            );
                        return Ok(AdminProviderOAuthSingleImportTokens {
                            access_token: access_token.to_string(),
                            auth_config,
                            expires_at,
                        });
                    }
                }
                return Err(response);
            }
        };

        let (mut auth_config, access_token, returned_refresh_token, expires_at) =
            build_provider_oauth_auth_config_from_token_payload(provider_type, &token_payload);
        let Some(access_token) = access_token else {
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "token refresh 返回缺少 access_token",
            ));
        };
        let refresh_token = returned_refresh_token
            .or_else(|| Some(refresh_token.to_string()))
            .filter(|value| !value.trim().is_empty());
        if let Some(refresh_token) = refresh_token.as_ref() {
            auth_config.insert("refresh_token".to_string(), json!(refresh_token));
        }
        return Ok(AdminProviderOAuthSingleImportTokens {
            access_token,
            auth_config,
            expires_at,
        });
    }

    let Some(access_token) = access_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Refresh Token 或 Access Token 不能为空",
        ));
    };
    if provider_type.eq_ignore_ascii_case("claude_code") {
        let now_unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        if let Err(detail) =
            validate_claude_access_token_import(access_token, imported_expires_at, now_unix_secs)
        {
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
    }
    if !provider_type_supports_access_token_import(provider_type) {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Access Token 导入仅支持 Claude Code / Codex / ChatGPT Web / Grok Provider",
        ));
    }

    let (auth_config, expires_at) = build_provider_access_token_import_auth_config(
        provider_type,
        access_token,
        None,
        imported_expires_at,
        None,
    );
    Ok(AdminProviderOAuthSingleImportTokens {
        access_token: access_token.to_string(),
        auth_config,
        expires_at,
    })
}

async fn resolve_admin_provider_oauth_windsurf_single_import_tokens(
    state: &AdminAppState<'_>,
    provider_type: &str,
    name: Option<String>,
    raw_payload: &serde_json::Map<String, serde_json::Value>,
    refresh_token: Option<&str>,
    request_proxy: Option<ProxySnapshot>,
) -> Result<AdminProviderOAuthSingleImportTokens, Response<Body>> {
    let ctx = ProviderOAuthTransportContext {
        provider_id: String::new(),
        provider_type: provider_type.to_string(),
        endpoint_id: None,
        key_id: None,
        auth_type: Some("oauth".to_string()),
        decrypted_api_key: None,
        decrypted_auth_config: None,
        provider_config: None,
        endpoint_config: None,
        key_config: None,
        network: aether_oauth::network::OAuthNetworkContext::provider_operation(
            request_proxy.clone(),
        ),
    };
    let executor = crate::oauth::GatewayOAuthHttpExecutor::new(*state);
    let service = ProviderOAuthService::with_builtin_adapters();
    let result = service
        .import_credentials(
            &executor,
            &ctx,
            ProviderOAuthImportInput {
                provider_type: provider_type.to_string(),
                name,
                refresh_token: refresh_token.map(ToOwned::to_owned),
                raw_credentials: Some(serde_json::Value::Object(raw_payload.clone())),
                network: ctx.network.clone(),
            },
        )
        .await
        .map_err(|error| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                sanitize_windsurf_import_error(&error),
            )
        })?;
    let access_token = result.token_set.access_token.trim().to_string();
    if access_token.is_empty() {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Windsurf 凭据验证返回缺少 apiKey/sessionToken",
        ));
    }
    let auth_config = result.auth_config.as_object().cloned().ok_or_else(|| {
        build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Windsurf 凭据验证返回缺少 auth_config",
        )
    })?;
    Ok(AdminProviderOAuthSingleImportTokens {
        access_token,
        auth_config,
        expires_at: result.token_set.expires_at_unix_secs,
    })
}

pub(super) async fn handle_admin_provider_oauth_import_refresh_token(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let Some(provider_id) = admin_provider_oauth_import_provider_id(request_context.path()) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let Some(request_body) = request_body else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "请求体必须是合法的 JSON 对象",
        ));
    };
    let mut raw_payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求体必须是合法的 JSON 对象",
            ));
        }
    };
    if import_payload_requests_legacy_agent_identity(&raw_payload) {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "旧版 Agent Identity 创建参数已停用，请使用 create_agent_identity 和 access_token",
        ));
    }
    let create_agent_identity = import_payload_requests_agent_identity(&raw_payload);
    let agent_identity_access_token_input = if create_agent_identity {
        codex_agent_identity_access_token_input(&raw_payload)
    } else {
        None
    };
    let name = raw_payload
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let proxy_node_id = raw_payload
        .get("proxy_node_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if provider_type == "claude_code" {
        flatten_claude_code_credentials_payload(&mut raw_payload);
    }
    let refresh_token_input = import_payload_string(&raw_payload, "refresh_token", "refreshToken");
    let access_token_input = import_payload_string_any(
        &raw_payload,
        &[
            "access_token",
            "accessToken",
            "sso_token",
            "ssoToken",
            "session_token",
            "sessionToken",
        ],
    )
    .or_else(|| provider_oauth_import_authorization_bearer_token_from_object(&raw_payload));
    let imported_expires_at =
        import_payload_u64_any(&raw_payload, &["expires_at", "expiresAt", "expired"]);
    let (refresh_token_input, access_token_input) = normalize_provider_import_tokens(
        &provider_type,
        refresh_token_input.as_deref(),
        access_token_input.as_deref(),
    );
    if provider_type == "claude_code"
        && refresh_token_input
            .as_deref()
            .into_iter()
            .chain(access_token_input.as_deref())
            .any(is_claude_session_key)
    {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Claude sessionKey 请使用 Cookie 授权，不能作为导入凭据",
        ));
    }
    if !create_agent_identity && refresh_token_input.is_none() && access_token_input.is_none() {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Refresh Token、Access Token 或 sso_token 不能为空",
        ));
    }
    if !is_fixed_provider_type_for_provider_oauth(&provider_type) {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不是固定类型，无法使用 provider-oauth",
        ));
    }
    if provider_type == "kiro" {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Kiro 不支持单条 Refresh Token 导入，请使用批量导入或设备授权。",
        ));
    }
    if create_agent_identity && provider_type != "codex" {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "仅 Codex Provider 支持使用 Access Token 创建 Agent Identity",
        ));
    }
    if create_agent_identity && refresh_token_input.is_some() {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "使用 Access Token 创建 Agent Identity 时不能同时提交 Refresh Token",
        ));
    }
    let template = admin_provider_oauth_template(&provider_type);
    if template.is_none() && !provider_type_supports_access_token_import(&provider_type) {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let endpoint_resolution =
        resolve_provider_oauth_runtime_endpoints(state, &provider, &provider_type).await?;
    let endpoints = endpoint_resolution.endpoints;
    let runtime_endpoint = endpoint_resolution.runtime_endpoint;
    let request_proxy = state
        .resolve_admin_provider_oauth_operation_proxy_snapshot(
            proxy_node_id.as_deref(),
            &[
                runtime_endpoint
                    .as_ref()
                    .and_then(|endpoint| endpoint.proxy.as_ref()),
                provider.proxy.as_ref(),
            ],
        )
        .await;
    let key_proxy = provider_oauth_key_proxy_value(proxy_node_id.as_deref());
    let mut agent_identity_enrollment = None;

    let resolved_import = if create_agent_identity {
        let Some(access_token) = agent_identity_access_token_input.as_deref() else {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "ChatGPT Access Token（JWT）不能为空",
            ));
        };
        let mut identity_hints = match codex_access_token_identity_hints(access_token) {
            Ok(hints) => hints,
            Err(detail) => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    detail,
                ));
            }
        };
        identity_hints.insert("provider_type".to_string(), json!("codex"));
        let enrollment =
            match prepare_codex_agent_identity_enrollment(state, &provider_id, &identity_hints)
                .await
            {
                Ok(enrollment) => enrollment,
                Err(response) => return Ok(response),
            };
        match resolve_admin_provider_oauth_codex_access_token_agent_identity_import(
            state,
            access_token,
            identity_hints,
            request_proxy.clone(),
        )
        .await
        {
            Ok(value) => {
                agent_identity_enrollment = Some(enrollment);
                value
            }
            Err(response) => {
                release_codex_agent_identity_enrollment(state, Some(enrollment)).await;
                return Ok(response);
            }
        }
    } else if provider_type == "windsurf" {
        if !import_payload_has_windsurf_credentials(&raw_payload) {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "Windsurf 凭据不能为空",
            ));
        }
        match resolve_admin_provider_oauth_windsurf_single_import_tokens(
            state,
            &provider_type,
            name.clone(),
            &raw_payload,
            refresh_token_input.as_deref(),
            request_proxy.clone(),
        )
        .await
        {
            Ok(value) => value,
            Err(response) => return Ok(response),
        }
    } else {
        if refresh_token_input.is_none() && access_token_input.is_none() {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "Refresh Token 或 Access Token 不能为空",
            ));
        }
        match resolve_admin_provider_oauth_single_import_tokens(
            state,
            template,
            &provider_type,
            refresh_token_input.as_deref(),
            access_token_input.as_deref(),
            imported_expires_at,
            request_proxy.clone(),
        )
        .await
        {
            Ok(value) => value,
            Err(response) => return Ok(response),
        }
    };
    let AdminProviderOAuthSingleImportTokens {
        access_token,
        mut auth_config,
        mut expires_at,
    } = resolved_import;
    if !create_agent_identity {
        apply_single_import_hints(&provider_type, &raw_payload, &mut auth_config);
        if let Some(header_access_token) =
            provider_oauth_import_authorization_bearer_token_from_object(&raw_payload)
        {
            if let Some(header_expires_at) =
                decode_access_token_expires_at(&header_access_token).or(imported_expires_at)
            {
                auth_config.insert("expires_at".to_string(), json!(header_expires_at));
                expires_at = Some(header_expires_at);
            } else {
                auth_config.remove("expires_at");
                expires_at = None;
            }
        }
    }
    let has_refresh_token = auth_config
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());

    let codex_oauth_account_leases = if !create_agent_identity && provider_type == "codex" {
        match acquire_codex_oauth_account_locks(state, &provider_id, &auth_config, "single-import")
            .await
        {
            Ok(leases) => leases,
            Err(error) => {
                return Ok(build_internal_control_error_response(
                    error.status_code(),
                    error.detail(),
                ));
            }
        }
    } else {
        Vec::new()
    };

    let api_formats = provider_oauth_active_api_formats(&endpoints);
    let duplicate = if create_agent_identity {
        let initial_duplicate_id = agent_identity_enrollment
            .as_ref()
            .and_then(|enrollment| enrollment.duplicate.as_ref())
            .map(|key| key.id.clone());
        match state
            .find_duplicate_provider_oauth_key(&provider_id, &auth_config, None)
            .await
        {
            Ok(duplicate) => {
                let current_duplicate_id = duplicate.as_ref().map(|key| key.id.as_str());
                if initial_duplicate_id.as_deref() != current_duplicate_id {
                    tracing::info!(
                        provider_id = %provider_id,
                        initial_duplicate_id = ?initial_duplicate_id,
                        current_duplicate_id,
                        "gateway Agent Identity duplicate changed during enrollment"
                    );
                }
                duplicate
            }
            Err(detail) => {
                release_codex_agent_identity_enrollment(state, agent_identity_enrollment).await;
                return Ok(build_internal_control_error_response(
                    http::StatusCode::CONFLICT,
                    detail,
                ));
            }
        }
    } else {
        match state
            .find_duplicate_provider_oauth_key(&provider_id, &auth_config, None)
            .await
        {
            Ok(duplicate) => duplicate,
            Err(detail) => {
                release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;
                return Ok(build_internal_control_error_response(
                    if provider_type == "codex" {
                        http::StatusCode::CONFLICT
                    } else {
                        http::StatusCode::BAD_REQUEST
                    },
                    detail,
                ));
            }
        }
    };

    let replaced = duplicate.is_some();
    if agent_identity_enrollment
        .as_ref()
        .is_some_and(|enrollment| enrollment.lease_lost.load(Ordering::Acquire))
    {
        release_codex_agent_identity_enrollment(state, agent_identity_enrollment).await;
        return Ok(build_internal_control_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "Agent Identity 创建锁已失效，请稍后重试",
        ));
    }
    let persisted_key_result = if let Some(existing_key) = duplicate {
        state
            .update_existing_provider_oauth_catalog_key(
                &existing_key,
                &provider_type,
                &access_token,
                &auth_config,
                &api_formats,
                key_proxy.clone(),
                expires_at,
            )
            .await
    } else {
        let name = name.unwrap_or_else(|| {
            admin_provider_oauth_key_name_from_auth_config(&provider_type, &auth_config, None)
        });
        state
            .create_provider_oauth_catalog_key(
                &provider_id,
                &provider_type,
                &name,
                &access_token,
                &auth_config,
                &api_formats,
                key_proxy.clone(),
                expires_at,
            )
            .await
    };
    let persisted_key = match persisted_key_result {
        Ok(Some(key)) => key,
        Ok(None) => {
            release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;
            release_codex_agent_identity_enrollment(state, agent_identity_enrollment).await;
            return Ok(build_internal_control_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "provider oauth write unavailable",
            ));
        }
        Err(error) => {
            release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;
            release_codex_agent_identity_enrollment(state, agent_identity_enrollment).await;
            return Err(error);
        }
    };
    release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;

    let agent_identity_task_ready = if create_agent_identity {
        match runtime_endpoint.as_ref() {
            Some(endpoint) => match state
                .read_provider_transport_snapshot_uncached(
                    &provider_id,
                    &endpoint.id,
                    &persisted_key.id,
                )
                .await
            {
                Ok(Some(transport)) => {
                    match state.resolve_local_oauth_request_auth(&transport).await {
                        Ok(Some(_)) => true,
                        Ok(None) => false,
                        Err(error) => {
                            tracing::warn!(
                                provider_id = %provider_id,
                                key_id = %persisted_key.id,
                                error = ?error,
                                "gateway Agent Identity initial task registration failed"
                            );
                            false
                        }
                    }
                }
                Ok(None) => false,
                Err(error) => {
                    tracing::warn!(
                        provider_id = %provider_id,
                        key_id = %persisted_key.id,
                        error = ?error,
                        "gateway Agent Identity pending transport reload failed"
                    );
                    false
                }
            },
            None => false,
        }
    } else {
        true
    };
    release_codex_agent_identity_enrollment(state, agent_identity_enrollment).await;

    spawn_provider_oauth_account_state_refresh_after_update(
        state.cloned_app(),
        provider.clone(),
        persisted_key.id.clone(),
        request_proxy.clone(),
    );

    if !agent_identity_task_ready {
        return Ok((
            http::StatusCode::ACCEPTED,
            Json(json!({
                "detail": "Agent Identity 已安全保存，但 task 初始化暂未完成，系统将自动重试",
                "key_id": persisted_key.id,
                "provider_type": provider_type,
                "expires_at": serde_json::Value::Null,
                "has_refresh_token": false,
                "temporary": false,
                "email": auth_config
                    .get("email")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                "replaced": replaced,
                "task_ready": false,
                "recoverable": true,
            })),
        )
            .into_response());
    }

    Ok(Json(json!({
        "key_id": persisted_key.id,
        "provider_type": provider_type,
        "expires_at": expires_at,
        "has_refresh_token": has_refresh_token,
        "temporary": auth_config
            .get("access_token_import_temporary")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        "email": auth_config.get("email").cloned().unwrap_or(serde_json::Value::Null),
        "replaced": replaced,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_single_import_hints, codex_access_token_identity_hints,
        codex_agent_identity_access_token_input, import_payload_requests_agent_identity,
        import_payload_requests_legacy_agent_identity, import_payload_string_any,
        import_payload_u64_any, sanitize_windsurf_import_error,
    };
    use crate::handlers::admin::provider::oauth::duplicates::codex_agent_identity_account_lock_keys;
    use aether_oauth::core::OAuthError;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;

    fn unsigned_jwt(payload: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("{header}.{payload}.signature")
    }

    #[test]
    fn single_import_accepts_session_token_alias() {
        let payload = json!({
            "session_token": "session-1",
        })
        .as_object()
        .cloned()
        .expect("payload should be an object");

        assert_eq!(
            import_payload_string_any(&payload, &["access_token", "session_token"]).as_deref(),
            Some("session-1")
        );
    }

    #[test]
    fn agent_identity_reads_access_token_and_ignores_session_token_alias() {
        let payload = json!({
            "accessToken": "access-token",
            "sessionToken": "session-token-must-not-win",
        })
        .as_object()
        .cloned()
        .expect("payload should be an object");

        assert_eq!(
            codex_agent_identity_access_token_input(&payload).as_deref(),
            Some("access-token")
        );
    }

    #[test]
    fn new_agent_identity_flag_does_not_treat_session_token_as_access_token() {
        let payload = json!({
            "sessionToken": "session-token-must-not-be-used",
        })
        .as_object()
        .cloned()
        .expect("payload should be an object");

        assert_eq!(codex_agent_identity_access_token_input(&payload), None);
    }

    #[test]
    fn access_token_agent_identity_hints_require_and_extract_chatgpt_identity() {
        let access_token = unsigned_jwt(json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "account-1",
                "chatgpt_user_id": "user-1",
                "chatgpt_plan_type": "plus",
                "chatgpt_account_is_fedramp": true
            },
            "https://api.openai.com/profile": {
                "email": "agent@example.com"
            }
        }));

        let hints = codex_access_token_identity_hints(&access_token)
            .expect("access token identity hints should parse");

        assert_eq!(hints.get("account_id"), Some(&json!("account-1")));
        assert_eq!(hints.get("user_id"), Some(&json!("user-1")));
        assert_eq!(hints.get("plan_type"), Some(&json!("plus")));
        assert_eq!(hints.get("is_fedramp"), Some(&json!(true)));
        assert_eq!(hints.get("email"), Some(&json!("agent@example.com")));
        assert!(!hints.contains_key("access_token"));
        assert!(!hints.contains_key("id_token"));
    }

    #[test]
    fn access_token_agent_identity_hints_reject_missing_identity() {
        let access_token = unsigned_jwt(json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "account-1"
            }
        }));

        assert_eq!(
            codex_access_token_identity_hints(&access_token),
            Err("ChatGPT Access Token 缺少账号身份字段")
        );
    }

    #[test]
    fn agent_identity_enrollment_lock_is_stable_and_account_scoped() {
        let first = json!({
            "account_id": "account-1",
            "user_id": "user-1",
        })
        .as_object()
        .cloned()
        .expect("identity hints should be an object");
        let second = json!({
            "account_id": "account-2",
            "user_id": "user-1",
        })
        .as_object()
        .cloned()
        .expect("identity hints should be an object");

        let first_keys = codex_agent_identity_account_lock_keys("provider-1", &first);
        assert_eq!(
            first_keys,
            codex_agent_identity_account_lock_keys("provider-1", &first)
        );
        assert_ne!(
            first_keys,
            codex_agent_identity_account_lock_keys("provider-1", &second)
        );
        assert_ne!(
            first_keys,
            codex_agent_identity_account_lock_keys("provider-2", &first)
        );
        assert!(first_keys
            .iter()
            .all(|key| !key.contains("account-1") && !key.contains("user-1")));
    }

    #[test]
    fn agent_identity_flag_is_explicit_boolean_and_rejects_legacy_alias() {
        let payload = json!({
            "create_agent_identity": true,
        })
        .as_object()
        .cloned()
        .expect("payload should be an object");
        assert!(import_payload_requests_agent_identity(&payload));

        let string_payload = json!({
            "create_agent_identity": "true",
        })
        .as_object()
        .cloned()
        .expect("payload should be an object");
        assert!(!import_payload_requests_agent_identity(&string_payload));

        let legacy_payload = json!({
            "create_agent_identity_from_session_token": true,
        })
        .as_object()
        .cloned()
        .expect("payload should be an object");
        assert!(!import_payload_requests_agent_identity(&legacy_payload));
        assert!(import_payload_requests_legacy_agent_identity(
            &legacy_payload
        ));
    }

    #[test]
    fn single_import_accepts_iso_expired_alias() {
        let payload = json!({
            "expired": "2030-01-01T00:00:00Z",
        })
        .as_object()
        .cloned()
        .expect("payload should be an object");

        assert_eq!(
            import_payload_u64_any(&payload, &["expires_at", "expiresAt", "expired"]),
            Some(1_893_456_000)
        );
    }

    #[test]
    fn single_import_applies_antigravity_identity_hints() {
        let payload = json!({
            "cloudaicompanionProject": {
                "id": "project-antigravity-1"
            },
            "clientVersion": "1.99.0",
            "sessionId": "session-antigravity-1",
            "userAgent": "antigravity",
            "email": "anti@example.com"
        })
        .as_object()
        .cloned()
        .expect("payload should be an object");
        let mut auth_config = serde_json::Map::new();

        apply_single_import_hints("antigravity", &payload, &mut auth_config);

        assert_eq!(auth_config.get("email"), Some(&json!("anti@example.com")));
        assert_eq!(
            auth_config.get("project_id"),
            Some(&json!("project-antigravity-1"))
        );
        assert_eq!(auth_config.get("client_version"), Some(&json!("1.99.0")));
        assert_eq!(
            auth_config.get("session_id"),
            Some(&json!("session-antigravity-1"))
        );
        assert_eq!(auth_config.get("user_agent"), Some(&json!("antigravity")));
    }

    #[test]
    fn single_import_preserves_distinct_top_level_access_token_for_export() {
        let payload = json!({
            "access_token": "jwt-access-token",
            "headers": {
                "authorization": "Bearer imported-session-token"
            },
        })
        .as_object()
        .cloned()
        .expect("payload should be an object");
        let mut auth_config = serde_json::Map::new();

        apply_single_import_hints("codex", &payload, &mut auth_config);

        assert_eq!(
            auth_config.get("access_token"),
            Some(&json!("jwt-access-token"))
        );
        assert_eq!(
            auth_config.get("headers"),
            Some(&json!({"authorization":"Bearer imported-session-token"}))
        );
    }

    #[test]
    fn single_import_does_not_preserve_header_bearer_as_export_access_token() {
        let payload = json!({
            "access_token": "imported-session-token",
            "headers": {
                "authorization": "Bearer imported-session-token"
            },
        })
        .as_object()
        .cloned()
        .expect("payload should be an object");
        let mut auth_config = serde_json::Map::new();

        apply_single_import_hints("codex", &payload, &mut auth_config);

        assert!(auth_config.get("access_token").is_none());
        assert_eq!(
            auth_config.get("headers"),
            Some(&json!({"authorization":"Bearer imported-session-token"}))
        );
    }

    #[test]
    fn single_import_does_not_preserve_access_token_from_refresh_token_payload_for_export() {
        let payload = json!({
            "refresh_token": "refresh-token",
            "access_token": "refreshed-access-token",
        })
        .as_object()
        .cloned()
        .expect("payload should be an object");
        let mut auth_config = serde_json::Map::new();

        apply_single_import_hints("codex", &payload, &mut auth_config);

        assert!(auth_config.get("access_token").is_none());
    }

    #[test]
    fn windsurf_import_error_redacts_http_body() {
        let error = OAuthError::HttpStatus {
            status_code: 400,
            body_excerpt: "token=secret-token password=secret-password".to_string(),
        };

        let detail = sanitize_windsurf_import_error(&error);

        assert_eq!(detail, "Windsurf 凭据验证失败: HTTP 400");
        assert!(!detail.contains("secret-token"));
        assert!(!detail.contains("secret-password"));
    }

    #[test]
    fn windsurf_import_error_redacts_invalid_response_detail() {
        let error =
            OAuthError::invalid_response("RegisterUser failed with firebase_id_token=secret-token");

        let detail = sanitize_windsurf_import_error(&error);

        assert_eq!(detail, "Windsurf 凭据验证失败");
        assert!(!detail.contains("secret-token"));
        assert!(!detail.contains("firebase_id_token"));
    }

    #[test]
    fn windsurf_import_error_keeps_safe_post_auth_stage() {
        let error = OAuthError::invalid_response("WindsurfPostAuth response missing sessionToken");

        let detail = sanitize_windsurf_import_error(&error);

        assert_eq!(
            detail,
            "Windsurf 凭据验证失败: PostAuth 未返回 sessionToken"
        );
    }
}
