use std::collections::BTreeMap;

use serde_json::Value;

use crate::auth::{
    build_claude_passthrough_headers, build_complete_passthrough_headers_with_auth,
    build_openai_passthrough_headers, build_passthrough_headers, ensure_upstream_auth_header,
};
use crate::rules::{apply_local_body_rules, apply_local_header_rules};
use crate::snapshot::GatewayProviderTransportSnapshot;
use crate::url::{build_openai_chat_url, build_openai_responses_url};
use crate::vertex::uses_vertex_api_key_query_auth;

#[derive(Debug, Clone, Copy)]
pub struct StandardProviderRequestHeadersInput<'a> {
    pub transport: &'a GatewayProviderTransportSnapshot,
    pub provider_api_format: &'a str,
    pub same_format: bool,
    pub headers: &'a http::HeaderMap,
    pub auth_header: &'a str,
    pub auth_value: &'a str,
    pub extra_headers: &'a BTreeMap<String, String>,
    pub header_rules: Option<&'a Value>,
    pub provider_request_body: &'a Value,
    pub original_request_body: &'a Value,
    pub upstream_is_stream: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardProviderRequestHeaders {
    pub headers: BTreeMap<String, String>,
    pub auth_header: String,
    pub auth_value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardPlanFallbackAcceptPolicy {
    None,
    TextEventStreamIfStreaming,
    TextEventStreamRequired,
    ProviderEventStreamIfMissing,
}

#[derive(Debug)]
pub struct StandardPlanFallbackHeadersInput<'a> {
    pub request_headers: &'a http::HeaderMap,
    pub existing_provider_request_headers: BTreeMap<String, String>,
    pub auth_header: Option<&'a str>,
    pub auth_value: Option<&'a str>,
    pub extra_headers: &'a BTreeMap<String, String>,
    pub content_type: Option<&'a str>,
    pub provider_api_format: &'a str,
    pub client_api_format: &'a str,
    pub upstream_is_stream: bool,
    pub build_from_request_when_empty: bool,
    pub accept_policy: StandardPlanFallbackAcceptPolicy,
}

pub fn build_standard_plan_fallback_openai_chat_url(
    upstream_base_url: &str,
    request_query: Option<&str>,
) -> String {
    build_openai_chat_url(upstream_base_url, request_query)
}

pub fn build_standard_plan_fallback_openai_responses_url(
    upstream_base_url: &str,
    request_query: Option<&str>,
    compact: bool,
) -> String {
    build_openai_responses_url(upstream_base_url, request_query, compact)
}

pub fn build_standard_plan_fallback_headers(
    input: StandardPlanFallbackHeadersInput<'_>,
) -> BTreeMap<String, String> {
    let auth_pair = input.auth_header.zip(input.auth_value);
    let mut headers = if !input.existing_provider_request_headers.is_empty() {
        input.existing_provider_request_headers
    } else if input.build_from_request_when_empty {
        match auth_pair {
            Some((auth_header, auth_value))
                if input.provider_api_format == input.client_api_format =>
            {
                build_complete_passthrough_headers_with_auth(
                    input.request_headers,
                    auth_header,
                    auth_value,
                    input.extra_headers,
                    input.content_type,
                )
            }
            Some((auth_header, auth_value)) if input.provider_api_format.starts_with("claude:") => {
                build_claude_passthrough_headers(
                    input.request_headers,
                    auth_header,
                    auth_value,
                    input.extra_headers,
                    input.content_type,
                )
            }
            Some((auth_header, auth_value)) => build_openai_passthrough_headers(
                input.request_headers,
                auth_header,
                auth_value,
                input.extra_headers,
                input.content_type,
            ),
            None => build_passthrough_headers(
                input.request_headers,
                input.extra_headers,
                input.content_type,
            ),
        }
    } else {
        input.existing_provider_request_headers
    };

    if let Some((auth_header, auth_value)) = auth_pair {
        ensure_upstream_auth_header(&mut headers, auth_header, auth_value);
    }

    match input.accept_policy {
        StandardPlanFallbackAcceptPolicy::None => {}
        StandardPlanFallbackAcceptPolicy::TextEventStreamIfStreaming => {
            if input.upstream_is_stream {
                headers
                    .entry("accept".to_string())
                    .or_insert_with(|| "text/event-stream".to_string());
            }
        }
        StandardPlanFallbackAcceptPolicy::TextEventStreamRequired => {
            headers.insert("accept".to_string(), "text/event-stream".to_string());
        }
        StandardPlanFallbackAcceptPolicy::ProviderEventStreamIfMissing => {
            headers
                .entry("accept".to_string())
                .or_insert_with(|| "application/vnd.amazon.eventstream".to_string());
        }
    }

    headers
}

