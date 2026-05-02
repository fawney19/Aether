use aether_contracts::{ExecutionTimeouts, ProxySnapshot};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use tracing::warn;

use super::snapshot::GatewayProviderTransportSnapshot;

const TUNNEL_BASE_URL_EXTRA_KEY: &str = "tunnel_base_url";
const TUNNEL_OWNER_INSTANCE_ID_EXTRA_KEY: &str = "tunnel_owner_instance_id";
const TUNNEL_OWNER_OBSERVED_AT_EXTRA_KEY: &str = "tunnel_owner_observed_at_unix_secs";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportTunnelAttachmentOwner {
    pub gateway_instance_id: String,
    pub relay_base_url: String,
    pub observed_at_unix_secs: u64,
}

#[async_trait]
pub trait TransportTunnelAffinityLookup: Send + Sync {
    async fn lookup_tunnel_attachment_owner(
        &self,
        node_id: &str,
    ) -> Result<Option<TransportTunnelAttachmentOwner>, String>;
}

pub fn resolve_transport_execution_timeouts(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<ExecutionTimeouts> {
    let total_ms = transport
        .provider
        .request_timeout_secs
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| (value * 1000.0).round() as u64);
    let first_byte_ms = transport
        .provider
        .stream_first_byte_timeout_secs
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| (value * 1000.0).round() as u64);

    if total_ms.is_none() && first_byte_ms.is_none() {
        return None;
    }

    Some(ExecutionTimeouts {
        total_ms,
        first_byte_ms,
        ..ExecutionTimeouts::default()
    })
}

