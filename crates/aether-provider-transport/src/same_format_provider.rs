use std::collections::BTreeMap;

use serde_json::Value;

use crate::antigravity::is_antigravity_provider_transport;
use crate::auth::{
    build_complete_passthrough_headers, build_complete_passthrough_headers_with_auth,
    resolve_local_gemini_auth, resolve_local_standard_auth,
};
use crate::claude_code::build_claude_code_passthrough_headers;
use crate::claude_code::local_claude_code_transport_unsupported_reason_with_network;
use crate::kiro::{
    build_kiro_provider_headers, build_kiro_provider_request_body, is_kiro_provider_transport,
    local_kiro_request_transport_unsupported_reason_with_network, KiroAuthConfig,
    KiroProviderHeadersInput,
};
use crate::policy::{
    local_gemini_transport_unsupported_reason_with_network,
    local_standard_transport_unsupported_reason_with_network,
};
use crate::rules::{apply_local_body_rules, apply_local_header_rules};
use crate::snapshot::GatewayProviderTransportSnapshot;
use crate::vertex::{
    is_vertex_api_key_transport_context,
    local_vertex_api_key_gemini_transport_unsupported_reason_with_network,
};
use crate::{build_transport_request_url, ensure_upstream_auth_header, TransportRequestUrlParams};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameFormatProviderFamily {
    Standard,
    Gemini,
}

