use super::super::snapshot::GatewayProviderTransportSnapshot;
use super::super::{
    body_rules_are_locally_supported, header_rules_are_locally_supported,
    resolve_transport_tls_profile, supports_local_oauth_request_auth_resolution,
    transport_proxy_is_locally_supported,
};
use super::auth::supports_local_claude_code_auth;

pub fn local_claude_code_transport_unsupported_reason_with_network(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> Option<&'static str> {
    if !transport.provider.is_active || !transport.endpoint.is_active || !transport.key.is_active {
        return if !transport.provider.is_active {
            Some("provider_inactive")
        } else if !transport.endpoint.is_active {
            Some("endpoint_inactive")
        } else {
            Some("key_inactive")
        };
    }
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("claude_code")
    {
        return Some("transport_provider_type_unsupported");
    }
    if !transport
        .endpoint
        .api_format
        .trim()
        .eq_ignore_ascii_case(api_format.trim())
    {
        return Some("transport_api_format_mismatch");
    }
    if !header_rules_are_locally_supported(transport.endpoint.header_rules.as_ref()) {
        return Some("transport_header_rules_unsupported");
    }
    if !body_rules_are_locally_supported(transport.endpoint.body_rules.as_ref()) {
        return Some("transport_body_rules_unsupported");
    }
    if transport.key.decrypted_auth_config.is_some()
        && !supports_local_oauth_request_auth_resolution(transport)
    {
        return Some("transport_oauth_resolution_unsupported");
    }
    if !supports_local_claude_code_auth(transport) {
        return Some("transport_auth_unavailable");
    }
    if !transport_proxy_is_locally_supported(transport) {
        return Some("transport_proxy_unsupported");
    }
    if transport.key.fingerprint.is_some() && resolve_transport_tls_profile(transport).is_none() {
        return Some("transport_tls_profile_unsupported");
    }

    None
}

pub fn supports_local_claude_code_transport_with_network(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> bool {
    local_claude_code_transport_unsupported_reason_with_network(transport, api_format).is_none()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::super::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use super::{
        local_claude_code_transport_unsupported_reason_with_network,
        supports_local_claude_code_transport_with_network,
    };

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Claude Code".to_string(),
                provider_type: "claude_code".to_string(),
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
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: "bearer".to_string(),
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
                fingerprint: Some(json!({"tls_profile":"chrome_136"})),
                decrypted_api_key: "sk-ant-123".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn supports_claude_code_transport_when_auth_and_tls_are_valid() {
        assert!(supports_local_claude_code_transport_with_network(
            &sample_transport(),
            "claude:messages"
        ));
    }

    #[test]
    fn reports_auth_unavailable_for_claude_code_without_local_auth() {
        let mut transport = sample_transport();
        transport.key.auth_type = "api_key".to_string();
        transport.key.decrypted_api_key = "__placeholder__".to_string();

        assert_eq!(
            local_claude_code_transport_unsupported_reason_with_network(
                &transport,
                "claude:messages"
            ),
            Some("transport_auth_unavailable")
        );
    }
}
