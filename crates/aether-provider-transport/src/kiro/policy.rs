use super::super::snapshot::GatewayProviderTransportSnapshot;
use super::super::{resolve_transport_tls_profile, transport_proxy_is_locally_supported};
use super::{
    body_rules_are_locally_supported, header_rules_are_locally_supported,
    supports_local_kiro_request_auth_resolution, supports_local_kiro_request_shape, PROVIDER_TYPE,
};

pub fn local_kiro_request_transport_unsupported_reason_with_network(
    transport: &GatewayProviderTransportSnapshot,
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
        .eq_ignore_ascii_case(PROVIDER_TYPE)
    {
        return Some("transport_provider_type_unsupported");
    }
    if !aether_ai_formats::api_format_alias_matches(
        &transport.endpoint.api_format,
        "claude:messages",
    ) {
        return Some("transport_api_format_mismatch");
    }
    if !header_rules_are_locally_supported(transport.endpoint.header_rules.as_ref()) {
        return Some("transport_header_rules_unsupported");
    }
    if !body_rules_are_locally_supported(transport.endpoint.body_rules.as_ref()) {
        return Some("transport_body_rules_unsupported");
    }
    if transport.key.decrypted_auth_config.is_some()
        && !supports_local_kiro_request_auth_resolution(transport)
    {
        return Some("transport_oauth_resolution_unsupported");
    }
    if !supports_local_kiro_request_auth_resolution(transport) {
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

pub fn supports_local_kiro_request_transport(transport: &GatewayProviderTransportSnapshot) -> bool {
    if !transport.provider.is_active || !transport.endpoint.is_active || !transport.key.is_active {
        return false;
    }
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(PROVIDER_TYPE)
    {
        return false;
    }
    if !aether_ai_formats::api_format_alias_matches(
        &transport.endpoint.api_format,
        "claude:messages",
    ) {
        return false;
    }
    supports_local_kiro_request_shape(
        transport.endpoint.header_rules.as_ref(),
        transport.endpoint.body_rules.as_ref(),
    ) && supports_local_kiro_request_auth_resolution(transport)
}

pub fn supports_local_kiro_request_transport_with_network(
    transport: &GatewayProviderTransportSnapshot,
) -> bool {
    local_kiro_request_transport_unsupported_reason_with_network(transport).is_none()
}

#[cfg(test)]
mod tests {
    use super::super::super::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use super::{
        local_kiro_request_transport_unsupported_reason_with_network,
        supports_local_kiro_request_transport, supports_local_kiro_request_transport_with_network,
    };

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Kiro".to_string(),
                provider_type: "kiro".to_string(),
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
                endpoint_kind: Some("messages".to_string()),
                is_active: true,
                base_url: "https://kiro.example".to_string(),
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
                fingerprint: None,
                decrypted_api_key: "__placeholder__".to_string(),
                decrypted_auth_config: Some(
                    r#"{
                        "access_token":"cached-token",
                        "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr",
                        "machine_id":"123e4567-e89b-12d3-a456-426614174000"
                    }"#
                    .to_string(),
                ),
            },
        }
    }

    #[test]
    fn supports_kiro_request_transport_when_cached_access_token_exists() {
        assert!(supports_local_kiro_request_transport(&sample_transport()));
        assert!(supports_local_kiro_request_transport_with_network(
            &sample_transport()
        ));
    }

    #[test]
    fn supports_kiro_request_transport_when_refresh_only_auth_exists() {
        let mut transport = sample_transport();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config = Some(
            r#"{
                "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr"
            }"#
            .to_string(),
        );

        assert!(supports_local_kiro_request_transport(&transport));
        assert!(supports_local_kiro_request_transport_with_network(
            &transport
        ));
    }

    #[test]
    fn reports_auth_unavailable_when_kiro_key_cannot_resolve_request_auth() {
        let mut transport = sample_transport();
        transport.key.auth_type = "api_key".to_string();
        transport.key.decrypted_auth_config = None;

        assert_eq!(
            local_kiro_request_transport_unsupported_reason_with_network(&transport),
            Some("transport_auth_unavailable")
        );
    }
}
