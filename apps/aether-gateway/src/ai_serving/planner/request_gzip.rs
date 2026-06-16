use aether_ai_serving::AiRequestGzipPolicy;
use serde_json::Value;

use crate::ai_serving::is_openai_responses_family_format;

use super::state::GatewayProviderTransportSnapshot;

const DEFAULT_CODEX_REQUEST_GZIP_MIN_BYTES: usize = 64 * 1024;

pub(crate) fn resolve_transport_request_gzip_policy(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<AiRequestGzipPolicy> {
    transport_request_gzip_policy_from_config(transport.endpoint.config.as_ref())
        .or_else(|| transport_request_gzip_policy_from_config(transport.provider.config.as_ref()))
        .or_else(|| default_transport_request_gzip_policy(transport))
}

fn default_transport_request_gzip_policy(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<AiRequestGzipPolicy> {
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("codex")
    {
        return None;
    }
    if !is_codex_request_gzip_endpoint_api_format(transport.endpoint.api_format.as_str()) {
        return None;
    }

    Some(AiRequestGzipPolicy {
        enabled: Some(true),
        min_bytes: Some(DEFAULT_CODEX_REQUEST_GZIP_MIN_BYTES),
    })
}

fn is_codex_request_gzip_endpoint_api_format(api_format: &str) -> bool {
    is_openai_responses_family_format(api_format)
        || api_format.trim().eq_ignore_ascii_case("openai:image")
}

fn transport_request_gzip_policy_from_config(
    config: Option<&Value>,
) -> Option<AiRequestGzipPolicy> {
    let object = config?.as_object()?;

    for key in ["request_gzip", "request_body_gzip"] {
        if let Some(policy) = object
            .get(key)
            .and_then(transport_request_gzip_policy_from_value)
        {
            return Some(policy);
        }
    }

    let enabled = first_config_bool(
        object,
        &["request_gzip_enabled", "request_body_gzip_enabled"],
    );
    let min_bytes = first_config_usize(
        object,
        &["request_gzip_min_bytes", "request_body_gzip_min_bytes"],
    );

    match (enabled, min_bytes) {
        (Some(false), _) => Some(AiRequestGzipPolicy {
            enabled: Some(false),
            min_bytes: None,
        }),
        (Some(true), min_bytes) => Some(AiRequestGzipPolicy {
            enabled: Some(true),
            min_bytes,
        }),
        (None, Some(min_bytes)) => Some(AiRequestGzipPolicy {
            enabled: Some(true),
            min_bytes: Some(min_bytes),
        }),
        (None, None) => None,
    }
}

fn transport_request_gzip_policy_from_value(value: &Value) -> Option<AiRequestGzipPolicy> {
    if let Some(enabled) = value.as_bool() {
        return Some(AiRequestGzipPolicy {
            enabled: Some(enabled),
            min_bytes: None,
        });
    }

    let object = value.as_object()?;
    let enabled = first_config_bool(object, &["enabled"]);
    let min_bytes = first_config_usize(object, &["min_bytes"]);

    match (enabled, min_bytes) {
        (Some(false), _) => Some(AiRequestGzipPolicy {
            enabled: Some(false),
            min_bytes: None,
        }),
        (Some(true), min_bytes) => Some(AiRequestGzipPolicy {
            enabled: Some(true),
            min_bytes,
        }),
        (None, Some(min_bytes)) => Some(AiRequestGzipPolicy {
            enabled: Some(true),
            min_bytes: Some(min_bytes),
        }),
        (None, None) => None,
    }
}

fn first_config_bool(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(config_bool))
}

fn config_bool(value: &Value) -> Option<bool> {
    value.as_bool().or_else(|| {
        value.as_str().and_then(|text| {
            let normalized = text.trim();
            if normalized.eq_ignore_ascii_case("true") {
                Some(true)
            } else if normalized.eq_ignore_ascii_case("false") {
                Some(false)
            } else {
                None
            }
        })
    })
}

fn first_config_usize(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<usize> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(config_usize))
}

fn config_usize(value: &Value) -> Option<usize> {
    value
        .as_u64()
        .and_then(|number| usize::try_from(number).ok())
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<usize>().ok())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use serde_json::{json, Value};

    fn sample_transport(
        provider_type: &str,
        endpoint_api_format: &str,
        provider_config: Option<Value>,
        endpoint_config: Option<Value>,
    ) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Provider".to_string(),
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
                config: provider_config,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: endpoint_api_format.to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://api.example.test".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: endpoint_config,
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
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn endpoint_request_gzip_policy_overrides_provider_policy() {
        let transport = sample_transport(
            "openai",
            "openai:responses",
            Some(json!({"request_gzip": false})),
            Some(json!({"request_gzip": {"enabled": true, "min_bytes": 1024}})),
        );

        assert_eq!(
            resolve_transport_request_gzip_policy(&transport),
            Some(AiRequestGzipPolicy {
                enabled: Some(true),
                min_bytes: Some(1024),
            })
        );
    }

    #[test]
    fn endpoint_request_gzip_false_disables_provider_and_codex_defaults() {
        let transport = sample_transport(
            "codex",
            "openai:responses",
            Some(json!({"request_gzip": {"enabled": true, "min_bytes": 1024}})),
            Some(json!({"request_gzip": false})),
        );

        assert_eq!(
            resolve_transport_request_gzip_policy(&transport),
            Some(AiRequestGzipPolicy {
                enabled: Some(false),
                min_bytes: None,
            })
        );
    }

    #[test]
    fn request_gzip_policy_supports_top_level_aliases() {
        let transport = sample_transport(
            "openai",
            "openai:responses",
            None,
            Some(json!({
                "request_body_gzip_enabled": true,
                "request_body_gzip_min_bytes": "4096"
            })),
        );

        assert_eq!(
            resolve_transport_request_gzip_policy(&transport),
            Some(AiRequestGzipPolicy {
                enabled: Some(true),
                min_bytes: Some(4096),
            })
        );
    }

    #[test]
    fn request_gzip_policy_treats_min_bytes_only_as_enabled() {
        let transport = sample_transport(
            "openai",
            "openai:responses",
            None,
            Some(json!({"request_gzip_min_bytes": 1})),
        );

        assert_eq!(
            resolve_transport_request_gzip_policy(&transport),
            Some(AiRequestGzipPolicy {
                enabled: Some(true),
                min_bytes: Some(1),
            })
        );
    }

    #[test]
    fn codex_responses_endpoint_gets_default_request_gzip_policy() {
        let transport = sample_transport("codex", "openai:responses", None, None);

        assert_eq!(
            resolve_transport_request_gzip_policy(&transport),
            Some(AiRequestGzipPolicy {
                enabled: Some(true),
                min_bytes: Some(DEFAULT_CODEX_REQUEST_GZIP_MIN_BYTES),
            })
        );
    }

    #[test]
    fn codex_image_endpoint_gets_default_request_gzip_policy() {
        let transport = sample_transport("codex", "openai:image", None, None);

        assert_eq!(
            resolve_transport_request_gzip_policy(&transport),
            Some(AiRequestGzipPolicy {
                enabled: Some(true),
                min_bytes: Some(DEFAULT_CODEX_REQUEST_GZIP_MIN_BYTES),
            })
        );
    }

    #[test]
    fn non_codex_endpoint_does_not_get_default_request_gzip_policy() {
        let transport = sample_transport("openai", "openai:responses", None, None);

        assert_eq!(resolve_transport_request_gzip_policy(&transport), None);
    }
}
