use super::provider_types::{
    provider_type_supports_local_openai_chat_transport,
    provider_type_supports_local_same_format_transport,
};
use super::snapshot::GatewayProviderTransportSnapshot;
use super::{
    body_rules_are_locally_supported, header_rules_are_locally_supported,
    resolve_transport_tls_profile, supports_local_oauth_request_auth_resolution,
    transport_proxy_is_locally_supported,
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
    if transport.key.fingerprint.is_some() && resolve_transport_tls_profile(transport).is_none() {
        return Some("transport_tls_profile_unsupported");
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
        if transport.key.fingerprint.is_some() && resolve_transport_tls_profile(transport).is_none()
        {
            return Some("transport_tls_profile_unsupported");
        }
    } else if transport.provider.proxy.is_some()
        || transport.endpoint.proxy.is_some()
        || transport.key.proxy.is_some()
        || transport
            .key
            .fingerprint
            .as_ref()
            .and_then(|value| value.get("tls_profile"))
            .and_then(|value| value.as_str())
            .is_some_and(|value| !value.trim().is_empty())
    {
        return Some("transport_proxy_or_tls_unsupported");
    }

    if !provider_type_supported(&transport.provider.provider_type) {
        return Some("transport_provider_type_unsupported");
    }

    None
}

fn same_api_format(left: &str, right: &str) -> bool {
    aether_ai_formats::legacy_openai_format_alias_matches(left, right)
}
