use super::support::{AdminProviderOpsQuotaAlertConfigRequest, AdminProviderOpsSaveConfigRequest};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::{
    canonicalize_provider_ops_base_url, masked_secret_display, open_provider_ops_credential,
    provider_ops_credential_binding_from_config, provider_ops_credential_field_is_secret,
    seal_provider_ops_credential, ProviderOpsCredentialBinding,
    PROVIDER_OPS_PERSISTENT_SECRET_FIELDS, PROVIDER_OPS_TRANSIENT_METADATA_FIELDS,
};
use crate::GatewayError;
use aether_admin::provider::ops as admin_provider_ops_pure;
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogProviderConfigCasUpdate, StoredProviderCatalogEndpoint,
    StoredProviderCatalogProvider,
};
use serde_json::json;

const PROVIDER_OPS_QUOTA_ALERT_DEFAULT_FETCH_INTERVAL_SECS: u64 = 30;
const PROVIDER_OPS_QUOTA_ALERT_MIN_FETCH_INTERVAL_SECS: u64 = 30;
const PROVIDER_OPS_QUOTA_ALERT_MAX_FETCH_INTERVAL_SECS: u64 = 86_400;
const PROVIDER_OPS_CREDENTIAL_MIGRATION_RETRIES: usize = 8;

struct AdminProviderOpsDecodedCredentials {
    values: serde_json::Map<String, serde_json::Value>,
    protected_values: serde_json::Map<String, serde_json::Value>,
    migration_required: bool,
}

pub(crate) struct AdminProviderOpsCredentialSnapshot {
    pub(crate) provider: StoredProviderCatalogProvider,
    pub(crate) credentials: serde_json::Map<String, serde_json::Value>,
    pub(crate) binding: ProviderOpsCredentialBinding,
}

