use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::DataLayerError;
use aether_data_contracts::repository::oauth_providers::{
    EncryptedSecretUpdate, OAuthProviderReadRepository, OAuthProviderWriteRepository,
    StoredOAuthProviderConfig, UpsertOAuthProviderConfigOutcome, UpsertOAuthProviderConfigRecord,
};

#[derive(Debug, Default)]
pub struct InMemoryOAuthProviderRepository {
    items: RwLock<BTreeMap<String, StoredOAuthProviderConfig>>,
}

impl InMemoryOAuthProviderRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredOAuthProviderConfig>,
    {
        let items = items
            .into_iter()
            .map(|item| (item.provider_type.clone(), item))
            .collect();
        Self {
            items: RwLock::new(items),
        }
    }

    fn now_unix_secs() -> Option<u64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
    }
}

#[async_trait]
impl OAuthProviderReadRepository for InMemoryOAuthProviderRepository {
    async fn list_oauth_provider_configs(
        &self,
    ) -> Result<Vec<StoredOAuthProviderConfig>, DataLayerError> {
        let items = self.items.read().expect("oauth provider repository lock");
        Ok(items.values().cloned().collect())
    }

    async fn get_oauth_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<Option<StoredOAuthProviderConfig>, DataLayerError> {
        let items = self.items.read().expect("oauth provider repository lock");
        Ok(items.get(provider_type).cloned())
    }

    async fn count_locked_users_if_provider_disabled(
        &self,
        _provider_type: &str,
        _ldap_exclusive: bool,
    ) -> Result<usize, DataLayerError> {
        Ok(0)
    }
}

#[async_trait]
impl OAuthProviderWriteRepository for InMemoryOAuthProviderRepository {
    async fn upsert_oauth_provider_config_guarded(
        &self,
        record: &UpsertOAuthProviderConfigRecord,
        _ldap_exclusive: bool,
        force_disable: bool,
        locked_users_snapshot: usize,
    ) -> Result<UpsertOAuthProviderConfigOutcome, DataLayerError> {
        record.validate()?;

        let mut items = self.items.write().expect("oauth provider repository lock");
        let now = Self::now_unix_secs();
        let existing = items.get(&record.provider_type).cloned();
        if !force_disable
            && locked_users_snapshot > 0
            && existing
                .as_ref()
                .is_some_and(|provider| provider.is_enabled && !record.is_enabled)
        {
            return Ok(
                UpsertOAuthProviderConfigOutcome::DisableRequiresConfirmation {
                    affected_count: locked_users_snapshot,
                },
            );
        }
        let created_at = existing
            .as_ref()
            .and_then(|item| item.created_at_unix_ms)
            .or(now);
        let client_secret_encrypted = match (&record.client_secret_encrypted, existing.as_ref()) {
            (EncryptedSecretUpdate::Preserve, Some(item)) => item.client_secret_encrypted.clone(),
            (EncryptedSecretUpdate::Preserve, None) => None,
            (EncryptedSecretUpdate::Clear, _) => None,
            (EncryptedSecretUpdate::Set(value), _) => Some(value.clone()),
        };

        let item = StoredOAuthProviderConfig::new(
            record.provider_type.clone(),
            record.display_name.clone(),
            record.client_id.clone(),
            record.redirect_uri.clone(),
            record.frontend_callback_url.clone(),
        )?
        .with_config_fields(
            client_secret_encrypted,
            record.authorization_url_override.clone(),
            record.token_url_override.clone(),
            record.userinfo_url_override.clone(),
            record.scopes.clone(),
            record.attribute_mapping.clone(),
            record.extra_config.clone(),
            record.icon_url.clone(),
            record.is_enabled,
        )
        .with_timestamps(created_at, now);

        items.insert(record.provider_type.clone(), item.clone());
        Ok(UpsertOAuthProviderConfigOutcome::Upserted(item))
    }

    async fn compare_and_swap_oauth_provider_client_secret(
        &self,
        provider_type: &str,
        expected: &str,
        replacement: &str,
    ) -> Result<bool, DataLayerError> {
        let mut items = self.items.write().expect("oauth provider repository lock");
        let Some(item) = items.get_mut(provider_type) else {
            return Ok(false);
        };
        if item.client_secret_encrypted.as_deref() != Some(expected) {
            return Ok(false);
        }
        item.client_secret_encrypted = Some(replacement.to_string());
        Ok(true)
    }

