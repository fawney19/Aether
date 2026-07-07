use std::{sync::OnceLock, time::Duration};

use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use axum::http::Uri;
use base64::Engine as _;
use hmac::Mac;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info};

use crate::wallet_runtime::{
    local_rejection_from_wallet_access, resolve_wallet_auth_gate, resolve_wallet_auth_gate_uncached,
};
use crate::{AppState, GatewayError};

use super::super::GatewayControlDecision;
use super::credentials::{
    build_auth_context_cache_key, current_unix_secs, extract_request_credentials,
    extract_trusted_admin_headers, hash_api_key,
};
use super::gate::GatewayLocalAuthRejection;
use super::principal::derive_principal_candidate;
use super::types::{
    GatewayCredentialCarrier, GatewayPrincipalCandidate, GatewayTrustedAuthHeaders,
};
use crate::cache::AuthContextInflightRegistration;
use crate::headers::header_value_str;

const AUTH_CONTEXT_CACHE_TTL: Duration = Duration::from_secs(60);
const AUTH_CONTEXT_NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(10);
const AUTH_CONTEXT_CACHE_MAX_ENTRIES: usize = 10_000;
const AUTH_CONTEXT_CACHE_MAX_ENTRIES_ENV: &str = "AETHER_GATEWAY_AUTH_CONTEXT_CACHE_MAX_ENTRIES";
const AUTH_CONTEXT_CACHE_REFRESH_ON_HIT_ENV: &str =
    "AETHER_GATEWAY_AUTH_CONTEXT_CACHE_REFRESH_ON_HIT";
const AUTH_CONTEXT_NEGATIVE_CACHE_TTL_SECS_ENV: &str =
    "AETHER_GATEWAY_AUTH_CONTEXT_NEGATIVE_CACHE_TTL_SECS";
const AUTH_CONTEXT_NEGATIVE_CACHE_KEY_PREFIX: &str = "negative:";

#[derive(Debug, Clone, Deserialize)]
struct AntigravityBearerBridgeConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    auth_user_id: String,
    #[serde(default)]
    auth_api_key_id: String,
    #[serde(default)]
    bearer_sha256_allowlist: Vec<String>,
    #[serde(default)]
    allow_unverified_google_bearer: bool,
}

impl AntigravityBearerBridgeConfig {
    fn bearer_validation_mode(&self, raw_bearer: &str) -> Option<&'static str> {
        if !self.bearer_sha256_allowlist.is_empty() {
            let bearer_hash = hash_api_key(raw_bearer);
            return self
                .bearer_sha256_allowlist
                .iter()
                .any(|allowed| allowed.trim().eq_ignore_ascii_case(&bearer_hash))
                .then_some("sha256_allowlist");
        }

