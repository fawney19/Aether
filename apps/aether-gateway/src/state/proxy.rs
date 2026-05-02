use aether_contracts::ProxySnapshot;
use aether_data::repository::proxy_nodes::proxy_node_accepts_new_tunnels;
use serde_json::{json, Map, Value};

use super::AppState;
use crate::provider_transport::{GatewayProviderTransportSnapshot, TransportTunnelAffinityLookup};

const TUNNEL_BASE_URL_EXTRA_KEY: &str = "tunnel_base_url";
const TUNNEL_OWNER_INSTANCE_ID_EXTRA_KEY: &str = "tunnel_owner_instance_id";
const TUNNEL_OWNER_OBSERVED_AT_EXTRA_KEY: &str = "tunnel_owner_observed_at_unix_secs";

impl AppState {
    pub(crate) async fn read_system_proxy_node_id(&self) -> Option<String> {
        self.read_system_config_json_value("system_proxy_node_id")
            .await
            .ok()
            .flatten()
            .and_then(|value| value.as_str().map(str::trim).map(ToOwned::to_owned))
            .filter(|value| !value.is_empty())
    }

    pub(crate) async fn resolve_proxy_node_snapshot(
        &self,
        node_id: Option<&str>,
    ) -> Option<ProxySnapshot> {
        let node_id = node_id.map(str::trim).filter(|value| !value.is_empty())?;
        let node = self.find_proxy_node(node_id).await.ok().flatten()?;
        if node.status.trim() != "online" {
            return None;
        }
        if !proxy_node_accepts_new_tunnels(&node) {
            return None;
        }
        if node.tunnel_mode && node.tunnel_connected {
            let mut extra = Map::new();
            let owner = self
                .lookup_tunnel_attachment_owner(node_id)
                .await
                .ok()
                .flatten();
            if let Some(owner) = owner {
                extra.insert(
                    TUNNEL_BASE_URL_EXTRA_KEY.to_string(),
                    Value::String(owner.relay_base_url),
                );
                extra.insert(
                    TUNNEL_OWNER_INSTANCE_ID_EXTRA_KEY.to_string(),
                    Value::String(owner.gateway_instance_id),
                );
                extra.insert(
                    TUNNEL_OWNER_OBSERVED_AT_EXTRA_KEY.to_string(),
                    json!(owner.observed_at_unix_secs),
                );
            } else if !self.tunnel.has_local_proxy(node_id) {
                return None;
            }
            return Some(ProxySnapshot {
                enabled: Some(true),
                mode: Some("tunnel".to_string()),
                node_id: Some(node.id),
                label: Some(node.name),
                url: None,
                extra: if extra.is_empty() {
                    None
                } else {
                    Some(Value::Object(extra))
                },
            });
        }
        if !node.is_manual {
            return None;
        }
        let proxy_url = node
            .proxy_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        Some(ProxySnapshot {
            enabled: Some(true),
            mode: proxy_mode_from_url(Some(proxy_url)),
            node_id: Some(node.id),
            label: Some(node.name),
            url: proxy_url_with_node_auth(
                proxy_url,
                node.proxy_username.as_deref(),
                node.proxy_password.as_deref(),
            )
            .or_else(|| Some(proxy_url.to_string())),
            extra: None,
        })
    }

    pub(crate) async fn resolve_system_proxy_snapshot(&self) -> Option<ProxySnapshot> {
        let node_id = self.read_system_proxy_node_id().await;
        self.resolve_proxy_node_snapshot(node_id.as_deref()).await
    }

