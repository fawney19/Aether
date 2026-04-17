use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderKeyCredentialKind {
    RawSecret,
    OAuthSession,
    ServiceAccount,
}

impl ProviderKeyCredentialKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::RawSecret => "raw_secret",
            Self::OAuthSession => "oauth_session",
            Self::ServiceAccount => "service_account",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderKeyRuntimeAuthKind {
    ApiKey,
    Bearer,
    ServiceAccount,
    Unknown,
}

impl ProviderKeyRuntimeAuthKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::Bearer => "bearer",
            Self::ServiceAccount => "service_account",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderKeyAuthSemantics {
    credential_kind: ProviderKeyCredentialKind,
    runtime_auth_kind: ProviderKeyRuntimeAuthKind,
    oauth_managed: bool,
}

impl ProviderKeyAuthSemantics {
    pub(crate) const fn credential_kind(self) -> ProviderKeyCredentialKind {
        self.credential_kind
    }

    pub(crate) const fn runtime_auth_kind(self) -> ProviderKeyRuntimeAuthKind {
        self.runtime_auth_kind
    }

    pub(crate) const fn oauth_managed(self) -> bool {
        self.oauth_managed
    }

    pub(crate) const fn can_refresh_oauth(self) -> bool {
        self.oauth_managed
    }

    pub(crate) const fn can_export_oauth(self) -> bool {
        self.oauth_managed
    }

    pub(crate) const fn can_edit_oauth(self) -> bool {
        self.oauth_managed
    }

    pub(crate) const fn can_show_oauth_metadata(self) -> bool {
        self.oauth_managed
    }
}

fn normalized_auth_type(key: &StoredProviderCatalogKey) -> String {
    key.auth_type.trim().to_ascii_lowercase()
}

fn key_has_auth_config(key: &StoredProviderCatalogKey) -> bool {
    key.encrypted_auth_config
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
}

fn provider_uses_bearer_oauth_runtime(provider_type: &str) -> bool {
    matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "claude_code" | "codex" | "gemini_cli" | "antigravity" | "kiro"
    )
}

fn provider_key_is_legacy_kiro_oauth_session(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    auth_type: &str,
) -> bool {
    provider_type.trim().eq_ignore_ascii_case("kiro")
        && auth_type.eq_ignore_ascii_case("bearer")
        && key_has_auth_config(key)
}

pub(crate) fn provider_key_auth_semantics(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> ProviderKeyAuthSemantics {
    let auth_type = normalized_auth_type(key);
    let oauth_managed = auth_type == "oauth"
        || provider_key_is_legacy_kiro_oauth_session(key, provider_type, &auth_type);
    let credential_kind = if oauth_managed {
        ProviderKeyCredentialKind::OAuthSession
    } else if matches!(auth_type.as_str(), "service_account" | "vertex_ai") {
        ProviderKeyCredentialKind::ServiceAccount
    } else {
        ProviderKeyCredentialKind::RawSecret
    };

    let runtime_auth_kind = match credential_kind {
        ProviderKeyCredentialKind::OAuthSession => {
            if provider_uses_bearer_oauth_runtime(provider_type) {
                ProviderKeyRuntimeAuthKind::Bearer
            } else {
                ProviderKeyRuntimeAuthKind::Unknown
            }
        }
        ProviderKeyCredentialKind::ServiceAccount => ProviderKeyRuntimeAuthKind::ServiceAccount,
        ProviderKeyCredentialKind::RawSecret => match auth_type.as_str() {
            "bearer" => ProviderKeyRuntimeAuthKind::Bearer,
            "api_key" => ProviderKeyRuntimeAuthKind::ApiKey,
            _ => ProviderKeyRuntimeAuthKind::Unknown,
        },
    };

    ProviderKeyAuthSemantics {
        credential_kind,
        runtime_auth_kind,
        oauth_managed,
    }
}

pub(crate) fn provider_key_is_oauth_managed(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> bool {
    provider_key_auth_semantics(key, provider_type).oauth_managed()
}

#[cfg(test)]
mod tests {
    use super::{
        provider_key_auth_semantics, ProviderKeyCredentialKind, ProviderKeyRuntimeAuthKind,
    };
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

    fn sample_key(auth_type: &str) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            auth_type.to_string(),
            None,
            true,
        )
        .expect("key should build")
    }

    #[test]
    fn recognizes_oauth_managed_key() {
        let semantics = provider_key_auth_semantics(&sample_key("oauth"), "codex");

        assert!(semantics.oauth_managed());
        assert_eq!(
            semantics.credential_kind(),
            ProviderKeyCredentialKind::OAuthSession
        );
        assert_eq!(
            semantics.runtime_auth_kind(),
            ProviderKeyRuntimeAuthKind::Bearer
        );
    }

    #[test]
    fn recognizes_legacy_kiro_bearer_key_with_auth_config_as_oauth_managed() {
        let mut key = sample_key("bearer");
        key.encrypted_auth_config = Some("ciphertext".to_string());

        let semantics = provider_key_auth_semantics(&key, "kiro");

        assert!(semantics.oauth_managed());
        assert_eq!(
            semantics.credential_kind(),
            ProviderKeyCredentialKind::OAuthSession
        );
        assert_eq!(
            semantics.runtime_auth_kind(),
            ProviderKeyRuntimeAuthKind::Bearer
        );
    }

    #[test]
    fn keeps_plain_bearer_key_as_raw_secret() {
        let semantics = provider_key_auth_semantics(&sample_key("bearer"), "kiro");

        assert!(!semantics.oauth_managed());
        assert_eq!(
            semantics.credential_kind(),
            ProviderKeyCredentialKind::RawSecret
        );
        assert_eq!(
            semantics.runtime_auth_kind(),
            ProviderKeyRuntimeAuthKind::Bearer
        );
    }

    #[test]
    fn recognizes_service_account_key() {
        let semantics = provider_key_auth_semantics(&sample_key("service_account"), "vertex_ai");

        assert!(!semantics.oauth_managed());
        assert_eq!(
            semantics.credential_kind(),
            ProviderKeyCredentialKind::ServiceAccount
        );
        assert_eq!(
            semantics.runtime_auth_kind(),
            ProviderKeyRuntimeAuthKind::ServiceAccount
        );
    }
}
