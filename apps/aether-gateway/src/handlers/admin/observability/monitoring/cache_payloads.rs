use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::{masked_secret_display, open_auth_api_key_secret};
use crate::provider_key_auth::{
    provider_key_auth_config_is_agent_identity, provider_key_auth_config_uses_header_authorization,
};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

pub(super) fn admin_monitoring_masked_user_api_key_prefix(
    state: &AdminAppState<'_>,
    record: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
) -> Option<String> {
    let projection = open_auth_api_key_secret(state.app(), record).ok()?;
    Some(masked_secret_display(&projection.plaintext, 10, 4, "..."))
}

pub(super) fn admin_monitoring_masked_provider_key_prefix(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> Option<String> {
    match key.auth_type.trim() {
        "service_account" | "vertex_ai" => Some("[Service Account]".to_string()),
        "oauth" => {
            let auth_config = state.parse_catalog_auth_config_json(key);
            if provider_key_auth_config_is_agent_identity(provider_type, auth_config.as_ref()) {
                Some("[Agent Identity]".to_string())
            } else if provider_key_auth_config_uses_header_authorization(auth_config.as_ref()) {
                Some("[OAuth Header]".to_string())
            } else {
                Some("[OAuth Token]".to_string())
            }
        }
        _ => {
            let full_key = state
                .app()
                .decrypt_provider_catalog_key_api_key(key)
                .ok()
                .flatten()?;
            Some(masked_secret_display(&full_key, 8, 4, "***"))
        }
    }
}

pub(super) fn admin_monitoring_cache_affinity_sort_value(value: Option<&serde_json::Value>) -> f64 {
    let Some(value) = value else {
        return 0.0;
    };
    if let Some(number) = value.as_f64() {
        return number;
    }
    if let Some(number) = value.as_i64() {
        return number as f64;
    }
    if let Some(number) = value.as_u64() {
        return number as f64;
    }
    if let Some(text) = value.as_str() {
        if let Ok(number) = text.parse::<f64>() {
            return number;
        }
        if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(text) {
            return parsed.timestamp() as f64;
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::{
        admin_monitoring_masked_provider_key_prefix, admin_monitoring_masked_user_api_key_prefix,
    };
    use crate::handlers::admin::request::AdminAppState;
    use crate::AppState;
    use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
    use sha2::{Digest, Sha256};

    #[test]
    fn monitoring_labels_agent_identity_instead_of_oauth_token() {
        let app = AppState::new().expect("gateway should build");
        let state = AdminAppState::new(&app);
        let encrypted_placeholder =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "__placeholder__")
                .expect("placeholder should encrypt");
        let encrypted_auth_config = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"auth_mode":"agentIdentity","agent_runtime_id":"runtime-1","agent_private_key":"base64-private-key","task_id":"task-1"}"#,
        )
        .expect("auth config should encrypt");
        let key = StoredProviderCatalogKey::new(
            "key-agent".to_string(),
            "provider-codex".to_string(),
            "agent".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            None,
            encrypted_placeholder,
            Some(encrypted_auth_config),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("transport should build");

        assert_eq!(
            admin_monitoring_masked_provider_key_prefix(&state, &key, "codex").as_deref(),
            Some("[Agent Identity]")
        );
    }

    #[test]
    fn monitoring_never_exposes_complete_short_credentials() {
        let app = AppState::new().expect("gateway should build");
        let state = AdminAppState::new(&app);
        let plaintext = "short-key";
        let ciphertext = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, plaintext)
            .expect("secret should encrypt");
        let mut hasher = Sha256::new();
        hasher.update(plaintext.as_bytes());
        let record = aether_data::repository::auth::StoredAuthApiKeyExportRecord::new(
            "owner-1".to_string(),
            "key-1".to_string(),
            format!("{:x}", hasher.finalize()),
            Some(ciphertext),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            false,
            0,
            0,
            0.0,
            false,
        )
        .expect("API-key record should build");

        let masked = admin_monitoring_masked_user_api_key_prefix(&state, &record)
            .expect("secret should decrypt");
        assert_ne!(masked, plaintext);
        assert!(!masked.contains(plaintext));
    }
}
