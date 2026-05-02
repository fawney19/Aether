use std::collections::BTreeMap;

use serde_json::Value;

use crate::auth::{build_passthrough_headers_with_auth, resolve_local_openai_bearer_auth};
use crate::policy::local_standard_transport_unsupported_reason_with_network;
use crate::rules::apply_local_header_rules;
use crate::snapshot::GatewayProviderTransportSnapshot;
use crate::url::build_openai_responses_url;

#[derive(Debug, Clone, Copy)]
pub struct ProviderOpenAiImageHeadersInput<'a> {
    pub headers: &'a http::HeaderMap,
    pub auth_header: &'a str,
    pub auth_value: &'a str,
    pub header_rules: Option<&'a Value>,
    pub provider_request_body: &'a Value,
    pub original_request_body: &'a Value,
}

pub fn openai_image_transport_unsupported_reason(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> Option<&'static str> {
    local_standard_transport_unsupported_reason_with_network(transport, api_format)
}

pub fn resolve_openai_image_auth(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<(String, String)> {
    resolve_local_openai_bearer_auth(transport)
}

pub fn build_openai_image_upstream_url(
    transport: &GatewayProviderTransportSnapshot,
    request_query: Option<&str>,
) -> String {
    build_openai_responses_url(&transport.endpoint.base_url, request_query, false)
}

pub fn build_openai_image_headers(
    input: ProviderOpenAiImageHeadersInput<'_>,
) -> Option<BTreeMap<String, String>> {
    let mut provider_request_headers = build_passthrough_headers_with_auth(
        input.headers,
        input.auth_header,
        input.auth_value,
        &BTreeMap::new(),
    );
    provider_request_headers.insert("content-type".to_string(), "application/json".to_string());
    provider_request_headers.insert("accept".to_string(), "text/event-stream".to_string());
    if !apply_local_header_rules(
        &mut provider_request_headers,
        input.header_rules,
        &[input.auth_header, "content-type", "accept"],
        input.provider_request_body,
        Some(input.original_request_body),
    ) {
        return None;
    }
    Some(provider_request_headers)
}

#[cfg(test)]
mod tests {
    use http::HeaderMap;
    use serde_json::json;

    use super::{
        build_openai_image_headers, build_openai_image_upstream_url,
        ProviderOpenAiImageHeadersInput,
    };
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Provider".to_string(),
                provider_type: "codex".to_string(),
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
                api_format: "openai:image".to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://api.openai.com".to_string(),
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
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn builds_openai_image_url_on_responses_surface() {
        let url = build_openai_image_upstream_url(&sample_transport(), Some("trace=1"));

        assert_eq!(url, "https://api.openai.com/v1/responses?trace=1");
    }

    #[test]
    fn builds_json_eventstream_headers_and_applies_rules() {
        let headers = build_openai_image_headers(ProviderOpenAiImageHeadersInput {
            headers: &HeaderMap::new(),
            auth_header: "authorization",
            auth_value: "Bearer secret",
            header_rules: Some(&json!([
                {"action":"set","key":"x-image-route","value":"codex"}
            ])),
            provider_request_body: &json!({"model":"gpt-5.4-mini"}),
            original_request_body: &json!({"prompt":"draw"}),
        })
        .expect("headers should build");

        assert_eq!(
            headers.get("authorization"),
            Some(&"Bearer secret".to_string())
        );
        assert_eq!(
            headers.get("content-type"),
            Some(&"application/json".to_string())
        );
        assert_eq!(
            headers.get("accept"),
            Some(&"text/event-stream".to_string())
        );
        assert_eq!(headers.get("x-image-route"), Some(&"codex".to_string()));
    }
}
