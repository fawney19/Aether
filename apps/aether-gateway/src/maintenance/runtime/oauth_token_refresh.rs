use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use serde_json::Value;
use tracing::{info, warn};

use crate::admin_api::provider_oauth_maintenance_endpoint_for_provider;
use crate::provider_key_auth::provider_key_is_oauth_managed;
use crate::{AppState, GatewayError};

use super::system_config_bool;

const OAUTH_TOKEN_REFRESH_LOOKAHEAD_SECS: u64 = 120;
const OAUTH_REFRESH_FAILED_PREFIX: &str = "[REFRESH_FAILED] ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub(crate) struct OAuthTokenRefreshRunSummary {
    pub(crate) scanned: usize,
    pub(crate) eligible: usize,
    pub(crate) refreshed: usize,
    pub(crate) resolved: usize,
    pub(crate) skipped: usize,
    pub(crate) failed: usize,
}

pub(crate) async fn perform_oauth_token_refresh_once(
    state: &AppState,
) -> Result<OAuthTokenRefreshRunSummary, GatewayError> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return Ok(OAuthTokenRefreshRunSummary::default());
    }
    if !system_config_bool(&state.data, "enable_oauth_token_refresh", true)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
    {
        return Ok(OAuthTokenRefreshRunSummary::default());
    }

    // Maintenance must not let one malformed historical proxy credential
    // abort the scan for every provider. Read the rows first, then open each
    // row in isolation so a bad record can be skipped while database errors
    // and missing encryption configuration still fail closed.
    let providers = read_oauth_maintenance_providers(state).await?;
    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    if provider_ids.is_empty() {
        return Ok(OAuthTokenRefreshRunSummary::default());
    }

    let endpoints = read_oauth_maintenance_endpoints(state, &provider_ids).await?;
    // Read the catalog rows without opening/decrypting credentials in bulk.
    // A single legacy/plaintext row must not abort refresh for every healthy
    // key, and this maintenance scan must not trigger the normal lazy v2
    // credential rewrite path. Each candidate is opened in isolation below.
    let keys = state
        .data
        .list_provider_catalog_keys_by_provider_ids(&provider_ids)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let endpoints_by_provider = group_endpoints_by_provider(endpoints);
    let keys_by_provider = group_keys_by_provider(keys);
    let mut summary = OAuthTokenRefreshRunSummary::default();
    let refresh_cutoff_unix_secs =
        now_unix_secs().saturating_add(OAUTH_TOKEN_REFRESH_LOOKAHEAD_SECS);

    for provider in providers {
        let provider_keys = keys_by_provider
            .get(provider.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let provider_endpoints = endpoints_by_provider
            .get(provider.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for key in provider_keys {
            summary.scanned = summary.scanned.saturating_add(1);
            if !oauth_refresh_candidate(&provider, key, refresh_cutoff_unix_secs) {
                summary.skipped = summary.skipped.saturating_add(1);
                continue;
            }
            summary.eligible = summary.eligible.saturating_add(1);

            let Some(endpoint) = provider_oauth_maintenance_endpoint_for_provider(
                &provider.provider_type,
                provider_endpoints,
            ) else {
                summary.skipped = summary.skipped.saturating_add(1);
                continue;
            };

            let transport = match state
                .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
                .await
            {
                Ok(Some(transport)) => transport,
                Ok(None) => {
                    summary.skipped = summary.skipped.saturating_add(1);
                    continue;
                }
                Err(err) if is_nonfatal_legacy_catalog_credential_error(&err) => {
                    // Keep malformed historical credentials untouched. They
                    // are intentionally skipped while other keys continue.
                    summary.skipped = summary.skipped.saturating_add(1);
                    warn!(
                        event_name = "oauth_token_refresh_skipped_invalid_credential",
                        log_type = "ops",
                        worker = "oauth_token_refresh",
                        provider_id = %provider.id,
                        key_id = %key.id,
                        reason = "invalid_stored_credential",
                        "gateway skipped oauth refresh for an invalid stored credential"
                    );
                    continue;
                }
                Err(err) => return Err(err),
            };
            let is_agent_identity =
                crate::provider_transport::is_codex_agent_identity_transport(&transport);
            let needs_agent_task_recovery = is_agent_identity
                && agent_identity_needs_task_recovery(
                    transport.key.decrypted_auth_config.as_deref(),
                    key.oauth_invalid_reason.as_deref(),
                );
            if !needs_agent_task_recovery
                && !auth_config_has_refresh_token(transport.key.decrypted_auth_config.as_deref())
            {
                summary.skipped = summary.skipped.saturating_add(1);
                continue;
            }

            let refresh_result = if needs_agent_task_recovery {
                state
                    .force_local_oauth_refresh_entry(&transport)
                    .await
                    .map(|entry| entry.map(|_| ()))
                    .map_err(|err| GatewayError::Internal(err.to_string()))
            } else {
                state
                    .resolve_local_oauth_request_auth(&transport)
                    .await
                    .map(|auth| auth.map(|_| ()))
            };
            match refresh_result {
                Ok(Some(())) => {
                    summary.resolved = summary.resolved.saturating_add(1);
                    if provider_key_credentials_changed(state, key).await? {
                        summary.refreshed = summary.refreshed.saturating_add(1);
                    }
                }
                Ok(None) => {
                    summary.skipped = summary.skipped.saturating_add(1);
                }
                Err(_) => {
                    summary.failed = summary.failed.saturating_add(1);
                    warn!(
                        event_name = "oauth_token_refresh_failed",
                        log_type = "ops",
                        worker = "oauth_token_refresh",
                        provider_id = %provider.id,
                        key_id = %key.id,
                        "gateway oauth token auto refresh failed"
                    );
                }
            }
        }
    }

    if summary.eligible > 0 || summary.refreshed > 0 || summary.failed > 0 {
        info!(
            event_name = "oauth_token_refresh_completed",
            log_type = "ops",
            worker = "oauth_token_refresh",
            scanned = summary.scanned,
            eligible = summary.eligible,
            refreshed = summary.refreshed,
            resolved = summary.resolved,
            skipped = summary.skipped,
            failed = summary.failed,
            "gateway completed oauth token auto refresh scan"
        );
    }

    Ok(summary)
}

async fn read_oauth_maintenance_providers(
    state: &AppState,
) -> Result<Vec<StoredProviderCatalogProvider>, GatewayError> {
    let stored = state
        .data
        .list_provider_catalog_providers(true)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let mut opened = Vec::with_capacity(stored.len());
    for provider in stored {
        let provider_id = provider.id.clone();
        match state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await
        {
            Ok(mut rows) => {
                if let Some(row) = rows.pop() {
                    opened.push(row);
                }
            }
            Err(error) if is_nonfatal_stored_proxy_error(&error) => {
                warn!(
                    event_name = "oauth_token_refresh_skipped_invalid_provider_proxy",
                    log_type = "ops",
                    worker = "oauth_token_refresh",
                    provider_id = %provider_id,
                    reason = "invalid_stored_proxy_credential",
                    "gateway skipped oauth refresh for a provider with an invalid stored proxy credential"
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(opened)
}

async fn read_oauth_maintenance_endpoints(
    state: &AppState,
    provider_ids: &[String],
) -> Result<Vec<StoredProviderCatalogEndpoint>, GatewayError> {
    let stored = state
        .data
        .list_provider_catalog_endpoints_by_provider_ids(provider_ids)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let mut opened = Vec::with_capacity(stored.len());
    for endpoint in stored {
        let provider_id = endpoint.provider_id.clone();
        let endpoint_id = endpoint.id.clone();
        match state
            .read_provider_catalog_endpoints_by_ids(std::slice::from_ref(&endpoint_id))
            .await
        {
            Ok(mut rows) => {
                if let Some(row) = rows.pop() {
                    opened.push(row);
                }
            }
            Err(error) if is_nonfatal_stored_proxy_error(&error) => {
                warn!(
                    event_name = "oauth_token_refresh_skipped_invalid_endpoint_proxy",
                    log_type = "ops",
                    worker = "oauth_token_refresh",
                    provider_id = %provider_id,
                    endpoint_id = %endpoint_id,
                    reason = "invalid_stored_proxy_credential",
                    "gateway skipped oauth refresh for an endpoint with an invalid stored proxy credential"
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(opened)
}

fn group_endpoints_by_provider(
    endpoints: Vec<StoredProviderCatalogEndpoint>,
) -> BTreeMap<String, Vec<StoredProviderCatalogEndpoint>> {
    let mut grouped = BTreeMap::new();
    for endpoint in endpoints {
        grouped
            .entry(endpoint.provider_id.clone())
            .or_insert_with(Vec::new)
            .push(endpoint);
    }
    grouped
}

fn group_keys_by_provider(
    keys: Vec<StoredProviderCatalogKey>,
) -> BTreeMap<String, Vec<StoredProviderCatalogKey>> {
    let mut grouped = BTreeMap::new();
    for key in keys {
        grouped
            .entry(key.provider_id.clone())
            .or_insert_with(Vec::new)
            .push(key);
    }
    grouped
}

fn oauth_refresh_candidate(
    provider: &StoredProviderCatalogProvider,
    key: &StoredProviderCatalogKey,
    refresh_cutoff_unix_secs: u64,
) -> bool {
    let has_auth_config = key
        .encrypted_auth_config
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let regular_oauth_candidate = key.oauth_invalid_at_unix_secs.is_none()
        && key
            .expires_at_unix_secs
            .is_some_and(|expires_at| expires_at <= refresh_cutoff_unix_secs);
    // The catalog row is encrypted here, so exact Agent Identity validation is
    // deferred until the transport snapshot has decrypted auth_config.
    let possible_agent_candidate = provider.provider_type.trim().eq_ignore_ascii_case("codex")
        && key.auth_type.trim().eq_ignore_ascii_case("oauth")
        && (key.expires_at_unix_secs.is_none()
            || key
                .oauth_invalid_reason
                .as_deref()
                .is_some_and(|reason| reason.contains(OAUTH_REFRESH_FAILED_PREFIX)));
    key.is_active
        && has_auth_config
        && (regular_oauth_candidate || possible_agent_candidate)
        && provider_key_is_oauth_managed(key, provider.provider_type.as_str())
}

fn agent_identity_needs_task_recovery(
    auth_config: Option<&str>,
    oauth_invalid_reason: Option<&str>,
) -> bool {
    if oauth_invalid_reason.is_some_and(|reason| reason.contains(OAUTH_REFRESH_FAILED_PREFIX)) {
        return true;
    }
    auth_config
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .is_some_and(|config| {
            crate::provider_transport::is_codex_agent_identity_auth_config_value(&config)
                && !crate::provider_transport::codex_agent_identity_auth_config_has_task_id(&config)
        })
}

async fn provider_key_credentials_changed(
    state: &AppState,
    before: &StoredProviderCatalogKey,
) -> Result<bool, GatewayError> {
    let Some(after) = state
        .data
        .list_provider_catalog_keys_by_ids(std::slice::from_ref(&before.id))
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
        .into_iter()
        .next()
    else {
        return Ok(false);
    };
    Ok(after.encrypted_api_key != before.encrypted_api_key
        || after.encrypted_auth_config != before.encrypted_auth_config
        || after.expires_at_unix_secs != before.expires_at_unix_secs)
}

fn auth_config_has_refresh_token(auth_config: Option<&str>) -> bool {
    let Some(auth_config) = auth_config.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(auth_config) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    ["refresh_token", "refreshToken"].iter().any(|field| {
        object
            .get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
    })
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// Credential decoding errors are expected for rows written by older
/// versions of the service. They are non-fatal for a best-effort maintenance
/// scan, but normal request/admin paths still fail closed on the same error.
fn is_nonfatal_legacy_catalog_credential_error(error: &GatewayError) -> bool {
    is_nonfatal_legacy_provider_key_credential_error(error) || is_nonfatal_stored_proxy_error(error)
}

fn is_nonfatal_legacy_provider_key_credential_error(error: &GatewayError) -> bool {
    let GatewayError::Internal(message) = error else {
        return false;
    };
    let message = message.to_ascii_lowercase();
    // Missing encryption configuration is an operational failure and must
    // remain fail-closed.  Only errors that identify a stored field or a
    // malformed legacy ciphertext are safe to isolate to one key.
    if message.contains("encryption key is not configured") {
        return false;
    }
    message.contains("provider_api_keys.api_key")
        || message.contains("provider_api_keys.auth_config")
        || message.contains("provider_api_keys.api_formats")
        || message.contains("provider_api_keys.allowed_models")
        || message.contains("legacy provider catalog credential")
        || message.contains("stored provider catalog credential is empty")
        || message.contains("aether secret envelope has the wrong record binding")
        || message.contains("provider catalog credential is not an authenticated ciphertext")
        || message.contains("provider catalog credential contains reserved framing")
        || message.contains("provider catalog credential authentication failed")
        || message.contains("provider catalog credential envelope")
        || message
            .contains("provider catalog key provider binding changed during credential migration")
}

/// Stored provider/endpoint/key proxy secrets are opened independently by the
/// maintenance scan. A malformed historical row is safe to isolate, while
/// encryption/configuration failures remain fatal so operators are alerted.
fn is_nonfatal_stored_proxy_error(error: &GatewayError) -> bool {
    let GatewayError::Internal(message) = error else {
        return false;
    };
    let message = message.to_ascii_lowercase();
    message.contains("stored provider proxy credentials cannot be decrypted")
        || message.contains("stored endpoint proxy credentials cannot be decrypted")
        || message.contains("stored key proxy credentials cannot be decrypted")
        || message.contains("stored provider proxy changed during credential migration")
        || message.contains("stored endpoint proxy changed during credential migration")
        || message.contains("stored key changed during credential migration")
        || message.contains("stored provider proxy credential migration did not stabilize")
        || message.contains("stored endpoint proxy credential migration did not stabilize")
        || message.contains("stored key proxy credential migration did not stabilize")
}

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };

    use super::{
        agent_identity_needs_task_recovery, auth_config_has_refresh_token,
        is_nonfatal_legacy_catalog_credential_error, oauth_refresh_candidate,
    };
    use crate::GatewayError;

    #[test]
    fn legacy_antigravity_refresh_token_is_refreshable() {
        assert!(auth_config_has_refresh_token(Some(
            r#"{"refreshToken":"legacy-refresh-token"}"#,
        )));
    }

    #[test]
    fn expiring_antigravity_oauth_key_is_refresh_candidate() {
        let provider = StoredProviderCatalogProvider::new(
            "provider-antigravity".to_string(),
            "Antigravity".to_string(),
            None,
            "antigravity".to_string(),
        )
        .expect("provider should build");
        let mut key = StoredProviderCatalogKey::new(
            "key-antigravity".to_string(),
            provider.id.clone(),
            "Antigravity OAuth".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.encrypted_auth_config = Some("encrypted-auth-config".to_string());
        key.expires_at_unix_secs = Some(120);

        assert!(oauth_refresh_candidate(&provider, &key, 120));
    }

    #[test]
    fn pending_agent_identity_without_task_is_recoverable() {
        let config = serde_json::json!({
            "auth_mode": "agentIdentity",
            "agent_runtime_id": "runtime-1",
            "agent_private_key": "private-key-present",
        });
        assert!(agent_identity_needs_task_recovery(
            Some(&config.to_string()),
            None,
        ));
    }

    #[test]
    fn refresh_failure_marker_forces_agent_task_recovery() {
        assert!(agent_identity_needs_task_recovery(
            Some("{}"),
            Some("[REFRESH_FAILED] temporary"),
        ));
    }

    #[test]
    fn only_stored_catalog_credential_errors_are_non_fatal() {
        assert!(is_nonfatal_legacy_catalog_credential_error(
            &GatewayError::Internal(
                "provider catalog credential is not an authenticated ciphertext".to_string(),
            )
        ));
        assert!(is_nonfatal_legacy_catalog_credential_error(
            &GatewayError::Internal(
                "provider_api_keys.auth_config has an invalid provider catalog credential envelope"
                    .to_string(),
            )
        ));
        assert!(!is_nonfatal_legacy_catalog_credential_error(
            &GatewayError::Internal("postgres error: connection refused".to_string(),)
        ));
        assert!(!is_nonfatal_legacy_catalog_credential_error(
            &GatewayError::Internal(
                "provider catalog credential encryption key is not configured".to_string(),
            )
        ));
        for scope in ["provider", "endpoint", "key"] {
            assert!(is_nonfatal_legacy_catalog_credential_error(
                &GatewayError::Internal(format!(
                    "stored {scope} proxy credentials cannot be decrypted"
                ))
            ));
        }
        assert!(is_nonfatal_legacy_catalog_credential_error(
            &GatewayError::Internal("stored provider catalog credential is empty".to_string())
        ));
        assert!(is_nonfatal_legacy_catalog_credential_error(
            &GatewayError::Internal(
                "Aether secret envelope has the wrong record binding".to_string()
            )
        ));
        for field in ["api_formats", "allowed_models"] {
            assert!(is_nonfatal_legacy_catalog_credential_error(
                &GatewayError::Internal(format!(
                    "provider_api_keys.{field} contains a malformed value"
                ))
            ));
        }
        assert!(!is_nonfatal_legacy_catalog_credential_error(
            &GatewayError::Internal(
                "endpoint proxy credential encryption is unavailable".to_string(),
            )
        ));
    }
}
