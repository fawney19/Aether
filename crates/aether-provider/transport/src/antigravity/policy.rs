use serde_json::Value;

use super::super::snapshot::GatewayProviderTransportSnapshot;
use super::super::transport_proxy_is_locally_supported;
use super::auth::{
    resolve_local_antigravity_request_auth, AntigravityRequestAuth, AntigravityRequestAuthSupport,
    AntigravityRequestAuthUnsupportedReason, ANTIGRAVITY_PROVIDER_TYPE,
};
use super::request::{
    classify_antigravity_safe_request_body, AntigravityEnvelopeRequestType,
    AntigravityRequestEnvelopeUnsupportedReason,
};
use crate::rules::{body_rules_have_enabled_rules, header_rules_have_enabled_rules};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityRequestSideSpec {
    pub auth: AntigravityRequestAuth,
    pub request_type: AntigravityEnvelopeRequestType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravityRequestSideSupport {
    Supported(AntigravityRequestSideSpec),
    Unsupported(AntigravityRequestSideUnsupportedReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravityRequestSideUnsupportedReason {
    InactiveTransport,
    WrongProviderType,
    UnsupportedApiFormat,
    UnsupportedCustomPath,
    UnsupportedHeaderRules,
    UnsupportedBodyRules,
    UnsupportedNetworkConfig,
    UnsupportedAuth(AntigravityRequestAuthUnsupportedReason),
    UnsupportedEnvelope(AntigravityRequestEnvelopeUnsupportedReason),
}

pub fn is_antigravity_provider_transport(transport: &GatewayProviderTransportSnapshot) -> bool {
    transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(ANTIGRAVITY_PROVIDER_TYPE)
}

pub fn classify_local_antigravity_request_support(
    transport: &GatewayProviderTransportSnapshot,
    request_body: &Value,
    request_type: AntigravityEnvelopeRequestType,
) -> AntigravityRequestSideSupport {
    if !transport.provider.is_active || !transport.endpoint.is_active || !transport.key.is_active {
        return AntigravityRequestSideSupport::Unsupported(
            AntigravityRequestSideUnsupportedReason::InactiveTransport,
        );
    }
    if !is_antigravity_provider_transport(transport) {
        return AntigravityRequestSideSupport::Unsupported(
            AntigravityRequestSideUnsupportedReason::WrongProviderType,
        );
    }

    let endpoint_format =
        aether_ai_formats::normalize_api_format_alias(&transport.endpoint.api_format);
    if endpoint_format != "gemini:generate_content" {
        return AntigravityRequestSideSupport::Unsupported(
            AntigravityRequestSideUnsupportedReason::UnsupportedApiFormat,
        );
    }
    if transport
        .endpoint
        .custom_path
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return AntigravityRequestSideSupport::Unsupported(
            AntigravityRequestSideUnsupportedReason::UnsupportedCustomPath,
        );
    }
    if header_rules_have_enabled_rules(transport.endpoint.header_rules.as_ref()) {
        return AntigravityRequestSideSupport::Unsupported(
            AntigravityRequestSideUnsupportedReason::UnsupportedHeaderRules,
        );
    }
    if body_rules_have_enabled_rules(transport.endpoint.body_rules.as_ref()) {
        return AntigravityRequestSideSupport::Unsupported(
            AntigravityRequestSideUnsupportedReason::UnsupportedBodyRules,
        );
    }
    // A configured proxy is carried by the execution plan itself, so it only
    // disqualifies the local request when it cannot be resolved into a usable
    // snapshot. Transport profiles stay unsupported because the v1internal
    // payload never carries one.
    if !transport_proxy_is_locally_supported(transport) || transport.key.fingerprint.is_some() {
        return AntigravityRequestSideSupport::Unsupported(
            AntigravityRequestSideUnsupportedReason::UnsupportedNetworkConfig,
        );
    }

    let auth = match resolve_local_antigravity_request_auth(transport) {
        AntigravityRequestAuthSupport::Supported(auth) => auth,
        AntigravityRequestAuthSupport::Unsupported(reason) => {
            return AntigravityRequestSideSupport::Unsupported(
                AntigravityRequestSideUnsupportedReason::UnsupportedAuth(reason),
            );
        }
    };

    if let Err(reason) = classify_antigravity_safe_request_body(request_body) {
        return AntigravityRequestSideSupport::Unsupported(
            AntigravityRequestSideUnsupportedReason::UnsupportedEnvelope(reason),
        );
    }

    AntigravityRequestSideSupport::Supported(AntigravityRequestSideSpec { auth, request_type })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::request::AntigravityEnvelopeRequestType;
    use super::{
        classify_local_antigravity_request_support, AntigravityRequestSideSupport,
        AntigravityRequestSideUnsupportedReason,
    };
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Antigravity".to_string(),
                provider_type: "antigravity".to_string(),
                website: None,
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
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "gemini:generate_content".to_string(),
                api_family: Some("gemini".to_string()),
                endpoint_kind: Some("generate_content".to_string()),
                is_active: true,
                base_url: "https://daily-cloudcode-pa.googleapis.com".to_string(),
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
                auth_type: "oauth".to_string(),
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
                upstream_metadata: None,
                decrypted_api_key: "__placeholder__".to_string(),
                decrypted_auth_config: Some(
                    r#"{"provider_type":"antigravity","refresh_token":"rt","cloudaicompanionProject":"project-1"}"#
                        .to_string(),
                ),
            },
        }
    }

    fn classify(transport: &GatewayProviderTransportSnapshot) -> AntigravityRequestSideSupport {
        classify_local_antigravity_request_support(
            transport,
            &json!({"contents": []}),
            AntigravityEnvelopeRequestType::Agent,
        )
    }

    fn assert_unsupported_network_config(support: AntigravityRequestSideSupport) {
        assert_eq!(
            support,
            AntigravityRequestSideSupport::Unsupported(
                AntigravityRequestSideUnsupportedReason::UnsupportedNetworkConfig,
            )
        );
    }

    #[test]
    fn a_resolvable_tunnel_node_proxy_keeps_the_envelope_supported() {
        let mut transport = sample_transport();
        transport.provider.proxy = Some(json!({
            "enabled": true,
            "node_id": "702d158b-a432-4694-94cc-3bec13dbbc20",
        }));

        assert!(matches!(
            classify(&transport),
            AntigravityRequestSideSupport::Supported(_)
        ));
    }

    #[test]
    fn a_resolvable_url_proxy_keeps_the_envelope_supported() {
        for proxy_owner in ["provider", "endpoint", "key"] {
            let mut transport = sample_transport();
            let proxy = Some(json!({"enabled": true, "url": "http://127.0.0.1:17000"}));
            match proxy_owner {
                "provider" => transport.provider.proxy = proxy,
                "endpoint" => transport.endpoint.proxy = proxy,
                _ => transport.key.proxy = proxy,
            }

            assert!(
                matches!(
                    classify(&transport),
                    AntigravityRequestSideSupport::Supported(_)
                ),
                "a {proxy_owner} proxy should not disqualify the antigravity envelope"
            );
        }
    }

    #[test]
    fn a_proxy_without_a_route_still_disqualifies_the_envelope() {
        let mut transport = sample_transport();
        transport.provider.proxy = Some(json!({"enabled": true}));

        assert_unsupported_network_config(classify(&transport));
    }

    #[test]
    fn a_key_fingerprint_still_disqualifies_the_envelope() {
        let mut transport = sample_transport();
        transport.key.fingerprint = Some(json!({"transport_profile": "chrome"}));

        assert_unsupported_network_config(classify(&transport));
    }
}
