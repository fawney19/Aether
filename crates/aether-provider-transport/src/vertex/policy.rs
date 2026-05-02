use super::super::snapshot::GatewayProviderTransportSnapshot;
use super::super::{
    body_rules_are_locally_supported, header_rules_are_locally_supported,
    resolve_transport_tls_profile, transport_proxy_is_locally_supported,
};
use super::auth::resolve_local_vertex_api_key_query_auth;

fn is_vertex_transport_family(transport: &GatewayProviderTransportSnapshot) -> bool {
    transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(super::PROVIDER_TYPE)
        || super::looks_like_vertex_ai_host(&transport.endpoint.base_url)
}

pub fn local_vertex_api_key_gemini_transport_unsupported_reason_with_network(
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
    if aether_ai_formats::normalize_api_format_alias(&transport.endpoint.api_format)
        != "gemini:generate_content"
    {
        return Some("transport_api_format_mismatch");
    }
    if !is_vertex_transport_family(transport) {
        return Some("transport_provider_type_unsupported");
    }
    if !header_rules_are_locally_supported(transport.endpoint.header_rules.as_ref()) {
        return Some("transport_header_rules_unsupported");
    }
    if !body_rules_are_locally_supported(transport.endpoint.body_rules.as_ref()) {
        return Some("transport_body_rules_unsupported");
    }
    if resolve_local_vertex_api_key_query_auth(transport).is_none() {
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

pub fn supports_local_vertex_api_key_gemini_transport(
    transport: &GatewayProviderTransportSnapshot,
) -> bool {
    supports_local_vertex_api_key_same_format_transport(
        transport,
        &["gemini:generate_content"],
        false,
    )
}

pub fn supports_local_vertex_api_key_gemini_transport_with_network(
    transport: &GatewayProviderTransportSnapshot,
) -> bool {
    local_vertex_api_key_gemini_transport_unsupported_reason_with_network(transport).is_none()
}

pub fn supports_local_vertex_api_key_imagen_transport(
    transport: &GatewayProviderTransportSnapshot,
) -> bool {
    supports_local_vertex_api_key_same_format_transport(
        transport,
        &["gemini:generate_content"],
        false,
    )
}

pub fn supports_local_vertex_api_key_imagen_transport_with_network(
    transport: &GatewayProviderTransportSnapshot,
) -> bool {
    supports_local_vertex_api_key_same_format_transport(
        transport,
        &["gemini:generate_content"],
        true,
    )
}

fn supports_local_vertex_api_key_same_format_transport(
    transport: &GatewayProviderTransportSnapshot,
    api_formats: &[&str],
    allow_network_passthrough: bool,
) -> bool {
    if !transport.provider.is_active || !transport.endpoint.is_active || !transport.key.is_active {
        return false;
    }
    let endpoint_api_format =
        aether_ai_formats::normalize_api_format_alias(&transport.endpoint.api_format);
    if !api_formats
        .iter()
        .any(|api_format| endpoint_api_format.eq_ignore_ascii_case(api_format))
    {
        return false;
    }
    if !super::is_vertex_api_key_transport_context(transport) {
        return false;
    }
    if !header_rules_are_locally_supported(transport.endpoint.header_rules.as_ref())
        || !body_rules_are_locally_supported(transport.endpoint.body_rules.as_ref())
    {
        return false;
    }
    if resolve_local_vertex_api_key_query_auth(transport).is_none() {
        return false;
    }

    let has_custom_path = transport
        .endpoint
        .custom_path
        .as_deref()
        .is_some_and(|value: &str| !value.trim().is_empty());
    let has_tls_profile = resolve_transport_tls_profile(transport)
        .as_deref()
        .is_some_and(|value: &str| !value.trim().is_empty());
    if has_custom_path && !allow_network_passthrough {
        return false;
    }

    if allow_network_passthrough {
        if !transport_proxy_is_locally_supported(transport) {
            return false;
        }
        if transport.key.fingerprint.is_some() && resolve_transport_tls_profile(transport).is_none()
        {
            return false;
        }
    } else if transport.provider.proxy.is_some()
        || transport.endpoint.proxy.is_some()
        || transport.key.proxy.is_some()
        || has_tls_profile
    {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::super::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    use super::{
        local_vertex_api_key_gemini_transport_unsupported_reason_with_network,
        supports_local_vertex_api_key_gemini_transport,
        supports_local_vertex_api_key_gemini_transport_with_network,
    };

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Vertex".to_string(),
                provider_type: "vertex_ai".to_string(),
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
                api_format: "gemini:generate_content".to_string(),
                api_family: Some("gemini".to_string()),
                endpoint_kind: Some("generate_content".to_string()),
                is_active: true,
                base_url: "https://aiplatform.googleapis.com".to_string(),
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
                api_formats: Some(vec!["gemini:generate_content".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,

                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "vertex-secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn supports_vertex_api_key_same_format_subset() {
        assert!(supports_local_vertex_api_key_gemini_transport(
            &sample_transport()
        ));
    }

    #[test]
    fn supports_vertex_api_key_gemini_generate_content_subset() {
        let mut transport = sample_transport();
        transport.endpoint.api_format = "gemini:generate_content".to_string();
        assert!(supports_local_vertex_api_key_gemini_transport(&transport));
    }

    #[test]
    fn supports_custom_aiplatform_gemini_generate_content_subset() {
        let mut transport = sample_transport();
        transport.provider.provider_type = "custom".to_string();
        transport.endpoint.api_format = "gemini:generate_content".to_string();
        assert!(supports_local_vertex_api_key_gemini_transport(&transport));
    }

    #[test]
    fn rejects_vertex_service_account_subset() {
        let mut transport = sample_transport();
        transport.key.auth_type = "service_account".to_string();
        transport.key.decrypted_auth_config = Some("{\"project_id\":\"demo-project\"}".to_string());
        assert!(!supports_local_vertex_api_key_gemini_transport(&transport));
    }

    #[test]
    fn allows_network_passthrough_for_custom_path_with_local_proxy_support() {
        let mut transport = sample_transport();
        transport.endpoint.custom_path =
            Some("/v1/publishers/google/models/gemini-2.5-pro:generateContent".to_string());
        transport.key.proxy = Some(json!({"url":"http://proxy.example:8080"}));
        transport.key.fingerprint = Some(json!({"tls_profile":"chrome_136"}));
        assert!(!supports_local_vertex_api_key_gemini_transport(&transport));
        assert!(supports_local_vertex_api_key_gemini_transport_with_network(
            &transport
        ));
    }

    #[test]
    fn reports_auth_unavailable_when_vertex_api_key_query_auth_is_missing() {
        let mut transport = sample_transport();
        transport.key.auth_type = "service_account".to_string();
        transport.key.decrypted_api_key = "__placeholder__".to_string();

        assert_eq!(
            local_vertex_api_key_gemini_transport_unsupported_reason_with_network(&transport),
            Some("transport_auth_unavailable")
        );
    }
}
