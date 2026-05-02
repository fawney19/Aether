use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::handlers::admin::shared::decrypt_catalog_secret_with_fallbacks;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use axum::{body::Body, response::Response};
use serde_json::{Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) enum RefreshDispatch<T> {
    Continue(T),
    Respond(Response<Body>),
}

pub(super) struct RefreshRequestContext {
    pub(super) key_id: String,
    pub(super) key: StoredProviderCatalogKey,
    pub(super) provider: StoredProviderCatalogProvider,
    pub(super) provider_type: String,
    pub(super) trace_id: String,
    pub(super) transport: AdminGatewayProviderTransportSnapshot,
}

pub(super) struct RefreshSuccessContext {
    pub(super) provider_type: String,
    pub(super) refreshed_auth_config: Map<String, Value>,
    pub(super) refreshed_expires_at_unix_secs: Option<u64>,
    pub(super) account_state_recheck_attempted: bool,
    pub(super) account_state_recheck_error: Option<String>,
}

pub(super) fn decrypt_auth_config(
    state: &AdminAppState<'_>,
    encrypted_auth_config: &str,
) -> Option<String> {
    state.decrypt_catalog_secret_with_fallbacks(encrypted_auth_config)
}

pub(super) fn parse_auth_config_object(plaintext: &str) -> Map<String, Value> {
    serde_json::from_str::<Value>(plaintext)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

pub(super) fn refreshed_auth_config_object(
    state: &AdminAppState<'_>,
    encrypted_auth_config: Option<&str>,
) -> Map<String, Value> {
    encrypted_auth_config
        .and_then(|ciphertext| decrypt_auth_config(state, ciphertext))
        .map(|plaintext| parse_auth_config_object(&plaintext))
        .unwrap_or_default()
}

pub(super) fn auth_config_has_refresh_token(auth_config: &Map<String, Value>) -> bool {
    auth_config
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
}

pub(super) fn key_is_account_blocked(key: &StoredProviderCatalogKey, block_prefix: &str) -> bool {
    key.oauth_invalid_reason
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| value.starts_with(block_prefix))
}

pub(super) fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
