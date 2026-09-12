use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    AuthModuleReadRepository, AuthModuleWriteRepository, CompareAndSwapLdapConfigResult,
    LdapBindPasswordUpdate, StoredLdapModuleConfig, StoredOAuthProviderModuleConfig,
};
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryAuthModuleReadRepository {
    oauth_providers: RwLock<Vec<StoredOAuthProviderModuleConfig>>,
    ldap_config: RwLock<Option<StoredLdapModuleConfig>>,
}

impl InMemoryAuthModuleReadRepository {
    pub fn seed<I>(oauth_providers: I, ldap_config: Option<StoredLdapModuleConfig>) -> Self
    where
        I: IntoIterator<Item = StoredOAuthProviderModuleConfig>,
    {
        Self {
            oauth_providers: RwLock::new(oauth_providers.into_iter().collect()),
            ldap_config: RwLock::new(ldap_config),
        }
    }
}

#[async_trait]
impl AuthModuleReadRepository for InMemoryAuthModuleReadRepository {
    async fn list_enabled_oauth_providers(
        &self,
    ) -> Result<Vec<StoredOAuthProviderModuleConfig>, DataLayerError> {
        Ok(self
            .oauth_providers
            .read()
            .expect("auth module oauth provider repository lock")
            .clone())
    }

    async fn get_ldap_config(&self) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
        Ok(self
            .ldap_config
            .read()
            .expect("auth module ldap repository lock")
            .clone())
    }
}

#[async_trait]
impl AuthModuleWriteRepository for InMemoryAuthModuleReadRepository {
    async fn compare_and_swap_ldap_config(
        &self,
        expected: Option<&StoredLdapModuleConfig>,
        replacement: &StoredLdapModuleConfig,
        bind_password_update: &LdapBindPasswordUpdate,
    ) -> Result<CompareAndSwapLdapConfigResult, DataLayerError> {
        let mut config = self
            .ldap_config
            .write()
            .expect("auth module ldap repository lock");
        if config.as_ref() != expected {
            return Ok(CompareAndSwapLdapConfigResult::Conflict);
        }

        let bind_password_encrypted = match bind_password_update {
            LdapBindPasswordUpdate::Preserve => expected
                .ok_or_else(|| {
                    DataLayerError::InvalidConfiguration(
                        "LDAP bind password cannot be preserved while creating the singleton"
                            .to_string(),
                    )
                })?
                .bind_password_encrypted
                .clone(),
            LdapBindPasswordUpdate::Set(ciphertext) => Some(ciphertext.clone()),
            LdapBindPasswordUpdate::Clear => None,
        };
        let persisted = StoredLdapModuleConfig {
            bind_password_encrypted,
            ..replacement.clone()
        };
        *config = Some(persisted.clone());
        Ok(CompareAndSwapLdapConfigResult::Applied(persisted))
    }

    async fn delete_ldap_config_if_matches(
        &self,
        expected: &StoredLdapModuleConfig,
    ) -> Result<bool, DataLayerError> {
        let mut config = self
            .ldap_config
            .write()
            .expect("auth module ldap repository lock");
        if config.as_ref() != Some(expected) {
            return Ok(false);
        }
        config.take();
        Ok(true)
    }