#[derive(Debug, Clone, Copy)]
pub struct SameFormatProviderRequestBehaviorParams {
    pub require_streaming: bool,
    pub report_kind: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SameFormatProviderRequestBehavior {
    pub is_antigravity: bool,
    pub is_claude_code: bool,
    pub is_vertex: bool,
    pub is_kiro: bool,
    pub upstream_is_stream: bool,
    pub report_kind: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct SameFormatProviderRequestBodyInput<'a> {
    pub body_json: &'a Value,
    pub mapped_model: &'a str,
    pub family: SameFormatProviderFamily,
    pub body_rules: Option<&'a Value>,
    pub upstream_is_stream: bool,
    pub kiro_auth_config: Option<&'a KiroAuthConfig>,
    pub is_claude_code: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct SameFormatProviderUpstreamUrlParams<'a> {
    pub provider_api_format: &'a str,
    pub mapped_model: &'a str,
    pub upstream_is_stream: bool,
    pub request_query: Option<&'a str>,
    pub kiro_api_region: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct SameFormatProviderHeadersInput<'a> {
    pub headers: &'a http::HeaderMap,
    pub provider_request_body: &'a Value,
    pub original_request_body: &'a Value,
    pub header_rules: Option<&'a Value>,
    pub behavior: SameFormatProviderRequestBehavior,
    pub auth_header: Option<&'a str>,
    pub auth_value: Option<&'a str>,
    pub extra_headers: &'a BTreeMap<String, String>,
    pub key_fingerprint: Option<&'a Value>,
    pub kiro_auth_config: Option<&'a KiroAuthConfig>,
    pub kiro_machine_id: Option<&'a str>,
}

pub fn classify_same_format_provider_request_behavior(
    transport: &GatewayProviderTransportSnapshot,
    params: SameFormatProviderRequestBehaviorParams,
) -> SameFormatProviderRequestBehavior {
    let is_antigravity = is_antigravity_provider_transport(transport);
    let is_claude_code = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("claude_code");
    let is_vertex = is_vertex_api_key_transport_context(transport);
    let is_kiro = is_kiro_provider_transport(transport);
    let upstream_is_stream = is_kiro || is_antigravity || params.require_streaming;
    let report_kind = if is_kiro && !params.require_streaming {
        "claude_cli_sync_finalize"
    } else if is_antigravity && !params.require_streaming {
        match params.report_kind {
            "gemini_chat_sync_success" => "gemini_chat_sync_finalize",
            "gemini_cli_sync_success" => "gemini_cli_sync_finalize",
            _ => params.report_kind,
        }
    } else {
        params.report_kind
    };

    SameFormatProviderRequestBehavior {
        is_antigravity,
        is_claude_code,
        is_vertex,
        is_kiro,
        upstream_is_stream,
        report_kind,
    }
}

pub fn build_same_format_provider_request_body(
    input: SameFormatProviderRequestBodyInput<'_>,
) -> Option<Value> {
    if let Some(kiro_auth_config) = input.kiro_auth_config {
        return build_kiro_provider_request_body(
            input.body_json,
            input.mapped_model,
            kiro_auth_config,
            input.body_rules,
        );
    }

    let request_body_object = input.body_json.as_object()?;
    let mut provider_request_body = serde_json::Map::from_iter(
        request_body_object
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    match input.family {
        SameFormatProviderFamily::Standard => {
            provider_request_body.insert(
                "model".to_string(),
                Value::String(input.mapped_model.to_string()),
            );
            if input.upstream_is_stream {
                provider_request_body.insert("stream".to_string(), Value::Bool(true));
            }
        }
        SameFormatProviderFamily::Gemini => {
            provider_request_body.remove("model");
        }
    }
    let mut provider_request_body = Value::Object(provider_request_body);
    if input.is_claude_code {
        crate::claude_code::sanitize_claude_code_request_body(&mut provider_request_body);
    }
    if !apply_local_body_rules(
        &mut provider_request_body,
        input.body_rules,
        Some(input.body_json),
    ) {
        return None;
    }
    Some(provider_request_body)
}

pub fn build_same_format_provider_upstream_url(
    transport: &GatewayProviderTransportSnapshot,
    params: SameFormatProviderUpstreamUrlParams<'_>,
) -> Option<String> {
    build_transport_request_url(
        transport,
        TransportRequestUrlParams {
            provider_api_format: params.provider_api_format,
            mapped_model: Some(params.mapped_model),
            upstream_is_stream: params.upstream_is_stream,
            request_query: params.request_query,
            kiro_api_region: params.kiro_api_region,
        },
    )
}

pub fn build_same_format_provider_headers(
    input: SameFormatProviderHeadersInput<'_>,
) -> Option<BTreeMap<String, String>> {
    if let Some(kiro_auth_config) = input.kiro_auth_config {
        return build_kiro_provider_headers(KiroProviderHeadersInput {
            headers: input.headers,
            provider_request_body: input.provider_request_body,
            original_request_body: input.original_request_body,
            header_rules: input.header_rules,
            auth_header: input.auth_header.unwrap_or_default(),
            auth_value: input.auth_value.unwrap_or_default(),
            auth_config: kiro_auth_config,
            machine_id: input.kiro_machine_id.unwrap_or_default(),
        });
    }

    let auth_header = input.auth_header.unwrap_or_default();
    let auth_value = input.auth_value.unwrap_or_default();
    let mut provider_request_headers = if input.behavior.is_claude_code {
        build_claude_code_passthrough_headers(
            input.headers,
            auth_header,
            auth_value,
            input.extra_headers,
            input.behavior.upstream_is_stream,
            input.key_fingerprint,
        )
    } else if input.behavior.is_vertex {
        build_complete_passthrough_headers(
            input.headers,
            input.extra_headers,
            Some("application/json"),
        )
    } else {
        build_complete_passthrough_headers_with_auth(
            input.headers,
            auth_header,
            auth_value,
            input.extra_headers,
            Some("application/json"),
        )
    };

    let protected_headers = input
        .auth_header
        .filter(|value| !value.trim().is_empty())
        .map(|value| vec![value, "content-type"])
        .unwrap_or_else(|| vec!["content-type"]);
    if !apply_local_header_rules(
        &mut provider_request_headers,
        input.header_rules,
        &protected_headers,
        input.provider_request_body,
        Some(input.original_request_body),
    ) {
        return None;
    }
    if let (Some(auth_header), Some(auth_value)) = (input.auth_header, input.auth_value) {
        ensure_upstream_auth_header(&mut provider_request_headers, auth_header, auth_value);
    }
    if input.behavior.upstream_is_stream {
        provider_request_headers.insert("accept".to_string(), "text/event-stream".to_string());
    }
    Some(provider_request_headers)
}

pub fn same_format_provider_transport_supported(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: SameFormatProviderFamily,
    api_format: &str,
) -> bool {
    same_format_provider_transport_unsupported_reason(behavior, transport, family, api_format)
        .is_none()
}

pub fn same_format_provider_transport_unsupported_reason(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: SameFormatProviderFamily,
    api_format: &str,
) -> Option<&'static str> {
    if behavior.is_kiro {
        local_kiro_request_transport_unsupported_reason_with_network(transport)
    } else if behavior.is_antigravity {
        None
    } else if behavior.is_claude_code {
        local_claude_code_transport_unsupported_reason_with_network(transport, api_format)
    } else if behavior.is_vertex {
        local_vertex_api_key_gemini_transport_unsupported_reason_with_network(transport)
    } else {
        match family {
            SameFormatProviderFamily::Standard => {
                local_standard_transport_unsupported_reason_with_network(transport, api_format)
            }
            SameFormatProviderFamily::Gemini => {
                local_gemini_transport_unsupported_reason_with_network(transport, api_format)
            }
        }
    }
}

pub fn same_format_provider_transport_unsupported_reason_for_trace(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
) -> Option<&'static str> {
    let normalized_api_format =
        match aether_ai_formats::normalize_api_format_alias(provider_api_format).as_str() {
            "openai:chat" => "openai:chat",
            "openai:responses" => "openai:responses",
            "openai:responses:compact" => "openai:responses:compact",
            "claude:messages" => "claude:messages",
            "gemini:generate_content" => "gemini:generate_content",
            _ => return Some("transport_api_format_unsupported"),
        };
    let behavior = classify_same_format_provider_request_behavior(
        transport,
        SameFormatProviderRequestBehaviorParams {
            require_streaming: false,
            report_kind: "trace_candidate_metadata",
        },
    );
    if !behavior.is_antigravity
        && !behavior.is_claude_code
        && !behavior.is_vertex
        && !behavior.is_kiro
    {
        return None;
    }

