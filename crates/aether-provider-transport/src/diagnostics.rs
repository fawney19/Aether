use aether_ai_formats::protocol::matrix::{
    request_conversion_kind, request_conversion_requires_enable_flag,
};
use aether_contracts::ProxySnapshot;
use serde_json::{json, Map, Value};

use crate::conversion::{
    request_conversion_enabled_for_transport, request_conversion_transport_unsupported_reason,
    request_pair_allowed_for_transport,
};
use crate::network::{resolve_transport_tls_profile, transport_proxy_is_locally_supported};
use crate::policy::{
    local_gemini_transport_unsupported_reason_with_network,
    local_openai_chat_transport_unsupported_reason,
    local_standard_transport_unsupported_reason_with_network,
};
use crate::rules::{body_rules_are_locally_supported, header_rules_are_locally_supported};
use crate::same_format_provider::same_format_provider_transport_unsupported_reason_for_trace;
use crate::snapshot::GatewayProviderTransportSnapshot;

pub fn build_request_trace_proxy_value(
    transport: Option<&GatewayProviderTransportSnapshot>,
    resolved_proxy: Option<&ProxySnapshot>,
) -> Option<Value> {
    let resolved_proxy = resolved_proxy?;
    let mut object = Map::new();

    if let Some(node_id) = trimmed_non_empty(resolved_proxy.node_id.as_deref()) {
        object.insert("node_id".to_string(), Value::String(node_id));
    }
    if let Some(node_name) = trimmed_non_empty(resolved_proxy.label.as_deref()) {
        object.insert("node_name".to_string(), Value::String(node_name));
    }
    if let Some(url) = sanitize_trace_proxy_url(resolved_proxy.url.as_deref()) {
        object.insert("url".to_string(), Value::String(url));
    }
    if let Some(source) = resolve_request_trace_proxy_source(transport, true) {
        object.insert("source".to_string(), Value::String(source.to_string()));
    }

    (!object.is_empty()).then_some(Value::Object(object))
}

pub fn append_transport_diagnostics_to_value(
    value: Value,
    transport: Option<&GatewayProviderTransportSnapshot>,
    client_api_format: &str,
    provider_api_format: &str,
) -> Value {
    let Value::Object(mut object) = value else {
        return value;
    };
    object.insert(
        "transport_diagnostics".to_string(),
        transport
            .map(|transport| {
                build_transport_diagnostics(transport, client_api_format, provider_api_format)
            })
            .unwrap_or_else(|| json!({ "transport_snapshot_available": false })),
    );
    Value::Object(object)
}

pub fn build_transport_diagnostics(
    transport: &GatewayProviderTransportSnapshot,
    client_api_format: &str,
    provider_api_format: &str,
) -> Value {
    let resolved_tls_profile = resolve_transport_tls_profile(transport);
    let configured_tls_profile = transport
        .key
        .fingerprint
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|value| value.get("tls_profile"))
        .cloned()
        .unwrap_or(Value::Null);
    let has_oauth_config = transport.key.decrypted_auth_config.is_some();
    let oauth_resolution_supported =
        !has_oauth_config || crate::supports_local_oauth_request_auth_resolution(transport);
    let request_transport_unsupported_reason = resolve_request_transport_unsupported_reason(
        transport,
        client_api_format,
        provider_api_format,
    );

    json!({
        "transport_snapshot_available": true,
        "provider_type": transport.provider.provider_type,
        "provider_is_active": transport.provider.is_active,
        "endpoint_is_active": transport.endpoint.is_active,
        "key_is_active": transport.key.is_active,
        "provider_enable_format_conversion": transport.provider.enable_format_conversion,
        "provider_keep_priority_on_conversion": transport.provider.keep_priority_on_conversion,
        "endpoint_format_acceptance_config": transport.endpoint.format_acceptance_config,
        "endpoint_custom_path": transport.endpoint.custom_path,
        "header_rules": transport.endpoint.header_rules,
        "header_rules_supported": header_rules_are_locally_supported(transport.endpoint.header_rules.as_ref()),
        "body_rules": transport.endpoint.body_rules,
        "body_rules_supported": body_rules_are_locally_supported(transport.endpoint.body_rules.as_ref()),
        "proxy": {
            "locally_supported": transport_proxy_is_locally_supported(transport),
            "provider": summarize_proxy_config(transport.provider.proxy.as_ref()),
            "endpoint": summarize_proxy_config(transport.endpoint.proxy.as_ref()),
            "key": summarize_proxy_config(transport.key.proxy.as_ref()),
        },
        "auth": {
            "key_auth_type": transport.key.auth_type,
            "has_oauth_config": has_oauth_config,
            "oauth_request_auth_resolution_supported": oauth_resolution_supported,
        },
        "fingerprint": transport.key.fingerprint,
        "configured_tls_profile": configured_tls_profile,
        "resolved_tls_profile": resolved_tls_profile,
        "request_pair": {
            "client_api_format": client_api_format,
            "provider_api_format": provider_api_format,
            "requires_conversion_enable_flag": request_conversion_requires_enable_flag(
                client_api_format,
                provider_api_format,
            ),
            "conversion_enabled": request_conversion_enabled_for_transport(
                transport,
                client_api_format,
                provider_api_format,
            ),
            "pair_allowed": request_pair_allowed_for_transport(
                transport,
                client_api_format,
                provider_api_format,
            ),
            "transport_unsupported_reason": request_transport_unsupported_reason,
        },
    })
}

