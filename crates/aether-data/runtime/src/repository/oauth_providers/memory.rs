use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::DataLayerError;
use aether_data_contracts::repository::oauth_providers::{
    EncryptedSecretUpdate, OAuthProviderReadRepository, OAuthProviderWriteRepository,
    StoredOAuthProviderConfig, UpsertOAuthProviderConfigRecord,
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
    async fn upsert_oauth_provider_config(
        &self,
        record: &UpsertOAuthProviderConfigRecord,
    ) -> Result<StoredOAuthProviderConfig, DataLayerError> {
        record.validate()?;

        let mut items = self.items.write().expect("oauth provider repository lock");
        let now = Self::now_unix_secs();
        let existing = items.get(&record.provider_type).cloned();
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
        Ok(item)
    }

    async fn delete_oauth_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<bool, DataLayerError> {
        let mut items = self.items.write().expect("oauth provider repository lock");
        Ok(items.remove(provider_type).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryOAuthProviderRepository;
    use crate::repository::oauth_providers::{
        EncryptedSecretUpdate, OAuthProviderReadRepository, OAuthProviderWriteRepository,
        StoredOAuthProviderConfig, UpsertOAuthProviderConfigRecord,
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
        UpsertOAuthProviderConfigRecord {
            provider_type: provider_type.to_string(),
            display_name: format!("{provider_type} display"),
            client_id: format!("{provider_type}-client"),
            client_secret_encrypted: EncryptedSecretUpdate::Preserve,
            authorization_url_override: Some(format!("https://{provider_type}.example.com/auth")),
            token_url_override: Some(format!("https://{provider_type}.example.com/token")),
            userinfo_url_override: None,
            scopes: Some(vec!["openid".to_string(), "profile".to_string()]),
            redirect_uri: format!("https://{provider_type}.example.com/redirect"),
            frontend_callback_url: "https://frontend.example.com/auth/callback".to_string(),
            attribute_mapping: Some(serde_json::json!({"email": "email"})),
            extra_config: Some(serde_json::json!({"team": true})),
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

        let created = repository
            .upsert_oauth_provider_config(&UpsertOAuthProviderConfigRecord {
                client_secret_encrypted: EncryptedSecretUpdate::Set("secret-1".to_string()),
                ..sample_upsert("google")
            })
            .await
            .expect("create should succeed");
        assert_eq!(created.client_secret_encrypted.as_deref(), Some("secret-1"));

        let updated = repository
            .upsert_oauth_provider_config(&UpsertOAuthProviderConfigRecord {
                client_secret_encrypted: EncryptedSecretUpdate::Clear,
                ..sample_upsert("google")
            })
            .await
            .expect("update should succeed");
        assert!(updated.client_secret_encrypted.is_none());

        let deleted = repository
            .delete_oauth_provider_config("google")
            .await
            .expect("delete should succeed");
        assert!(deleted);
    }
}
