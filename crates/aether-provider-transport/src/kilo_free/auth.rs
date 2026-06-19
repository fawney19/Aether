use super::super::snapshot::GatewayProviderTransportSnapshot;

pub const KILO_FREE_PROVIDER_TYPE: &str = "kilo_free";
pub const KILO_FREE_BEARER_TOKEN: &str = "free";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiloFreeRequestAuth {
    pub bearer_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KiloFreeRequestAuthSupport {
    Supported(KiloFreeRequestAuth),
    Unsupported(KiloFreeRequestAuthUnsupportedReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KiloFreeRequestAuthUnsupportedReason {
    WrongProviderType,
}

pub fn resolve_local_kilo_free_request_auth(
    transport: &GatewayProviderTransportSnapshot,
) -> KiloFreeRequestAuthSupport {
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(KILO_FREE_PROVIDER_TYPE)
    {
        return KiloFreeRequestAuthSupport::Unsupported(
            KiloFreeRequestAuthUnsupportedReason::WrongProviderType,
        );
    }

    KiloFreeRequestAuthSupport::Supported(KiloFreeRequestAuth {
        bearer_token: KILO_FREE_BEARER_TOKEN.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport(provider_type: &str) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Kilo Free".to_string(),
                provider_type: provider_type.to_string(),
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
    fn resolves_bearer_free_for_kilo_free_provider() {
        let transport = sample_transport("kilo_free");
        let result = resolve_local_kilo_free_request_auth(&transport);
        assert_eq!(
            result,
            KiloFreeRequestAuthSupport::Supported(KiloFreeRequestAuth {
                bearer_token: "free".to_string(),
            })
        );
    }

    #[test]
    fn rejects_wrong_provider_type() {
        let transport = sample_transport("openai");
        let result = resolve_local_kilo_free_request_auth(&transport);
        assert_eq!(
            result,
            KiloFreeRequestAuthSupport::Unsupported(
                KiloFreeRequestAuthUnsupportedReason::WrongProviderType
            )
        );
    }
}
