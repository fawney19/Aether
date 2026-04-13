use super::super::configs::is_sensitive_admin_system_config_key;
use crate::api::ai::admin_endpoint_signature_parts;
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::decrypt_catalog_secret_with_fallbacks;
pub(crate) use aether_admin::system::ADMIN_SYSTEM_CONFIG_EXPORT_VERSION;
use aether_admin::system::ADMIN_SYSTEM_PROVIDER_OPS_SENSITIVE_CREDENTIAL_FIELDS;
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

pub(super) fn decrypt_admin_system_export_provider_config(
    state: &AdminAppState<'_>,
    config: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    let mut decrypted = config.cloned()?;
    let Some(credentials) = decrypted
        .get_mut("provider_ops")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|provider_ops| provider_ops.get_mut("connector"))
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|connector| connector.get_mut("credentials"))
        .and_then(serde_json::Value::as_object_mut)
    else {
        return Some(decrypted);
    };

    for field in ADMIN_SYSTEM_PROVIDER_OPS_SENSITIVE_CREDENTIAL_FIELDS {
        let Some(serde_json::Value::String(ciphertext)) = credentials.get(*field).cloned() else {
            continue;
        };
        if let Some(plaintext) = decrypt_admin_system_export_secret(state, &ciphertext) {
            credentials.insert((*field).to_string(), serde_json::Value::String(plaintext));
        }
    }

    Some(decrypted)
}
