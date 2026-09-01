use super::support::{
    AdminProviderOpsQuotaAlertConfigRequest, AdminProviderOpsRemoteQuotaConfigRequest,
    AdminProviderOpsSaveConfigRequest, ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS,
};
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_admin::provider::ops as admin_provider_ops_pure;
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogRuntimeCredentialsCas, StoredProviderCatalogEndpoint,
    StoredProviderCatalogProvider,
};
use serde_json::json;

const PROVIDER_OPS_QUOTA_ALERT_DEFAULT_FETCH_INTERVAL_SECS: u64 = 30;
const PROVIDER_OPS_QUOTA_ALERT_MIN_FETCH_INTERVAL_SECS: u64 = 30;
const PROVIDER_OPS_QUOTA_ALERT_MAX_FETCH_INTERVAL_SECS: u64 = 86_400;

pub(super) fn admin_provider_ops_config_object(
    provider: &StoredProviderCatalogProvider,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    admin_provider_ops_pure::admin_provider_ops_config_object(provider)
}

pub(crate) fn admin_provider_ops_remote_quota_worker_eligible(
    provider: &StoredProviderCatalogProvider,
) -> bool {
    let Some(provider_ops) = admin_provider_ops_config_object(provider) else {
        return false;
    };
    provider_ops
        .get("architecture_id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|architecture_id| architecture_id.eq_ignore_ascii_case("sub2api"))
        && admin_provider_ops_pure::parse_sub2api_remote_quota_config(provider_ops)
            .ok()
            .flatten()
            .is_some()
}

pub(super) fn admin_provider_ops_connector_object(
    provider_ops_config: &serde_json::Map<String, serde_json::Value>,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    admin_provider_ops_pure::admin_provider_ops_connector_object(provider_ops_config)
}

fn admin_provider_ops_masked_secret(
    state: &AdminAppState<'_>,
    field: &str,
    ciphertext: &str,
) -> serde_json::Value {
    let plaintext = state
        .decrypt_catalog_secret_with_fallbacks(ciphertext)
        .unwrap_or_else(|| ciphertext.to_string());
    if plaintext.is_empty() {
        return serde_json::Value::String(String::new());
    }

    let masked = if field == "password" {
        "********".to_string()
    } else if plaintext.len() > 12 {
        format!(
            "{}****{}",
            &plaintext[..4],
            &plaintext[plaintext.len().saturating_sub(4)..]
        )
    } else if plaintext.len() > 8 {
        format!(
            "{}****{}",
            &plaintext[..2],
            &plaintext[plaintext.len().saturating_sub(2)..]
        )
    } else {
        "*".repeat(plaintext.len())
    };

    serde_json::Value::String(masked)
}

fn admin_provider_ops_masked_credentials(
    state: &AdminAppState<'_>,
    raw_credentials: Option<&serde_json::Value>,
) -> serde_json::Value {
    let Some(credentials) = raw_credentials.and_then(serde_json::Value::as_object) else {
        return json!({});
    };

    let mut masked = serde_json::Map::new();
    for (key, value) in credentials {
        if key.starts_with('_') {
            continue;
        }
        if ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS.contains(&key.as_str()) {
            if let Some(ciphertext) = value.as_str().filter(|value| !value.is_empty()) {
                masked.insert(
                    key.clone(),
                    admin_provider_ops_masked_secret(state, key, ciphertext),
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

pub(super) fn admin_provider_ops_decrypted_credentials(
    state: &AdminAppState<'_>,
    raw_credentials: Option<&serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let Some(credentials) = raw_credentials.and_then(serde_json::Value::as_object) else {
        return serde_json::Map::new();
    };

    let mut decrypted = serde_json::Map::new();
    for (key, value) in credentials {
        if ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS.contains(&key.as_str()) {
            if let Some(ciphertext) = value.as_str() {
                let plaintext = state
                    .decrypt_catalog_secret_with_fallbacks(ciphertext)
                    .unwrap_or_else(|| ciphertext.to_string());
                decrypted.insert(key.clone(), serde_json::Value::String(plaintext));
                continue;
            }
        }
        decrypted.insert(key.clone(), value.clone());
    }
    decrypted
}

fn admin_provider_ops_sensitive_placeholder_or_empty(value: Option<&serde_json::Value>) -> bool {
    admin_provider_ops_pure::admin_provider_ops_sensitive_placeholder_or_empty(value)
}

pub(super) fn admin_provider_ops_merge_credentials(
    state: &AdminAppState<'_>,
    architecture_id: &str,
    provider: &StoredProviderCatalogProvider,
    mut request_credentials: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut saved_credentials = admin_provider_ops_decrypted_credentials(
        state,
        admin_provider_ops_config_object(provider)
            .and_then(admin_provider_ops_connector_object)
            .and_then(|connector| connector.get("credentials")),
    );
    let preserve_internal_runtime_fields =
        admin_provider_ops_pure::normalize_architecture_id(architecture_id) == "sub2api";
    if !preserve_internal_runtime_fields {
        saved_credentials.retain(|key, _| !key.starts_with('_'));
    }

    for field in ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS {
        if field.starts_with('_') {
            continue;
        }
        if admin_provider_ops_sensitive_placeholder_or_empty(request_credentials.get(*field))
            && saved_credentials.contains_key(*field)
        {
            if let Some(saved_value) = saved_credentials.get(*field) {
                request_credentials.insert((*field).to_string(), saved_value.clone());
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

    request_credentials
}

fn admin_provider_ops_encrypt_credentials(
    state: &AdminAppState<'_>,
    credentials: serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let mut encrypted = serde_json::Map::new();
    for (key, value) in credentials {
        if ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS.contains(&key.as_str()) {
            if let Some(plaintext) = value.as_str() {
                if plaintext.is_empty() {
                    encrypted.insert(key, value);
                } else {
                    let ciphertext = state
                        .encrypt_catalog_secret_with_fallbacks(plaintext)
                        .ok_or_else(|| "gateway 未配置 Provider Ops 加密密钥".to_string())?;
                    encrypted.insert(key, serde_json::Value::String(ciphertext));
                }
                continue;
            }
        }
        encrypted.insert(key, value);
    }
    Ok(encrypted)
}

pub(super) async fn persist_admin_provider_ops_runtime_credentials(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    updated_credentials: &serde_json::Map<String, serde_json::Value>,
) -> Result<bool, GatewayError> {
    if updated_credentials.is_empty() || !state.has_provider_catalog_data_writer() {
        return Ok(false);
    }

    let rotating_credentials = [
        "refresh_token",
        "_cached_access_token",
        "_cached_token_expires_at",
    ]
    .into_iter()
    .filter_map(|key| {
        updated_credentials
            .get(key)
            .cloned()
            .map(|value| (key.to_string(), value))
    })
    .collect();
    let encrypted_credentials = admin_provider_ops_encrypt_credentials(state, rotating_credentials)
        .map_err(GatewayError::Internal)?;
    if encrypted_credentials.is_empty() {
        return Ok(false);
    }

    let update = ProviderCatalogRuntimeCredentialsCas {
        provider_id: provider.id.clone(),
        expected_provider_config: provider.config.clone(),
        expected_provider_website: provider.website.clone(),
        expected_provider_proxy: provider.proxy.clone(),
        encrypted_credentials,
    };
    Ok(state
        .compare_and_patch_provider_ops_runtime_credentials(&update)
        .await?
        .unwrap_or(false))
}

pub(super) fn build_admin_provider_ops_saved_config_value(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    payload: AdminProviderOpsSaveConfigRequest,
) -> Result<serde_json::Value, String> {
    let auth_type = payload.connector.auth_type.trim().to_string();
    if auth_type.is_empty() || !admin_provider_ops_is_supported_auth_type(auth_type.as_str()) {
        return Err("connector.auth_type 必须是合法的认证类型".to_string());
    }

    let merged_credentials = admin_provider_ops_merge_credentials(
        state,
        payload.architecture_id.as_str(),
        provider,
        payload.connector.credentials,
    );
    let encrypted_credentials = admin_provider_ops_encrypt_credentials(state, merged_credentials)?;

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
    let existing_remote_quota = provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("provider_ops"))
        .and_then(serde_json::Value::as_object)
        .and_then(|provider_ops| provider_ops.get("remote_quota"));
    let remote_quota = normalize_admin_provider_ops_remote_quota(
        payload.architecture_id.as_str(),
        payload.remote_quota,
        existing_remote_quota,
    )?;

    Ok(json!({
        "architecture_id": payload.architecture_id,
        "base_url": payload.base_url,
        "connector": {
            "auth_type": auth_type,
            "config": payload.connector.config,
            "credentials": encrypted_credentials,
        },
        "actions": actions,
        "schedule": payload.schedule,
        "quota_alert": quota_alert,
        "remote_quota": remote_quota,
    }))
}

fn normalize_admin_provider_ops_remote_quota(
    architecture_id: &str,
    request: Option<AdminProviderOpsRemoteQuotaConfigRequest>,
    existing: Option<&serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let request = request.or_else(|| {
        existing.and_then(|value| {
            serde_json::from_value::<AdminProviderOpsRemoteQuotaConfigRequest>(value.clone()).ok()
        })
    });
    let Some(request) = request else {
        return Ok(default_admin_provider_ops_remote_quota());
    };
    if request.enabled && architecture_id != "sub2api" {
        return Err("remote_quota 仅支持 Sub2API Provider".to_string());
    }
    let group_id = request.group_id.and_then(|value| match value {
        serde_json::Value::String(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        }
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    });
    if request.enabled && group_id.is_none() {
        return Err("remote_quota.group_id 不能为空".to_string());
    }
    let progress_endpoint = request
        .progress_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("/api/v1/subscriptions/progress");
    admin_provider_ops_pure::validate_sub2api_same_origin_endpoint(progress_endpoint)?;
    Ok(json!({
        "enabled": request.enabled,
        "group_id": group_id,
        "progress_endpoint": progress_endpoint,
    }))
}

fn default_admin_provider_ops_remote_quota() -> serde_json::Value {
    json!({
        "enabled": false,
        "group_id": null,
        "progress_endpoint": "/api/v1/subscriptions/progress",
    })
}

fn safe_admin_provider_ops_remote_quota(
    provider_ops_config: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let remote_quota = provider_ops_config
        .get("remote_quota")
        .and_then(serde_json::Value::as_object);
    json!({
        "enabled": remote_quota
            .and_then(|config| config.get("enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        "group_id": remote_quota
            .and_then(|config| config.get("group_id"))
            .filter(|value| value.is_string() || value.is_number())
            .cloned(),
        "progress_endpoint": remote_quota
            .and_then(|config| config.get("progress_endpoint"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("/api/v1/subscriptions/progress"),
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

pub(super) fn build_admin_provider_ops_config_payload(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider: Option<&StoredProviderCatalogProvider>,
    endpoints: &[StoredProviderCatalogEndpoint],
) -> serde_json::Value {
    let Some(provider) = provider else {
        return json!({
            "provider_id": provider_id,
            "is_configured": false,
        });
    };
    let Some(provider_ops_config) = admin_provider_ops_config_object(provider) else {
        return json!({
            "provider_id": provider_id,
            "is_configured": false,
        });
    };
    let connector = admin_provider_ops_connector_object(provider_ops_config);

    json!({
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
            "credentials": admin_provider_ops_masked_credentials(
                state,
                connector.and_then(|connector| connector.get("credentials")),
            ),
        },
        "quota_alert": provider_ops_config
            .get("quota_alert")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(default_admin_provider_ops_quota_alert),
        "remote_quota": safe_admin_provider_ops_remote_quota(provider_ops_config),
    })
}
