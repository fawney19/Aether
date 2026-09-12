use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyCredentialsCasUpdate, StoredProviderCatalogKey,
};

use super::AppState;
use crate::handlers::shared::{
    open_provider_catalog_credential, seal_provider_catalog_credential,
    ProviderCatalogCredentialField, ProviderCatalogCredentialProjection,
};
use crate::GatewayError;

impl AppState {
    pub(super) fn protect_provider_catalog_key_credentials(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, GatewayError> {
        let mut protected = key.clone();
        protected.encrypted_api_key = self
            .project_provider_catalog_key_credential(
                key,
                ProviderCatalogCredentialField::ApiKey,
                key.encrypted_api_key.as_deref(),
            )?
            .map(|projection| projection.protected);
        protected.encrypted_auth_config = self
            .project_provider_catalog_key_credential(
                key,
                ProviderCatalogCredentialField::AuthConfig,
                key.encrypted_auth_config.as_deref(),
            )?
            .map(|projection| projection.protected);
        Ok(protected)
    }

    pub(super) async fn open_provider_catalog_key_credentials_once(
        &self,
        key: &mut StoredProviderCatalogKey,
    ) -> Result<bool, GatewayError> {
        let observed_api_key = key.encrypted_api_key.clone();
        let observed_auth_config = key.encrypted_auth_config.clone();
        let api_key = self.project_provider_catalog_key_credential(
            key,
            ProviderCatalogCredentialField::ApiKey,
            observed_api_key.as_deref(),
        )?;
        let auth_config = self.project_provider_catalog_key_credential(
            key,
            ProviderCatalogCredentialField::AuthConfig,
            observed_auth_config.as_deref(),
        )?;
        let migration_required = api_key
            .as_ref()
            .is_some_and(|projection| projection.migration_required)
            || auth_config
                .as_ref()
                .is_some_and(|projection| projection.migration_required);
        if !migration_required {
            return Ok(true);
        }
        if !self.has_provider_catalog_data_writer() {
            return Err(provider_catalog_credential_error(
                "stored provider catalog credentials require migration but the catalog writer is unavailable",
            ));
        }

        let protected_api_key = api_key.map(|projection| projection.protected);
        let protected_auth_config = auth_config.map(|projection| projection.protected);
        let updated = self
            .data
            .compare_and_swap_provider_catalog_key_credentials(
                &ProviderCatalogKeyCredentialsCasUpdate {
                    key_id: key.id.clone(),
                    expected_provider_id: key.provider_id.clone(),
                    expected_encrypted_api_key: observed_api_key,
                    expected_encrypted_auth_config: observed_auth_config,
                    encrypted_api_key: protected_api_key.clone(),
                    encrypted_auth_config: protected_auth_config.clone(),
                },
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if updated {
            key.encrypted_api_key = protected_api_key;
            key.encrypted_auth_config = protected_auth_config;
        }
        Ok(updated)
    }

    pub(crate) fn decrypt_provider_catalog_key_api_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<Option<String>, GatewayError> {
        self.project_provider_catalog_key_credential(
            key,
            ProviderCatalogCredentialField::ApiKey,
            key.encrypted_api_key.as_deref(),
        )
        .map(|projection| projection.map(|projection| projection.plaintext))
    }

    pub(crate) fn decrypt_provider_catalog_key_auth_config(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<Option<String>, GatewayError> {
        self.project_provider_catalog_key_credential(
            key,
            ProviderCatalogCredentialField::AuthConfig,
            key.encrypted_auth_config.as_deref(),
        )
        .map(|projection| projection.map(|projection| projection.plaintext))
    }

    pub(crate) fn seal_provider_catalog_key_api_key(
        &self,
        provider_id: &str,
        key_id: &str,
        plaintext: &str,
    ) -> Result<String, GatewayError> {
        seal_provider_catalog_credential(
            self,
            provider_id,
            key_id,
            ProviderCatalogCredentialField::ApiKey,
            plaintext,
        )
        .map_err(provider_catalog_credential_error)
    }

    pub(crate) fn seal_provider_catalog_key_auth_config(
        &self,
        provider_id: &str,
        key_id: &str,
        plaintext: &str,
    ) -> Result<String, GatewayError> {
        seal_provider_catalog_credential(
            self,
            provider_id,
            key_id,
            ProviderCatalogCredentialField::AuthConfig,
            plaintext,
        )
        .map_err(provider_catalog_credential_error)
    }

    pub(super) fn validate_protected_provider_catalog_key_api_key(
        &self,
        provider_id: &str,
        key_id: &str,
        stored: &str,
    ) -> Result<(), GatewayError> {
        self.validate_protected_provider_catalog_key_credential(
            provider_id,
            key_id,
            ProviderCatalogCredentialField::ApiKey,
            stored,
        )
    }

    pub(super) fn validate_protected_provider_catalog_key_auth_config(
        &self,
        provider_id: &str,
        key_id: &str,
        stored: &str,
    ) -> Result<(), GatewayError> {
        self.validate_protected_provider_catalog_key_credential(
            provider_id,
            key_id,
            ProviderCatalogCredentialField::AuthConfig,
            stored,
        )
    }

    fn validate_protected_provider_catalog_key_credential(
        &self,
        provider_id: &str,
        key_id: &str,
        field: ProviderCatalogCredentialField,
        stored: &str,
    ) -> Result<(), GatewayError> {
        let projection = open_provider_catalog_credential(self, provider_id, key_id, field, stored)
            .map_err(provider_catalog_credential_error)?;
        if projection.migration_required || projection.protected != stored {
            return Err(provider_catalog_credential_error(
                "provider catalog credential write requires a bound v2 ciphertext",
            ));
        }
        Ok(())
    }

    fn project_provider_catalog_key_credential(
        &self,
        key: &StoredProviderCatalogKey,
        field: ProviderCatalogCredentialField,
        stored: Option<&str>,
    ) -> Result<Option<ProviderCatalogCredentialProjection>, GatewayError> {
        let Some(stored) = stored else {
            return Ok(None);
        };
        if stored.is_empty() {
            return Err(provider_catalog_credential_error(
                "stored provider catalog credential is empty",
            ));
        }
        open_provider_catalog_credential(self, &key.provider_id, &key.id, field, stored)
            .map(Some)
            .map_err(provider_catalog_credential_error)
    }
}

fn provider_catalog_credential_error(message: &'static str) -> GatewayError {
    GatewayError::Internal(message.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogKeyCredentialsCasUpdate, ProviderCatalogReadRepository,
        ProviderCatalogWriteRepository, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };

    use crate::{data::GatewayDataState, AppState};

    fn sample_provider(id: &str) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            id.to_string(),
            format!("Provider {id}"),
            Some("https://example.test".to_string()),
            "openai".to_string(),
        )
        .expect("provider should build")
    }

    fn sample_key(
        id: &str,
        provider_id: &str,
        encrypted_api_key: Option<String>,
        encrypted_auth_config: Option<String>,
    ) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            id.to_string(),
            provider_id.to_string(),
            format!("Key {id}"),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            None,
            encrypted_api_key,
            encrypted_auth_config,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    fn state_with_repository(repository: Arc<InMemoryProviderCatalogReadRepository>) -> AppState {
        AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    #[tokio::test]
    async fn app_state_reads_redacted_key_summaries_without_opening_credentials() {
        for (api_key, auth_config) in [
            (Some("summary"), None),
            (None, Some("{}")),
            (Some("summary"), Some("{}")),
        ] {
            let health = serde_json::json!({"openai:chat": {"health_score": 0.75}});
            let key = sample_key(
                "key-1",
                "provider-1",
                api_key.map(ToOwned::to_owned),
                auth_config.map(ToOwned::to_owned),
            )
            .with_health_fields(Some(health.clone()), None);
            let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
                vec![sample_provider("provider-1")],
                Vec::new(),
                vec![key],
            ));
            let state = AppState::new()
                .expect("test state should build")
                .with_data_state_for_tests(
                    GatewayDataState::with_provider_catalog_reader_for_tests(repository)
                        .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
                );
            let provider_ids = ["provider-1".to_string()];

            let summaries = state
                .list_provider_catalog_key_summaries_by_provider_ids(&provider_ids)
                .await
                .expect("redacted summaries should not require credential authentication");

            assert_eq!(summaries.len(), 1);
            assert_eq!(summaries[0].health_by_format.as_ref(), Some(&health));
            assert_eq!(summaries[0].encrypted_api_key.as_deref(), api_key);
            assert_eq!(summaries[0].encrypted_auth_config.as_deref(), auth_config);
            assert!(state
                .list_provider_catalog_keys_by_provider_ids(&provider_ids)
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn app_state_migrates_both_legacy_fields_with_one_exact_cas() {
        let legacy_api =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "legacy-api-key")
                .expect("legacy API key should encrypt");
        let legacy_auth = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"refresh_token":"legacy-refresh"}"#,
        )
        .expect("legacy auth config should encrypt");
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            Vec::new(),
            vec![sample_key(
                "key-1",
                "provider-1",
                Some(legacy_api),
                Some(legacy_auth),
            )],
        ));
        let state = state_with_repository(Arc::clone(&repository));

        let opened = state
            .list_provider_catalog_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("legacy key should migrate")
            .into_iter()
            .next()
            .expect("key should exist");
        assert_eq!(
            state
                .decrypt_provider_catalog_key_api_key(&opened)
                .expect("API key should open")
                .as_deref(),
            Some("legacy-api-key")
        );
        assert_eq!(
            state
                .decrypt_provider_catalog_key_auth_config(&opened)
                .expect("auth config should open")
                .as_deref(),
            Some(r#"{"refresh_token":"legacy-refresh"}"#)
        );

        let stored = repository
            .list_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("stored key should read")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert!(stored
            .encrypted_api_key
            .as_deref()
            .is_some_and(|value| value.starts_with("aether-provider-catalog-credential-v2:")));
        assert!(stored
            .encrypted_auth_config
            .as_deref()
            .is_some_and(|value| value.starts_with("aether-provider-catalog-credential-v2:")));
    }

    #[tokio::test]
    async fn app_state_rejects_ciphertext_copied_to_another_key() {
        let empty_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            Vec::new(),
            Vec::new(),
        ));
        let bootstrap = state_with_repository(Arc::clone(&empty_repository));
        let copied = bootstrap
            .seal_provider_catalog_key_api_key("provider-1", "key-1", "secret")
            .expect("credential should seal");
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            Vec::new(),
            vec![sample_key("key-2", "provider-1", Some(copied), None)],
        ));
        let state = state_with_repository(repository);

        assert!(state
            .list_provider_catalog_keys_by_ids(&["key-2".to_string()])
            .await
            .is_err());
    }

    #[tokio::test]
    async fn credential_cas_fences_provider_and_both_ciphertexts() {
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1"), sample_provider("provider-2")],
            Vec::new(),
            vec![sample_key(
                "key-1",
                "provider-2",
                Some("api-before".to_string()),
                Some("auth-before".to_string()),
            )],
        );
        let update = ProviderCatalogKeyCredentialsCasUpdate {
            key_id: "key-1".to_string(),
            expected_provider_id: "provider-1".to_string(),
            expected_encrypted_api_key: Some("api-before".to_string()),
            expected_encrypted_auth_config: Some("auth-before".to_string()),
            encrypted_api_key: Some("api-after".to_string()),
            encrypted_auth_config: Some("auth-after".to_string()),
        };

        assert!(!repository
            .compare_and_swap_key_credentials(&update)
            .await
            .expect("provider-fenced CAS should execute"));
        let stored = repository
            .list_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("key should read")
            .into_iter()
            .next()
            .expect("key should exist");
        assert_eq!(stored.encrypted_api_key.as_deref(), Some("api-before"));
        assert_eq!(stored.encrypted_auth_config.as_deref(), Some("auth-before"));
    }
}