    let family = if normalized_api_format.starts_with("gemini:") {
        SameFormatProviderFamily::Gemini
    } else {
        SameFormatProviderFamily::Standard
    };
    same_format_provider_transport_unsupported_reason(
        &behavior,
        transport,
        family,
        normalized_api_format,
    )
}

pub fn should_try_same_format_provider_oauth_auth(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: SameFormatProviderFamily,
) -> bool {
    behavior.is_kiro
        || matches!(family, SameFormatProviderFamily::Standard)
            && resolve_local_standard_auth(transport).is_none()
        || matches!(family, SameFormatProviderFamily::Gemini)
            && !behavior.is_vertex
            && resolve_local_gemini_auth(transport).is_none()
}

pub fn resolve_same_format_provider_direct_auth(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: SameFormatProviderFamily,
) -> Option<(String, String)> {
    if behavior.is_vertex {
        None
    } else {
        match family {
            SameFormatProviderFamily::Standard => resolve_local_standard_auth(transport),
            SameFormatProviderFamily::Gemini => resolve_local_gemini_auth(transport),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };
    use serde_json::json;

    fn sample_transport(provider_type: &str) -> GatewayProviderTransportSnapshot {
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
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai:chat".to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://api.example.test".to_string(),
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
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn classifies_streaming_and_report_kind_for_provider_private_transports() {
        let kiro = sample_transport("kiro");
        let behavior = classify_same_format_provider_request_behavior(
            &kiro,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                report_kind: "claude_chat_sync_success",
            },
        );

        assert!(behavior.is_kiro);
        assert!(behavior.upstream_is_stream);
        assert_eq!(behavior.report_kind, "claude_cli_sync_finalize");

        let antigravity = sample_transport("antigravity");
        let behavior = classify_same_format_provider_request_behavior(
            &antigravity,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                report_kind: "gemini_chat_sync_success",
            },
        );

        assert!(behavior.is_antigravity);
        assert!(behavior.upstream_is_stream);
        assert_eq!(behavior.report_kind, "gemini_chat_sync_finalize");
    }

    #[test]
    fn resolves_direct_auth_except_vertex() {
        let transport = sample_transport("openai");
        let behavior = classify_same_format_provider_request_behavior(
            &transport,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                report_kind: "openai_chat_sync_success",
            },
        );

        assert_eq!(
            resolve_same_format_provider_direct_auth(
                &behavior,
                &transport,
                SameFormatProviderFamily::Standard,
            ),
            Some(("x-api-key".to_string(), "secret".to_string()))
        );
    }

    #[test]
    fn builds_same_format_standard_body_with_mapped_model_and_stream_flag() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "model": "client-model",
                "messages": [{"role": "user", "content": "hello"}]
            }),
            mapped_model: "upstream-model",
            family: SameFormatProviderFamily::Standard,
            body_rules: None,
            upstream_is_stream: true,
            kiro_auth_config: None,
            is_claude_code: false,
        })
        .expect("body should build");

        assert_eq!(body.get("model"), Some(&json!("upstream-model")));
        assert_eq!(body.get("stream"), Some(&json!(true)));
    }

    #[test]
    fn builds_same_format_headers_with_auth_and_stream_accept() {
        let provider_request_body = json!({"model": "upstream-model"});
        let original_request_body = json!({"model": "client-model"});
        let headers = build_same_format_provider_headers(SameFormatProviderHeadersInput {
            headers: &http::HeaderMap::new(),
            provider_request_body: &provider_request_body,
            original_request_body: &original_request_body,
            header_rules: None,
            behavior: SameFormatProviderRequestBehavior {
                is_antigravity: false,
                is_claude_code: false,
                is_vertex: false,
                is_kiro: false,
                upstream_is_stream: true,
                report_kind: "openai_chat_stream_success",
            },
            auth_header: Some("x-api-key"),
            auth_value: Some("secret"),
            extra_headers: &BTreeMap::new(),
            key_fingerprint: None,
            kiro_auth_config: None,
            kiro_machine_id: None,
        })
        .expect("headers should build");

        assert_eq!(headers.get("x-api-key").map(String::as_str), Some("secret"));
        assert_eq!(
            headers.get("content-type").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            headers.get("accept").map(String::as_str),
            Some("text/event-stream")
        );
    }
}