    async fn compare_and_swap_ldap_bind_password(
        &self,
        expected: &str,
        replacement: &str,
    ) -> Result<bool, DataLayerError> {
        let mut config = self
            .ldap_config
            .write()
            .expect("auth module ldap repository lock");
        let Some(config) = config.as_mut() else {
            return Ok(false);
        };
        if config.bind_password_encrypted.as_deref() != Some(expected) {
            return Ok(false);
        }
        config.bind_password_encrypted = Some(replacement.to_string());
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryAuthModuleReadRepository;
    use crate::repository::auth_modules::{
        AuthModuleReadRepository, AuthModuleWriteRepository, CompareAndSwapLdapConfigResult,
        LdapBindPasswordUpdate, StoredLdapModuleConfig, StoredOAuthProviderModuleConfig,
    };

    fn ldap_config() -> StoredLdapModuleConfig {
        StoredLdapModuleConfig {
            server_url: "ldaps://ldap.example.com".to_string(),
            bind_dn: "cn=admin,dc=example,dc=com".to_string(),
            bind_password_encrypted: Some("encrypted-password".to_string()),
            base_dn: "dc=example,dc=com".to_string(),
            user_search_filter: Some("(uid={username})".to_string()),
            username_attr: Some("uid".to_string()),
            email_attr: Some("mail".to_string()),
            display_name_attr: Some("displayName".to_string()),
            is_enabled: true,
            is_exclusive: false,
            use_starttls: true,
            connect_timeout: Some(10),
        }
    }

    #[tokio::test]
    async fn reads_seeded_auth_module_configs() {
        let repository = InMemoryAuthModuleReadRepository::seed(
            vec![StoredOAuthProviderModuleConfig::new(
                "linuxdo".to_string(),
                "Linux DO".to_string(),
                "client-id".to_string(),
                Some("encrypted".to_string()),
                "https://example.com/callback".to_string(),
            )
            .expect("oauth provider should build")],
            Some(ldap_config()),
        );

        let oauth = repository
            .list_enabled_oauth_providers()
            .await
            .expect("oauth providers should load");
        let ldap = repository
            .get_ldap_config()
            .await
            .expect("ldap config should load");

        assert_eq!(oauth.len(), 1);
        assert_eq!(oauth[0].provider_type, "linuxdo");
        assert_eq!(
            ldap.expect("ldap config should exist").server_url,
            "ldaps://ldap.example.com"
        );
    }

    #[tokio::test]
    async fn ldap_compensation_delete_requires_an_exact_match() {
        let expected = ldap_config();
        let repository = InMemoryAuthModuleReadRepository::seed(Vec::new(), Some(expected.clone()));
        let mismatched = StoredLdapModuleConfig {
            is_enabled: false,
            ..expected.clone()
        };

        assert!(!repository
            .delete_ldap_config_if_matches(&mismatched)
            .await
            .expect("mismatched delete should execute"));
        assert!(repository
            .delete_ldap_config_if_matches(&expected)
            .await
            .expect("matching delete should execute"));
        assert!(repository
            .get_ldap_config()
            .await
            .expect("LDAP config should remain readable")
            .is_none());
    }

    #[tokio::test]
    async fn ldap_compare_and_swap_separates_preserve_set_and_clear() {
        let original = ldap_config();
        let repository = InMemoryAuthModuleReadRepository::seed(Vec::new(), Some(original.clone()));
        let replacement = StoredLdapModuleConfig {
            server_url: "ldap://updated.example.com".to_string(),
            bind_password_encrypted: Some("stale-ciphertext-must-be-ignored".to_string()),
            ..original.clone()
        };

        let preserved = repository
            .compare_and_swap_ldap_config(
                Some(&original),
                &replacement,
                &LdapBindPasswordUpdate::Preserve,
            )
            .await
            .expect("preserve CAS should execute");
        let CompareAndSwapLdapConfigResult::Applied(preserved) = preserved else {
            panic!("fresh snapshot should apply");
        };
        assert_eq!(
            preserved.bind_password_encrypted.as_deref(),
            Some("encrypted-password")
        );

        let set = repository
            .compare_and_swap_ldap_config(
                Some(&preserved),
                &preserved,
                &LdapBindPasswordUpdate::Set("rotated-ciphertext".to_string()),
            )
            .await
            .expect("set CAS should execute");
        let CompareAndSwapLdapConfigResult::Applied(set) = set else {
            panic!("fresh snapshot should apply");
        };
        assert_eq!(
            set.bind_password_encrypted.as_deref(),
            Some("rotated-ciphertext")
        );

        let cleared = repository
            .compare_and_swap_ldap_config(Some(&set), &set, &LdapBindPasswordUpdate::Clear)
            .await
            .expect("clear CAS should execute");
        let CompareAndSwapLdapConfigResult::Applied(cleared) = cleared else {
            panic!("fresh snapshot should apply");
        };
        assert!(cleared.bind_password_encrypted.is_none());
    }

    #[tokio::test]
    async fn ldap_compare_and_swap_rejects_stale_password_and_config_snapshots() {
        let original = ldap_config();
        let repository = InMemoryAuthModuleReadRepository::seed(Vec::new(), Some(original.clone()));
        assert!(repository
            .compare_and_swap_ldap_bind_password("encrypted-password", "rotated-ciphertext")
            .await
            .expect("password rotation should execute"));

        let stale_password_result = repository
            .compare_and_swap_ldap_config(
                Some(&original),
                &StoredLdapModuleConfig {
                    base_dn: "dc=updated,dc=example".to_string(),
                    ..original.clone()
                },
                &LdapBindPasswordUpdate::Preserve,
            )
            .await
            .expect("stale password CAS should execute");
        assert_eq!(
            stale_password_result,
            CompareAndSwapLdapConfigResult::Conflict
        );
        assert_eq!(
            repository
                .get_ldap_config()
                .await
                .expect("LDAP config should load")
                .and_then(|config| config.bind_password_encrypted)
                .as_deref(),
            Some("rotated-ciphertext")
        );

        let current = repository
            .get_ldap_config()
            .await
            .expect("LDAP config should load")
            .expect("LDAP config should exist");
        let changed = StoredLdapModuleConfig {
            is_enabled: false,
            ..current.clone()
        };
        let applied = repository
            .compare_and_swap_ldap_config(
                Some(&current),
                &changed,
                &LdapBindPasswordUpdate::Preserve,
            )
            .await
            .expect("fresh config CAS should execute");
        assert!(matches!(
            applied,
            CompareAndSwapLdapConfigResult::Applied(_)
        ));
        let stale_config_result = repository
            .compare_and_swap_ldap_config(
                Some(&current),
                &current,
                &LdapBindPasswordUpdate::Preserve,
            )
            .await
            .expect("stale config CAS should execute");
        assert_eq!(
            stale_config_result,
            CompareAndSwapLdapConfigResult::Conflict
        );
    }

    #[tokio::test]
    async fn ldap_compare_and_swap_allows_only_one_initial_create() {
        let repository = InMemoryAuthModuleReadRepository::default();
        let replacement = StoredLdapModuleConfig {
            bind_password_encrypted: None,
            ..ldap_config()
        };

        let first = repository
            .compare_and_swap_ldap_config(
                None,
                &replacement,
                &LdapBindPasswordUpdate::Set("first-ciphertext".to_string()),
            )
            .await
            .expect("first create should execute");
        assert!(matches!(first, CompareAndSwapLdapConfigResult::Applied(_)));

        let second = repository
            .compare_and_swap_ldap_config(
                None,
                &replacement,
                &LdapBindPasswordUpdate::Set("second-ciphertext".to_string()),
            )
            .await
            .expect("second create should execute");
        assert_eq!(second, CompareAndSwapLdapConfigResult::Conflict);
        assert_eq!(
            repository
                .get_ldap_config()
                .await
                .expect("LDAP config should load")
                .and_then(|config| config.bind_password_encrypted)
                .as_deref(),
            Some("first-ciphertext")
        );
    }
}
