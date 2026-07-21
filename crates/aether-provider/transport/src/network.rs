use aether_contracts::{
    ExecutionTimeouts, ProxySnapshot, ResolvedTransportProfile, TRANSPORT_BACKEND_REQWEST_RUSTLS,
    TRANSPORT_HTTP_MODE_AUTO, TRANSPORT_POOL_SCOPE_KEY,
};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use tracing::warn;

use crate::grok::grok_browser_resolved_transport_profile_from_auth_config;

use super::snapshot::GatewayProviderTransportSnapshot;

const TUNNEL_BASE_URL_EXTRA_KEY: &str = "tunnel_base_url";
const TUNNEL_OWNER_INSTANCE_ID_EXTRA_KEY: &str = "tunnel_owner_instance_id";
const TUNNEL_OWNER_OBSERVED_AT_EXTRA_KEY: &str = "tunnel_owner_observed_at_unix_secs";
const DEFAULT_PROVIDER_STREAM_FIRST_BYTE_TIMEOUT_SECS: f64 = 30.0;

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
    Some(ExecutionTimeouts {
        total_ms: transport
            .provider
            .request_timeout_secs
            .filter(|value| value.is_finite() && *value > 0.0)
            .map(timeout_secs_to_ms),
        first_byte_ms: Some(timeout_secs_to_ms(
            transport
                .provider
                .stream_first_byte_timeout_secs
                .filter(|value| value.is_finite() && *value > 0.0)
                .unwrap_or(DEFAULT_PROVIDER_STREAM_FIRST_BYTE_TIMEOUT_SECS),
        )),
        ..ExecutionTimeouts::default()
    })
}

