use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use aether_provider_transport::provider_types::{
    fixed_provider_key_inherits_api_formats, provider_type_is_fixed,
};
use std::collections::BTreeSet;

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
    Mixed,
    Unknown,
}

impl ProviderKeyRuntimeAuthKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::Bearer => "bearer",
            Self::ServiceAccount => "service_account",
            Self::Mixed => "mixed",
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

fn key_has_auth_type_overrides(key: &StoredProviderCatalogKey) -> bool {
    key.auth_type_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .is_some_and(|items| !items.is_empty())
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
        ProviderKeyCredentialKind::RawSecret => {
            if key_has_auth_type_overrides(key) {
                ProviderKeyRuntimeAuthKind::Mixed
            } else {
                match auth_type.as_str() {
                    "bearer" => ProviderKeyRuntimeAuthKind::Bearer,
                    "api_key" => ProviderKeyRuntimeAuthKind::ApiKey,
                    _ => ProviderKeyRuntimeAuthKind::Unknown,
                }
            }
        }
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

pub(crate) fn provider_key_configured_api_formats(key: &StoredProviderCatalogKey) -> Vec<String> {
    let mut seen = BTreeSet::new();
    key.api_formats
        .as_ref()
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(crate::ai_serving::normalize_api_format_alias)
                .filter(|value| seen.insert(value.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub(crate) fn provider_active_api_formats(
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Vec<String> {
    let mut formats = Vec::new();
    let mut seen = BTreeSet::new();
    for endpoint in endpoints.iter().filter(|endpoint| endpoint.is_active) {
        let api_format = crate::ai_serving::normalize_api_format_alias(&endpoint.api_format);
        if api_format.is_empty() || !seen.insert(api_format.clone()) {
            continue;
        }
        formats.push(api_format);
    }
    formats
}

pub(crate) fn provider_key_inherits_provider_api_formats(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> bool {
    fixed_provider_key_inherits_api_formats(
        provider_type,
        &key.auth_type,
        key.encrypted_auth_config.as_deref(),
    )
}

pub(crate) fn provider_key_effective_api_formats(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Vec<String> {
    if provider_key_inherits_provider_api_formats(key, provider_type) {
        provider_active_api_formats(endpoints)
    } else {
        provider_key_configured_api_formats(key)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        provider_active_api_formats, provider_key_auth_semantics,
        provider_key_configured_api_formats, provider_key_effective_api_formats,
        provider_key_inherits_provider_api_formats, ProviderKeyCredentialKind,
        ProviderKeyRuntimeAuthKind,
    };
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    };
    use serde_json::json;

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

    fn sample_endpoint(api_format: &str, is_active: bool) -> StoredProviderCatalogEndpoint {
        let mut endpoint = StoredProviderCatalogEndpoint::new(
            format!("endpoint-{api_format}"),
            "provider-1".to_string(),
            api_format.to_string(),
            None,
            None,
            true,
        )
        .expect("endpoint should build");
        endpoint.is_active = is_active;
        endpoint.base_url = "https://example.invalid".to_string();
        endpoint
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

    #[test]
    fn deduplicates_active_provider_api_formats() {
        let endpoints = vec![
            sample_endpoint("openai:responses", true),
            sample_endpoint("openai:image", true),
            sample_endpoint("openai:responses", true),
            sample_endpoint("openai:responses:compact", false),
        ];

        assert_eq!(
            provider_active_api_formats(&endpoints),
            vec!["openai:responses".to_string(), "openai:image".to_string()]
        );
    }

    #[test]
    fn fixed_oauth_key_with_null_formats_inherits_provider_formats() {
        let key = sample_key("oauth");
        let endpoints = vec![
            sample_endpoint("openai:responses", true),
            sample_endpoint("openai:image", true),
        ];

        assert!(provider_key_inherits_provider_api_formats(&key, "codex"));
        assert_eq!(
            provider_key_effective_api_formats(&key, "codex", &endpoints),
            vec!["openai:responses".to_string(), "openai:image".to_string()]
        );
    }

    #[test]
    fn fixed_oauth_key_with_legacy_explicit_formats_still_inherits_provider_formats() {
        let mut key = sample_key("oauth");
        key.api_formats = Some(json!(["openai:responses:compact"]));
        let endpoints = vec![
            sample_endpoint("openai:responses", true),
            sample_endpoint("openai:image", true),
        ];

        assert!(provider_key_inherits_provider_api_formats(&key, "codex"));
        assert_eq!(
            provider_key_effective_api_formats(&key, "codex", &endpoints),
            vec!["openai:responses".to_string(), "openai:image".to_string()]
        );
    }

    #[test]
    fn configured_kiro_bearer_key_inherits_provider_formats() {
        let mut key = sample_key("bearer");
        key.encrypted_auth_config = Some("encrypted-auth-config".to_string());
        key.api_formats = Some(json!(["openai:responses:compact"]));
        let endpoints = vec![
            sample_endpoint("claude:messages", true),
            sample_endpoint("openai:chat", true),
        ];

        assert!(provider_key_inherits_provider_api_formats(&key, "kiro"));
        assert_eq!(
            provider_key_effective_api_formats(&key, "kiro", &endpoints),
            vec!["claude:messages".to_string(), "openai:chat".to_string()]
        );
    }

    #[test]
    fn explicit_formats_do_not_inherit_for_non_fixed_key() {
        let mut key = sample_key("oauth");
        key.api_formats = Some(json!(["openai:responses:compact"]));
        let endpoints = vec![
            sample_endpoint("openai:responses", true),
            sample_endpoint("openai:image", true),
        ];

        assert!(!provider_key_inherits_provider_api_formats(&key, "openai"));
        assert_eq!(
            provider_key_configured_api_formats(&key),
            vec!["openai:responses:compact".to_string()]
        );
        assert_eq!(
            provider_key_effective_api_formats(&key, "openai", &endpoints),
            vec!["openai:responses:compact".to_string()]
        );
    }
}
