use super::super::configs::is_sensitive_admin_system_config_key;
use crate::api::ai::admin_endpoint_signature_parts;
use crate::handlers::admin::request::{AdminAppState, SystemExportMode};
use crate::handlers::shared::{
    decrypt_catalog_secret_with_fallbacks, PROVIDER_OPS_PERSISTENT_SECRET_FIELDS,
    PROVIDER_OPS_TRANSIENT_METADATA_FIELDS, PROVIDER_OPS_TRANSIENT_SECRET_FIELDS,
};
use aether_admin::provider::redaction::{
    admin_secret_safe_body_rules, admin_secret_safe_header_rules, admin_secret_safe_json,
    admin_secret_safe_proxy, admin_secret_safe_url,
};
pub(crate) use aether_admin::system::ADMIN_SYSTEM_CONFIG_EXPORT_VERSION;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint;

pub(crate) const ADMIN_SYSTEM_EXPORT_PAGE_LIMIT: usize = 10_000;

pub(crate) fn decrypt_admin_system_export_secret(
    state: &AdminAppState<'_>,
    ciphertext: &str,
) -> Option<String> {
    decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)
}

pub(super) fn normalize_admin_system_export_api_formats(
    raw_formats: Option<&serde_json::Value>,
) -> Vec<String> {
    aether_admin::system::normalize_admin_system_export_api_formats(raw_formats, |value| {
        admin_endpoint_signature_parts(value).map(|(signature, _, _)| signature.to_string())
    })
}

pub(super) fn resolve_admin_system_export_key_api_formats(
    raw_formats: Option<&serde_json::Value>,
    provider_endpoint_formats: &[String],
) -> Vec<String> {
    aether_admin::system::resolve_admin_system_export_key_api_formats(
        raw_formats,
        provider_endpoint_formats,
        |value| {
            admin_endpoint_signature_parts(value).map(|(signature, _, _)| signature.to_string())
        },
    )
}

pub(super) fn collect_admin_system_export_provider_endpoint_formats(
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Vec<String> {
    aether_admin::system::collect_admin_system_export_provider_endpoint_formats(
        endpoints,
        |value| {
            admin_endpoint_signature_parts(value).map(|(signature, _, _)| signature.to_string())
        },
    )
}

pub(super) fn project_admin_system_export_provider_config(
    _state: &AdminAppState<'_>,
    mode: SystemExportMode,
    config: Option<&serde_json::Value>,
    provider_ops_plaintext_credentials: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<Option<serde_json::Value>, crate::GatewayError> {
    if !mode.credentials_are_exported() {
        return Ok(project_admin_system_export_json(mode, config));
    }
    let Some(mut decrypted) = config.cloned() else {
        return Ok(None);
    };
    let Some(credentials) = decrypted
        .get_mut("provider_ops")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|provider_ops| provider_ops.get_mut("connector"))
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|connector| connector.get_mut("credentials"))
        .and_then(serde_json::Value::as_object_mut)
    else {
        return Ok(Some(decrypted));
    };

    for field in PROVIDER_OPS_TRANSIENT_SECRET_FIELDS
        .iter()
        .chain(PROVIDER_OPS_TRANSIENT_METADATA_FIELDS)
    {
        credentials.remove(*field);
    }
    let plaintext_credentials = provider_ops_plaintext_credentials.ok_or_else(|| {
        crate::GatewayError::Internal(
            "RecoveryBackup 缺少已认证的 Provider Ops 凭据快照".to_string(),
        )
    })?;
    for field in PROVIDER_OPS_PERSISTENT_SECRET_FIELDS {
        if let Some(plaintext) = plaintext_credentials.get(*field) {
            credentials.insert((*field).to_string(), plaintext.clone());
        }
    }

    Ok(Some(decrypted))
}

pub(crate) fn project_admin_system_export_json(
    mode: SystemExportMode,
    value: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    value.map(|value| {
        if mode.credentials_are_exported() {
            value.clone()
        } else {
            admin_secret_safe_json(Some(value))
        }
    })
}

pub(super) fn project_admin_system_export_header_rules(
    mode: SystemExportMode,
    value: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    value.map(|value| {
        if mode.credentials_are_exported() {
            value.clone()
        } else {
            admin_secret_safe_header_rules(Some(value))
        }
    })
}

pub(super) fn project_admin_system_export_body_rules(
    mode: SystemExportMode,
    value: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    value.map(|value| {
        if mode.credentials_are_exported() {
            value.clone()
        } else {
            admin_secret_safe_body_rules(Some(value))
        }
    })
}

pub(super) fn project_admin_system_export_proxy(
    mode: SystemExportMode,
    value: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    value.map(|value| {
        if mode.credentials_are_exported() {
            value.clone()
        } else {
            admin_secret_safe_proxy(Some(value))
        }
    })
}

pub(crate) fn project_admin_system_export_optional_url(
    mode: SystemExportMode,
    value: Option<&str>,
) -> Option<String> {
    value.and_then(|value| {
        if mode.credentials_are_exported() {
            Some(value.to_string())
        } else {
            admin_secret_safe_url(Some(value))
                .as_str()
                .map(ToOwned::to_owned)
        }
    })
}

pub(crate) fn project_admin_system_export_url(mode: SystemExportMode, value: &str) -> String {
    project_admin_system_export_optional_url(mode, Some(value)).unwrap_or_default()
}