pub fn apply_standard_provider_request_body_rules(
    mut provider_request_body: Value,
    body_rules: Option<&Value>,
    original_request_body: &Value,
) -> Option<Value> {
    if !apply_local_body_rules(
        &mut provider_request_body,
        body_rules,
        Some(original_request_body),
    ) {
        return None;
    }
    Some(provider_request_body)
}

pub fn build_standard_provider_request_headers(
    input: StandardProviderRequestHeadersInput<'_>,
) -> Option<StandardProviderRequestHeaders> {
    let uses_vertex_query_auth =
        uses_vertex_api_key_query_auth(input.transport, input.provider_api_format);
    let mut headers = if input.same_format {
        build_complete_passthrough_headers_with_auth(
            input.headers,
            input.auth_header,
            input.auth_value,
            input.extra_headers,
            Some("application/json"),
        )
    } else if input.provider_api_format.starts_with("claude:") {
        build_claude_passthrough_headers(
            input.headers,
            input.auth_header,
            input.auth_value,
            input.extra_headers,
            Some("application/json"),
        )
    } else {
        build_openai_passthrough_headers(
            input.headers,
            input.auth_header,
            input.auth_value,
            input.extra_headers,
            Some("application/json"),
        )
    };

    let protected_headers = if uses_vertex_query_auth {
        &["content-type"][..]
    } else {
        &[input.auth_header, "content-type"][..]
    };
    if !apply_local_header_rules(
        &mut headers,
        input.header_rules,
        protected_headers,
        input.provider_request_body,
        Some(input.original_request_body),
    ) {
        return None;
    }

    let (auth_header, auth_value) = if uses_vertex_query_auth {
        headers.remove("x-goog-api-key");
        (String::new(), String::new())
    } else {
        ensure_upstream_auth_header(&mut headers, input.auth_header, input.auth_value);
        (input.auth_header.to_string(), input.auth_value.to_string())
    };
    if input.upstream_is_stream {
        headers
            .entry("accept".to_string())
            .or_insert_with(|| "text/event-stream".to_string());
    }

    Some(StandardProviderRequestHeaders {
        headers,
        auth_header,
        auth_value,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use http::HeaderMap;
    use serde_json::json;

    use super::{
        apply_standard_provider_request_body_rules, build_standard_plan_fallback_headers,
        build_standard_plan_fallback_openai_chat_url,
        build_standard_plan_fallback_openai_responses_url, build_standard_provider_request_headers,
        StandardPlanFallbackAcceptPolicy, StandardPlanFallbackHeadersInput,
        StandardProviderRequestHeadersInput,
    };
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport(api_format: &str) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Provider".to_string(),
                provider_type: "openai".to_string(),
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
                api_format: api_format.to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://api.example.com".to_string(),
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
    fn builds_same_format_headers_with_complete_passthrough_and_stream_accept() {
        let mut request_headers = HeaderMap::new();
        request_headers.insert("x-client", "demo".parse().expect("header"));
        let transport = sample_transport("openai:chat");
        let resolved =
            build_standard_provider_request_headers(StandardProviderRequestHeadersInput {
                transport: &transport,
                provider_api_format: "openai:chat",
                same_format: true,
                headers: &request_headers,
                auth_header: "authorization",
                auth_value: "Bearer secret",
                extra_headers: &BTreeMap::new(),
                header_rules: None,
                provider_request_body: &json!({"model":"gpt-5"}),
                original_request_body: &json!({"model":"gpt-5"}),
                upstream_is_stream: true,
            })
            .expect("headers should build");

        assert_eq!(resolved.auth_header, "authorization");
        assert_eq!(resolved.auth_value, "Bearer secret");
        assert_eq!(
            resolved.headers.get("authorization"),
            Some(&"Bearer secret".to_string())
        );
        assert_eq!(
            resolved.headers.get("accept"),
            Some(&"text/event-stream".to_string())
        );
        assert_eq!(resolved.headers.get("x-client"), Some(&"demo".to_string()));
    }

    #[test]
    fn applies_header_rules_after_base_headers() {
        let transport = sample_transport("claude:messages");
        let resolved =
            build_standard_provider_request_headers(StandardProviderRequestHeadersInput {
                transport: &transport,
                provider_api_format: "claude:messages",
                same_format: false,
                headers: &HeaderMap::new(),
                auth_header: "x-api-key",
                auth_value: "secret",
                extra_headers: &BTreeMap::new(),
                header_rules: Some(&json!([
                    {"action":"set","key":"x-route","value":"standard"}
                ])),
                provider_request_body: &json!({"model":"claude"}),
                original_request_body: &json!({"model":"claude"}),
                upstream_is_stream: false,
            })
            .expect("headers should build");

        assert_eq!(
            resolved.headers.get("x-api-key"),
            Some(&"secret".to_string())
        );
        assert_eq!(
            resolved.headers.get("x-route"),
            Some(&"standard".to_string())
        );
        assert_eq!(
            resolved.headers.get("anthropic-version"),
            Some(&"2023-06-01".to_string())
        );
    }

    #[test]
    fn applies_standard_body_rules_to_surface_built_body() {
        let body = apply_standard_provider_request_body_rules(
            json!({"model":"gpt-5"}),
            Some(&json!([
                {"action":"set","path":"metadata.source","value":"standard"}
            ])),
            &json!({"model":"client"}),
        )
        .expect("body rules should apply");

        assert_eq!(body["metadata"]["source"], json!("standard"));
    }

    #[test]
    fn builds_plan_fallback_headers_from_request_when_enabled() {
        let mut request_headers = HeaderMap::new();
        request_headers.insert("x-client", "demo".parse().expect("header"));

        let headers = build_standard_plan_fallback_headers(StandardPlanFallbackHeadersInput {
            request_headers: &request_headers,
            existing_provider_request_headers: BTreeMap::new(),
            auth_header: Some("authorization"),
            auth_value: Some("Bearer secret"),
            extra_headers: &BTreeMap::new(),
            content_type: Some("application/json"),
            provider_api_format: "openai:chat",
            client_api_format: "openai:chat",
            upstream_is_stream: true,
            build_from_request_when_empty: true,
            accept_policy: StandardPlanFallbackAcceptPolicy::TextEventStreamIfStreaming,
        });

        assert_eq!(
            headers.get("authorization"),
            Some(&"Bearer secret".to_string())
        );
        assert_eq!(headers.get("x-client"), Some(&"demo".to_string()));
        assert_eq!(
            headers.get("accept"),
            Some(&"text/event-stream".to_string())
        );
    }

    #[test]
    fn plan_fallback_headers_preserve_empty_existing_mode() {
        let headers = build_standard_plan_fallback_headers(StandardPlanFallbackHeadersInput {
            request_headers: &HeaderMap::new(),
            existing_provider_request_headers: BTreeMap::new(),
            auth_header: Some("authorization"),
            auth_value: Some("Bearer secret"),
            extra_headers: &BTreeMap::new(),
            content_type: Some("application/json"),
            provider_api_format: "openai:responses",
            client_api_format: "openai:responses",
            upstream_is_stream: true,
            build_from_request_when_empty: false,
            accept_policy: StandardPlanFallbackAcceptPolicy::TextEventStreamIfStreaming,
        });

        assert_eq!(
            headers.get("authorization"),
            Some(&"Bearer secret".to_string())
        );
        assert_eq!(
            headers.get("accept"),
            Some(&"text/event-stream".to_string())
        );
        assert!(!headers.contains_key("content-type"));
    }

    #[test]
    fn plan_fallback_url_helpers_route_openai_surfaces() {
        assert_eq!(
            build_standard_plan_fallback_openai_chat_url("https://api.example.com", Some("x=1")),
            "https://api.example.com/v1/chat/completions?x=1"
        );
        assert_eq!(
            build_standard_plan_fallback_openai_responses_url(
                "https://api.example.com",
                Some("x=1"),
                true,
            ),
            "https://api.example.com/v1/responses/compact?x=1"
        );
    }
}