fn timeout_secs_to_ms(secs: f64) -> u64 {
    ((secs * 1000.0).round() as u64).max(1)
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

pub fn resolve_transport_profile_id(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<String> {
    resolve_transport_profile(transport).map(|profile| profile.profile_id)
}

pub fn resolve_transport_profile(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<ResolvedTransportProfile> {
    resolve_transport_profile_from_fingerprint(transport.key.fingerprint.as_ref()).or_else(|| {
        resolve_transport_profile_from_provider_config(transport.provider.config.as_ref())
            .or_else(|| resolve_grok_browser_transport_profile(transport))
    })
}

fn resolve_grok_browser_transport_profile(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<ResolvedTransportProfile> {
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("grok")
    {
        return None;
    }
    let auth_config = transport
        .key
        .decrypted_auth_config
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str::<Value>(value).ok())?;
    let object = auth_config.as_object()?;
    let has_session = json_string_field(object, "sso_token")
        .or_else(|| json_string_field(object, "access_token"))
        .or_else(|| json_string_field(object, "token"))
        .is_some();
    if !has_session {
        return None;
    }
    grok_browser_resolved_transport_profile_from_auth_config(object, "grok_auth_config")
}

fn resolve_transport_profile_from_provider_config(
    config: Option<&Value>,
) -> Option<ResolvedTransportProfile> {
    let fingerprint = config?.get("fingerprint");
    resolve_transport_profile_from_fingerprint(fingerprint)
}

fn resolve_transport_profile_from_fingerprint(
    fingerprint: Option<&Value>,
) -> Option<ResolvedTransportProfile> {
    let fingerprint = fingerprint?;
    fingerprint
        .get("transport_profile")
        .and_then(parse_transport_profile_value)
}

pub fn transport_profile_is_configured(transport: &GatewayProviderTransportSnapshot) -> bool {
    transport_profile_configured_in_fingerprint(transport.key.fingerprint.as_ref())
        || transport_profile_configured_in_provider_config(transport.provider.config.as_ref())
}

fn transport_profile_configured_in_provider_config(config: Option<&Value>) -> bool {
    let fingerprint = config.and_then(|value| value.get("fingerprint"));
    transport_profile_configured_in_fingerprint(fingerprint)
}

fn transport_profile_configured_in_fingerprint(fingerprint: Option<&Value>) -> bool {
    fingerprint
        .and_then(|value| value.get("transport_profile"))
        .is_some_and(|value| !value.is_null())
}

fn parse_transport_profile_value(value: &Value) -> Option<ResolvedTransportProfile> {
    if let Some(profile_id) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(ResolvedTransportProfile {
            profile_id: profile_id.to_string(),
            backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.to_string(),
            http_mode: TRANSPORT_HTTP_MODE_AUTO.to_string(),
            pool_scope: TRANSPORT_POOL_SCOPE_KEY.to_string(),
            header_fingerprint: None,
            extra: None,
        });
    }

    let object = value.as_object()?;
    let profile_id = json_string_field(object, "profile_id")
        .or_else(|| json_string_field(object, "id"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let backend = json_string_field(object, "backend")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| TRANSPORT_BACKEND_REQWEST_RUSTLS.to_string());
    let http_mode = json_string_field(object, "http_mode")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| TRANSPORT_HTTP_MODE_AUTO.to_string());
    let pool_scope = json_string_field(object, "pool_scope")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| TRANSPORT_POOL_SCOPE_KEY.to_string());
    let header_fingerprint = object.get("header_fingerprint").cloned();
    let extra = object.get("extra").cloned();

    Some(ResolvedTransportProfile {
        profile_id,
        backend,
        http_mode,
        pool_scope,
        header_fingerprint,
        extra,
    })
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
        resolve_transport_execution_timeouts, resolve_transport_profile,
        resolve_transport_profile_id, resolve_transport_proxy_snapshot,
        resolve_transport_proxy_snapshot_with_tunnel_affinity, transport_profile_is_configured,
        transport_proxy_is_locally_supported, TransportTunnelAffinityLookup,
        TransportTunnelAttachmentOwner,
    };
    use aether_contracts::TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE;

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
                fingerprint: Some(json!({"transport_profile":"chrome_136"})),
                upstream_metadata: None,
                decrypted_api_key: "sk-test".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn transport_execution_timeouts_use_provider_defaults_when_unset() {
        let transport = sample_transport();

        let timeouts = resolve_transport_execution_timeouts(&transport)
            .expect("default provider timeouts should resolve");

        assert_eq!(timeouts.total_ms, None);
        assert_eq!(timeouts.first_byte_ms, Some(30_000));
    }

    #[test]
    fn transport_execution_timeouts_preserve_configured_values_independently() {
        let mut transport = sample_transport();
        transport.provider.request_timeout_secs = Some(12.0);

        let timeouts = resolve_transport_execution_timeouts(&transport)
            .expect("provider timeouts should resolve");

        assert_eq!(timeouts.total_ms, Some(12_000));
        assert_eq!(timeouts.first_byte_ms, Some(30_000));
    }

    #[test]
    fn transport_execution_timeouts_preserve_the_configurable_maximum() {
        let mut transport = sample_transport();
        transport.provider.request_timeout_secs =
            Some(aether_contracts::MAX_EXECUTION_REQUEST_TIMEOUT_SECS as f64);

        let timeouts = resolve_transport_execution_timeouts(&transport)
            .expect("provider timeouts should resolve");

        assert_eq!(
            timeouts.total_ms,
            Some(aether_contracts::MAX_EXECUTION_REQUEST_TIMEOUT_MS)
        );
    }

    #[test]
    fn transport_execution_timeouts_preserve_configured_first_byte_value() {
        let mut transport = sample_transport();
        transport.provider.stream_first_byte_timeout_secs = Some(7.5);

        let timeouts = resolve_transport_execution_timeouts(&transport)
            .expect("provider timeouts should resolve");

        assert_eq!(timeouts.total_ms, None);
        assert_eq!(timeouts.first_byte_ms, Some(7_500));
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
    fn resolves_transport_profile_id_from_key_fingerprint() {
        assert_eq!(
            resolve_transport_profile_id(&sample_transport()).as_deref(),
            Some("chrome_136")
        );
        assert!(transport_proxy_is_locally_supported(&sample_transport()));
    }

    #[test]
    fn resolves_transport_profile_from_key_fingerprint_before_provider_default() {
        let mut transport = sample_transport();
        transport.provider.config = Some(json!({
            "fingerprint": {"transport_profile": "provider_profile"}
        }));
        transport.key.fingerprint = Some(json!({
            "transport_profile": {
                "profile_id": "key_profile",
                "backend": "reqwest_rustls",
                "http_mode": "http1_only"
            }
        }));

        let profile = resolve_transport_profile(&transport).expect("profile");

        assert_eq!(profile.profile_id, "key_profile");
        assert_eq!(profile.backend, "reqwest_rustls");
        assert_eq!(profile.http_mode, "http1_only");
        assert_eq!(profile.pool_scope, "key");
    }

    #[test]
    fn resolves_transport_profile_from_provider_default() {
        let mut transport = sample_transport();
        transport.key.fingerprint = None;
        transport.provider.config = Some(json!({
            "fingerprint": {"transport_profile": "provider_profile"}
        }));

        let profile = resolve_transport_profile(&transport).expect("profile");

        assert_eq!(profile.profile_id, "provider_profile");
        assert_eq!(profile.backend, "reqwest_rustls");
    }

    #[test]
    fn maps_string_transport_profile_to_resolved_profile() {
        let profile = resolve_transport_profile(&sample_transport()).expect("profile");

        assert_eq!(profile.profile_id, "chrome_136");
        assert_eq!(profile.backend, "reqwest_rustls");
        assert_eq!(profile.http_mode, "auto");
        assert_eq!(profile.pool_scope, "key");
    }

    #[test]
    fn resolves_h2c_prior_knowledge_transport_profile() {
        let mut transport = sample_transport();
        transport.key.fingerprint = Some(json!({
            "transport_profile": {
                "profile_id": "mock-h2c",
                "backend": "reqwest_rustls",
                "http_mode": "h2c_prior_knowledge"
            }
        }));

        let profile = resolve_transport_profile(&transport).expect("profile");

        assert_eq!(profile.profile_id, "mock-h2c");
        assert_eq!(profile.http_mode, TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE);
    }

    #[test]
    fn resolves_no_transport_profile_without_fingerprint_configuration() {
        let mut transport = sample_transport();
        transport.key.fingerprint = None;
        transport.provider.config = None;

        assert!(resolve_transport_profile(&transport).is_none());
        assert!(!transport_profile_is_configured(&transport));
    }

    #[test]
    fn resolves_grok_browser_transport_profile_from_session_auth_config() {
        let mut transport = sample_transport();
        transport.provider.provider_type = "grok".to_string();
        transport.key.fingerprint = None;
        transport.provider.config = None;
        transport.key.decrypted_auth_config = Some(
            json!({
                "sso_token": "sso-token",
                "browser_profile": "chrome136",
                "cf_clearance": "clearance"
            })
            .to_string(),
        );

        let profile = resolve_transport_profile(&transport).expect("profile");

        assert_eq!(profile.profile_id, "chrome136");
        assert_eq!(profile.backend, "browser_wreq");
        assert_eq!(profile.http_mode, "auto");
        assert_eq!(profile.pool_scope, "key");
        assert_eq!(
            profile
                .extra
                .as_ref()
                .and_then(|value| value.get("browser_profile"))
                .and_then(Value::as_str),
            Some("chrome136")
        );
    }

    #[test]
    fn resolves_grok_browser_transport_profile_default_from_session_auth_config() {
        let mut transport = sample_transport();
        transport.provider.provider_type = "grok".to_string();
        transport.key.fingerprint = None;
        transport.provider.config = None;
        transport.key.decrypted_auth_config = Some(
            json!({
                "sso_token": "sso-token"
            })
            .to_string(),
        );

        let profile = resolve_transport_profile(&transport).expect("profile");

        assert_eq!(profile.profile_id, "chrome136");
        assert_eq!(profile.backend, "browser_wreq");
        assert_eq!(
            profile
                .extra
                .as_ref()
                .and_then(|value| value.get("source"))
                .and_then(Value::as_str),
            Some("grok_auth_config")
        );
    }

    #[test]
    fn resolves_grok_browser_transport_profile_normalizes_auth_config_alias() {
        let mut transport = sample_transport();
        transport.provider.provider_type = "grok".to_string();
        transport.key.fingerprint = None;
        transport.provider.config = None;
        transport.key.decrypted_auth_config = Some(
            json!({
                "sso_token": "sso-token",
                "browser_profile": "Chrome-137"
            })
            .to_string(),
        );

        let profile = resolve_transport_profile(&transport).expect("profile");

        assert_eq!(profile.profile_id, "chrome137");
        assert_eq!(
            profile
                .extra
                .as_ref()
                .and_then(|value| value.get("browser_profile"))
                .and_then(Value::as_str),
            Some("chrome137")
        );
    }

    #[test]
    fn resolves_grok_browser_transport_profile_from_legacy_user_agent() {
        let mut transport = sample_transport();
        transport.provider.provider_type = "grok".to_string();
        transport.key.fingerprint = None;
        transport.provider.config = None;
        transport.key.decrypted_auth_config = Some(
            json!({
                "sso_token": "sso-token",
                "user_agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36"
            })
            .to_string(),
        );

        let profile = resolve_transport_profile(&transport).expect("profile");

        assert_eq!(profile.profile_id, "chrome137");
        assert_eq!(
            profile
                .extra
                .as_ref()
                .and_then(|value| value.get("browser_profile"))
                .and_then(Value::as_str),
            Some("chrome137")
        );
    }

    #[test]
    fn key_fingerprint_wins_over_grok_auth_config_fallback() {
        let mut transport = sample_transport();
        transport.provider.provider_type = "grok".to_string();
        transport.provider.config = None;
        transport.key.fingerprint = Some(json!({
            "transport_profile": {
                "profile_id": "chrome136",
                "backend": "browser_wreq",
                "extra": {"browser_profile": "chrome136", "source": "key"}
            }
        }));
        transport.key.decrypted_auth_config = Some(
            json!({
                "sso_token": "sso-token",
                "browser_profile": "chrome137"
            })
            .to_string(),
        );

        let profile = resolve_transport_profile(&transport).expect("profile");

        assert_eq!(profile.profile_id, "chrome136");
        assert_eq!(
            profile
                .extra
                .as_ref()
                .and_then(|value| value.get("source"))
                .and_then(Value::as_str),
            Some("key")
        );
    }

    #[test]
    fn provider_fingerprint_wins_over_grok_auth_config_fallback() {
        let mut transport = sample_transport();
        transport.provider.provider_type = "grok".to_string();
        transport.key.fingerprint = None;
        transport.provider.config = Some(json!({
            "fingerprint": {
                "transport_profile": {
                    "profile_id": "chrome136",
                    "backend": "browser_wreq",
                    "extra": {"browser_profile": "chrome136", "source": "provider"}
                }
            }
        }));
        transport.key.decrypted_auth_config = Some(
            json!({
                "sso_token": "sso-token",
                "browser_profile": "chrome137"
            })
            .to_string(),
        );

        let profile = resolve_transport_profile(&transport).expect("profile");

        assert_eq!(profile.profile_id, "chrome136");
        assert_eq!(
            profile
                .extra
                .as_ref()
                .and_then(|value| value.get("source"))
                .and_then(Value::as_str),
            Some("provider")
        );
    }

    #[test]
    fn rejects_unsupported_grok_auth_config_browser_profile() {
        let mut transport = sample_transport();
        transport.provider.provider_type = "grok".to_string();
        transport.key.fingerprint = None;
        transport.provider.config = None;
        transport.key.decrypted_auth_config = Some(
            json!({
                "sso_token": "sso-token",
                "browser_profile": "safari999"
            })
            .to_string(),
        );

        assert!(resolve_transport_profile(&transport).is_none());
    }

    #[test]
    fn rejects_unsupported_grok_auth_config_user_agent_profile() {
        let mut transport = sample_transport();
        transport.provider.provider_type = "grok".to_string();
        transport.key.fingerprint = None;
        transport.provider.config = None;
        transport.key.decrypted_auth_config = Some(
            json!({
                "sso_token": "sso-token",
                "user_agent": "Mozilla/5.0 Version/18.0 Safari/605.1.15"
            })
            .to_string(),
        );

        assert!(resolve_transport_profile(&transport).is_none());
    }
}
