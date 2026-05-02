use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::DataLayerError;
use async_trait::async_trait;

use super::auth_config::{absorb_local_auth_config_safe_subset, LocalAuthConfigAbsorption};

#[path = "snapshot_mapping.rs"]
mod snapshot_mapping;

use self::snapshot_mapping::{fallback_encryption_keys, map_endpoint, map_key, map_provider};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct GatewayProviderTransportSnapshot {
    pub provider: GatewayProviderTransportProvider,
    pub endpoint: GatewayProviderTransportEndpoint,
    pub key: GatewayProviderTransportKey,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct GatewayProviderTransportProvider {
    pub id: String,
    pub name: String,
    pub provider_type: String,
    pub website: Option<String>,
    pub is_active: bool,
    pub keep_priority_on_conversion: bool,
    pub enable_format_conversion: bool,
    pub concurrent_limit: Option<i32>,
    pub max_retries: Option<i32>,
    pub proxy: Option<serde_json::Value>,
    pub request_timeout_secs: Option<f64>,
    pub stream_first_byte_timeout_secs: Option<f64>,
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct GatewayProviderTransportEndpoint {
    pub id: String,
    pub provider_id: String,
    pub api_format: String,
    pub api_family: Option<String>,
    pub endpoint_kind: Option<String>,
    pub is_active: bool,
    pub base_url: String,
    pub header_rules: Option<serde_json::Value>,
    pub body_rules: Option<serde_json::Value>,
    pub max_retries: Option<i32>,
    pub custom_path: Option<String>,
    pub config: Option<serde_json::Value>,
    pub format_acceptance_config: Option<serde_json::Value>,
    pub proxy: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct GatewayProviderTransportKey {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    pub auth_type: String,
    pub is_active: bool,
    pub api_formats: Option<Vec<String>>,
    pub auth_type_by_format: Option<serde_json::Value>,
    pub allow_auth_channel_mismatch_formats: Option<serde_json::Value>,
    pub allowed_models: Option<Vec<String>>,
    pub capabilities: Option<serde_json::Value>,
    pub rate_multipliers: Option<serde_json::Value>,
    pub global_priority_by_format: Option<serde_json::Value>,
    pub expires_at_unix_secs: Option<u64>,
    pub proxy: Option<serde_json::Value>,
    pub fingerprint: Option<serde_json::Value>,
    pub decrypted_api_key: String,
    pub decrypted_auth_config: Option<String>,
}

#[async_trait]
pub trait ProviderTransportSnapshotSource: Send + Sync {
    fn encryption_key(&self) -> Option<&str>;

    async fn list_provider_catalog_providers_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError>;

    async fn list_provider_catalog_endpoints_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError>;

    async fn list_provider_catalog_keys_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError>;
}

pub async fn read_provider_transport_snapshot(
    state: &dyn ProviderTransportSnapshotSource,
    provider_id: &str,
    endpoint_id: &str,
    key_id: &str,
) -> Result<Option<GatewayProviderTransportSnapshot>, DataLayerError> {
    let Some(encryption_key) = state.encryption_key() else {
        return Ok(None);
    };
    let fallback_encryption_keys = fallback_encryption_keys(encryption_key);

    let providers = state
        .list_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await?;
    let endpoints = state
        .list_provider_catalog_endpoints_by_ids(&[endpoint_id.to_string()])
        .await?;
    let keys = state
        .list_provider_catalog_keys_by_ids(&[key_id.to_string()])
        .await?;

    let Some(provider) = providers.into_iter().next() else {
        return Ok(None);
    };
    let Some(endpoint) = endpoints.into_iter().next() else {
        return Ok(None);
    };
    let Some(key) = keys.into_iter().next() else {
        return Ok(None);
    };

    if endpoint.provider_id != provider.id {
        return Err(DataLayerError::UnexpectedValue(format!(
            "provider_endpoints.provider_id mismatch: expected {}, got {}",
            provider.id, endpoint.provider_id
        )));
    }
    if key.provider_id != provider.id {
        return Err(DataLayerError::UnexpectedValue(format!(
            "provider_api_keys.provider_id mismatch: expected {}, got {}",
            provider.id, key.provider_id
        )));
    }

    let provider = map_provider(provider);
    let mut endpoint = map_endpoint(endpoint);
    let mut key = map_key(key, encryption_key, &fallback_encryption_keys)?;

    if let LocalAuthConfigAbsorption::Absorbed {
        base_url,
        header_rules,
        custom_path,
    } = absorb_local_auth_config_safe_subset(
        &endpoint.base_url,
        endpoint.header_rules.clone(),
        endpoint.custom_path.clone(),
        key.decrypted_auth_config.as_deref(),
    ) {
        endpoint.base_url = base_url;
        endpoint.header_rules = header_rules;
        endpoint.custom_path = custom_path;
        key.decrypted_auth_config = None;
    }

    Ok(Some(GatewayProviderTransportSnapshot {
        provider,
        endpoint,
        key,
    }))
}
#[cfg(test)]
mod tests {
    use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use aether_data_contracts::DataLayerError;
    use async_trait::async_trait;

    use super::super::policy::{
        supports_local_openai_chat_transport, supports_local_standard_transport_with_network,
    };
    use super::{
        map_key, read_provider_transport_snapshot, GatewayProviderTransportSnapshot,
        ProviderTransportSnapshotSource,
    };

    struct TestSnapshotSource {
        providers: Vec<StoredProviderCatalogProvider>,
        endpoints: Vec<StoredProviderCatalogEndpoint>,
        keys: Vec<StoredProviderCatalogKey>,
        encryption_key: Option<String>,
    }

    impl TestSnapshotSource {
        fn new(
            providers: Vec<StoredProviderCatalogProvider>,
            endpoints: Vec<StoredProviderCatalogEndpoint>,
            keys: Vec<StoredProviderCatalogKey>,
            encryption_key: impl Into<Option<String>>,
        ) -> Self {
            Self {
                providers,
                endpoints,
                keys,
                encryption_key: encryption_key.into(),
            }
        }
    }

    #[async_trait]
    impl ProviderTransportSnapshotSource for TestSnapshotSource {
        fn encryption_key(&self) -> Option<&str> {
            self.encryption_key.as_deref()
        }

        async fn list_provider_catalog_providers_by_ids(
            &self,
            ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
            Ok(self
                .providers
                .iter()
                .filter(|provider| ids.iter().any(|id| id == &provider.id))
                .cloned()
                .collect())
        }

        async fn list_provider_catalog_endpoints_by_ids(
            &self,
            ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
            Ok(self
                .endpoints
                .iter()
                .filter(|endpoint| ids.iter().any(|id| id == &endpoint.id))
                .cloned()
                .collect())
        }

        async fn list_provider_catalog_keys_by_ids(
            &self,
            ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
            Ok(self
                .keys
                .iter()
                .filter(|key| ids.iter().any(|id| id == &key.id))
                .cloned()
                .collect())
        }
    }

    fn sample_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "OpenAI".to_string(),
            Some("https://openai.com".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            true,
            Some(32),
            Some(3),
            Some(serde_json::json!({"url":"http://provider-proxy"})),
            Some(20.0),
            Some(8.0),
            Some(serde_json::json!({"region":"global"})),
        )
    }

    fn sample_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-1".to_string(),
            "provider-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.com".to_string(),
            Some(serde_json::json!([{"action":"set","key":"x-test","value":"1"}])),
            Some(serde_json::json!([{"action":"drop","path":"stream"}])),
            Some(2),
            Some("/v1/chat/completions".to_string()),
            Some(serde_json::json!({"api_version":"v1"})),
            Some(serde_json::json!({"allow":["openai:chat"]})),
            Some(serde_json::json!({"url":"http://endpoint-proxy"})),
        )
        .expect("endpoint transport fields should build")
    }

    fn sample_key() -> StoredProviderCatalogKey {
        let encrypted_api_key =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-live-openai")
                .expect("api key ciphertext should build");
        let encrypted_auth_config = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            "{\"refresh_token\":\"rt-1\",\"project\":\"demo\"}",
        )
        .expect("auth config ciphertext should build");
        StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "prod-key".to_string(),
            "api_key".to_string(),
            Some(serde_json::json!({"cache_1h": true})),
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:chat", "openai:responses"])),
            encrypted_api_key,
            Some(encrypted_auth_config),
            Some(serde_json::json!({"openai:chat": 0.8})),
            Some(serde_json::json!({"openai:chat": 1})),
            Some(serde_json::json!(["gpt-4.1", "gpt-4.1-mini"])),
            Some(1_800_000_000),
            Some(serde_json::json!({"node_id":"proxy-node-1"})),
            Some(serde_json::json!({"tls_profile":"chrome_136"})),
        )
        .expect("key transport fields should build")
    }

    fn read_state() -> TestSnapshotSource {
        TestSnapshotSource::new(
            vec![sample_provider()],
            vec![sample_endpoint()],
            vec![sample_key()],
            Some(DEVELOPMENT_ENCRYPTION_KEY.to_string()),
        )
    }

    #[tokio::test]
    async fn reads_decrypted_provider_transport_snapshot() {
        let state = read_state();

        let snapshot =
            read_provider_transport_snapshot(&state, "provider-1", "endpoint-1", "key-1")
                .await
                .expect("snapshot should read")
                .expect("snapshot should exist");

        assert_eq!(
            snapshot,
            GatewayProviderTransportSnapshot {
                provider: super::GatewayProviderTransportProvider {
                    id: "provider-1".to_string(),
                    name: "OpenAI".to_string(),
                    provider_type: "custom".to_string(),
                    website: Some("https://openai.com".to_string()),
                    is_active: true,
                    keep_priority_on_conversion: false,
                    enable_format_conversion: true,
                    concurrent_limit: Some(32),
                    max_retries: Some(3),
                    proxy: Some(serde_json::json!({"url":"http://provider-proxy"})),
                    request_timeout_secs: Some(20.0),
                    stream_first_byte_timeout_secs: Some(8.0),
                    config: Some(serde_json::json!({"region":"global"})),
                },
                endpoint: super::GatewayProviderTransportEndpoint {
                    id: "endpoint-1".to_string(),
                    provider_id: "provider-1".to_string(),
                    api_format: "openai:chat".to_string(),
                    api_family: Some("openai".to_string()),
                    endpoint_kind: Some("chat".to_string()),
                    is_active: true,
                    base_url: "https://api.openai.com".to_string(),
                    header_rules: Some(
                        serde_json::json!([{"action":"set","key":"x-test","value":"1"}]),
                    ),
                    body_rules: Some(serde_json::json!([{"action":"drop","path":"stream"}])),
                    max_retries: Some(2),
                    custom_path: Some("/v1/chat/completions".to_string()),
                    config: Some(serde_json::json!({"api_version":"v1"})),
                    format_acceptance_config: Some(serde_json::json!({"allow":["openai:chat"]}),),
                    proxy: Some(serde_json::json!({"url":"http://endpoint-proxy"})),
                },
                key: super::GatewayProviderTransportKey {
                    id: "key-1".to_string(),
                    provider_id: "provider-1".to_string(),
                    name: "prod-key".to_string(),
                    auth_type: "api_key".to_string(),
                    is_active: true,
                    api_formats: Some(vec![
                        "openai:chat".to_string(),
                        "openai:responses".to_string(),
                    ]),
                    auth_type_by_format: None,
                    allow_auth_channel_mismatch_formats: None,

                    allowed_models: Some(vec!["gpt-4.1".to_string(), "gpt-4.1-mini".to_string(),]),
                    capabilities: Some(serde_json::json!({"cache_1h": true})),
                    rate_multipliers: Some(serde_json::json!({"openai:chat": 0.8})),
                    global_priority_by_format: Some(serde_json::json!({"openai:chat": 1})),
                    expires_at_unix_secs: Some(1_800_000_000),
                    proxy: Some(serde_json::json!({"node_id":"proxy-node-1"})),
                    fingerprint: Some(serde_json::json!({"tls_profile":"chrome_136"})),
                    decrypted_api_key: "sk-live-openai".to_string(),
                    decrypted_auth_config: Some(
                        "{\"refresh_token\":\"rt-1\",\"project\":\"demo\"}".to_string(),
                    ),
                },
            }
        );
    }

    #[tokio::test]
    async fn reads_snapshot_when_provider_key_api_key_is_null() {
        let mut key = sample_key();
        key.auth_type = "service_account".to_string();
        key.encrypted_api_key = None;
        let state = TestSnapshotSource::new(
            vec![sample_provider()],
            vec![sample_endpoint()],
            vec![key],
            Some(DEVELOPMENT_ENCRYPTION_KEY.to_string()),
        );

        let snapshot =
            read_provider_transport_snapshot(&state, "provider-1", "endpoint-1", "key-1")
                .await
                .expect("snapshot should read")
                .expect("snapshot should exist");

        assert_eq!(snapshot.key.decrypted_api_key, "");
        assert_eq!(
            snapshot.key.decrypted_auth_config.as_deref(),
            Some("{\"refresh_token\":\"rt-1\",\"project\":\"demo\"}")
        );
    }

    #[tokio::test]
    async fn returns_none_when_encryption_key_is_not_configured() {
        let state = TestSnapshotSource::new(
            vec![sample_provider()],
            vec![sample_endpoint()],
            vec![sample_key()],
            None,
        );

        let snapshot =
            read_provider_transport_snapshot(&state, "provider-1", "endpoint-1", "key-1")
                .await
                .expect("snapshot read should not error");
        assert!(snapshot.is_none());
    }

    #[tokio::test]
    async fn absorbs_safe_auth_config_into_local_transport_fields() {
        let provider = sample_provider();
        let endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-safe-1".to_string(),
            "provider-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.com".to_string(),
            Some(serde_json::json!([{"action":"set","key":"x-test","value":"1"}])),
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport fields should build");
        let encrypted_api_key =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-live-openai")
                .expect("api key ciphertext should build");
        let encrypted_auth_config = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"headers":{"x-account-id":"acc-1"},"query":{"tenant":"demo"}}"#,
        )
        .expect("auth config ciphertext should build");
        let key = StoredProviderCatalogKey::new(
            "key-safe-1".to_string(),
            "provider-1".to_string(),
            "safe-key".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:chat"])),
            encrypted_api_key,
            Some(encrypted_auth_config),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport fields should build");
        let state = TestSnapshotSource::new(
            vec![provider],
            vec![endpoint],
            vec![key],
            Some(DEVELOPMENT_ENCRYPTION_KEY.to_string()),
        );

        let snapshot =
            read_provider_transport_snapshot(&state, "provider-1", "endpoint-safe-1", "key-safe-1")
                .await
                .expect("snapshot read should succeed")
                .expect("snapshot should exist");

        assert_eq!(snapshot.key.decrypted_auth_config, None);
        assert_eq!(
            snapshot.endpoint.base_url,
            "https://api.openai.com?tenant=demo"
        );
        assert_eq!(snapshot.endpoint.custom_path.as_deref(), None);
        assert_eq!(
            snapshot.endpoint.header_rules,
            Some(serde_json::json!([
                {"action":"set","key":"x-test","value":"1"},
                {"action":"set","key":"x-account-id","value":"acc-1"}
            ]))
        );
        assert!(supports_local_openai_chat_transport(&snapshot));
    }

    #[tokio::test]
    async fn accepts_plaintext_legacy_key_material() {
        let provider = sample_provider();
        let endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-legacy-1".to_string(),
            "provider-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.com".to_string(),
            Some(serde_json::json!([{"action":"set","key":"x-test","value":"1"}])),
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport fields should build");
        let key = StoredProviderCatalogKey::new(
            "key-legacy-1".to_string(),
            "provider-1".to_string(),
            "legacy-key".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:chat"])),
            "sk-plaintext-openai".to_string(),
            Some(r#"{"headers":{"x-account-id":"acc-legacy"}}"#.to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport fields should build");
        let state = TestSnapshotSource::new(
            vec![provider],
            vec![endpoint],
            vec![key],
            Some(DEVELOPMENT_ENCRYPTION_KEY.to_string()),
        );

        let snapshot = read_provider_transport_snapshot(
            &state,
            "provider-1",
            "endpoint-legacy-1",
            "key-legacy-1",
        )
        .await
        .expect("snapshot read should succeed")
        .expect("snapshot should exist");

        assert_eq!(snapshot.key.decrypted_api_key, "sk-plaintext-openai");
        assert_eq!(snapshot.key.decrypted_auth_config, None);
        assert_eq!(
            snapshot.endpoint.header_rules,
            Some(serde_json::json!([
                {"action":"set","key":"x-test","value":"1"},
                {"action":"set","key":"x-account-id","value":"acc-legacy"}
            ]))
        );
    }

    #[tokio::test]
    async fn rejects_fernet_shaped_key_material_when_encryption_key_is_wrong() {
        let key = sample_key();
        let error = map_key(key, "wrong-encryption-key", &[])
            .expect_err("snapshot read should fail for Fernet-shaped data with wrong key");

        assert!(matches!(error, DataLayerError::UnexpectedValue(message)
            if message.contains("failed to decrypt provider_api_keys.api_key")));
    }

    #[test]
    fn decrypts_fernet_shaped_key_material_with_fallback_encryption_key() {
        let key = sample_key();

        let mapped = map_key(
            key,
            "wrong-encryption-key",
            &[DEVELOPMENT_ENCRYPTION_KEY.to_string()],
        )
        .expect("fallback key should decrypt");

        assert_eq!(mapped.decrypted_api_key, "sk-live-openai");
        assert_eq!(
            mapped.decrypted_auth_config.as_deref(),
            Some("{\"refresh_token\":\"rt-1\",\"project\":\"demo\"}")
        );
    }

    #[test]
    fn accepts_stringified_allowed_models_in_transport_key() {
        let encrypted_api_key =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-live-openai")
                .expect("api key ciphertext should build");
        let key = StoredProviderCatalogKey::new(
            "key-compat-1".to_string(),
            "provider-1".to_string(),
            "compat-key".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:chat"])),
            encrypted_api_key,
            None,
            None,
            None,
            Some(serde_json::json!("[\"gpt-5.2\", \"gpt-5\"]")),
            None,
            None,
            None,
        )
        .expect("key transport fields should build");

        let mapped =
            map_key(key, DEVELOPMENT_ENCRYPTION_KEY, &[]).expect("stringified list should parse");

        assert_eq!(
            mapped.allowed_models,
            Some(vec!["gpt-5.2".to_string(), "gpt-5".to_string()])
        );
    }

    #[test]
    fn accepts_single_string_api_format_in_transport_key() {
        let encrypted_api_key =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-live-openai")
                .expect("api key ciphertext should build");
        let key = StoredProviderCatalogKey::new(
            "key-compat-2".to_string(),
            "provider-1".to_string(),
            "compat-key".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!("openai:chat")),
            encrypted_api_key,
            None,
            None,
            None,
            Some(serde_json::json!("gpt-5.2")),
            None,
            None,
            None,
        )
        .expect("key transport fields should build");

        let mapped =
            map_key(key, DEVELOPMENT_ENCRYPTION_KEY, &[]).expect("single string should parse");

        assert_eq!(mapped.api_formats, Some(vec!["openai:chat".to_string()]));
        assert_eq!(mapped.allowed_models, Some(vec!["gpt-5.2".to_string()]));
    }

    #[tokio::test]
    async fn keeps_unsupported_auth_config_blocking_local_transport() {
        let provider = sample_provider();
        let endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-safe-2".to_string(),
            "provider-1".to_string(),
            "openai:responses".to_string(),
            Some("openai".to_string()),
            Some("responses".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.com".to_string(),
            None,
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport fields should build");
        let encrypted_api_key =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-live-openai")
                .expect("api key ciphertext should build");
        let encrypted_auth_config = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"refresh_token":"rt-1","project":"demo"}"#,
        )
        .expect("auth config ciphertext should build");
        let key = StoredProviderCatalogKey::new(
            "key-safe-2".to_string(),
            "provider-1".to_string(),
            "unsafe-key".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:responses"])),
            encrypted_api_key,
            Some(encrypted_auth_config),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport fields should build");
        let state = TestSnapshotSource::new(
            vec![provider],
            vec![endpoint],
            vec![key],
            Some(DEVELOPMENT_ENCRYPTION_KEY.to_string()),
        );

        let snapshot =
            read_provider_transport_snapshot(&state, "provider-1", "endpoint-safe-2", "key-safe-2")
                .await
                .expect("snapshot read should succeed")
                .expect("snapshot should exist");

        assert_eq!(
            snapshot.key.decrypted_auth_config.as_deref(),
            Some(r#"{"refresh_token":"rt-1","project":"demo"}"#)
        );
        assert!(!supports_local_standard_transport_with_network(
            &snapshot,
            "openai:responses"
        ));
    }

    #[tokio::test]
    async fn absorbs_query_only_auth_config_into_gemini_base_url() {
        let provider = sample_provider();
        let endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-safe-3".to_string(),
            "provider-1".to_string(),
            "gemini:generate_content".to_string(),
            Some("gemini".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://generativelanguage.googleapis.com/v1beta".to_string(),
            None,
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport fields should build");
        let encrypted_api_key =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-live-openai")
                .expect("api key ciphertext should build");
        let encrypted_auth_config = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"query":{"alt":"sse"}}"#,
        )
        .expect("auth config ciphertext should build");
        let key = StoredProviderCatalogKey::new(
            "key-safe-3".to_string(),
            "provider-1".to_string(),
            "safe-key".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["gemini:generate_content"])),
            encrypted_api_key,
            Some(encrypted_auth_config),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport fields should build");
        let state = TestSnapshotSource::new(
            vec![provider],
            vec![endpoint],
            vec![key],
            Some(DEVELOPMENT_ENCRYPTION_KEY.to_string()),
        );

        let snapshot =
            read_provider_transport_snapshot(&state, "provider-1", "endpoint-safe-3", "key-safe-3")
                .await
                .expect("snapshot read should succeed")
                .expect("snapshot should exist");

        assert_eq!(
            snapshot.endpoint.base_url,
            "https://generativelanguage.googleapis.com/v1beta?alt=sse"
        );
        assert_eq!(snapshot.endpoint.custom_path, None);
        assert_eq!(snapshot.key.decrypted_auth_config, None);
    }

    #[tokio::test]
    async fn absorbs_transport_subset_when_metadata_is_present() {
        let provider = sample_provider();
        let endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-safe-4".to_string(),
            "provider-1".to_string(),
            "openai:responses".to_string(),
            Some("openai".to_string()),
            Some("responses".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.com/v1".to_string(),
            Some(serde_json::json!([{"action":"set","key":"x-base","value":"1"}])),
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport fields should build");
        let encrypted_api_key =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-live-openai")
                .expect("api key ciphertext should build");
        let encrypted_auth_config = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{
                "email":"user@example.com",
                "plan_type":"plus",
                "transport":{
                    "extraHeaders":{"x-org-id":"org-1"},
                    "queryParams":{"tenant":"demo","retry":2}
                }
            }"#,
        )
        .expect("auth config ciphertext should build");
        let key = StoredProviderCatalogKey::new(
            "key-safe-4".to_string(),
            "provider-1".to_string(),
            "safe-key".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:responses"])),
            encrypted_api_key,
            Some(encrypted_auth_config),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport fields should build");
        let state = TestSnapshotSource::new(
            vec![provider],
            vec![endpoint],
            vec![key],
            Some(DEVELOPMENT_ENCRYPTION_KEY.to_string()),
        );

        let snapshot =
            read_provider_transport_snapshot(&state, "provider-1", "endpoint-safe-4", "key-safe-4")
                .await
                .expect("snapshot read should succeed")
                .expect("snapshot should exist");

        assert_eq!(snapshot.key.decrypted_auth_config, None);
        assert_eq!(
            snapshot.endpoint.base_url,
            "https://api.openai.com/v1?retry=2&tenant=demo"
        );
        assert_eq!(
            snapshot.endpoint.header_rules,
            Some(serde_json::json!([
                {"action":"set","key":"x-base","value":"1"},
                {"action":"set","key":"x-org-id","value":"org-1"}
            ]))
        );
        assert!(supports_local_standard_transport_with_network(
            &snapshot,
            "openai:responses"
        ));
    }

    #[tokio::test]
    async fn normalizes_json_null_transport_fields_before_local_support_checks() {
        let provider = sample_provider().with_transport_fields(
            true,
            false,
            false,
            None,
            Some(2),
            Some(serde_json::Value::Null),
            Some(20.0),
            Some(8.0),
            Some(serde_json::Value::Null),
        );
        let endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-null-json".to_string(),
            "provider-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.com".to_string(),
            Some(serde_json::Value::Null),
            Some(serde_json::Value::Null),
            Some(2),
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::Value::Null),
            Some(serde_json::Value::Null),
        )
        .expect("endpoint transport fields should build");
        let encrypted_api_key =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-live-openai")
                .expect("api key ciphertext should build");
        let key = StoredProviderCatalogKey::new(
            "key-null-json".to_string(),
            "provider-1".to_string(),
            "safe-key".to_string(),
            "api_key".to_string(),
            Some(serde_json::Value::Null),
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::Value::Null),
            encrypted_api_key,
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::Value::Null),
            Some(serde_json::Value::Null),
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::Value::Null),
        )
        .expect("key transport fields should build");
        let state = TestSnapshotSource::new(
            vec![provider],
            vec![endpoint],
            vec![key],
            Some(DEVELOPMENT_ENCRYPTION_KEY.to_string()),
        );

        let snapshot = read_provider_transport_snapshot(
            &state,
            "provider-1",
            "endpoint-null-json",
            "key-null-json",
        )
        .await
        .expect("snapshot read should succeed")
        .expect("snapshot should exist");

        assert_eq!(snapshot.provider.proxy, None);
        assert_eq!(snapshot.provider.config, None);
        assert_eq!(snapshot.endpoint.header_rules, None);
        assert_eq!(snapshot.endpoint.body_rules, None);
        assert_eq!(snapshot.endpoint.config, None);
        assert_eq!(snapshot.endpoint.format_acceptance_config, None);
        assert_eq!(snapshot.endpoint.proxy, None);
        assert_eq!(snapshot.key.api_formats, None);
        assert_eq!(snapshot.key.allowed_models, None);
        assert_eq!(snapshot.key.capabilities, None);
        assert_eq!(snapshot.key.rate_multipliers, None);
        assert_eq!(snapshot.key.global_priority_by_format, None);
        assert_eq!(snapshot.key.proxy, None);
        assert_eq!(snapshot.key.fingerprint, None);
        assert!(supports_local_openai_chat_transport(&snapshot));
    }
}
