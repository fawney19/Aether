use super::super::snapshot::GatewayProviderTransportSnapshot;
use super::auth::{
    resolve_local_kilo_free_request_auth, KiloFreeRequestAuth, KiloFreeRequestAuthSupport,
    KiloFreeRequestAuthUnsupportedReason, KILO_FREE_PROVIDER_TYPE,
};
use crate::rules::{body_rules_have_enabled_rules, header_rules_have_enabled_rules};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiloFreeRequestSideSpec {
    pub auth: KiloFreeRequestAuth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KiloFreeRequestSideSupport {
    Supported(KiloFreeRequestSideSpec),
    Unsupported(KiloFreeRequestSideUnsupportedReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KiloFreeRequestSideUnsupportedReason {
    InactiveTransport,
    WrongProviderType,
    UnsupportedApiFormat,
    UnsupportedCustomPath,
    UnsupportedHeaderRules,
    UnsupportedBodyRules,
    UnsupportedNetworkConfig,
    UnsupportedAuth(KiloFreeRequestAuthUnsupportedReason),
}

pub fn is_kilo_free_provider_transport(transport: &GatewayProviderTransportSnapshot) -> bool {
    transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(KILO_FREE_PROVIDER_TYPE)
}

pub fn classify_local_kilo_free_request_support(
    transport: &GatewayProviderTransportSnapshot,
) -> KiloFreeRequestSideSupport {
    if !transport.provider.is_active || !transport.endpoint.is_active || !transport.key.is_active {
        return KiloFreeRequestSideSupport::Unsupported(
            KiloFreeRequestSideUnsupportedReason::InactiveTransport,
        );
    }
    if !is_kilo_free_provider_transport(transport) {
        return KiloFreeRequestSideSupport::Unsupported(
            KiloFreeRequestSideUnsupportedReason::WrongProviderType,
        );
    }

    let endpoint_format =
        aether_ai_formats::normalize_api_format_alias(&transport.endpoint.api_format);
    if !endpoint_format.starts_with("openai:") {
        return KiloFreeRequestSideSupport::Unsupported(
            KiloFreeRequestSideUnsupportedReason::UnsupportedApiFormat,
        );
    }
    if transport
        .endpoint
        .custom_path
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return KiloFreeRequestSideSupport::Unsupported(
            KiloFreeRequestSideUnsupportedReason::UnsupportedCustomPath,
        );
    }
    if header_rules_have_enabled_rules(transport.endpoint.header_rules.as_ref()) {
        return KiloFreeRequestSideSupport::Unsupported(
            KiloFreeRequestSideUnsupportedReason::UnsupportedHeaderRules,
        );
    }
    if body_rules_have_enabled_rules(transport.endpoint.body_rules.as_ref()) {
        return KiloFreeRequestSideSupport::Unsupported(
            KiloFreeRequestSideUnsupportedReason::UnsupportedBodyRules,
        );
    }
    if transport.provider.proxy.is_some()
        || transport.endpoint.proxy.is_some()
        || transport.key.proxy.is_some()
        || transport.key.fingerprint.is_some()
    {
        return KiloFreeRequestSideSupport::Unsupported(
            KiloFreeRequestSideUnsupportedReason::UnsupportedNetworkConfig,
        );
    }

    let auth = match resolve_local_kilo_free_request_auth(transport) {
        KiloFreeRequestAuthSupport::Supported(auth) => auth,
        KiloFreeRequestAuthSupport::Unsupported(reason) => {
            return KiloFreeRequestSideSupport::Unsupported(
                KiloFreeRequestSideUnsupportedReason::UnsupportedAuth(reason),
            );
        }
    };

    KiloFreeRequestSideSupport::Supported(KiloFreeRequestSideSpec { auth })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Kilo Free".to_string(),
                provider_type: "kilo_free".to_string(),
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
                api_format: "openai:chat".to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://api.kilo.ai/api/gateway".to_string(),
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
                name: "free-key".to_string(),
                auth_type: "bearer".to_string(),
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
                decrypted_api_key: String::new(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn supports_active_kilo_free_transport() {
        let transport = sample_transport();
        let result = classify_local_kilo_free_request_support(&transport);
        assert!(matches!(result, KiloFreeRequestSideSupport::Supported(_)));
    }

    #[test]
    fn rejects_inactive_provider() {
        let mut transport = sample_transport();
        transport.provider.is_active = false;
        let result = classify_local_kilo_free_request_support(&transport);
        assert!(matches!(
            result,
            KiloFreeRequestSideSupport::Unsupported(
                KiloFreeRequestSideUnsupportedReason::InactiveTransport
            )
        ));
    }

    #[test]
    fn rejects_non_openai_format() {
        let mut transport = sample_transport();
        transport.endpoint.api_format = "claude:messages".to_string();
        let result = classify_local_kilo_free_request_support(&transport);
        assert!(matches!(
            result,
            KiloFreeRequestSideSupport::Unsupported(
                KiloFreeRequestSideUnsupportedReason::UnsupportedApiFormat
            )
        ));
    }
}
