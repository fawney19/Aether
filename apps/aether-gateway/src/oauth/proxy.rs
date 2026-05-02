use crate::admin_api::AdminAppState;
use crate::AppState;
use aether_contracts::ProxySnapshot;
use aether_oauth::network::{OAuthNetworkContext, OAuthNetworkPolicy, OAuthTimeouts};

pub(crate) async fn resolve_identity_oauth_network_context(
    state: &AppState,
) -> OAuthNetworkContext {
    let proxy = state.resolve_system_proxy_snapshot().await;
    OAuthNetworkContext {
        policy: OAuthNetworkPolicy::DirectOrSystemProxy,
        requirement: aether_oauth::network::NetworkRequirement::Optional,
        timeouts: if proxy.is_some() {
            OAuthTimeouts::PROXY_DEFAULT
        } else {
            OAuthTimeouts::DIRECT_DEFAULT
        },
        proxy,
    }
}

pub(crate) async fn resolve_provider_oauth_operation_proxy_snapshot(
    state: &AdminAppState<'_>,
    temporary_proxy_node_id: Option<&str>,
    configured_proxies: &[Option<&serde_json::Value>],
) -> Option<ProxySnapshot> {
    if let Some(snapshot) = state
        .resolve_admin_proxy_node_snapshot(temporary_proxy_node_id)
        .await
    {
        return Some(snapshot);
    }

    for proxy in configured_proxies {
        if let Some(snapshot) = state
            .app()
            .resolve_configured_proxy_snapshot_with_tunnel_affinity(*proxy)
            .await
        {
            return Some(snapshot);
        }
    }
    state.app().resolve_system_proxy_snapshot().await
}