impl std::fmt::Debug for AdminProviderOpsCredentialSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminProviderOpsCredentialSnapshot")
            .field("provider_id", &self.provider.id)
            .field("credentials", &"[REDACTED]")
            .field("binding", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

pub(super) struct AdminProviderOpsMergedCredentialSnapshot {
    pub(super) provider: StoredProviderCatalogProvider,
    pub(super) credentials: serde_json::Map<String, serde_json::Value>,
    pub(super) saved_binding: ProviderOpsCredentialBinding,
    pub(super) reused_saved_secret: bool,
}

pub(super) struct AdminProviderOpsSavedConfigSnapshot {
    pub(super) provider: StoredProviderCatalogProvider,
    pub(super) provider_ops_config: serde_json::Value,
}

pub(super) fn admin_provider_ops_config_object(
    provider: &StoredProviderCatalogProvider,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    admin_provider_ops_pure::admin_provider_ops_config_object(provider)
}

pub(super) fn admin_provider_ops_connector_object(
    provider_ops_config: &serde_json::Map<String, serde_json::Value>,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    admin_provider_ops_pure::admin_provider_ops_connector_object(provider_ops_config)
}

pub(super) fn admin_provider_ops_binding_from_config(
    provider_id: &str,
    provider_ops_config: &serde_json::Map<String, serde_json::Value>,
    effective_base_url: &str,
) -> Result<ProviderOpsCredentialBinding, String> {
    provider_ops_credential_binding_from_config(
        provider_id,
        provider_ops_config,
        effective_base_url,
    )
    .map_err(ToString::to_string)
}

async fn admin_provider_ops_binding_for_provider(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
) -> Result<(ProviderOpsCredentialBinding, bool), GatewayError> {
    let provider_ops_config = admin_provider_ops_config_object(provider)
        .ok_or_else(|| GatewayError::Internal("Provider Ops 配置格式无效".to_string()))?;
    let explicit_base_url = provider_ops_config
        .get("base_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let endpoints = if explicit_base_url.is_some() {
        Vec::new()
    } else {
        state
            .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
            .await?
    };
    let effective_base_url =
        resolve_admin_provider_ops_base_url(provider, &endpoints, Some(provider_ops_config))
            .ok_or_else(|| GatewayError::Internal("Provider Ops 未配置 base_url".to_string()))?;
    let binding = admin_provider_ops_binding_from_config(
        &provider.id,
        provider_ops_config,
        &effective_base_url,
    )
    .map_err(GatewayError::Internal)?;
    let needs_materialized_base_url = explicit_base_url != Some(binding.destination.base_url());
    Ok((binding, needs_materialized_base_url))
}

fn admin_provider_ops_masked_secret(field: &str, plaintext: &str) -> serde_json::Value {
    if plaintext.is_empty() {
        return serde_json::Value::String(String::new());
    }

    let masked = if field == "password" {
        "********".to_string()
    } else {
        masked_secret_display(plaintext, 4, 4, "****")
    };

    serde_json::Value::String(masked)
}

fn admin_provider_ops_masked_credentials(
    credentials: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let mut masked = serde_json::Map::new();
    for (key, value) in credentials {
        if key.starts_with('_') {
            continue;
        }
        if provider_ops_credential_field_is_secret(key) {
            if let Some(ciphertext) = value.as_str().filter(|value| !value.is_empty()) {
                masked.insert(
                    key.clone(),
                    admin_provider_ops_masked_secret(key, ciphertext),
                );
                continue;
            }
        }
        masked.insert(key.clone(), value.clone());
    }
    serde_json::Value::Object(masked)
}

fn admin_provider_ops_is_supported_auth_type(auth_type: &str) -> bool {
    admin_provider_ops_pure::admin_provider_ops_is_supported_auth_type(auth_type)
}

fn admin_provider_ops_decode_credentials(
    state: &AdminAppState<'_>,
    binding: &ProviderOpsCredentialBinding,
    raw_credentials: Option<&serde_json::Value>,
) -> Result<AdminProviderOpsDecodedCredentials, String> {
    let Some(credentials) = raw_credentials.and_then(serde_json::Value::as_object) else {
        return Ok(AdminProviderOpsDecodedCredentials {
            values: serde_json::Map::new(),
            protected_values: serde_json::Map::new(),
            migration_required: false,
        });
    };

    let mut values = serde_json::Map::new();
    let mut protected_values = credentials.clone();
    let mut migration_required = false;
    for (key, value) in credentials {
        if provider_ops_credential_field_is_secret(key) {
            if let Some(stored_value) = value.as_str() {
                if stored_value.is_empty() {
                    values.insert(key.clone(), value.clone());
                    continue;
                }
                let projection =
                    open_provider_ops_credential(state.app(), binding, key, stored_value).map_err(
                        |message| format!("已保存的 Provider Ops 凭据无法解密: {message}"),
                    )?;
                migration_required |= projection.migration_required;
                protected_values
                    .insert(key.clone(), serde_json::Value::String(projection.protected));
                values.insert(key.clone(), serde_json::Value::String(projection.plaintext));
                continue;
            }
        }
        values.insert(key.clone(), value.clone());
    }
    Ok(AdminProviderOpsDecodedCredentials {
        values,
        protected_values,
        migration_required,
    })
}

fn admin_provider_ops_sensitive_placeholder_or_empty(value: Option<&serde_json::Value>) -> bool {
    admin_provider_ops_pure::admin_provider_ops_sensitive_placeholder_or_empty(value)
}

pub(super) async fn admin_provider_ops_merge_credentials(
    state: &AdminAppState<'_>,
    architecture_id: &str,
    provider: &StoredProviderCatalogProvider,
    mut request_credentials: serde_json::Map<String, serde_json::Value>,
) -> Result<AdminProviderOpsMergedCredentialSnapshot, String> {
    let snapshot = admin_provider_ops_credential_snapshot(state, provider)
        .await
        .map_err(|_| "已保存的 Provider Ops 凭据无法解密或迁移".to_string())?;
    let mut saved_credentials = snapshot.credentials;
    let preserve_internal_runtime_fields =
        admin_provider_ops_pure::normalize_architecture_id(architecture_id) == "sub2api";
    if !preserve_internal_runtime_fields {
        saved_credentials.retain(|key, _| !key.starts_with('_'));
    }

    let mut reused_saved_secret = false;
    for field in PROVIDER_OPS_PERSISTENT_SECRET_FIELDS {
        if field.starts_with('_') {
            continue;
        }
        if admin_provider_ops_sensitive_placeholder_or_empty(request_credentials.get(*field))
            && saved_credentials.contains_key(*field)
        {
            if let Some(saved_value) = saved_credentials.get(*field) {
                request_credentials.insert((*field).to_string(), saved_value.clone());
                reused_saved_secret = true;
            }
        }
    }

    if preserve_internal_runtime_fields {
        for (key, value) in saved_credentials {
            if key.starts_with('_') && !request_credentials.contains_key(&key) {
                request_credentials.insert(key, value);
            }
        }
    }

    Ok(AdminProviderOpsMergedCredentialSnapshot {
        provider: snapshot.provider,
        credentials: request_credentials,
        saved_binding: snapshot.binding,
        reused_saved_secret,
    })
}

fn admin_provider_ops_encrypt_credentials(
    state: &AdminAppState<'_>,
    binding: &ProviderOpsCredentialBinding,
    credentials: serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let mut encrypted = serde_json::Map::new();
    for (key, value) in credentials {
        if provider_ops_credential_field_is_secret(&key) {
            if let Some(plaintext) = value.as_str() {
                if plaintext.is_empty() {
                    encrypted.insert(key, value);
                } else {
                    let ciphertext =
                        seal_provider_ops_credential(state.app(), binding, &key, plaintext)
                            .map_err(ToString::to_string)?;
                    encrypted.insert(key, serde_json::Value::String(ciphertext));
                }
                continue;
            }
        }
        encrypted.insert(key, value);
    }
    Ok(encrypted)
}

fn admin_provider_ops_config_with_credentials(
    provider: &StoredProviderCatalogProvider,
    credentials: serde_json::Map<String, serde_json::Value>,
    binding: &ProviderOpsCredentialBinding,
) -> Result<Option<serde_json::Value>, String> {
    let mut provider_config = provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .cloned()
        .ok_or_else(|| "Provider Ops 配置格式无效".to_string())?;
    let mut provider_ops_config = provider_config
        .get("provider_ops")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .ok_or_else(|| "Provider Ops 配置格式无效".to_string())?;
    let mut connector_config = provider_ops_config
        .get("connector")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .ok_or_else(|| "Provider Ops connector 配置格式无效".to_string())?;

    connector_config.insert(
        "credentials".to_string(),
        serde_json::Value::Object(credentials),
    );
    provider_ops_config.insert(
        "connector".to_string(),
        serde_json::Value::Object(connector_config),
    );
    provider_ops_config.insert(
        "base_url".to_string(),
        serde_json::Value::String(binding.destination.base_url().to_string()),
    );
    provider_config.insert(
        "provider_ops".to_string(),
        serde_json::Value::Object(provider_ops_config),
    );
    Ok(Some(serde_json::Value::Object(provider_config)))
}

pub(crate) async fn admin_provider_ops_credential_snapshot(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
) -> Result<AdminProviderOpsCredentialSnapshot, GatewayError> {
    let mut current = provider.clone();
    for _ in 0..PROVIDER_OPS_CREDENTIAL_MIGRATION_RETRIES {
        let (binding, needs_materialized_base_url) =
            admin_provider_ops_binding_for_provider(state, &current).await?;
        let raw_credentials = admin_provider_ops_config_object(&current)
            .and_then(admin_provider_ops_connector_object)
            .and_then(|connector| connector.get("credentials"));
        let decoded = admin_provider_ops_decode_credentials(state, &binding, raw_credentials)
            .map_err(GatewayError::Internal)?;
        if !decoded.migration_required && !needs_materialized_base_url {
            return Ok(AdminProviderOpsCredentialSnapshot {
                provider: current,
                credentials: decoded.values,
                binding,
            });
        }

        let migrated_config = admin_provider_ops_config_with_credentials(
            &current,
            decoded.protected_values,
            &binding,
        )
        .map_err(GatewayError::Internal)?;
        let update = ProviderCatalogProviderConfigCasUpdate {
            provider_id: current.id.clone(),
            expected_config: current.config.clone(),
            config: migrated_config.clone(),
        };
        if state
            .compare_and_swap_provider_catalog_provider_config(&update)
            .await?
        {
            current.config = migrated_config;
            return Ok(AdminProviderOpsCredentialSnapshot {
                provider: current,
                credentials: decoded.values,
                binding,
            });
        }

        current = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&current.id))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| GatewayError::Internal("Provider Ops Provider 不存在".to_string()))?;
    }

    Err(GatewayError::Internal(
        "Provider Ops 凭据迁移未能稳定完成".to_string(),
    ))
}

