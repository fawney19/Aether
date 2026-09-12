use async_trait::async_trait;

fn redacted_optional_secret<T>(value: &Option<T>) -> Option<&'static str> {
    value.as_ref().map(|_| "[REDACTED]")
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredOAuthProviderModuleConfig {
    pub provider_type: String,
    pub display_name: String,
    pub client_id: String,
    pub client_secret_encrypted: Option<String>,
    pub redirect_uri: String,
}

impl std::fmt::Debug for StoredOAuthProviderModuleConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredOAuthProviderModuleConfig")
            .field("provider_type", &self.provider_type)
            .field("display_name", &self.display_name)
            .field("client_id", &self.client_id)
            .field(
                "client_secret_encrypted",
                &redacted_optional_secret(&self.client_secret_encrypted),
            )
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

impl StoredOAuthProviderModuleConfig {
    pub fn new(
        provider_type: String,
        display_name: String,
        client_id: String,
        client_secret_encrypted: Option<String>,
        redirect_uri: String,
    ) -> Result<Self, crate::DataLayerError> {
        if provider_type.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "oauth_providers.provider_type is empty".to_string(),
            ));
        }
        if display_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "oauth_providers.display_name is empty".to_string(),
            ));
        }
        Ok(Self {
            provider_type,
            display_name,
            client_id,
            client_secret_encrypted,
            redirect_uri,
        })
    }
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredLdapModuleConfig {
    pub server_url: String,
    pub bind_dn: String,
    pub bind_password_encrypted: Option<String>,
    pub base_dn: String,
    pub user_search_filter: Option<String>,
    pub username_attr: Option<String>,
    pub email_attr: Option<String>,
    pub display_name_attr: Option<String>,
    pub is_enabled: bool,
    pub is_exclusive: bool,
    pub use_starttls: bool,
    pub connect_timeout: Option<i32>,
}

impl std::fmt::Debug for StoredLdapModuleConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredLdapModuleConfig")
            .field("server_url", &self.server_url)
            .field("bind_dn", &self.bind_dn)
            .field(
                "bind_password_encrypted",
                &redacted_optional_secret(&self.bind_password_encrypted),
            )
            .field("base_dn", &self.base_dn)
            .field("user_search_filter", &self.user_search_filter)
            .field("username_attr", &self.username_attr)
            .field("email_attr", &self.email_attr)
            .field("display_name_attr", &self.display_name_attr)
            .field("is_enabled", &self.is_enabled)
            .field("is_exclusive", &self.is_exclusive)
            .field("use_starttls", &self.use_starttls)
            .field("connect_timeout", &self.connect_timeout)
            .finish()
    }
}

/// Explicit mutation semantics for the LDAP bind password.
///
/// The password is deliberately kept separate from [`StoredLdapModuleConfig`] updates so a
/// caller that only changes non-secret fields cannot accidentally write a stale ciphertext back
/// to storage.
#[derive(Clone, PartialEq, Eq)]
pub enum LdapBindPasswordUpdate {
    Preserve,
    Set(String),
    Clear,
}

impl std::fmt::Debug for LdapBindPasswordUpdate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Preserve => formatter.write_str("Preserve"),
            Self::Set(_) => formatter.write_str("Set([REDACTED])"),
            Self::Clear => formatter.write_str("Clear"),
        }
    }
}

// The successful branch intentionally returns the complete persisted
// configuration so callers can continue with the exact CAS snapshot. Boxing
// it would change this public repository contract and add needless allocation
// on the normal (successful) path.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareAndSwapLdapConfigResult {
    Applied(StoredLdapModuleConfig),
    Conflict,
}

#[async_trait]
pub trait AuthModuleReadRepository: Send + Sync {
    async fn list_enabled_oauth_providers(
        &self,
    ) -> Result<Vec<StoredOAuthProviderModuleConfig>, crate::DataLayerError>;

    async fn get_ldap_config(
        &self,
    ) -> Result<Option<StoredLdapModuleConfig>, crate::DataLayerError>;
}

#[async_trait]
pub trait AuthModuleWriteRepository: Send + Sync {
    /// Atomically create or replace the singleton LDAP configuration.
    ///
    /// `expected` is the complete snapshot observed by the caller. `None` means the caller
    /// expects the singleton not to exist. Implementations must compare every persisted config
    /// field, including the encrypted password, before applying the replacement. The password
    /// field in `replacement` is never authoritative; only `bind_password_update` controls the
    /// stored secret.
    async fn compare_and_swap_ldap_config(
        &self,
        expected: Option<&StoredLdapModuleConfig>,
        replacement: &StoredLdapModuleConfig,
        bind_password_update: &LdapBindPasswordUpdate,
    ) -> Result<CompareAndSwapLdapConfigResult, crate::DataLayerError>;

    /// Delete the singleton LDAP configuration only when every persisted field still matches the
    /// supplied snapshot. This is used by aggregate-import compensation and must not remove a
    /// configuration that another operation changed after it was created.
    async fn delete_ldap_config_if_matches(
        &self,
        expected: &StoredLdapModuleConfig,
    ) -> Result<bool, crate::DataLayerError>;

    async fn compare_and_swap_ldap_bind_password(
        &self,
        _expected: &str,
        _replacement: &str,
    ) -> Result<bool, crate::DataLayerError> {
        Err(crate::DataLayerError::InvalidConfiguration(
            "LDAP bind password compare-and-swap is not supported by this repository".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{LdapBindPasswordUpdate, StoredLdapModuleConfig, StoredOAuthProviderModuleConfig};

    #[test]
    fn auth_module_debug_output_redacts_encrypted_secrets() {
        let oauth_secret = "debug-secret-oauth-ciphertext";
        let oauth = StoredOAuthProviderModuleConfig::new(
            "linuxdo".to_string(),
            "Linux.do".to_string(),
            "client-id".to_string(),
            Some(oauth_secret.to_string()),
            "https://example.com/callback".to_string(),
        )
        .expect("OAuth module config should build");
        let ldap_secret = "debug-secret-ldap-ciphertext";
        let ldap = StoredLdapModuleConfig {
            server_url: "ldaps://ldap.example.com".to_string(),
            bind_dn: "cn=admin,dc=example,dc=com".to_string(),
            bind_password_encrypted: Some(ldap_secret.to_string()),
            base_dn: "dc=example,dc=com".to_string(),
            user_search_filter: None,
            username_attr: None,
            email_attr: None,
            display_name_attr: None,
            is_enabled: true,
            is_exclusive: false,
            use_starttls: false,
            connect_timeout: Some(5),
        };

        for (rendered, secret) in [
            (format!("{oauth:?}"), oauth_secret),
            (format!("{ldap:?}"), ldap_secret),
            (
                format!("{:?}", LdapBindPasswordUpdate::Set(ldap_secret.to_string())),
                ldap_secret,
            ),
        ] {
            assert!(!rendered.contains(secret));
            assert!(rendered.contains("[REDACTED]"));
        }
    }
}