pub fn resolve_transport_proxy_snapshot(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<ProxySnapshot> {
    let raw = effective_proxy_config(transport)?;
    proxy_snapshot_from_value(raw)
}

pub async fn resolve_transport_proxy_snapshot_with_tunnel_affinity(
    lookup: &dyn TransportTunnelAffinityLookup,
    transport: &GatewayProviderTransportSnapshot,
) -> Option<ProxySnapshot> {
    let mut snapshot = resolve_transport_proxy_snapshot(transport)?;
    let Some(node_id) = snapshot
        .node_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Some(snapshot);
    };

    let owner = match lookup.lookup_tunnel_attachment_owner(node_id).await {
        Ok(owner) => owner,
        Err(error) => {
            warn!(error = %error, node_id = node_id, "failed to load tunnel attachment owner");
            None
        }
    };
    let Some(owner) = owner else {
        return Some(snapshot);
    };

    let mut extra = snapshot
        .extra
        .take()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let configured_tunnel_base_url = extra
        .get(TUNNEL_BASE_URL_EXTRA_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if configured_tunnel_base_url.is_none() {
        extra.insert(
            TUNNEL_BASE_URL_EXTRA_KEY.to_string(),
            Value::String(owner.relay_base_url.clone()),
        );
    }
    extra.insert(
        TUNNEL_OWNER_INSTANCE_ID_EXTRA_KEY.to_string(),
        Value::String(owner.gateway_instance_id),
    );
    extra.insert(
        TUNNEL_OWNER_OBSERVED_AT_EXTRA_KEY.to_string(),
        json!(owner.observed_at_unix_secs),
    );
    snapshot.extra = Some(Value::Object(extra));
    Some(snapshot)
}

pub fn transport_proxy_is_locally_supported(transport: &GatewayProviderTransportSnapshot) -> bool {
    let has_configured_proxy = transport.provider.proxy.is_some()
        || transport.endpoint.proxy.is_some()
        || transport.key.proxy.is_some();
    if !has_configured_proxy {
        return true;
    }

    let Some(snapshot) = resolve_transport_proxy_snapshot(transport) else {
        return false;
    };

    if snapshot.enabled == Some(false) {
        return true;
    }

    snapshot
        .url
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
        || snapshot
            .node_id
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
}

pub fn resolve_transport_tls_profile(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<String> {
    transport
        .key
        .fingerprint
        .as_ref()
        .and_then(|value| value.get("tls_profile"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn effective_proxy_config(transport: &GatewayProviderTransportSnapshot) -> Option<&Value> {
    [
        transport.key.proxy.as_ref(),
        transport.endpoint.proxy.as_ref(),
        transport.provider.proxy.as_ref(),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| proxy_enabled(candidate))
}

fn proxy_enabled(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|object| object.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn proxy_snapshot_from_value(value: &Value) -> Option<ProxySnapshot> {
    let object = value.as_object()?;
    let enabled = object.get("enabled").and_then(Value::as_bool);
    let mode = json_string_field(object, "mode");
    let node_id = json_string_field(object, "node_id");
    let label = json_string_field(object, "label");
    let url = json_string_field(object, "url").or_else(|| json_string_field(object, "proxy_url"));

    let mut extra = Map::new();
    for (key, value) in object {
        if matches!(
            key.as_str(),
            "enabled" | "mode" | "node_id" | "label" | "url" | "proxy_url"
        ) {
            continue;
        }
        extra.insert(key.clone(), value.clone());
    }

    Some(ProxySnapshot {
        enabled,
        mode,
        node_id,
        label,
        url,
        extra: if extra.is_empty() {
            None
        } else {
            Some(Value::Object(extra))
        },
    })
}

fn json_string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use async_trait::async_trait;
    use serde_json::{json, Value};

    use super::super::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use super::{
        resolve_transport_proxy_snapshot, resolve_transport_proxy_snapshot_with_tunnel_affinity,
        resolve_transport_tls_profile, transport_proxy_is_locally_supported,
        TransportTunnelAffinityLookup, TransportTunnelAttachmentOwner,
    };

    #[derive(Default)]
    struct TestTunnelAffinityLookup {
        owners: BTreeMap<String, TransportTunnelAttachmentOwner>,
    }

    #[async_trait]
    impl TransportTunnelAffinityLookup for TestTunnelAffinityLookup {
        async fn lookup_tunnel_attachment_owner(
            &self,
            node_id: &str,
        ) -> Result<Option<TransportTunnelAttachmentOwner>, String> {
            Ok(self.owners.get(node_id).cloned())
        }
    }

    fn sample_lookup() -> TestTunnelAffinityLookup {
        let mut owners = BTreeMap::new();
        owners.insert(
            "proxy-node-1".to_string(),
            TransportTunnelAttachmentOwner {
                gateway_instance_id: "gateway-b".to_string(),
                relay_base_url: "http://gateway-b.internal".to_string(),
                observed_at_unix_secs: 4_102_444_800u64,
            },
        );
        TestTunnelAffinityLookup { owners }
    }

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "provider".to_string(),
                provider_type: "custom".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: false,
                concurrent_limit: None,
                max_retries: None,
                proxy: Some(json!({"url":"http://provider-proxy:8080"})),
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai:chat".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                is_active: true,
                base_url: "https://api.openai.example".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: Some(json!({"enabled":false,"url":"http://endpoint-proxy:8080"})),
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: "api_key".to_string(),
                is_active: true,
                api_formats: None,
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,

                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: Some(json!({"node_id":"proxy-node-1","kind":"manual"})),
                fingerprint: Some(json!({"tls_profile":"chrome_136"})),
                decrypted_api_key: "sk-test".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn resolves_transport_proxy_with_key_precedence() {
        let snapshot = resolve_transport_proxy_snapshot(&sample_transport())
            .expect("proxy snapshot should resolve");
        assert_eq!(snapshot.node_id.as_deref(), Some("proxy-node-1"));
        assert_eq!(snapshot.url, None);
        assert_eq!(snapshot.extra, Some(json!({"kind":"manual"})));
    }

    #[tokio::test]
    async fn enriches_transport_proxy_snapshot_with_tunnel_owner_hint() {
        let state = sample_lookup();

        let snapshot =
            resolve_transport_proxy_snapshot_with_tunnel_affinity(&state, &sample_transport())
                .await
                .expect("proxy snapshot should resolve");

        assert_eq!(snapshot.node_id.as_deref(), Some("proxy-node-1"));
        assert_eq!(
            snapshot
                .extra
                .as_ref()
                .and_then(|value| value.get("tunnel_base_url"))
                .and_then(Value::as_str),
            Some("http://gateway-b.internal")
        );
        assert_eq!(
            snapshot
                .extra
                .as_ref()
                .and_then(|value| value.get("tunnel_owner_instance_id"))
                .and_then(Value::as_str),
            Some("gateway-b")
        );
    }

    #[tokio::test]
    async fn preserves_explicit_tunnel_base_url_when_owner_hint_exists() {
        let mut transport = sample_transport();
        transport.key.proxy = Some(json!({
            "node_id": "proxy-node-1",
            "kind": "manual",
            "tunnel_base_url": "http://configured-gateway.internal",
        }));
        let state = sample_lookup();

        let snapshot = resolve_transport_proxy_snapshot_with_tunnel_affinity(&state, &transport)
            .await
            .expect("proxy snapshot should resolve");

        assert_eq!(
            snapshot
                .extra
                .as_ref()
                .and_then(|value| value.get("tunnel_base_url"))
                .and_then(Value::as_str),
            Some("http://configured-gateway.internal")
        );
        assert_eq!(
            snapshot
                .extra
                .as_ref()
                .and_then(|value| value.get("tunnel_owner_instance_id"))
                .and_then(Value::as_str),
            Some("gateway-b")
        );
    }

    #[test]
    fn resolves_transport_tls_profile_from_key_fingerprint() {
        assert_eq!(
            resolve_transport_tls_profile(&sample_transport()).as_deref(),
            Some("chrome_136")
        );
        assert!(transport_proxy_is_locally_supported(&sample_transport()));
    }
}