        self.allow_unverified_google_bearer
            .then_some("explicit_unverified")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct GatewayControlAuthContext {
    pub(crate) user_id: String,
    pub(crate) api_key_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) api_key_name: Option<String>,
    pub(crate) balance_remaining: Option<f64>,
    pub(crate) access_allowed: bool,
    #[serde(skip)]
    pub(crate) user_rate_limit: Option<i32>,
    #[serde(skip)]
    pub(crate) api_key_rate_limit: Option<i32>,
    #[serde(skip)]
    pub(crate) api_key_is_standalone: bool,
    #[serde(skip)]
    pub(crate) admin_bypass_limits: bool,
    #[serde(skip)]
    pub(crate) local_rejection: Option<GatewayLocalAuthRejection>,
    #[serde(skip)]
    pub(crate) allowed_models: Option<Vec<String>>,
    #[serde(skip)]
    pub(crate) ip_rules: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayAdminPrincipalContext {
    pub(crate) user_id: String,
    pub(crate) user_role: String,
    pub(crate) session_id: Option<String>,
    pub(crate) management_token_id: Option<String>,
    pub(crate) management_token_permissions: Option<Vec<String>>,
}

pub(in super::super) enum ControlDecisionAuthResolution {
    Resolved(GatewayControlDecision),
}

pub(in super::super) async fn resolve_control_decision_auth(
    state: &AppState,
    headers: &http::HeaderMap,
    uri: &Uri,
    trace_id: &str,
    mut decision: GatewayControlDecision,
) -> Result<ControlDecisionAuthResolution, GatewayError> {
    if let Some(admin_principal) =
        resolve_trusted_admin_principal(headers, decision.auth_endpoint_signature.as_deref())
    {
        log_admin_principal_resolution(trace_id, &decision, "trusted_headers", &admin_principal);
        decision.admin_principal = Some(admin_principal);
    } else if let Some(admin_principal) = resolve_local_admin_principal(
        state,
        headers,
        uri,
        decision.auth_endpoint_signature.as_deref(),
    )
    .await?
    {
        log_admin_principal_resolution(trace_id, &decision, "local_session", &admin_principal);
        decision.admin_principal = Some(admin_principal);
    }

    let auth_context_cache_key = decision
        .auth_endpoint_signature
        .as_deref()
        .and_then(|signature| build_auth_context_cache_key(headers, uri, signature));

    let mut resolved_auth_context = None;
    if let Some(cache_key) = auth_context_cache_key.as_deref() {
        if let Some(auth_context) = get_cached_auth_context(state, cache_key) {
            resolved_auth_context = if auth_context_cache_refresh_on_hit() {
                Some(
                    refresh_cached_auth_context_or_reuse(
                        state,
                        cache_key,
                        auth_context,
                        decision.auth_endpoint_signature.as_deref(),
                    )
                    .await?,
                )
            } else {
                Some(auth_context)
            };
        }
    }

    if resolved_auth_context.is_none() {
        resolved_auth_context = resolve_data_backed_auth_context_cached(
            state,
            auth_context_cache_key.as_deref(),
            headers,
            uri,
            decision.auth_endpoint_signature.as_deref(),
            true,
        )
        .await?;
        if let (Some(cache_key), Some(auth_context)) = (
            auth_context_cache_key.as_ref(),
            resolved_auth_context.as_ref(),
        ) {
            put_cached_auth_context(state, cache_key.clone(), auth_context.clone());
        }
    }

    if let Some(auth_context) = resolved_auth_context {
        apply_resolved_auth_context_to_decision(trace_id, &mut decision, auth_context);
    }

    if decision.local_auth_rejection.is_some() {
        log_local_auth_rejection(trace_id, &decision);
        return Ok(ControlDecisionAuthResolution::Resolved(decision));
    }

    if decision.is_execution_runtime_candidate() {
        return Ok(ControlDecisionAuthResolution::Resolved(decision));
    }

    if decision.auth_context.is_some() {
        return Ok(ControlDecisionAuthResolution::Resolved(decision));
    }

    if allows_missing_data_backed_auth_context(&decision) {
        return Ok(ControlDecisionAuthResolution::Resolved(decision));
    }

    Ok(ControlDecisionAuthResolution::Resolved(decision))
}

fn log_admin_principal_resolution(
    trace_id: &str,
    decision: &GatewayControlDecision,
    resolution: &'static str,
    admin_principal: &GatewayAdminPrincipalContext,
) {
    debug!(
        event_name = "admin_principal_resolved",
        log_type = "debug",
        debug_context = "control_auth",
        trace_id = %trace_id,
        route_class = decision.route_class.as_deref().unwrap_or("unknown"),
        route_family = decision.route_family.as_deref().unwrap_or("unknown"),
        route_kind = decision.route_kind.as_deref().unwrap_or("unknown"),
        resolution,
        admin_user_id = admin_principal.user_id.as_str(),
        admin_user_role = admin_principal.user_role.as_str(),
        admin_session_id = admin_principal.session_id.as_deref().unwrap_or("-"),
        admin_management_token_id = admin_principal.management_token_id.as_deref().unwrap_or("-"),
        "resolved admin principal for control decision"
    );
}

fn log_auth_context_resolution(
    trace_id: &str,
    decision: &GatewayControlDecision,
    auth_context: &GatewayControlAuthContext,
) {
    let balance_remaining = auth_context
        .balance_remaining
        .map(|value| format!("{value:.4}"))
        .unwrap_or_else(|| "-".to_string());
    info!(
        event_name = "auth_context_resolved",
        log_type = "event",
        status = if auth_context.access_allowed {
            "allowed"
        } else {
            "blocked"
        },
        trace_id = %trace_id,
        route_class = decision.route_class.as_deref().unwrap_or("unknown"),
        route_family = decision.route_family.as_deref().unwrap_or("unknown"),
        route_kind = decision.route_kind.as_deref().unwrap_or("unknown"),
        user_id = auth_context.user_id.as_str(),
        api_key_id = auth_context.api_key_id.as_str(),
        api_key_name = auth_context.api_key_name.as_deref().unwrap_or("-"),
        balance_remaining = balance_remaining.as_str(),
        access_allowed = auth_context.access_allowed,
        api_key_is_standalone = auth_context.api_key_is_standalone,
        has_local_rejection = auth_context.local_rejection.is_some(),
        "resolved data-backed auth context for control decision"
    );
}

fn log_local_auth_rejection(trace_id: &str, decision: &GatewayControlDecision) {
    let Some(rejection) = decision.local_auth_rejection.as_ref() else {
        return;
    };
    let (rejection_kind, rejection_detail) = match rejection {
        GatewayLocalAuthRejection::InvalidApiKey => ("invalid_api_key", "-".to_string()),
        GatewayLocalAuthRejection::LockedApiKey => ("locked_api_key", "-".to_string()),
        GatewayLocalAuthRejection::WalletUnavailable => ("wallet_unavailable", "-".to_string()),
        GatewayLocalAuthRejection::BalanceDenied { remaining } => (
            "balance_denied",
            remaining
                .map(|value| format!("remaining_usd={value:.4}"))
                .unwrap_or_else(|| "remaining_usd=unknown".to_string()),
        ),
        GatewayLocalAuthRejection::ProviderNotAllowed { provider } => {
            ("provider_not_allowed", provider.clone())
        }
        GatewayLocalAuthRejection::ApiFormatNotAllowed { api_format } => {
            ("api_format_not_allowed", api_format.clone())
        }
        GatewayLocalAuthRejection::ModelNotAllowed { model } => {
            ("model_not_allowed", model.clone())
        }
        GatewayLocalAuthRejection::IpNotAllowed { remote_ip } => {
            ("ip_not_allowed", remote_ip.clone())
        }
    };
    info!(
        event_name = "local_auth_rejected",
        log_type = "event",
        status = "rejected",
        trace_id = %trace_id,
        route_class = decision.route_class.as_deref().unwrap_or("unknown"),
        route_family = decision.route_family.as_deref().unwrap_or("unknown"),
        route_kind = decision.route_kind.as_deref().unwrap_or("unknown"),
        rejection_kind,
        rejection_detail = %rejection_detail,
        "rejected local control request during auth gate resolution"
    );
}

fn allows_missing_data_backed_auth_context(decision: &GatewayControlDecision) -> bool {
    matches!(
        decision.route_kind.as_deref(),
        Some("chat" | "cli" | "compact")
    )
}

fn resolve_trusted_admin_principal(
    headers: &http::HeaderMap,
    auth_endpoint_signature: Option<&str>,
) -> Option<GatewayAdminPrincipalContext> {
    if !auth_endpoint_signature
        .map(str::trim)
        .unwrap_or_default()
        .starts_with("admin:")
    {
        return None;
    }
    let trusted_headers = extract_trusted_admin_headers(headers)?;
    Some(GatewayAdminPrincipalContext {
        user_id: trusted_headers.user_id,
        user_role: trusted_headers.user_role,
        session_id: trusted_headers.session_id,
        management_token_id: trusted_headers.management_token_id,
        management_token_permissions: None,
    })
}

async fn resolve_local_admin_principal(
    state: &AppState,
    headers: &http::HeaderMap,
    uri: &Uri,
    auth_endpoint_signature: Option<&str>,
) -> Result<Option<GatewayAdminPrincipalContext>, GatewayError> {
    let Some(signature) = auth_endpoint_signature
        .map(str::trim)
        .filter(|value| value.starts_with("admin:"))
    else {
        return Ok(None);
    };
    let extracted = extract_request_credentials(headers, uri, signature);
    let Some(access_token) = extracted.bundle.authorization_bearer.as_deref() else {
        return Ok(None);
    };
    let claims = match decode_local_auth_token(access_token, "access") {
        Ok(claims) => claims,
        Err(_) => return Ok(None),
    };
    if claims
        .get("role")
        .and_then(Value::as_str)
        .is_some_and(|role| !crate::roles::can_access_admin_console(role))
    {
        return Ok(None);
    }

    resolve_local_admin_principal_from_claims(state, headers, uri, &claims).await
}

async fn resolve_local_admin_principal_from_claims(
    state: &AppState,
    headers: &http::HeaderMap,
    uri: &Uri,
    claims: &serde_json::Map<String, Value>,
) -> Result<Option<GatewayAdminPrincipalContext>, GatewayError> {
    let Some(user_id) = claims.get("user_id").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(session_id) = claims.get("session_id").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(client_device_id) = extract_local_admin_client_device_id(headers, uri) else {
        return Ok(None);
    };

    let Some(user) = state.find_user_auth_by_id(user_id).await? else {
        return Ok(None);
    };
    if !user.is_active || user.is_deleted || !crate::roles::can_access_admin_console(&user.role) {
        return Ok(None);
    }

    let now = chrono::Utc::now();
    let Some(session) = state.find_user_session(user_id, session_id).await? else {
        return Ok(None);
    };
    if session.is_revoked()
        || session.is_expired(now)
        || session.client_device_id != client_device_id
    {
        return Ok(None);
    }

    if session.should_touch(now) {
        let _ = state
            .touch_user_session(
                user_id,
                session_id,
                now,
                None,
                local_admin_user_agent(headers).as_deref(),
            )
            .await;
    }

    Ok(Some(GatewayAdminPrincipalContext {
        user_id: user.id,
        user_role: user.role,
        session_id: Some(session.id),
        management_token_id: None,
        management_token_permissions: None,
    }))
}

fn extract_local_admin_client_device_id(headers: &http::HeaderMap, uri: &Uri) -> Option<String> {
    let header_value = header_value_str(headers, "x-client-device-id");
    let query_value = uri.query().and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "client_device_id")
            .map(|(_, value)| value.into_owned())
    });
    let candidate = header_value.or(query_value)?;
    let candidate = candidate.trim();
    if candidate.is_empty()
        || candidate.len() > 128
        || !candidate
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return None;
    }
    Some(candidate.to_string())
}

fn local_admin_user_agent(headers: &http::HeaderMap) -> Option<String> {
    header_value_str(headers, http::header::USER_AGENT.as_str())
        .map(|value| value.chars().take(1000).collect())
}

