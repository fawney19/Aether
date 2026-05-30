use super::provider_types::{
    provider_type_supports_local_embedding_transport,
    provider_type_supports_local_openai_chat_transport,
    provider_type_supports_local_same_format_transport,
};
use super::snapshot::GatewayProviderTransportSnapshot;
use super::{
    body_rules_are_locally_supported, header_rules_are_locally_supported,
    resolve_transport_profile, supports_local_oauth_request_auth_resolution,
    transport_profile_is_configured, transport_proxy_is_locally_supported,
};

pub fn supports_local_openai_chat_transport(transport: &GatewayProviderTransportSnapshot) -> bool {
    local_openai_chat_transport_unsupported_reason(transport).is_none()
}

pub fn supports_local_standard_transport(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> bool {
    local_standard_transport_unsupported_reason(transport, api_format).is_none()
}

pub fn supports_local_gemini_transport(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> bool {
    local_gemini_transport_unsupported_reason(transport, api_format).is_none()
}

pub fn supports_local_standard_transport_with_network(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> bool {
    local_standard_transport_unsupported_reason_with_network(transport, api_format).is_none()
}

pub fn supports_local_gemini_transport_with_network(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> bool {
    local_gemini_transport_unsupported_reason_with_network(transport, api_format).is_none()
}

pub fn local_openai_chat_transport_unsupported_reason(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<&'static str> {
    if !transport.provider.is_active {
        return Some("provider_inactive");
    }
    if !transport.endpoint.is_active {
        return Some("endpoint_inactive");
    }
    if !transport.key.is_active {
        return Some("key_inactive");
    }
    if !transport
        .endpoint
        .api_format
        .trim()
        .eq_ignore_ascii_case("openai:chat")
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
    if !transport_proxy_is_locally_supported(transport) {
        return Some("transport_proxy_unsupported");
    }
    if transport_profile_is_configured(transport) && resolve_transport_profile(transport).is_none()
    {
        return Some("transport_profile_unsupported");
    }
    if !provider_type_supports_local_openai_chat_transport(&transport.provider.provider_type) {
        return Some("transport_provider_type_unsupported");
    }

    None
}

pub fn local_standard_transport_unsupported_reason(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> Option<&'static str> {
    local_same_format_transport_unsupported_reason(
        transport,
        api_format,
        false,
        provider_type_supports_local_same_format_transport,
    )
}

pub fn local_gemini_transport_unsupported_reason(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> Option<&'static str> {
    local_same_format_transport_unsupported_reason(
        transport,
        api_format,
        false,
        provider_type_supports_local_same_format_transport,
    )
}

pub fn local_standard_transport_unsupported_reason_with_network(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> Option<&'static str> {
    local_same_format_transport_unsupported_reason(
        transport,
        api_format,
        true,
        provider_type_supports_local_same_format_transport,
    )
}

pub fn local_gemini_transport_unsupported_reason_with_network(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> Option<&'static str> {
    local_same_format_transport_unsupported_reason(
        transport,
        api_format,
        true,
        provider_type_supports_local_same_format_transport,
    )
}

fn local_same_format_transport_unsupported_reason(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
    allow_network_passthrough: bool,
    provider_type_supported: fn(&str) -> bool,
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
    if !same_api_format(&transport.endpoint.api_format, api_format) {
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
    let has_custom_path = transport
        .endpoint
        .custom_path
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    if has_custom_path && !allow_network_passthrough {
        return Some("transport_custom_path_unsupported");
    }
    if allow_network_passthrough {
        if !transport_proxy_is_locally_supported(transport) {
            return Some("transport_proxy_unsupported");
        }
        if transport_profile_is_configured(transport)
            && resolve_transport_profile(transport).is_none()
        {
            return Some("transport_profile_unsupported");
        }
    } else if transport.provider.proxy.is_some()
        || transport.endpoint.proxy.is_some()
        || transport.key.proxy.is_some()
        || transport_profile_is_configured(transport)
    {
        return Some("transport_proxy_or_profile_unsupported");
    }

    if !provider_type_supported(&transport.provider.provider_type) {
        return Some("transport_provider_type_unsupported");
    }
    if aether_ai_formats::is_embedding_api_format(api_format) {
        if !endpoint_kind_allows_embedding(transport.endpoint.endpoint_kind.as_deref()) {
            return Some("transport_endpoint_kind_unsupported");
        }
        if !provider_type_supports_local_embedding_transport(
            &transport.provider.provider_type,
            api_format,
        ) {
            return Some("transport_provider_type_unsupported");
        }
    }
    if aether_ai_formats::is_rerank_api_format(api_format) {
        if !endpoint_kind_allows_rerank(transport.endpoint.endpoint_kind.as_deref()) {
            return Some("transport_endpoint_kind_unsupported");
        }
        if !provider_type_supports_local_embedding_transport(
            &transport.provider.provider_type,
            api_format,
        ) {
            return Some("transport_provider_type_unsupported");
        }
    }

    None
}

fn endpoint_kind_allows_embedding(endpoint_kind: Option<&str>) -> bool {
    endpoint_kind
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "embedding" | "embeddings"
            )
        })
        .unwrap_or(true)
}

fn endpoint_kind_allows_rerank(endpoint_kind: Option<&str>) -> bool {
    endpoint_kind
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "rerank" | "reranking"))
        .unwrap_or(true)
}

