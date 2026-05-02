use url::Url;

use super::super::auth::resolve_local_auth_type_for_transport_format;
use super::super::snapshot::GatewayProviderTransportSnapshot;

const VERTEX_AI_HOST: &str = "aiplatform.googleapis.com";

pub fn looks_like_vertex_ai_host(base_url: &str) -> bool {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return false;
    }

    let Ok(parsed) = Url::parse(trimmed) else {
        return false;
    };
    let Some(host) = parsed
        .host_str()
        .map(|value| value.trim().to_ascii_lowercase())
    else {
        return false;
    };

    host == VERTEX_AI_HOST
        || host.ends_with(&format!(".{VERTEX_AI_HOST}"))
        || host.ends_with(&format!("-{VERTEX_AI_HOST}"))
}

pub fn is_vertex_api_key_transport_context(transport: &GatewayProviderTransportSnapshot) -> bool {
    if transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(super::PROVIDER_TYPE)
    {
        return resolve_local_auth_type_for_transport_format(transport)
            .eq_ignore_ascii_case("api_key");
    }

    if !looks_like_vertex_ai_host(&transport.endpoint.base_url) {
        return false;
    }

    let endpoint_api_format = transport.endpoint.api_format.trim().to_ascii_lowercase();
    if !endpoint_api_format.starts_with("gemini:") && !endpoint_api_format.starts_with("claude:") {
        return false;
    }

    resolve_local_auth_type_for_transport_format(transport).eq_ignore_ascii_case("api_key")
}

pub fn uses_vertex_api_key_query_auth(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
) -> bool {
    is_vertex_api_key_transport_context(transport)
        && provider_api_format
            .trim()
            .to_ascii_lowercase()
            .starts_with("gemini:")
}

#[cfg(test)]
mod tests {
    use super::{
        is_vertex_api_key_transport_context, looks_like_vertex_ai_host,
        uses_vertex_api_key_query_auth,
    };
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Vertex".to_string(),
                provider_type: "custom".to_string(),
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
                endpoint_kind: Some("cli".to_string()),
                is_active: true,
                base_url: "https://aiplatform.googleapis.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: Some("/v1/publishers/google/models/{model}:{action}".to_string()),
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
    fn detects_vertex_host() {
        assert!(looks_like_vertex_ai_host(
            "https://aiplatform.googleapis.com"
        ));
        assert!(looks_like_vertex_ai_host(
            "https://us-central1-aiplatform.googleapis.com"
        ));
        assert!(!looks_like_vertex_ai_host("https://example.com"));
    }

    #[test]
    fn infers_vertex_api_key_context_for_custom_aiplatform_transport() {
        assert!(is_vertex_api_key_transport_context(&sample_transport()));
    }

    #[test]
    fn rejects_non_api_key_custom_aiplatform_transport() {
        let mut transport = sample_transport();
        transport.key.auth_type = "bearer".to_string();
        assert!(!is_vertex_api_key_transport_context(&transport));
    }

    #[test]
    fn detects_vertex_query_auth_usage_for_gemini_formats() {
        let transport = sample_transport();
        assert!(uses_vertex_api_key_query_auth(
            &transport,
            "gemini:generate_content"
        ));
        assert!(!uses_vertex_api_key_query_auth(
            &transport,
            "claude:messages"
        ));
    }
}