pub(super) async fn persist_admin_provider_ops_runtime_credentials(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    updated_credentials: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<StoredProviderCatalogProvider>, GatewayError> {
    if updated_credentials.is_empty() || !state.has_provider_catalog_data_writer() {
        return Ok(None);
    }
    for key in updated_credentials.keys() {
        if key != "refresh_token"
            && key != "_cached_access_token"
            && !PROVIDER_OPS_TRANSIENT_METADATA_FIELDS.contains(&key.as_str())
        {
            return Err(GatewayError::Internal(format!(
                "不允许持久化未知的 Provider Ops runtime credential 字段 '{key}'"
            )));
        }
    }

    let mut current = provider.clone();
    for _ in 0..PROVIDER_OPS_CREDENTIAL_MIGRATION_RETRIES {
        let snapshot = admin_provider_ops_credential_snapshot(state, &current).await?;
        let mut decrypted_credentials = snapshot.credentials;
        for (key, value) in updated_credentials {
            decrypted_credentials.insert(key.clone(), value.clone());
        }
        let encrypted_credentials =
            admin_provider_ops_encrypt_credentials(state, &snapshot.binding, decrypted_credentials)
                .map_err(GatewayError::Internal)?;
        let config = admin_provider_ops_config_with_credentials(
            &snapshot.provider,
            encrypted_credentials,
            &snapshot.binding,
        )
        .map_err(GatewayError::Internal)?;
        let update = ProviderCatalogProviderConfigCasUpdate {
            provider_id: snapshot.provider.id.clone(),
            expected_config: snapshot.provider.config.clone(),
            config,
        };
        if state
            .compare_and_swap_provider_catalog_provider_config(&update)
            .await?
        {
            return Ok(state
                .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider.id))
                .await?
                .into_iter()
                .next());
        }
        current = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider.id))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| GatewayError::Internal("Provider Ops Provider 不存在".to_string()))?;
    }
    Err(GatewayError::Internal(
        "Provider Ops runtime credential 并发更新未能稳定完成".to_string(),
    ))
}

