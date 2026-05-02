use super::snapshot::GatewayProviderTransportSnapshot;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderTransportSnapshotCacheKey {
    provider_id: String,
    endpoint_id: String,
    key_id: String,
}

impl ProviderTransportSnapshotCacheKey {
    pub fn new(provider_id: &str, endpoint_id: &str, key_id: &str) -> Option<Self> {
        let provider_id = provider_id.trim();
        let endpoint_id = endpoint_id.trim();
        let key_id = key_id.trim();
        if provider_id.is_empty() || endpoint_id.is_empty() || key_id.is_empty() {
            return None;
        }
        Some(Self {
            provider_id: provider_id.to_string(),
            endpoint_id: endpoint_id.to_string(),
            key_id: key_id.to_string(),
        })
    }
}

pub fn provider_transport_snapshot_looks_refreshed(
    current: &GatewayProviderTransportSnapshot,
    refreshed: &GatewayProviderTransportSnapshot,
) -> bool {
    current.key.decrypted_api_key != refreshed.key.decrypted_api_key
        || current.key.decrypted_auth_config != refreshed.key.decrypted_auth_config
        || current.key.expires_at_unix_secs != refreshed.key.expires_at_unix_secs
}

#[cfg(test)]
mod tests {
    use super::{provider_transport_snapshot_looks_refreshed, ProviderTransportSnapshotCacheKey};
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_snapshot() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Provider".to_string(),
                provider_type: "openai".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: false,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai".to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://example.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "Key".to_string(),
                auth_type: "bearer".to_string(),
                is_active: true,
                api_formats: None,
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,

                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: Some(1),
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "sk-test".to_string(),
                decrypted_auth_config: Some("{\"token\":\"x\"}".to_string()),
            },
        }
    }

    #[test]
    fn cache_key_requires_non_empty_segments() {
        assert!(ProviderTransportSnapshotCacheKey::new("provider", "endpoint", "key").is_some());
        assert!(ProviderTransportSnapshotCacheKey::new("", "endpoint", "key").is_none());
        assert!(ProviderTransportSnapshotCacheKey::new("provider", " ", "key").is_none());
        assert!(ProviderTransportSnapshotCacheKey::new("provider", "endpoint", "").is_none());
    }

    #[test]
    fn refresh_detection_tracks_key_material_and_expiry() {
        let current = sample_snapshot();
        let mut refreshed = current.clone();
        assert!(!provider_transport_snapshot_looks_refreshed(
            &current, &refreshed
        ));

        refreshed.key.decrypted_api_key = "sk-updated".to_string();
        assert!(provider_transport_snapshot_looks_refreshed(
            &current, &refreshed
        ));

        let mut refreshed = current.clone();
        refreshed.key.decrypted_auth_config = Some("{\"token\":\"y\"}".to_string());
        assert!(provider_transport_snapshot_looks_refreshed(
            &current, &refreshed
        ));

        let mut refreshed = current.clone();
        refreshed.key.expires_at_unix_secs = Some(2);
        assert!(provider_transport_snapshot_looks_refreshed(
            &current, &refreshed
        ));
    }
}