fn local_auth_secret() -> String {
    std::env::var("JWT_SECRET_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "aether-rust-dev-jwt-secret".to_string())
}

fn decode_local_auth_token(
    token: &str,
    expected_type: &str,
) -> Result<serde_json::Map<String, Value>, String> {
    let mut parts = token.split('.');
    let Some(header_segment) = parts.next() else {
        return Err("invalid token".to_string());
    };
    let Some(payload_segment) = parts.next() else {
        return Err("invalid token".to_string());
    };
    let Some(signature_segment) = parts.next() else {
        return Err("invalid token".to_string());
    };
    if parts.next().is_some() {
        return Err("invalid token".to_string());
    }

    let signing_input = format!("{header_segment}.{payload_segment}");
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature_segment)
        .map_err(|_| "invalid token".to_string())?;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(local_auth_secret().as_bytes())
        .map_err(|_| "invalid token".to_string())?;
    mac.update(signing_input.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| "invalid token".to_string())?;

    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_segment)
        .map_err(|_| "invalid token".to_string())?;
    let payload =
        serde_json::from_slice::<Value>(&payload_bytes).map_err(|_| "invalid token".to_string())?;
    let payload = payload
        .as_object()
        .cloned()
        .ok_or_else(|| "invalid token".to_string())?;
    let actual_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if actual_type != expected_type {
        return Err("invalid token".to_string());
    }
    let exp = payload
        .get("exp")
        .and_then(Value::as_i64)
        .ok_or_else(|| "invalid token".to_string())?;
    if exp <= chrono::Utc::now().timestamp() {
        return Err("expired token".to_string());
    }
    Ok(payload)
}

pub(crate) async fn resolve_execution_runtime_auth_context(
    state: &AppState,
    decision: &GatewayControlDecision,
    headers: &http::HeaderMap,
    uri: &Uri,
    trace_id: &str,
) -> Result<Option<GatewayControlAuthContext>, GatewayError> {
    let _ = trace_id;

    if let Some(auth_context) = decision.auth_context.clone() {
        if !auth_context_cache_refresh_on_hit() {
            return Ok(Some(auth_context));
        }
        return refresh_decision_auth_context_on_hit(
            state,
            headers,
            uri,
            decision.auth_endpoint_signature.as_deref(),
            auth_context,
        )
        .await
        .map(Some);
    }

    let Some(auth_endpoint_signature) = decision.auth_endpoint_signature.as_deref() else {
        return Ok(None);
    };
    let Some(cache_key) = build_auth_context_cache_key(headers, uri, auth_endpoint_signature)
    else {
        return Ok(None);
    };

    if let Some(auth_context) = get_cached_auth_context(state, &cache_key) {
        if !auth_context_cache_refresh_on_hit() {
            return Ok(Some(auth_context));
        }

        let refreshed = refresh_cached_auth_context_or_reuse(
            state,
            &cache_key,
            auth_context,
            Some(auth_endpoint_signature),
        )
        .await?;
        return Ok(Some(refreshed));
    }

    if let Some(auth_context) = resolve_data_backed_auth_context_cached(
        state,
        Some(cache_key.as_str()),
        headers,
        uri,
        Some(auth_endpoint_signature),
        true,
    )
    .await?
    {
        if auth_context.user_id.is_empty() || auth_context.api_key_id.is_empty() {
            return Ok(None);
        }
        put_cached_auth_context(state, cache_key, auth_context.clone());
        return Ok(Some(auth_context));
    }

    Ok(None)
}

async fn refresh_decision_auth_context_on_hit(
    state: &AppState,
    headers: &http::HeaderMap,
    uri: &Uri,
    auth_endpoint_signature: Option<&str>,
    auth_context: GatewayControlAuthContext,
) -> Result<GatewayControlAuthContext, GatewayError> {
    let Some(auth_endpoint_signature) = auth_endpoint_signature else {
        return Ok(auth_context);
    };
    let Some(cache_key) = build_auth_context_cache_key(headers, uri, auth_endpoint_signature)
    else {
        return refresh_execution_runtime_auth_context(
            state,
            auth_context,
            Some(auth_endpoint_signature),
        )
        .await;
    };
    refresh_cached_auth_context_or_reuse(
        state,
        &cache_key,
        auth_context,
        Some(auth_endpoint_signature),
    )
    .await
}

async fn refresh_cached_auth_context_or_reuse(
    state: &AppState,
    cache_key: &str,
    auth_context: GatewayControlAuthContext,
    auth_endpoint_signature: Option<&str>,
) -> Result<GatewayControlAuthContext, GatewayError> {
    match state.auth_context_cache.register_inflight(cache_key) {
        AuthContextInflightRegistration::Leader(_guard) => {
            let refreshed = refresh_execution_runtime_auth_context(
                state,
                auth_context,
                auth_endpoint_signature,
            )
            .await?;
            put_cached_auth_context(state, cache_key.to_string(), refreshed.clone());
            Ok(refreshed)
        }
        AuthContextInflightRegistration::Follower => Ok(auth_context),
        AuthContextInflightRegistration::Bypass => {
            refresh_execution_runtime_auth_context(state, auth_context, auth_endpoint_signature)
                .await
        }
    }
}

async fn resolve_data_backed_auth_context_cached(
    state: &AppState,
    cache_key: Option<&str>,
    headers: &http::HeaderMap,
    uri: &Uri,
    auth_endpoint_signature: Option<&str>,
    cache_negative: bool,
) -> Result<Option<GatewayControlAuthContext>, GatewayError> {
    let Some(cache_key) = cache_key else {
        return resolve_data_backed_auth_context(state, headers, uri, auth_endpoint_signature)
            .await;
    };
    loop {
        let notified = state.auth_context_cache.notified();
        match state.auth_context_cache.register_inflight(cache_key) {
            AuthContextInflightRegistration::Leader(_guard) => {
                let resolved =
                    resolve_data_backed_auth_context(state, headers, uri, auth_endpoint_signature)
                        .await?;
                if let Some(auth_context) = resolved.as_ref() {
                    if cache_negative
                        || (!auth_context.user_id.is_empty() && !auth_context.api_key_id.is_empty())
                    {
                        put_cached_auth_context(state, cache_key.to_string(), auth_context.clone());
                    }
                }
                return Ok(resolved);
            }
            AuthContextInflightRegistration::Follower => {
                notified.await;
                if let Some(auth_context) = get_cached_auth_context(state, cache_key) {
                    return Ok(Some(auth_context));
                }
                if !cache_negative {
                    return Ok(None);
                }
            }
            AuthContextInflightRegistration::Bypass => {
                return resolve_data_backed_auth_context(
                    state,
                    headers,
                    uri,
                    auth_endpoint_signature,
                )
                .await;
            }
        }
    }
}

pub(crate) async fn refresh_execution_runtime_auth_context(
    state: &AppState,
    auth_context: GatewayControlAuthContext,
    auth_endpoint_signature: Option<&str>,
) -> Result<GatewayControlAuthContext, GatewayError> {
    if auth_context.local_rejection.is_some() || !auth_context.access_allowed {
        return Ok(auth_context);
    }
    let Some(auth_endpoint_signature) = auth_endpoint_signature
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(auth_context);
    };
    if !state.has_auth_api_key_reader()
        || auth_context.user_id.trim().is_empty()
        || auth_context.api_key_id.trim().is_empty()
    {
        return Ok(auth_context);
    }

    let snapshot = state
        .data
        .read_auth_api_key_snapshot(
            &auth_context.user_id,
            &auth_context.api_key_id,
            current_unix_secs(),
        )
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Some(snapshot) = snapshot else {
        let mut denied = auth_context;
        denied.access_allowed = false;
        denied.local_rejection = Some(GatewayLocalAuthRejection::InvalidApiKey);
        denied.balance_remaining = None;
        return Ok(denied);
    };

    let wallet_access = resolve_wallet_auth_gate_uncached(state, &snapshot).await?;
    Ok(build_data_backed_auth_context(
        state,
        snapshot,
        auth_endpoint_signature,
        Some(true),
        auth_context.balance_remaining,
        wallet_access,
    )
    .await)
}

fn put_cached_auth_context(
    state: &AppState,
    cache_key: String,
    auth_context: GatewayControlAuthContext,
) {
    let (cache_key, ttl) = if is_negative_auth_context(&auth_context) {
        let ttl = auth_context_negative_cache_ttl();
        if ttl.is_zero() {
            return;
        }
        (
            negative_auth_context_cache_key(&cache_key),
            AUTH_CONTEXT_CACHE_TTL.max(ttl),
        )
    } else {
        (cache_key, AUTH_CONTEXT_CACHE_TTL)
    };
    state.auth_context_cache.insert(
        cache_key,
        auth_context,
        ttl,
        auth_context_cache_max_entries(),
    );
}

