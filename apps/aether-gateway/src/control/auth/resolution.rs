use std::time::Duration;

use axum::http::Uri;
use base64::Engine as _;
use hmac::Mac;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info};

use crate::wallet_runtime::{local_rejection_from_wallet_access, resolve_wallet_auth_gate};
use crate::{AppState, GatewayError};

use super::super::GatewayControlDecision;
use super::credentials::{
    build_auth_context_cache_key, current_unix_secs, extract_request_credentials,
    extract_trusted_admin_headers,
};
use super::gate::GatewayLocalAuthRejection;
use super::principal::derive_principal_candidate;
use super::types::{GatewayPrincipalCandidate, GatewayTrustedAuthHeaders};
use crate::headers::header_value_str;

const AUTH_CONTEXT_CACHE_TTL: Duration = Duration::from_secs(60);
const AUTH_CONTEXT_CACHE_MAX_ENTRIES: usize = 256;

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
    pub(crate) local_rejection: Option<GatewayLocalAuthRejection>,
    #[serde(skip)]
    pub(crate) allowed_models: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayAdminPrincipalContext {
    pub(crate) user_id: String,
    pub(crate) user_role: String,
    pub(crate) session_id: Option<String>,
    pub(crate) management_token_id: Option<String>,
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

    if let Some(auth_context) = resolve_data_backed_auth_context(
        state,
        headers,
        uri,
        decision.auth_endpoint_signature.as_deref(),
    )
    .await?
    {
        log_auth_context_resolution(trace_id, &decision, &auth_context);
        decision.local_auth_rejection = auth_context.local_rejection.clone();
        if !auth_context.user_id.is_empty() && !auth_context.api_key_id.is_empty() {
            if let Some(cache_key) = decision
                .auth_endpoint_signature
                .as_deref()
                .and_then(|signature| build_auth_context_cache_key(headers, uri, signature))
            {
                put_cached_auth_context(state, cache_key, auth_context.clone());
            }
            decision.auth_context = Some(auth_context);
        }
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
        .is_some_and(|role| !role.eq_ignore_ascii_case("admin"))
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
    if !user.is_active || user.is_deleted || !user.role.eq_ignore_ascii_case("admin") {
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
        user_role: "admin".to_string(),
        session_id: Some(session.id),
        management_token_id: None,
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
        return Ok(Some(auth_context));
    }

    let Some(auth_endpoint_signature) = decision.auth_endpoint_signature.as_deref() else {
        return Ok(None);
    };
    let Some(cache_key) = build_auth_context_cache_key(headers, uri, auth_endpoint_signature)
    else {
        return Ok(None);
    };

    if let Some(auth_context) = get_cached_auth_context(state, &cache_key) {
        return Ok(Some(auth_context));
    }

    if let Some(auth_context) =
        resolve_data_backed_auth_context(state, headers, uri, Some(auth_endpoint_signature)).await?
    {
        if auth_context.user_id.is_empty() || auth_context.api_key_id.is_empty() {
            return Ok(None);
        }
        put_cached_auth_context(state, cache_key, auth_context.clone());
        return Ok(Some(auth_context));
    }

    Ok(None)
}

fn put_cached_auth_context(
    state: &AppState,
    cache_key: String,
    auth_context: GatewayControlAuthContext,
) {
    state.auth_context_cache.insert(
        cache_key,
        auth_context,
        AUTH_CONTEXT_CACHE_TTL,
        AUTH_CONTEXT_CACHE_MAX_ENTRIES,
    );
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
                .data
                .read_auth_api_key_snapshot_by_key_hash(&key_hash, now_unix_secs)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
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
                    local_rejection: Some(GatewayLocalAuthRejection::InvalidApiKey),
                    allowed_models: None,
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
        Some(
            GatewayPrincipalCandidate::DeferredBearerToken { .. }
            | GatewayPrincipalCandidate::DeferredCookieHeader { .. },
        ) => Ok(None),
        None => Ok(None),
    }
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
            local_rejection: Some(GatewayLocalAuthRejection::InvalidApiKey),
            allowed_models: None,
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
    let access_allowed = header_access_allowed
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
    let requested_provider_allowed =
        auth_snapshot_allows_requested_provider(state, &snapshot, requested_provider).await;
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
    } else if snapshot
        .effective_allowed_api_formats()
        .is_some_and(|allowed| !contains_api_format_or_alias(allowed, auth_endpoint_signature))
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
        access_allowed,
        user_rate_limit: snapshot.user_rate_limit,
        api_key_rate_limit: snapshot.api_key_rate_limit,
        api_key_is_standalone: snapshot.api_key_is_standalone,
        local_rejection,
        allowed_models,
    }
}

fn contains_api_format_or_alias(items: &[String], target: &str) -> bool {
    items.iter().any(|item| api_format_matches(item, target))
}

fn normalize_api_format_alias(value: &str) -> String {
    crate::ai_pipeline::normalize_legacy_openai_format_alias(value)
}

fn api_format_matches(left: &str, right: &str) -> bool {
    normalize_api_format_alias(left) == normalize_api_format_alias(right)
}

async fn auth_snapshot_allows_requested_provider(
    state: &AppState,
    snapshot: &crate::data::auth::GatewayAuthApiKeySnapshot,
    requested_provider: &str,
) -> bool {
    let Some(allowed_providers) = snapshot.effective_allowed_providers() else {
        return true;
    };
    let requested_provider = requested_provider.trim();
    if requested_provider.is_empty() {
        return true;
    }
    if allowed_providers.is_empty() {
        return false;
    }
    if allowed_providers
        .iter()
        .any(|value| value.trim().eq_ignore_ascii_case(requested_provider))
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

    providers.into_iter().any(|provider| {
        provider
            .provider_type
            .trim()
            .eq_ignore_ascii_case(requested_provider)
            && allowed_providers.iter().any(|value| {
                aether_scheduler_core::provider_matches_allowed_value(
                    value,
                    &provider.id,
                    &provider.name,
                    &provider.provider_type,
                )
            })
    })
}

fn get_cached_auth_context(state: &AppState, cache_key: &str) -> Option<GatewayControlAuthContext> {
    state
        .auth_context_cache
        .get_fresh(cache_key, AUTH_CONTEXT_CACHE_TTL)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_data::repository::auth::{
        InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
    };
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
    use axum::http::{HeaderMap, Uri};

    use super::{resolve_data_backed_auth_context, GatewayLocalAuthRejection};
    use crate::control::auth::credentials::hash_api_key;
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