    async fn delete_oauth_provider_config_if_unlinked(
        &self,
        provider_type: &str,
        has_links_snapshot: bool,
    ) -> Result<bool, DataLayerError> {
        if has_links_snapshot {
            return Ok(false);
        }
        let mut items = self.items.write().expect("oauth provider repository lock");
        Ok(items.remove(provider_type).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryOAuthProviderRepository;
    use crate::repository::oauth_providers::{
        EncryptedSecretUpdate, OAuthProviderReadRepository, OAuthProviderWriteRepository,
        StoredOAuthProviderConfig, UpsertOAuthProviderConfigOutcome,
        UpsertOAuthProviderConfigRecord,
    };

    fn sample_provider(provider_type: &str) -> StoredOAuthProviderConfig {
        StoredOAuthProviderConfig::new(
            provider_type.to_string(),
            format!("{provider_type} display"),
            format!("{provider_type}-client"),
            format!("https://{provider_type}.example.com/redirect"),
            "https://frontend.example.com/auth/callback".to_string(),
        )
        .expect("provider should build")
    }

    fn sample_upsert(provider_type: &str) -> UpsertOAuthProviderConfigRecord {
        let is_custom_oidc = provider_type.starts_with("custom_oidc");
        let endpoint_host = if is_custom_oidc {
            "idp.example".to_string()
        } else {
            format!("{provider_type}.example.com")
        };
        UpsertOAuthProviderConfigRecord {
            provider_type: provider_type.to_string(),
            display_name: format!("{provider_type} display"),
            client_id: format!("{provider_type}-client"),
            client_secret_encrypted: EncryptedSecretUpdate::Preserve,
            authorization_url_override: Some(format!("https://{endpoint_host}/auth")),
            token_url_override: Some(format!("https://{endpoint_host}/token")),
            userinfo_url_override: is_custom_oidc
                .then(|| format!("https://{endpoint_host}/userinfo")),
            scopes: Some(vec!["openid".to_string(), "profile".to_string()]),
            redirect_uri: format!("https://{provider_type}.example.com/redirect"),
            frontend_callback_url: "https://frontend.example.com/auth/callback".to_string(),
            attribute_mapping: Some(serde_json::json!({"email": "email"})),
            extra_config: is_custom_oidc.then(|| {
                serde_json::json!({
                    "allowed_domains": [endpoint_host],
                    "team": true,
                })
            }),
            icon_url: None,
            is_enabled: true,
        }
    }

    #[tokio::test]
    async fn reads_and_mutates_oauth_provider_configs() {
        let repository = InMemoryOAuthProviderRepository::seed(vec![
            sample_provider("linuxdo"),
            sample_provider("github"),
        ]);

        let listed = repository
            .list_oauth_provider_configs()
            .await
            .expect("list should succeed");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].provider_type, "github");
        assert_eq!(listed[1].provider_type, "linuxdo");

        let UpsertOAuthProviderConfigOutcome::Upserted(created) = repository
            .upsert_oauth_provider_config_guarded(
                &UpsertOAuthProviderConfigRecord {
                    client_secret_encrypted: EncryptedSecretUpdate::Set("secret-1".to_string()),
                    ..sample_upsert("custom_oidc")
                },
                false,
                false,
                0,
            )
            .await
            .expect("create should succeed")
        else {
            panic!("create unexpectedly required confirmation");
        };
        assert_eq!(created.client_secret_encrypted.as_deref(), Some("secret-1"));

        let UpsertOAuthProviderConfigOutcome::Upserted(updated) = repository
            .upsert_oauth_provider_config_guarded(
                &UpsertOAuthProviderConfigRecord {
                    client_secret_encrypted: EncryptedSecretUpdate::Clear,
                    ..sample_upsert("custom_oidc")
                },
                false,
                false,
                0,
            )
            .await
            .expect("update should succeed")
        else {
            panic!("update unexpectedly required confirmation");
        };
        assert!(updated.client_secret_encrypted.is_none());

        let deleted = repository
            .delete_oauth_provider_config_if_unlinked("custom_oidc", false)
            .await
            .expect("delete should succeed");
        assert!(deleted);
    }

    #[tokio::test]
    async fn client_secret_cas_preserves_concurrent_non_secret_fields_and_timestamp() {
        let repository = InMemoryOAuthProviderRepository::default();
        repository
            .upsert_oauth_provider_config(&UpsertOAuthProviderConfigRecord {
                client_secret_encrypted: EncryptedSecretUpdate::Set("legacy-secret".to_string()),
                ..sample_upsert("custom_oidc")
            })
            .await
            .expect("provider should create");

        let concurrent = repository
            .upsert_oauth_provider_config(&UpsertOAuthProviderConfigRecord {
                display_name: "concurrent display update".to_string(),
                client_secret_encrypted: EncryptedSecretUpdate::Preserve,
                ..sample_upsert("custom_oidc")
            })
            .await
            .expect("non-secret update should persist");
        assert!(repository
            .compare_and_swap_oauth_provider_client_secret(
                "custom_oidc",
                "legacy-secret",
                "record-bound-v2",
            )
            .await
            .expect("secret CAS should execute"));

        let migrated = repository
            .get_oauth_provider_config("custom_oidc")
            .await
            .expect("provider should read")
            .expect("provider should exist");
        assert_eq!(migrated.display_name, "concurrent display update");
        assert_eq!(
            migrated.updated_at_unix_secs,
            concurrent.updated_at_unix_secs
        );
        assert_eq!(
            migrated.client_secret_encrypted.as_deref(),
            Some("record-bound-v2")
        );
        assert!(!repository
            .compare_and_swap_oauth_provider_client_secret(
                "custom_oidc",
                "legacy-secret",
                "must-not-win",
            )
            .await
            .expect("stale CAS should execute"));
        assert_eq!(
            repository
                .get_oauth_provider_config("custom_oidc")
                .await
                .expect("provider should read")
                .expect("provider should exist")
                .client_secret_encrypted
                .as_deref(),
            Some("record-bound-v2")
        );
    }
}