fn auth_context_cache_max_entries() -> usize {
    static MAX_ENTRIES: OnceLock<usize> = OnceLock::new();
    *MAX_ENTRIES.get_or_init(|| {
        std::env::var(AUTH_CONTEXT_CACHE_MAX_ENTRIES_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(AUTH_CONTEXT_CACHE_MAX_ENTRIES)
    })
}

fn auth_context_cache_refresh_on_hit() -> bool {
    static REFRESH_ON_HIT: OnceLock<bool> = OnceLock::new();
    *REFRESH_ON_HIT.get_or_init(|| {
        std::env::var(AUTH_CONTEXT_CACHE_REFRESH_ON_HIT_ENV)
            .ok()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(true)
    })
}

fn auth_context_negative_cache_ttl() -> Duration {
    static NEGATIVE_TTL: OnceLock<Duration> = OnceLock::new();
    *NEGATIVE_TTL.get_or_init(|| {
        std::env::var(AUTH_CONTEXT_NEGATIVE_CACHE_TTL_SECS_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(AUTH_CONTEXT_NEGATIVE_CACHE_TTL)
    })
}

fn negative_auth_context_cache_key(cache_key: &str) -> String {
    format!("{AUTH_CONTEXT_NEGATIVE_CACHE_KEY_PREFIX}{cache_key}")
}

fn is_negative_auth_context(auth_context: &GatewayControlAuthContext) -> bool {
    auth_context.user_id.is_empty()
        || auth_context.api_key_id.is_empty()
        || matches!(
            auth_context.local_rejection,
            Some(GatewayLocalAuthRejection::InvalidApiKey)
        )
}

fn apply_resolved_auth_context_to_decision(
    trace_id: &str,
    decision: &mut GatewayControlDecision,
    auth_context: GatewayControlAuthContext,
) {
    log_auth_context_resolution(trace_id, decision, &auth_context);
    decision.local_auth_rejection = auth_context.local_rejection.clone();
    if !auth_context.user_id.is_empty() && !auth_context.api_key_id.is_empty() {
        decision.auth_context = Some(auth_context);
    }
}

pub(super) async fn resolve_data_backed_auth_context(
    state: &AppState,
    headers: &http::HeaderMap,
    uri: &Uri,
    auth_endpoint_signature: Option<&str>,
) -> Result<Option<GatewayControlAuthContext>, GatewayError> {
    let Some(signature) = auth_endpoint_signature
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    if !state.has_auth_api_key_reader() {
        return Ok(None);
    }
    let extracted = extract_request_credentials(headers, uri, signature);
    let principal = derive_principal_candidate(&extracted);
    let now_unix_secs = current_unix_secs();

    match principal {
        Some(GatewayPrincipalCandidate::TrustedHeaders(trusted_headers)) => {
            resolve_trusted_auth_context(state, signature, trusted_headers, now_unix_secs).await
        }
        Some(GatewayPrincipalCandidate::ApiKeyHash { key_hash, .. }) => {
            let snapshot = state
                .read_cached_auth_api_key_snapshot_by_key_hash(&key_hash, now_unix_secs)
                .await?;
            let Some(snapshot) = snapshot else {
                return Ok(Some(GatewayControlAuthContext {
                    user_id: String::new(),
                    api_key_id: String::new(),
                    username: None,
                    api_key_name: None,
                    balance_remaining: None,
                    access_allowed: false,
                    user_rate_limit: None,
                    api_key_rate_limit: None,
                    api_key_is_standalone: false,
                    admin_bypass_limits: false,
                    local_rejection: Some(GatewayLocalAuthRejection::InvalidApiKey),
                    allowed_models: None,
                    ip_rules: None,
                }));
            };

            state
                .touch_auth_api_key_last_used_best_effort(&snapshot.api_key_id)
                .await;

            let wallet_access = resolve_wallet_auth_gate(state, &snapshot).await?;
            Ok(Some(
                build_data_backed_auth_context(
                    state,
                    snapshot,
                    signature,
                    None,
                    None,
                    wallet_access,
                )
                .await,
            ))
        }
        Some(GatewayPrincipalCandidate::DeferredBearerToken { raw, carrier }) => {
            if let Some(auth_context) = resolve_antigravity_bearer_bridge_auth_context(
                state,
                signature,
                raw.as_str(),
                carrier,
                now_unix_secs,
            )
            .await?
            {
                return Ok(Some(auth_context));
            }
            Ok(None)
        }
        Some(GatewayPrincipalCandidate::DeferredCookieHeader { .. }) => Ok(None),
        None => Ok(None),
    }
}

async fn resolve_antigravity_bearer_bridge_auth_context(
    state: &AppState,
    auth_endpoint_signature: &str,
    raw_bearer: &str,
    carrier: GatewayCredentialCarrier,
    now_unix_secs: u64,
) -> Result<Option<GatewayControlAuthContext>, GatewayError> {
    if carrier != GatewayCredentialCarrier::AuthorizationBearer
        || !auth_endpoint_signature
            .trim()
            .eq_ignore_ascii_case("antigravity:v1internal")
    {
        return Ok(None);
    }

    let Some(config_value) = state
        .read_system_config_json_value(crate::constants::ANTIGRAVITY_BEARER_BRIDGE_CONFIG_KEY)
        .await?
    else {
        return Ok(None);
    };
    if config_value.is_null() {
        return Ok(None);
    }
    let config: AntigravityBearerBridgeConfig =
        serde_json::from_value(config_value).map_err(|err| {
            GatewayError::Internal(format!(
                "{} invalid: {err}",
                crate::constants::ANTIGRAVITY_BEARER_BRIDGE_CONFIG_KEY
            ))
        })?;
    if !config.enabled {
        return Ok(None);
    }
    let Some(validation_mode) = config.bearer_validation_mode(raw_bearer) else {
        return Ok(None);
    };
    let user_id = config.auth_user_id.trim();
    let api_key_id = config.auth_api_key_id.trim();
    if user_id.is_empty() || api_key_id.is_empty() {
        return Err(GatewayError::Internal(format!(
            "{} requires auth_user_id and auth_api_key_id",
            crate::constants::ANTIGRAVITY_BEARER_BRIDGE_CONFIG_KEY
        )));
    }

    let snapshot = state
        .data
        .read_auth_api_key_snapshot(user_id, api_key_id, now_unix_secs)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Some(snapshot) = snapshot else {
        return Ok(Some(GatewayControlAuthContext {
            user_id: user_id.to_string(),
            api_key_id: api_key_id.to_string(),
            username: None,
            api_key_name: None,
            balance_remaining: None,
            access_allowed: false,
            user_rate_limit: None,
            api_key_rate_limit: None,
            api_key_is_standalone: false,
            admin_bypass_limits: false,
            local_rejection: Some(GatewayLocalAuthRejection::InvalidApiKey),
            allowed_models: None,
            ip_rules: None,
        }));
    };

    let wallet_access = resolve_wallet_auth_gate(state, &snapshot).await?;
    let auth_context = build_data_backed_auth_context(
        state,
        snapshot,
        auth_endpoint_signature,
        None,
        None,
        wallet_access,
    )
    .await;
    info!(
        event_name = "antigravity_bearer_bridge_auth_context_resolved",
        log_type = "event",
        validation_mode,
        user_id = auth_context.user_id.as_str(),
        api_key_id = auth_context.api_key_id.as_str(),
        access_allowed = auth_context.access_allowed,
        has_local_rejection = auth_context.local_rejection.is_some(),
        "resolved Antigravity bearer bridge auth context"
    );
    Ok(Some(auth_context))
}

async fn resolve_trusted_auth_context(
    state: &AppState,
    auth_endpoint_signature: &str,
    trusted_headers: GatewayTrustedAuthHeaders,
    now_unix_secs: u64,
) -> Result<Option<GatewayControlAuthContext>, GatewayError> {
    let snapshot = state
        .data
        .read_auth_api_key_snapshot(
            &trusted_headers.user_id,
            &trusted_headers.api_key_id,
            now_unix_secs,
        )
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Some(snapshot) = snapshot else {
        return Ok(Some(GatewayControlAuthContext {
            user_id: trusted_headers.user_id,
            api_key_id: trusted_headers.api_key_id,
            username: None,
            api_key_name: None,
            balance_remaining: trusted_headers.balance_remaining,
            access_allowed: false,
            user_rate_limit: None,
            api_key_rate_limit: None,
            api_key_is_standalone: false,
            admin_bypass_limits: false,
            local_rejection: Some(GatewayLocalAuthRejection::InvalidApiKey),
            allowed_models: None,
            ip_rules: None,
        }));
    };

    let wallet_access = resolve_wallet_auth_gate(state, &snapshot).await?;
    Ok(Some(
        build_data_backed_auth_context(
            state,
            snapshot,
            auth_endpoint_signature,
            trusted_headers.access_allowed,
            trusted_headers.balance_remaining,
            wallet_access,
        )
        .await,
    ))
}

async fn build_data_backed_auth_context(
    state: &AppState,
    snapshot: crate::data::auth::GatewayAuthApiKeySnapshot,
    auth_endpoint_signature: &str,
    header_access_allowed: Option<bool>,
    balance_remaining: Option<f64>,
    wallet_access: Option<aether_wallet::WalletAccessDecision>,
) -> GatewayControlAuthContext {
    let allowed_models = snapshot
        .effective_allowed_models()
        .map(|items| items.to_vec());
    let invalid_api_key = !snapshot.user_is_active
        || snapshot.user_is_deleted
        || !snapshot.api_key_is_active
        || snapshot
            .api_key_expires_at_unix_secs
            .is_some_and(|expires_at| expires_at < current_unix_secs());
    let locked_api_key = snapshot.api_key_is_locked && !snapshot.api_key_is_standalone;
    let key_access_allowed = header_access_allowed
        .map(|value| value && snapshot.currently_usable)
        .unwrap_or(snapshot.currently_usable);
    let wallet_remaining = wallet_access
        .as_ref()
        .and_then(|decision| decision.remaining);
    let requested_provider = auth_endpoint_signature
        .split_once(':')
        .map(|(provider, _)| provider)
        .unwrap_or(auth_endpoint_signature)
        .trim();
    let identity_only = auth_gate_identity_only(auth_endpoint_signature);
    let requested_provider_allowed = identity_only
        || auth_snapshot_allows_requested_provider(state, &snapshot, auth_endpoint_signature).await;
    let local_rejection = if invalid_api_key {
        Some(GatewayLocalAuthRejection::InvalidApiKey)
    } else if locked_api_key {
        Some(GatewayLocalAuthRejection::LockedApiKey)
    } else if let Some(rejection) = wallet_access
        .as_ref()
        .and_then(local_rejection_from_wallet_access)
    {
        Some(rejection)
    } else if header_access_allowed.is_some_and(|value| !value) && snapshot.currently_usable {
        Some(GatewayLocalAuthRejection::BalanceDenied {
            remaining: balance_remaining.or(wallet_remaining),
        })
    } else if !requested_provider.is_empty() && !requested_provider_allowed {
        Some(GatewayLocalAuthRejection::ProviderNotAllowed {
            provider: requested_provider.to_string(),
        })
    } else if !identity_only
        && snapshot
            .effective_allowed_api_formats()
            .is_some_and(|allowed| {
                !contains_api_format_or_alias(
                    allowed,
                    auth_gate_api_format(auth_endpoint_signature).as_str(),
                )
            })
    {
        Some(GatewayLocalAuthRejection::ApiFormatNotAllowed {
            api_format: auth_endpoint_signature.to_string(),
        })
    } else {
        None
    };

    GatewayControlAuthContext {
        username: Some(snapshot.username.clone()),
        api_key_name: snapshot.api_key_name.clone(),
        user_id: snapshot.user_id,
        api_key_id: snapshot.api_key_id,
        balance_remaining: wallet_remaining.or(balance_remaining),
        access_allowed: key_access_allowed && local_rejection.is_none(),
        user_rate_limit: snapshot.user_rate_limit,
        api_key_rate_limit: snapshot.api_key_rate_limit,
        api_key_is_standalone: snapshot.api_key_is_standalone,
        admin_bypass_limits: snapshot.user_role.eq_ignore_ascii_case("admin")
            && !snapshot.api_key_is_standalone,
        local_rejection,
        allowed_models,
        ip_rules: snapshot.api_key_ip_rules,
    }
}

fn contains_api_format_or_alias(items: &[String], target: &str) -> bool {
    items.iter().any(|item| api_format_matches(item, target))
}

fn normalize_api_format_alias(value: &str) -> String {
    crate::ai_serving::normalize_api_format_alias(value)
}

fn auth_gate_api_format(auth_endpoint_signature: &str) -> String {
    let normalized = normalize_api_format_alias(auth_endpoint_signature);
    if normalized == "antigravity:v1internal" {
        "gemini:generate_content".to_string()
    } else {
        normalized
    }
}

fn auth_gate_identity_only(auth_endpoint_signature: &str) -> bool {
    matches!(
        auth_endpoint_signature.trim().to_ascii_lowercase().as_str(),
        "aether:ccswitch_usage"
    )
}

fn api_format_matches(left: &str, right: &str) -> bool {
    aether_scheduler_core::api_format_matches_allowed_value(left, right)
}

async fn auth_snapshot_allows_requested_provider(
    state: &AppState,
    snapshot: &crate::data::auth::GatewayAuthApiKeySnapshot,
    auth_endpoint_signature: &str,
) -> bool {
    let Some(allowed_providers) = snapshot.effective_allowed_providers() else {
        return true;
    };
    let requested_api_format = normalize_api_format_alias(auth_endpoint_signature);
    let requested_provider = requested_api_format
        .split_once(':')
        .map(|(provider, _)| provider)
        .unwrap_or(requested_api_format.as_str())
        .trim();
    if requested_provider.is_empty() {
        return true;
    }
    if allowed_providers.is_empty() {
        return false;
    }
    if allowed_providers
        .iter()
        .any(|value| allowed_provider_value_matches_requested_provider(value, requested_provider))
    {
        return true;
    }
    if !state.has_provider_catalog_data_reader() {
        return true;
    }

    let providers = match state.list_provider_catalog_providers(true).await {
        Ok(value) => value,
        Err(err) => {
            debug!(
                "skip local provider auth gate for requested provider {}: provider catalog lookup failed: {:?}",
                requested_provider,
                err
            );
            return true;
        }
    };

    let allowed_catalog_providers = providers
        .into_iter()
        .filter(|provider| {
            allowed_providers.iter().any(|value| {
                aether_scheduler_core::provider_matches_allowed_value(
                    value,
                    &provider.id,
                    &provider.name,
                    &provider.provider_type,
                )
            })
        })
        .collect::<Vec<_>>();
    if allowed_catalog_providers
        .iter()
        .any(|provider| provider_matches_requested_provider(provider, requested_provider))
    {
        return true;
    }

    let allowed_provider_ids = allowed_catalog_providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    if allowed_provider_ids.is_empty() {
        return false;
    }

    let endpoints = match state
        .list_provider_catalog_endpoints_by_provider_ids(&allowed_provider_ids)
        .await
    {
        Ok(value) => value,
        Err(err) => {
            debug!(
                "skip local provider auth gate for requested provider {}: provider endpoint lookup failed: {:?}",
                requested_provider, err
            );
            return true;
        }
    };

    endpoints.iter().any(|endpoint| {
        endpoint_matches_requested_provider(endpoint, &requested_api_format, requested_provider)
    })
}

fn allowed_provider_value_matches_requested_provider(
    allowed_value: &str,
    requested_provider: &str,
) -> bool {
    aether_scheduler_core::provider_matches_allowed_value(
        allowed_value,
        requested_provider,
        requested_provider,
        requested_provider,
    )
}

fn provider_matches_requested_provider(
    provider: &StoredProviderCatalogProvider,
    requested_provider: &str,
) -> bool {
    aether_scheduler_core::provider_matches_allowed_value(
        requested_provider,
        &provider.id,
        &provider.name,
        &provider.provider_type,
    )
}

fn endpoint_matches_requested_provider(
    endpoint: &StoredProviderCatalogEndpoint,
    requested_api_format: &str,
    requested_provider: &str,
) -> bool {
    if !endpoint.is_active {
        return false;
    }
    if api_format_matches(&endpoint.api_format, requested_api_format) {
        return true;
    }
    let endpoint_api_format = normalize_api_format_alias(&endpoint.api_format);
    if crate::ai_serving::request_conversion_kind(requested_api_format, &endpoint_api_format)
        .is_some()
    {
        return true;
    }
    if endpoint.api_family.as_deref().is_some_and(|family| {
        allowed_provider_value_matches_requested_provider(family, requested_provider)
    }) {
        return true;
    }
    let endpoint_provider = endpoint_api_format
        .split_once(':')
        .map(|(provider, _)| provider)
        .unwrap_or(endpoint_api_format.as_str());
    allowed_provider_value_matches_requested_provider(endpoint_provider, requested_provider)
}

fn get_cached_auth_context(state: &AppState, cache_key: &str) -> Option<GatewayControlAuthContext> {
    let negative_ttl = auth_context_negative_cache_ttl();
    if !negative_ttl.is_zero() {
        if let Some(auth_context) = state
            .auth_context_cache
            .get_fresh(&negative_auth_context_cache_key(cache_key), negative_ttl)
        {
            return Some(auth_context);
        }
    }
    state
        .auth_context_cache
        .get_fresh(cache_key, AUTH_CONTEXT_CACHE_TTL)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use aether_data::repository::auth::{
        InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
    };
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data::repository::wallet::{
        InMemoryWalletRepository, StoredWalletSnapshot, WalletReadRepository,
    };
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
    };
    use axum::http::{HeaderMap, Uri};
    use futures_util::future::join_all;

    use super::{
        get_cached_auth_context, resolve_control_decision_auth, resolve_data_backed_auth_context,
        resolve_execution_runtime_auth_context, ControlDecisionAuthResolution,
        GatewayLocalAuthRejection,
    };
    use crate::control::auth::credentials::{build_auth_context_cache_key, hash_api_key};
    use crate::control::GatewayControlDecision;
    use crate::data::GatewayDataState;
    use crate::AppState;

    fn sample_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot::new(
            user_id.to_string(),
            "alice".to_string(),
            Some("alice@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-4.1"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800),
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-4.1"])),
        )
        .expect("snapshot should build")
    }

    fn uri(path: &str) -> Uri {
        path.parse().expect("uri should parse")
    }

    fn sample_provider(id: &str, name: &str, provider_type: &str) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            id.to_string(),
            name.to_string(),
            None,
            provider_type.to_string(),
        )
        .expect("provider should build")
    }

    fn sample_endpoint(
        id: &str,
        provider_id: &str,
        api_format: &str,
    ) -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            id.to_string(),
            provider_id.to_string(),
            api_format.to_string(),
            None,
            None,
            true,
        )
        .expect("endpoint should build")
    }

    #[tokio::test]
    async fn control_auth_caches_invalid_api_key_rejections() {
        let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed([]));
        let data = GatewayDataState::with_auth_api_key_repository_for_tests(repository);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data);
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer sk-missing-for-negative-cache".parse().unwrap(),
        );
        let request_uri = uri("/v1/chat/completions");
        let decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        );

        let ControlDecisionAuthResolution::Resolved(first) = resolve_control_decision_auth(
            &state,
            &headers,
            &request_uri,
            "trace-invalid-auth-cache",
            decision,
        )
        .await
        .expect("auth resolution should succeed");

        assert_eq!(
            first.local_auth_rejection,
            Some(GatewayLocalAuthRejection::InvalidApiKey)
        );
        let cache_key = build_auth_context_cache_key(&headers, &request_uri, "openai:chat")
            .expect("cache key should exist");
        let cached = get_cached_auth_context(&state, &cache_key)
            .expect("invalid API key rejection should be cached");
        assert_eq!(
            cached.local_rejection,
            Some(GatewayLocalAuthRejection::InvalidApiKey)
        );
        assert!(cached.user_id.is_empty());
        assert!(cached.api_key_id.is_empty());
    }

    #[tokio::test]
    async fn data_backed_api_key_auth_touches_last_used_once_per_throttle_window() {
        let api_key = "sk-test-touch";
        let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some(hash_api_key(api_key)),
            sample_snapshot("key-1", "user-1"),
        )]));
        let data = GatewayDataState::with_auth_api_key_repository_for_tests(repository.clone());
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data);

        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            format!("Bearer {api_key}").parse().unwrap(),
        );

        let first = resolve_data_backed_auth_context(
            &state,
            &headers,
            &uri("/v1/chat/completions"),
            Some("openai:chat"),
        )
        .await
        .expect("resolution should succeed")
        .expect("auth context should exist");
        assert_eq!(first.user_id, "user-1");
        assert_eq!(first.api_key_id, "key-1");
        assert_eq!(repository.touch_count("key-1"), 1);

        let second = resolve_data_backed_auth_context(
            &state,
            &headers,
            &uri("/v1/chat/completions"),
            Some("openai:chat"),
        )
        .await
        .expect("resolution should succeed")
        .expect("auth context should exist");
        assert_eq!(second.api_key_id, "key-1");
        assert_eq!(repository.touch_count("key-1"), 1);
    }

    #[tokio::test]
    async fn control_auth_context_singleflights_concurrent_cache_misses() {
        let api_key = "sk-test-concurrent-auth-miss";
        let repository = Arc::new(
            InMemoryAuthApiKeySnapshotRepository::seed(vec![(
                Some(hash_api_key(api_key)),
                sample_snapshot("key-concurrent-auth-miss", "user-concurrent-auth-miss"),
            )])
            .with_lookup_delay_for_tests(Duration::from_millis(20)),
        );
        let data = GatewayDataState::with_auth_api_key_repository_for_tests(repository.clone());
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data);
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            format!("Bearer {api_key}").parse().unwrap(),
        );
        let request_uri = uri("/v1/chat/completions");

        let tasks = (0..32).map(|index| {
            let decision = GatewayControlDecision::synthetic(
                "/v1/chat/completions",
                Some("ai_public".to_string()),
                Some("openai".to_string()),
                Some("chat".to_string()),
                Some("openai:chat".to_string()),
            );
            let trace_id = format!("trace-concurrent-auth-miss-{index}");
            let state = &state;
            let headers = &headers;
            let request_uri = &request_uri;
            async move {
                resolve_control_decision_auth(state, headers, request_uri, &trace_id, decision)
                    .await
            }
        });

        for result in join_all(tasks).await {
            let ControlDecisionAuthResolution::Resolved(decision) =
                result.expect("auth resolution should succeed");
            let auth_context = decision
                .auth_context
                .expect("auth context should be resolved");
            assert_eq!(auth_context.user_id, "user-concurrent-auth-miss");
            assert_eq!(auth_context.api_key_id, "key-concurrent-auth-miss");
        }
        assert_eq!(
            repository.key_hash_lookup_count(&hash_api_key(api_key)),
            1,
            "concurrent cache misses for one auth context should only load one snapshot"
        );
    }

    #[tokio::test]
    async fn data_backed_auth_context_marks_wallet_denial_as_not_allowed() {
        let api_key = "sk-test-empty-wallet";
        let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some(hash_api_key(api_key)),
            sample_snapshot("key-empty-wallet", "user-empty-wallet"),
        )]));
        let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
            StoredWalletSnapshot::new(
                "wallet-empty".to_string(),
                Some("user-empty-wallet".to_string()),
                None,
                0.0,
                0.0,
                "finite".to_string(),
                "USD".to_string(),
                "active".to_string(),
                0.0,
                0.0,
                0.0,
                0.0,
                100,
            )
            .expect("wallet should build"),
        ]));
        let data =
            GatewayDataState::with_auth_and_wallet_for_tests(auth_repository, wallet_repository);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data);

        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            format!("Bearer {api_key}").parse().unwrap(),
        );

        let auth_context = resolve_data_backed_auth_context(
            &state,
            &headers,
            &uri("/v1/chat/completions"),
            Some("openai:chat"),
        )
        .await
        .expect("resolution should succeed")
        .expect("auth context should exist");

        assert_eq!(
            auth_context.local_rejection,
            Some(GatewayLocalAuthRejection::BalanceDenied {
                remaining: Some(0.0),
            })
        );
        assert!(!auth_context.access_allowed);
    }

    #[tokio::test]
    async fn execution_runtime_auth_context_revalidates_cached_wallet_state() {
        let api_key = "sk-test-runtime-wallet-cache";
        let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some(hash_api_key(api_key)),
            sample_snapshot("key-runtime-wallet-cache", "user-runtime-wallet-cache"),
        )]));
        let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
            StoredWalletSnapshot::new(
                "wallet-runtime-cache".to_string(),
                Some("user-runtime-wallet-cache".to_string()),
                None,
                10.0,
                0.0,
                "finite".to_string(),
                "USD".to_string(),
                "active".to_string(),
                10.0,
                0.0,
                0.0,
                0.0,
                100,
            )
            .expect("wallet should build"),
        ]));
        let data = GatewayDataState::with_auth_and_wallet_for_tests(
            auth_repository,
            Arc::clone(&wallet_repository),
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data);
        let decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        );
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", api_key.parse().unwrap());

        let first = resolve_execution_runtime_auth_context(
            &state,
            &decision,
            &headers,
            &uri("/v1/chat/completions"),
            "trace-runtime-wallet-cache",
        )
        .await
        .expect("resolution should succeed")
        .expect("auth context should exist");
        assert!(first.access_allowed);

        wallet_repository
            .update_auth_user_wallet_snapshot(
                "user-runtime-wallet-cache",
                0.0,
                0.0,
                "finite",
                "USD",
                "active",
                10.0,
                10.0,
                0.0,
                0.0,
                Some(101),
            )
            .await
            .expect("wallet update should succeed")
            .expect("wallet should exist");

        let second = resolve_execution_runtime_auth_context(
            &state,
            &decision,
            &headers,
            &uri("/v1/chat/completions"),
            "trace-runtime-wallet-cache",
        )
        .await
        .expect("resolution should succeed")
        .expect("auth context should exist");

        assert_eq!(
            second.local_rejection,
            Some(GatewayLocalAuthRejection::BalanceDenied {
                remaining: Some(0.0),
            }),
            "cached auth context should revalidate wallet state before execution"
        );
        assert!(!second.access_allowed);
    }

    #[tokio::test]
    async fn execution_runtime_auth_context_singleflights_concurrent_cache_refreshes() {
        let api_key = "sk-test-runtime-auth-refresh";
        let auth_repository = Arc::new(
            InMemoryAuthApiKeySnapshotRepository::seed(vec![(
                Some(hash_api_key(api_key)),
                sample_snapshot("key-runtime-auth-refresh", "user-runtime-auth-refresh"),
            )])
            .with_lookup_delay_for_tests(Duration::from_millis(20)),
        );
        let data =
            GatewayDataState::with_auth_api_key_repository_for_tests(auth_repository.clone());
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data);
        let decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        );
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", api_key.parse().unwrap());
        let request_uri = uri("/v1/chat/completions");

        let first = resolve_execution_runtime_auth_context(
            &state,
            &decision,
            &headers,
            &request_uri,
            "trace-runtime-auth-refresh-prime",
        )
        .await
        .expect("resolution should succeed")
        .expect("auth context should exist");
        assert_eq!(first.api_key_id, "key-runtime-auth-refresh");
        assert_eq!(
            auth_repository.key_hash_lookup_count(&hash_api_key(api_key)),
            1
        );

        let tasks = (0..32).map(|index| {
            let trace_id = format!("trace-runtime-auth-refresh-{index}");
            let state = &state;
            let decision = &decision;
            let headers = &headers;
            let request_uri = &request_uri;
            async move {
                resolve_execution_runtime_auth_context(
                    state,
                    decision,
                    headers,
                    request_uri,
                    &trace_id,
                )
                .await
            }
        });

        for result in join_all(tasks).await {
            let auth_context = result
                .expect("resolution should succeed")
                .expect("auth context should exist");
            assert_eq!(auth_context.user_id, "user-runtime-auth-refresh");
            assert_eq!(auth_context.api_key_id, "key-runtime-auth-refresh");
        }
        assert_eq!(
            auth_repository.key_hash_lookup_count(&hash_api_key(api_key)),
            1,
            "cache refreshes should reuse the existing auth context under concurrent pressure"
        );
        assert_eq!(
            auth_repository.snapshot_lookup_count("key-runtime-auth-refresh"),
            1,
            "only one cached auth context refresh should read the snapshot by user/key id"
        );
    }

    #[tokio::test]
    async fn data_backed_auth_context_allows_provider_id_for_matching_provider_type() {
        let api_key = "sk-test-provider-id";
        let mut snapshot = sample_snapshot("key-2", "user-2");
        snapshot.user_allowed_providers = Some(vec!["provider-openai-1".to_string()]);
        snapshot.api_key_allowed_providers = Some(vec!["provider-openai-1".to_string()]);
        let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some(hash_api_key(api_key)),
            snapshot,
        )]));
        let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider(
                "provider-openai-1",
                "OpenAI Pool 1",
                "openai",
            )],
            Vec::new(),
            Vec::new(),
        ));
        let data = GatewayDataState::with_auth_api_key_reader_for_tests(repository)
            .with_provider_catalog_reader(provider_catalog);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data);

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", api_key.parse().unwrap());

        let auth_context = resolve_data_backed_auth_context(
            &state,
            &headers,
            &uri("/v1/chat/completions"),
            Some("openai:chat"),
        )
        .await
        .expect("resolution should succeed")
        .expect("auth context should exist");

        assert_eq!(auth_context.local_rejection, None);
    }

    #[tokio::test]
    async fn data_backed_auth_context_allows_provider_id_for_matching_endpoint_format() {
        let api_key = "sk-test-provider-endpoint";
        let mut snapshot = sample_snapshot("key-4", "user-4");
        snapshot.user_allowed_providers = Some(vec!["provider-custom-claude".to_string()]);
        snapshot.api_key_allowed_providers = Some(vec!["provider-custom-claude".to_string()]);
        snapshot.user_allowed_api_formats = None;
        snapshot.api_key_allowed_api_formats = None;
        let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some(hash_api_key(api_key)),
            snapshot,
        )]));
        let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider(
                "provider-custom-claude",
                "Custom Claude Gateway",
                "custom",
            )],
            vec![sample_endpoint(
                "endpoint-custom-claude",
                "provider-custom-claude",
                "claude:messages",
            )],
            Vec::new(),
        ));
        let data = GatewayDataState::with_auth_api_key_reader_for_tests(repository)
            .with_provider_catalog_reader(provider_catalog);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data);

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", api_key.parse().unwrap());

        let auth_context = resolve_data_backed_auth_context(
            &state,
            &headers,
            &uri("/v1/messages"),
            Some("claude:messages"),
        )
        .await
        .expect("resolution should succeed")
        .expect("auth context should exist");

        assert_eq!(auth_context.local_rejection, None);
    }

    #[tokio::test]
    async fn data_backed_auth_context_allows_antigravity_v1internal_for_gemini_generate_content_keys(
    ) {
        let api_key = "sk-test-antigravity-v1internal";
        let mut snapshot = sample_snapshot("key-ant-v1internal", "user-ant-v1internal");
        snapshot.user_allowed_providers = Some(vec!["antigravity".to_string()]);
        snapshot.api_key_allowed_providers = Some(vec!["antigravity".to_string()]);
        snapshot.user_allowed_api_formats = Some(vec!["gemini:generate_content".to_string()]);
        snapshot.api_key_allowed_api_formats = Some(vec!["gemini:generate_content".to_string()]);
        let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some(hash_api_key(api_key)),
            snapshot,
        )]));
        let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider(
                "provider-antigravity-1",
                "Antigravity",
                "antigravity",
            )],
            vec![sample_endpoint(
                "endpoint-antigravity-1",
                "provider-antigravity-1",
                "gemini:generate_content",
            )],
            Vec::new(),
        ));
        let data = GatewayDataState::with_auth_api_key_reader_for_tests(repository)
            .with_provider_catalog_reader(provider_catalog);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data);

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", api_key.parse().unwrap());
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer google-oauth-access-token".parse().unwrap(),
        );

        let auth_context = resolve_data_backed_auth_context(
            &state,
            &headers,
            &uri("/v1internal:streamGenerateContent?alt=sse"),
            Some("antigravity:v1internal"),
        )
        .await
        .expect("resolution should succeed")
        .expect("auth context should exist");

        assert_eq!(auth_context.local_rejection, None);
    }

    #[tokio::test]
    async fn data_backed_auth_context_allows_provider_id_for_convertible_endpoint_format() {
        let api_key = "sk-test-provider-convertible-endpoint";
        let mut snapshot = sample_snapshot("key-9", "user-9");
        snapshot.api_key_is_standalone = true;
        snapshot.user_allowed_providers = None;
        snapshot.api_key_allowed_providers = Some(vec!["provider-custom-openai".to_string()]);
        snapshot.user_allowed_api_formats = None;
        snapshot.api_key_allowed_api_formats = None;
        let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some(hash_api_key(api_key)),
            snapshot,
        )]));
        let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider(
                "provider-custom-openai",
                "Custom OpenAI Responses Gateway",
                "custom",
            )],
            vec![sample_endpoint(
                "endpoint-custom-openai-responses",
                "provider-custom-openai",
                "openai:responses",
            )],
            Vec::new(),
        ));
        let data = GatewayDataState::with_auth_api_key_reader_for_tests(repository)
            .with_provider_catalog_reader(provider_catalog);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data);

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", api_key.parse().unwrap());

        let auth_context = resolve_data_backed_auth_context(
            &state,
            &headers,
            &uri("/v1/messages?beta=true"),
            Some("claude:messages"),
        )
        .await
        .expect("resolution should succeed")
        .expect("auth context should exist");

        assert_eq!(auth_context.local_rejection, None);
    }

    #[tokio::test]
    async fn data_backed_auth_context_denies_retired_anthropic_provider_alias_for_claude_route() {
        let api_key = "sk-test-provider-retired-anthropic-alias";
        let mut snapshot = sample_snapshot("key-5", "user-5");
        snapshot.user_allowed_providers = Some(vec!["anthropic".to_string()]);
        snapshot.api_key_allowed_providers = Some(vec!["anthropic".to_string()]);
        snapshot.user_allowed_api_formats = None;
        snapshot.api_key_allowed_api_formats = None;
        let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some(hash_api_key(api_key)),
            snapshot,
        )]));
        let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-claude", "Claude", "custom")],
            Vec::new(),
            Vec::new(),
        ));
        let data = GatewayDataState::with_auth_api_key_reader_for_tests(repository)
            .with_provider_catalog_reader(provider_catalog);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data);

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", api_key.parse().unwrap());

        let auth_context = resolve_data_backed_auth_context(
            &state,
            &headers,
            &uri("/v1/messages"),
            Some("claude:messages"),
        )
        .await
        .expect("resolution should succeed")
        .expect("auth context should exist");

        assert_eq!(
            auth_context.local_rejection,
            Some(GatewayLocalAuthRejection::ProviderNotAllowed {
                provider: "claude".to_string(),
            })
        );
    }

    #[tokio::test]
    async fn data_backed_auth_context_treats_empty_allowed_lists_as_unrestricted() {
        let api_key = "sk-test-empty-restrictions";
        let mut snapshot = sample_snapshot("key-6", "user-6");
        snapshot.api_key_is_standalone = true;
        snapshot.user_allowed_providers = Some(vec!["openai".to_string()]);
        snapshot.user_allowed_api_formats = Some(vec!["openai:chat".to_string()]);
        snapshot.user_allowed_models = Some(vec!["gpt-4.1".to_string()]);
        snapshot.api_key_allowed_providers = Some(Vec::new());
        snapshot.api_key_allowed_api_formats = Some(Vec::new());
        snapshot.api_key_allowed_models = Some(Vec::new());
        let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some(hash_api_key(api_key)),
            snapshot,
        )]));
        let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-claude", "Claude", "custom")],
            vec![sample_endpoint(
                "endpoint-claude",
                "provider-claude",
                "claude:messages",
            )],
            Vec::new(),
        ));
        let data = GatewayDataState::with_auth_api_key_reader_for_tests(repository)
            .with_provider_catalog_reader(provider_catalog);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data);

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", api_key.parse().unwrap());

        let auth_context = resolve_data_backed_auth_context(
            &state,
            &headers,
            &uri("/v1/messages"),
            Some("claude:messages"),
        )
        .await
        .expect("resolution should succeed")
        .expect("auth context should exist");

        assert_eq!(auth_context.local_rejection, None);
    }

    #[tokio::test]
    async fn data_backed_auth_context_denies_provider_type_without_matching_allowed_provider() {
        let api_key = "sk-test-provider-miss";
        let mut snapshot = sample_snapshot("key-3", "user-3");
        snapshot.user_allowed_providers = Some(vec!["provider-claude-1".to_string()]);
        snapshot.api_key_allowed_providers = Some(vec!["provider-claude-1".to_string()]);
        let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some(hash_api_key(api_key)),
            snapshot,
        )]));
        let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider("provider-openai-1", "OpenAI Pool 1", "openai"),
                sample_provider("provider-claude-1", "Claude Pool 1", "claude"),
            ],
            Vec::new(),
            Vec::new(),
        ));
        let data = GatewayDataState::with_auth_api_key_reader_for_tests(repository)
            .with_provider_catalog_reader(provider_catalog);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data);

        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            format!("Bearer {api_key}").parse().unwrap(),
        );

        let auth_context = resolve_data_backed_auth_context(
            &state,
            &headers,
            &uri("/v1/chat/completions"),
            Some("openai:chat"),
        )
        .await
        .expect("resolution should succeed")
        .expect("auth context should exist");

        assert_eq!(
            auth_context.local_rejection,
            Some(GatewayLocalAuthRejection::ProviderNotAllowed {
                provider: "openai".to_string(),
            })
        );
    }
}
