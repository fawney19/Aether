use super::*;
use aether_contracts::ProxySnapshot;
use aether_data::repository::proxy_nodes::{proxy_node_accepts_new_tunnels, StoredProxyNode};
use aether_provider_transport::TransportTunnelAffinityLookup;
use serde_json::{json, Map, Value};
use url::Url;

const TUNNEL_BASE_URL_EXTRA_KEY: &str = "tunnel_base_url";
const TUNNEL_OWNER_INSTANCE_ID_EXTRA_KEY: &str = "tunnel_owner_instance_id";
const TUNNEL_OWNER_OBSERVED_AT_EXTRA_KEY: &str = "tunnel_owner_observed_at_unix_secs";

impl<'a> AdminAppState<'a> {
    pub(crate) async fn read_provider_transport_snapshot(
        &self,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> Result<Option<AdminGatewayProviderTransportSnapshot>, GatewayError> {
        self.app
            .read_provider_transport_snapshot(provider_id, endpoint_id, key_id)
            .await
    }

    pub(crate) async fn resolve_local_oauth_request_auth(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Result<Option<crate::provider_transport::LocalResolvedOAuthRequestAuth>, GatewayError>
    {
        self.app.resolve_local_oauth_request_auth(transport).await
    }

    pub(crate) async fn resolve_local_oauth_header_auth(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Result<Option<(String, String)>, GatewayError> {
        Ok(
            match self.resolve_local_oauth_request_auth(transport).await? {
                Some(crate::provider_transport::LocalResolvedOAuthRequestAuth::Header {
                    name,
                    value,
                }) => Some((name, value)),
                _ => None,
            },
        )
    }

    pub(crate) async fn resolve_local_oauth_kiro_request_auth(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Result<Option<AdminKiroRequestAuth>, GatewayError> {
        Ok(
            match self.resolve_local_oauth_request_auth(transport).await? {
                Some(crate::provider_transport::LocalResolvedOAuthRequestAuth::Kiro(auth)) => {
                    Some(auth)
                }
                _ => None,
            },
        )
    }

    pub(crate) fn resolve_local_antigravity_identity_headers(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Option<(String, BTreeMap<String, String>)> {
        match crate::provider_transport::antigravity::resolve_local_antigravity_request_auth(
            transport,
        ) {
            crate::provider_transport::antigravity::AntigravityRequestAuthSupport::Supported(
                auth,
            ) => Some((
                auth.project_id.clone(),
                crate::provider_transport::antigravity::build_antigravity_static_identity_headers(
                    &auth,
                ),
            )),
            crate::provider_transport::antigravity::AntigravityRequestAuthSupport::Unsupported(
                _,
            ) => None,
        }
    }

    pub(crate) async fn resolve_transport_proxy_snapshot_with_tunnel_affinity(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Option<aether_contracts::ProxySnapshot> {
        crate::provider_transport::resolve_transport_proxy_snapshot_with_tunnel_affinity(
            self.app, transport,
        )
        .await
    }

    pub(crate) fn fixed_provider_template(
        &self,
        provider_type: &str,
    ) -> Option<(&'static str, &'static [&'static str])> {
        crate::provider_transport::provider_types::fixed_provider_template(provider_type)
    }

    pub(crate) fn provider_type_is_fixed(&self, provider_type: &str) -> bool {
        crate::provider_transport::provider_types::provider_type_is_fixed(provider_type)
    }

    pub(crate) fn provider_type_enables_format_conversion_by_default(
        &self,
        provider_type: &str,
    ) -> bool {
        crate::provider_transport::provider_types::provider_type_enables_format_conversion_by_default(
            provider_type,
        )
    }

    pub(crate) async fn resolve_admin_connector_proxy_snapshot(
        &self,
        connector_config: Option<&Map<String, Value>>,
    ) -> Option<ProxySnapshot> {
        let explicit_node_id = connector_config
            .and_then(|config| admin_provider_transport_string_field(config, "proxy_node_id"));
        if let Some(snapshot) = self
            .resolve_admin_proxy_node_snapshot(explicit_node_id.as_deref())
            .await
        {
            return Some(snapshot);
        }

        if explicit_node_id.is_none() {
            let system_node_id = self
                .read_system_config_json_value("system_proxy_node_id")
                .await
                .ok()
                .flatten()
                .and_then(|value| value.as_str().map(str::trim).map(ToOwned::to_owned))
                .filter(|value| !value.is_empty());
            if let Some(snapshot) = self
                .resolve_admin_proxy_node_snapshot(system_node_id.as_deref())
                .await
            {
                return Some(snapshot);
            }
        }

        connector_config
            .and_then(|config| config.get("proxy"))
            .and_then(admin_provider_transport_legacy_proxy_snapshot)
    }

    pub(crate) async fn resolve_admin_proxy_node_snapshot(
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
            if let Ok(Some(owner)) = self.app().lookup_tunnel_attachment_owner(node_id).await {
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
            }
            return Some(ProxySnapshot {
                enabled: Some(true),
                mode: Some("tunnel".to_string()),
                node_id: Some(node_id.to_string()),
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
            mode: admin_provider_transport_proxy_mode(Some(proxy_url)),
            node_id: Some(node.id.clone()),
            label: Some(node.name.clone()),
            url: admin_provider_transport_proxy_url_with_node_auth(&node)
                .or_else(|| Some(proxy_url.to_string())),
            extra: None,
        })
    }

    pub(crate) fn supports_local_gemini_transport_with_network(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
        api_format: &str,
    ) -> bool {
        crate::provider_transport::policy::supports_local_gemini_transport_with_network(
            transport, api_format,
        )
    }

    pub(crate) fn resolve_local_gemini_auth(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Option<(String, String)> {
        crate::provider_transport::auth::resolve_local_gemini_auth(transport)
    }

    pub(crate) fn build_passthrough_headers_with_auth(
        &self,
        headers: &axum::http::HeaderMap,
        auth_header: &str,
        auth_value: &str,
        extra_headers: &BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        crate::provider_transport::auth::build_passthrough_headers_with_auth(
            headers,
            auth_header,
            auth_value,
            extra_headers,
        )
    }

    pub(crate) fn apply_local_header_rules(
        &self,
        headers: &mut BTreeMap<String, String>,
        rules: Option<&serde_json::Value>,
        protected_keys: &[&str],
        body: &serde_json::Value,
        original_body: Option<&serde_json::Value>,
    ) -> bool {
        crate::provider_transport::apply_local_header_rules(
            headers,
            rules,
            protected_keys,
            body,
            original_body,
        )
    }

    pub(crate) fn build_gemini_files_passthrough_url(
        &self,
        upstream_base_url: &str,
        path: &str,
        query: Option<&str>,
    ) -> Option<String> {
        crate::provider_transport::url::build_gemini_files_passthrough_url(
            upstream_base_url,
            path,
            query,
        )
    }

    pub(crate) fn resolve_transport_tls_profile(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Option<String> {
        crate::provider_transport::resolve_transport_tls_profile(transport)
    }

    pub(crate) fn resolve_transport_execution_timeouts(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Option<aether_contracts::ExecutionTimeouts> {
        crate::provider_transport::resolve_transport_execution_timeouts(transport)
    }

    pub(crate) fn build_passthrough_path_url(
        &self,
        upstream_base_url: &str,
        path: &str,
        query: Option<&str>,
        blocked_keys: &[&str],
    ) -> Option<String> {
        crate::provider_transport::url::build_passthrough_path_url(
            upstream_base_url,
            path,
            query,
            blocked_keys,
        )
    }

    pub(crate) fn build_claude_messages_url(
        &self,
        upstream_base_url: &str,
        query: Option<&str>,
    ) -> String {
        crate::provider_transport::url::build_claude_messages_url(upstream_base_url, query)
    }

    pub(crate) fn build_gemini_content_url(
        &self,
        upstream_base_url: &str,
        model: &str,
        stream: bool,
        query: Option<&str>,
    ) -> Option<String> {
        crate::provider_transport::url::build_gemini_content_url(
            upstream_base_url,
            model,
            stream,
            query,
        )
    }

    pub(crate) fn build_openai_chat_url(
        &self,
        upstream_base_url: &str,
        query: Option<&str>,
    ) -> String {
        crate::provider_transport::url::build_openai_chat_url(upstream_base_url, query)
    }
}

fn admin_provider_transport_legacy_proxy_snapshot(value: &Value) -> Option<ProxySnapshot> {
    match value {
        Value::String(proxy_url) => {
            let proxy_url = proxy_url.trim();
            if proxy_url.is_empty() {
                return None;
            }
            Some(ProxySnapshot {
                enabled: Some(true),
                mode: admin_provider_transport_proxy_mode(Some(proxy_url)),
                node_id: None,
                label: None,
                url: Some(proxy_url.to_string()),
                extra: None,
            })
        }
        Value::Object(object) => {
            if object.get("enabled").and_then(Value::as_bool) == Some(false) {
                return None;
            }
            let proxy_url = object
                .get("url")
                .or_else(|| object.get("proxy_url"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let username = object
                .get("username")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let password = object
                .get("password")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            Some(ProxySnapshot {
                enabled: Some(true),
                mode: object
                    .get("mode")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .or_else(|| admin_provider_transport_proxy_mode(Some(proxy_url))),
                node_id: None,
                label: object
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
                url: admin_provider_transport_inject_proxy_auth(proxy_url, username, password)
                    .or_else(|| Some(proxy_url.to_string())),
                extra: None,
            })
        }
        _ => None,
    }
}

fn admin_provider_transport_proxy_url_with_node_auth(node: &StoredProxyNode) -> Option<String> {
    let proxy_url = node
        .proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let username = node
        .proxy_username
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let password = node
        .proxy_password
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    admin_provider_transport_inject_proxy_auth(proxy_url, username, password)
}

fn admin_provider_transport_inject_proxy_auth(
    proxy_url: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Option<String> {
    let username = username.filter(|value| !value.is_empty())?;
    let mut parsed = Url::parse(proxy_url).ok()?;
    parsed.set_username(username).ok()?;
    parsed.set_password(password).ok()?;
    Some(parsed.to_string())
}

fn admin_provider_transport_proxy_mode(proxy_url: Option<&str>) -> Option<String> {
    proxy_url
        .and_then(|value| {
            Url::parse(value)
                .ok()
                .map(|parsed| parsed.scheme().to_string())
        })
        .or_else(|| {
            proxy_url.and_then(|value| {
                value
                    .split_once("://")
                    .map(|(scheme, _)| scheme.trim().to_ascii_lowercase())
                    .filter(|scheme| !scheme.is_empty())
            })
        })
}

fn admin_provider_transport_string_field(config: &Map<String, Value>, key: &str) -> Option<String> {
    config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}