pub(super) async fn build_admin_provider_ops_saved_config_value(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    payload: AdminProviderOpsSaveConfigRequest,
) -> Result<AdminProviderOpsSavedConfigSnapshot, String> {
    let architecture_id = payload.architecture_id.trim();
    let normalized_architecture_id =
        admin_provider_ops_pure::normalize_architecture_id(architecture_id);
    if architecture_id.is_empty() || architecture_id != normalized_architecture_id {
        return Err("architecture_id 必须是合法的 Provider Ops 架构".to_string());
    }
    let auth_type = payload.connector.auth_type.trim().to_string();
    if auth_type.is_empty() || !admin_provider_ops_is_supported_auth_type(auth_type.as_str()) {
        return Err("connector.auth_type 必须是合法的认证类型".to_string());
    }

    let merged = admin_provider_ops_merge_credentials(
        state,
        normalized_architecture_id,
        provider,
        payload.connector.credentials,
    )
    .await?;
    let canonical_base_url = payload
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| merged.saved_binding.destination.base_url());
    let canonical_destination =
        canonicalize_provider_ops_base_url(canonical_base_url).map_err(ToString::to_string)?;

    let actions = payload
        .actions
        .into_iter()
        .map(|(action_type, config)| {
            (
                action_type,
                json!({
                    "enabled": config.enabled,
                    "config": config.config,
                }),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>();
    let quota_alert = normalize_admin_provider_ops_quota_alert(payload.quota_alert)?;
    let mut provider_ops_config = json!({
        "architecture_id": normalized_architecture_id,
        "base_url": canonical_destination.base_url(),
        "connector": {
            "auth_type": auth_type,
            "config": payload.connector.config,
            "credentials": {},
        },
        "actions": actions,
        "schedule": payload.schedule,
        "quota_alert": quota_alert,
    });
    let new_binding = admin_provider_ops_binding_from_config(
        &merged.provider.id,
        provider_ops_config
            .as_object()
            .ok_or_else(|| "Provider Ops 配置格式无效".to_string())?,
        canonical_destination.base_url(),
    )?;
    let same_secret_destination = merged.saved_binding.provider_id == new_binding.provider_id
        && merged.saved_binding.architecture_id == new_binding.architecture_id
        && merged.saved_binding.auth_type == new_binding.auth_type
        && merged.saved_binding.destination == new_binding.destination;
    if merged.reused_saved_secret && !same_secret_destination {
        return Err("修改 Provider Ops 架构、认证类型或目标地址时必须重新填写凭据".to_string());
    }
    let mut merged_credentials = merged.credentials;
    if merged.saved_binding != new_binding {
        for field in PROVIDER_OPS_TRANSIENT_METADATA_FIELDS {
            merged_credentials.remove(*field);
        }
        merged_credentials.retain(|field, _| !field.starts_with("_cached_"));
    }
    let encrypted_credentials =
        admin_provider_ops_encrypt_credentials(state, &new_binding, merged_credentials)?;
    provider_ops_config["connector"]["credentials"] =
        serde_json::Value::Object(encrypted_credentials);

    Ok(AdminProviderOpsSavedConfigSnapshot {
        provider: merged.provider,
        provider_ops_config,
    })
}

fn normalize_admin_provider_ops_quota_alert(
    request: Option<AdminProviderOpsQuotaAlertConfigRequest>,
) -> Result<serde_json::Value, String> {
    let Some(request) = request else {
        return Ok(default_admin_provider_ops_quota_alert());
    };
    let threshold_amount = request.threshold_amount.unwrap_or(0.0);
    if threshold_amount < 0.0 {
        return Err("quota_alert.threshold_amount 必须大于等于 0".to_string());
    }
    let fetch_interval_seconds = request
        .fetch_interval_seconds
        .unwrap_or(PROVIDER_OPS_QUOTA_ALERT_DEFAULT_FETCH_INTERVAL_SECS);
    if !(PROVIDER_OPS_QUOTA_ALERT_MIN_FETCH_INTERVAL_SECS
        ..=PROVIDER_OPS_QUOTA_ALERT_MAX_FETCH_INTERVAL_SECS)
        .contains(&fetch_interval_seconds)
    {
        return Err(format!(
            "quota_alert.fetch_interval_seconds 必须在 {} 到 {} 秒之间",
            PROVIDER_OPS_QUOTA_ALERT_MIN_FETCH_INTERVAL_SECS,
            PROVIDER_OPS_QUOTA_ALERT_MAX_FETCH_INTERVAL_SECS
        ));
    }
    Ok(json!({
        "enabled": request.enabled,
        "threshold_amount": threshold_amount,
        "fetch_interval_seconds": fetch_interval_seconds,
    }))
}

fn default_admin_provider_ops_quota_alert() -> serde_json::Value {
    json!({
        "enabled": false,
        "threshold_amount": 0.0,
        "fetch_interval_seconds": PROVIDER_OPS_QUOTA_ALERT_DEFAULT_FETCH_INTERVAL_SECS,
    })
}

pub(super) fn resolve_admin_provider_ops_base_url(
    provider: &StoredProviderCatalogProvider,
    endpoints: &[StoredProviderCatalogEndpoint],
    provider_ops_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<String> {
    admin_provider_ops_pure::resolve_admin_provider_ops_base_url(
        provider,
        endpoints,
        provider_ops_config,
    )
}

pub(super) fn build_admin_provider_ops_status_payload(
    provider_id: &str,
    provider: Option<&StoredProviderCatalogProvider>,
) -> serde_json::Value {
    admin_provider_ops_pure::build_admin_provider_ops_status_payload(provider_id, provider)
}

pub(super) async fn build_admin_provider_ops_config_payload(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider: Option<&StoredProviderCatalogProvider>,
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Result<serde_json::Value, GatewayError> {
    let Some(provider) = provider else {
        return Ok(json!({
            "provider_id": provider_id,
            "is_configured": false,
        }));
    };
    if admin_provider_ops_config_object(provider).is_none() {
        return Ok(json!({
            "provider_id": provider_id,
            "is_configured": false,
        }));
    }
    let snapshot = admin_provider_ops_credential_snapshot(state, provider).await?;
    let provider = &snapshot.provider;
    let Some(provider_ops_config) = admin_provider_ops_config_object(provider) else {
        return Ok(json!({
            "provider_id": provider_id,
            "is_configured": false,
        }));
    };
    let connector = admin_provider_ops_connector_object(provider_ops_config);

    Ok(json!({
        "provider_id": provider_id,
        "is_configured": true,
        "architecture_id": provider_ops_config
            .get("architecture_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("generic_api"),
        "base_url": resolve_admin_provider_ops_base_url(
            provider,
            endpoints,
            Some(provider_ops_config),
        ),
        "connector": {
            "auth_type": connector
                .and_then(|connector| connector.get("auth_type"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("api_key"),
            "config": connector
                .and_then(|connector| connector.get("config"))
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or_else(|| json!({})),
            "credentials": admin_provider_ops_masked_credentials(&snapshot.credentials),
        },
        "quota_alert": provider_ops_config
            .get("quota_alert")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(default_admin_provider_ops_quota_alert),
    }))
}

#[cfg(test)]
mod tests {
    use super::{admin_provider_ops_credential_snapshot, open_provider_ops_credential};
    use crate::data::GatewayDataState;
    use crate::handlers::admin::request::AdminAppState;
    use crate::AppState;
    use aether_crypto::{
        decrypt_python_fernet_ciphertext, encrypt_python_fernet_plaintext,
        looks_like_python_fernet_ciphertext, DEVELOPMENT_ENCRYPTION_KEY,
    };
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogReadRepository, StoredProviderCatalogProvider,
    };
    use serde_json::json;
    use std::sync::Arc;

    const TEST_PROVIDER_ID: &str = "provider-ops-secret-test";
    const TEST_API_KEY: &str = "legacy-provider-ops-api-key";

    fn provider_with_api_key(api_key: &str) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            TEST_PROVIDER_ID.to_string(),
            "Provider Ops Secret Test".to_string(),
            None,
            "openai".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            Some(json!({
                "provider_ops": {
                    "architecture_id": "generic_api",
                    "base_url": "https://provider.example.com",
                    "connector": {
                        "auth_type": "api_key",
                        "config": {},
                        "credentials": {
                            "api_key": api_key,
                            "account_id": "account-1"
                        }
                    },
                    "actions": {},
                    "schedule": {}
                }
            })),
        )
    }

    fn state_with_provider(
        provider: StoredProviderCatalogProvider,
    ) -> (AppState, Arc<InMemoryProviderCatalogReadRepository>) {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            Vec::new(),
            Vec::new(),
        ));
        let state = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository.clone())
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        (state, repository)
    }

    async fn stored_provider(
        repository: &InMemoryProviderCatalogReadRepository,
    ) -> StoredProviderCatalogProvider {
        repository
            .list_providers_by_ids(&[TEST_PROVIDER_ID.to_string()])
            .await
            .expect("provider should read")
            .into_iter()
            .next()
            .expect("provider should exist")
    }

    #[tokio::test]
    async fn legacy_provider_ops_credentials_are_lazily_migrated() {
        let provider = provider_with_api_key(TEST_API_KEY);
        let (state, repository) = state_with_provider(provider.clone());
        let admin_state = AdminAppState::new(&state);

        let snapshot = admin_provider_ops_credential_snapshot(&admin_state, &provider)
            .await
            .expect("legacy Provider Ops credential should migrate");
        assert_eq!(snapshot.credentials["api_key"], TEST_API_KEY);
        assert_eq!(snapshot.credentials["account_id"], "account-1");

        let stored = stored_provider(repository.as_ref()).await;
        let ciphertext = stored
            .config
            .as_ref()
            .and_then(|config| config.pointer("/provider_ops/connector/credentials/api_key"))
            .and_then(serde_json::Value::as_str)
            .expect("stored API key should exist");
        assert_ne!(ciphertext, TEST_API_KEY);
        // New migrations use a binding-aware runtime-secret envelope.  Keep
        // the legacy Fernet assertion below only for the tamper fixture; a
        // migrated value must no longer be treated as an unbound Fernet blob.
        assert!(ciphertext.starts_with("aether-provider-ops-credential-v2:"));
        assert_eq!(
            open_provider_ops_credential(&state, &snapshot.binding, "api_key", ciphertext)
                .expect("migrated Provider Ops API key should decrypt")
                .plaintext,
            TEST_API_KEY
        );
    }

    #[tokio::test]
    async fn tampered_provider_ops_ciphertext_fails_closed() {
        let mut tampered =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, TEST_API_KEY)
                .expect("Provider Ops API key should encrypt");
        tampered.replace_range(tampered.len() - 2.., "AA");
        assert!(looks_like_python_fernet_ciphertext(&tampered));
        let provider = provider_with_api_key(&tampered);
        let (state, repository) = state_with_provider(provider.clone());
        let admin_state = AdminAppState::new(&state);

        let error = admin_provider_ops_credential_snapshot(&admin_state, &provider)
            .await
            .expect_err("tampered Provider Ops ciphertext must not be used as plaintext");
        assert!(format!("{error:?}").contains("无法解密"));

        let stored = stored_provider(repository.as_ref()).await;
        assert_eq!(
            stored
                .config
                .as_ref()
                .and_then(|config| {
                    config.pointer("/provider_ops/connector/credentials/api_key")
                })
                .and_then(serde_json::Value::as_str),
            Some(tampered.as_str())
        );
    }
}