fn same_api_format(left: &str, right: &str) -> bool {
    aether_ai_formats::api_format_alias_matches(left, right)
}

#[cfg(test)]
mod tests {
    use super::local_standard_transport_unsupported_reason_with_network;
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport(
        provider_type: &str,
        api_format: &str,
        endpoint_kind: Option<&str>,
    ) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "provider".to_string(),
                provider_type: provider_type.to_string(),
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
                api_format: api_format.to_string(),
                api_family: None,
                endpoint_kind: endpoint_kind.map(ToOwned::to_owned),
                is_active: true,
                base_url: "https://provider.example".to_string(),
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
                proxy: None,
                fingerprint: None,
                upstream_metadata: None,
                decrypted_api_key: "sk-test".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn rejects_unsupported_embedding_provider_format_pairs() {
        let openai_on_gemini = sample_transport("openai", "gemini:embedding", Some("embedding"));
        let gemini_on_openai = sample_transport("gemini", "openai:embedding", Some("embedding"));
        let chat_marked_embedding = sample_transport("openai", "openai:embedding", Some("chat"));

        assert_eq!(
            local_standard_transport_unsupported_reason_with_network(
                &openai_on_gemini,
                "gemini:embedding"
            ),
            Some("transport_provider_type_unsupported")
        );
        assert_eq!(
            local_standard_transport_unsupported_reason_with_network(
                &gemini_on_openai,
                "openai:embedding"
            ),
            Some("transport_provider_type_unsupported")
        );
        assert_eq!(
            local_standard_transport_unsupported_reason_with_network(
                &chat_marked_embedding,
                "openai:embedding"
            ),
            Some("transport_endpoint_kind_unsupported")
        );
    }

    #[test]
    fn accepts_supported_embedding_provider_format_pairs() {
        for (provider_type, api_format) in [
            ("openai", "openai:embedding"),
            ("gemini", "gemini:embedding"),
            ("google", "gemini:embedding"),
            ("jina", "jina:embedding"),
            ("doubao", "doubao:embedding"),
            ("volcengine", "doubao:embedding"),
            ("custom", "openai:embedding"),
            ("custom", "gemini:embedding"),
            ("custom", "jina:embedding"),
            ("custom", "doubao:embedding"),
        ] {
            let transport = sample_transport(provider_type, api_format, Some("embedding"));
            assert_eq!(
                local_standard_transport_unsupported_reason_with_network(&transport, api_format),
                None,
                "{provider_type} should support {api_format}"
            );
        }
    }

    #[test]
    fn embedding_policy_accepts_embedding_endpoint_kind_aliases_only() {
        for endpoint_kind in [None, Some(""), Some(" embedding "), Some("EMBEDDINGS")] {
            let transport = sample_transport("openai", "openai:embedding", endpoint_kind);
            assert_eq!(
                local_standard_transport_unsupported_reason_with_network(
                    &transport,
                    "openai:embedding"
                ),
                None,
                "endpoint kind {endpoint_kind:?} should be accepted"
            );
        }

        for endpoint_kind in [Some("chat"), Some("responses"), Some("image")] {
            let transport = sample_transport("openai", "openai:embedding", endpoint_kind);
            assert_eq!(
                local_standard_transport_unsupported_reason_with_network(
                    &transport,
                    "openai:embedding"
                ),
                Some("transport_endpoint_kind_unsupported"),
                "endpoint kind {endpoint_kind:?} should fail closed"
            );
        }
    }
}