fn summarize_proxy_config(proxy: Option<&Value>) -> Value {
    let Some(object) = proxy.and_then(Value::as_object) else {
        return Value::Null;
    };
    let has_url = object
        .get("url")
        .or_else(|| object.get("proxy_url"))
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    json!({
        "enabled": object.get("enabled").cloned().unwrap_or(Value::Null),
        "mode": object.get("mode").cloned().unwrap_or(Value::Null),
        "node_id": object.get("node_id").cloned().unwrap_or(Value::Null),
        "label": object.get("label").cloned().unwrap_or(Value::Null),
        "has_url": has_url,
    })
}

fn resolve_request_trace_proxy_source(
    transport: Option<&GatewayProviderTransportSnapshot>,
    has_resolved_proxy: bool,
) -> Option<&'static str> {
    let transport = transport?;
    if transport_has_explicit_proxy(transport.key.proxy.as_ref()) {
        return Some("key");
    }
    if transport_has_explicit_proxy(transport.endpoint.proxy.as_ref()) {
        return Some("endpoint");
    }
    if transport_has_explicit_proxy(transport.provider.proxy.as_ref()) {
        return Some("provider");
    }
    has_resolved_proxy.then_some("system")
}

fn transport_has_explicit_proxy(proxy: Option<&Value>) -> bool {
    let Some(object) = proxy.and_then(Value::as_object) else {
        return false;
    };
    let enabled = object
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !enabled {
        return false;
    }

    object
        .get("node_id")
        .or_else(|| object.get("url"))
        .or_else(|| object.get("proxy_url"))
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn sanitize_trace_proxy_url(url: Option<&str>) -> Option<String> {
    let raw = url.map(str::trim).filter(|value| !value.is_empty())?;
    let parsed = url::Url::parse(raw).ok()?;
    let scheme = parsed.scheme().trim();
    let host = parsed.host_str()?.trim();
    if scheme.is_empty() || host.is_empty() {
        return None;
    }

    let mut safe = format!("{scheme}://{host}");
    if let Some(port) = parsed.port() {
        safe.push(':');
        safe.push_str(port.to_string().as_str());
    }
    Some(safe)
}

fn trimmed_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_request_transport_unsupported_reason(
    transport: &GatewayProviderTransportSnapshot,
    client_api_format: &str,
    provider_api_format: &str,
) -> Option<&'static str> {
    let client_api_format = client_api_format.trim().to_ascii_lowercase();
    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();
    if client_api_format == provider_api_format {
        if let Some(skip_reason) = same_format_provider_transport_unsupported_reason_for_trace(
            transport,
            provider_api_format.as_str(),
        ) {
            return Some(skip_reason);
        }
        return match provider_api_format.as_str() {
            "openai:chat" => local_openai_chat_transport_unsupported_reason(transport),
            "gemini:generate_content" => local_gemini_transport_unsupported_reason_with_network(
                transport,
                provider_api_format.as_str(),
            ),
            _ => local_standard_transport_unsupported_reason_with_network(
                transport,
                provider_api_format.as_str(),
            ),
        };
    }
    match request_conversion_kind(client_api_format.as_str(), provider_api_format.as_str()) {
        Some(kind) => request_conversion_transport_unsupported_reason(transport, kind),
        None => Some("transport_api_format_unsupported"),
    }
}