    pub(crate) async fn resolve_transport_proxy_snapshot_with_tunnel_affinity(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<ProxySnapshot> {
        self.resolve_transport_proxy_with_source_with_tunnel_affinity(transport)
            .await
            .map(|(snapshot, _)| snapshot)
    }

    pub(crate) async fn resolve_transport_proxy_source_with_tunnel_affinity(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<&'static str> {
        self.resolve_transport_proxy_with_source_with_tunnel_affinity(transport)
            .await
            .map(|(_, source)| source)
    }

    pub(crate) async fn resolve_configured_proxy_snapshot_with_tunnel_affinity(
        &self,
        raw: Option<&Value>,
    ) -> Option<ProxySnapshot> {
        let object = raw?.as_object()?;
        if !proxy_enabled(object) {
            return None;
        }

        let node_id = json_string_field(object, "node_id");
        if let Some(snapshot) = self.resolve_proxy_node_snapshot(node_id.as_deref()).await {
            return Some(snapshot);
        }
        if let Some(node_id) = node_id.as_deref() {
            if !proxy_object_has_inline_url(object)
                && self.find_proxy_node(node_id).await.ok().flatten().is_some()
            {
                return None;
            }
        }

        proxy_snapshot_from_object(object)
    }

    async fn resolve_transport_proxy_with_source_with_tunnel_affinity(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<(ProxySnapshot, &'static str)> {
        if let Some(snapshot) = self
            .resolve_configured_proxy_snapshot_with_tunnel_affinity(transport.key.proxy.as_ref())
            .await
        {
            return Some((snapshot, "key"));
        }
        if let Some(snapshot) = self
            .resolve_configured_proxy_snapshot_with_tunnel_affinity(
                transport.endpoint.proxy.as_ref(),
            )
            .await
        {
            return Some((snapshot, "endpoint"));
        }
        if let Some(snapshot) = self
            .resolve_configured_proxy_snapshot_with_tunnel_affinity(
                transport.provider.proxy.as_ref(),
            )
            .await
        {
            return Some((snapshot, "provider"));
        }
        self.resolve_system_proxy_snapshot()
            .await
            .map(|snapshot| (snapshot, "system"))
    }
}

fn proxy_enabled(object: &Map<String, Value>) -> bool {
    object
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn proxy_snapshot_from_object(object: &Map<String, Value>) -> Option<ProxySnapshot> {
    let mode = json_string_field(object, "mode");
    let node_id = json_string_field(object, "node_id");
    let label = json_string_field(object, "label");
    let url = json_string_field(object, "url").or_else(|| json_string_field(object, "proxy_url"));

    if node_id.is_none() && url.is_none() {
        return None;
    }

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
        enabled: object.get("enabled").and_then(Value::as_bool),
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

fn proxy_object_has_inline_url(object: &Map<String, Value>) -> bool {
    json_string_field(object, "url").is_some() || json_string_field(object, "proxy_url").is_some()
}

fn json_string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn proxy_mode_from_url(proxy_url: Option<&str>) -> Option<String> {
    let proxy_url = proxy_url?.trim();
    if proxy_url.is_empty() {
        return None;
    }
    let scheme = url::Url::parse(proxy_url)
        .ok()
        .map(|value| value.scheme().to_ascii_lowercase())
        .unwrap_or_default();
    if scheme.starts_with("socks") {
        Some("socks".to_string())
    } else {
        Some("http".to_string())
    }
}

fn proxy_url_with_node_auth(
    proxy_url: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Option<String> {
    let username = username.map(str::trim).filter(|value| !value.is_empty())?;
    let mut parsed = url::Url::parse(proxy_url).ok()?;
    if parsed.set_username(username).is_err() {
        return None;
    }
    let password = password.map(str::trim).filter(|value| !value.is_empty());
    if parsed.set_password(password).is_err() {
        return None;
    }
    Some(parsed.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_data::repository::proxy_nodes::{InMemoryProxyNodeRepository, StoredProxyNode};
    use serde_json::json;

    use super::proxy_url_with_node_auth;
    use crate::{data::GatewayDataState, AppState};

    #[test]
    fn proxy_url_with_node_auth_omits_empty_password_separator() {
        assert_eq!(
            proxy_url_with_node_auth("socks5://proxy.example:1080", Some("alice"), None).as_deref(),
            Some("socks5://alice@proxy.example:1080")
        );
    }

    #[tokio::test]
    async fn resolve_proxy_node_snapshot_rejects_unroutable_tunnel_node() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_tunnel_node(
            "proxy-node-stale",
        )]));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                repository,
            ));

        let snapshot = state
            .resolve_proxy_node_snapshot(Some("proxy-node-stale"))
            .await;

        assert_eq!(snapshot, None);
    }

    #[tokio::test]
    async fn resolve_proxy_node_snapshot_keeps_tunnel_node_with_owner_hint() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_tunnel_node(
            "proxy-node-owned",
        )]));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_proxy_node_repository_for_tests(repository)
                    .with_system_config_values_for_tests(vec![(
                        "tunnel.attachments.proxy-node-owned".to_string(),
                        json!({
                            "gateway_instance_id": "gateway-owner",
                            "relay_base_url": "http://gateway-owner.internal",
                            "conn_count": 1,
                            "observed_at_unix_secs": 4_102_444_800u64,
                        }),
                    )]),
            );

        let snapshot = state
            .resolve_proxy_node_snapshot(Some("proxy-node-owned"))
            .await
            .expect("owned tunnel snapshot should resolve");

        assert_eq!(snapshot.mode.as_deref(), Some("tunnel"));
        assert_eq!(snapshot.node_id.as_deref(), Some("proxy-node-owned"));
        assert_eq!(
            snapshot
                .extra
                .as_ref()
                .and_then(|extra| extra.get("tunnel_base_url"))
                .and_then(serde_json::Value::as_str),
            Some("http://gateway-owner.internal")
        );
    }

    #[tokio::test]
    async fn resolve_configured_proxy_snapshot_rejects_unroutable_stored_tunnel_reference() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_tunnel_node(
            "proxy-node-stale",
        )]));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                repository,
            ));

        let snapshot = state
            .resolve_configured_proxy_snapshot_with_tunnel_affinity(Some(&json!({
                "node_id": "proxy-node-stale",
                "enabled": true,
            })))
            .await;

        assert_eq!(snapshot, None);
    }

    #[tokio::test]
    async fn resolve_configured_proxy_snapshot_keeps_inline_url_when_stored_node_is_unroutable() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_tunnel_node(
            "proxy-node-stale",
        )]));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                repository,
            ));

        let snapshot = state
            .resolve_configured_proxy_snapshot_with_tunnel_affinity(Some(&json!({
                "node_id": "proxy-node-stale",
                "url": "http://proxy.example:8080",
                "enabled": true,
            })))
            .await
            .expect("inline proxy URL should still resolve");

        assert_eq!(snapshot.node_id.as_deref(), Some("proxy-node-stale"));
        assert_eq!(snapshot.url.as_deref(), Some("http://proxy.example:8080"));
    }

    fn sample_tunnel_node(id: &str) -> StoredProxyNode {
        StoredProxyNode::new(
            id.to_string(),
            id.to_string(),
            "127.0.0.1".to_string(),
            0,
            false,
            "online".to_string(),
            15,
            1,
            0,
            0,
            0,
            0,
            true,
            true,
            1,
        )
        .expect("sample tunnel node should build")
    }
}