#[cfg(test)]
mod tests {
    use super::{build_request_trace_proxy_value, build_transport_diagnostics};
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use aether_contracts::ProxySnapshot;
    use serde_json::{json, Value};

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "RightCode".to_string(),
                provider_type: "codex".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: None,
                proxy: Some(json!({"enabled": true, "mode": "node", "node_id": "proxy-node-1"})),
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai:responses".to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://example.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: Some("/v1/responses".to_string()),
                config: None,
                format_acceptance_config: Some(json!({
                    "enabled": true,
                    "accept_formats": ["claude:messages"]
                })),
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "codex".to_string(),
                auth_type: "oauth".to_string(),
                is_active: true,
                api_formats: None,
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: Some(json!({
                    "tls_profile": "chrome_136",
                    "user_agent": "Mozilla/5.0"
                })),
                decrypted_api_key: "sk-test".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    fn sample_claude_code_transport_without_auth() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-cc-1".to_string(),
                name: "NekoCode".to_string(),
                provider_type: "claude_code".to_string(),
                website: Some("https://nekocode.ai".to_string()),
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-cc-1".to_string(),
                provider_id: "provider-cc-1".to_string(),
                api_format: "claude:messages".to_string(),
                api_family: Some("claude".to_string()),
                endpoint_kind: Some("cli".to_string()),
                is_active: true,
                base_url: "https://api.anthropic.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-cc-1".to_string(),
                provider_id: "provider-cc-1".to_string(),
                name: "CC".to_string(),
                auth_type: "api_key".to_string(),
                is_active: true,
                api_formats: Some(vec!["claude:messages".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "__placeholder__".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn transport_diagnostics_include_format_and_network_policy() {
        let diagnostics =
            build_transport_diagnostics(&sample_transport(), "claude:messages", "openai:responses");

        assert_eq!(diagnostics["provider_type"], "codex");
        assert_eq!(diagnostics["fingerprint"]["tls_profile"], "chrome_136");
        assert_eq!(diagnostics["resolved_tls_profile"], "chrome_136");
        assert_eq!(
            diagnostics["request_pair"]["conversion_enabled"],
            Value::Bool(true)
        );
        assert!(diagnostics["request_pair"]["transport_unsupported_reason"].is_null());
    }

    #[test]
    fn transport_diagnostics_use_provider_private_same_format_reason() {
        let diagnostics = build_transport_diagnostics(
            &sample_claude_code_transport_without_auth(),
            "claude:messages",
            "claude:messages",
        );

        assert_eq!(
            diagnostics["request_pair"]["transport_unsupported_reason"],
            Value::String("transport_auth_unavailable".to_string())
        );
    }

    #[test]
    fn request_trace_proxy_value_sanitizes_url_and_marks_config_source() {
        let transport = sample_transport();
        let proxy = ProxySnapshot {
            enabled: Some(true),
            mode: Some("node".to_string()),
            node_id: Some("proxy-node-1".to_string()),
            label: Some("Primary proxy".to_string()),
            url: Some("https://user:pass@example.test:9443/path".to_string()),
            extra: None,
        };

        let value = build_request_trace_proxy_value(Some(&transport), Some(&proxy)).unwrap();

        assert_eq!(value["node_id"], "proxy-node-1");
        assert_eq!(value["node_name"], "Primary proxy");
        assert_eq!(value["url"], "https://example.test:9443");
        assert_eq!(value["source"], "provider");
    }
}
