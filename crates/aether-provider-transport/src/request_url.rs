use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;
use url::form_urlencoded;

use crate::antigravity::{
    build_antigravity_v1internal_url, is_antigravity_provider_transport,
    AntigravityRequestUrlAction,
};
use crate::claude_code::build_claude_code_messages_url;
use crate::snapshot::GatewayProviderTransportSnapshot;
use crate::url::{
    build_claude_messages_url, build_gemini_content_url, build_openai_chat_url,
    build_openai_responses_url, build_passthrough_path_url,
};
use crate::vertex::{
    build_vertex_api_key_gemini_content_url, resolve_local_vertex_api_key_query_auth,
};

#[derive(Debug, Clone, Copy)]
pub struct TransportRequestUrlParams<'a> {
    pub provider_api_format: &'a str,
    pub mapped_model: Option<&'a str>,
    pub upstream_is_stream: bool,
    pub request_query: Option<&'a str>,
    pub kiro_api_region: Option<&'a str>,
}

pub fn build_transport_request_url(
    transport: &GatewayProviderTransportSnapshot,
    params: TransportRequestUrlParams<'_>,
) -> Option<String> {
    if let Some(url) = build_transport_hook_url(transport, params) {
        return Some(url);
    }

    let provider_api_format = params.provider_api_format.trim().to_ascii_lowercase();
    let custom_path = transport
        .endpoint
        .custom_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|path| expand_custom_path_template(path, build_path_params(params)));

    if let Some(path) = custom_path.as_deref() {
        let blocked_keys = if provider_api_format.starts_with("gemini:") {
            &["key"][..]
        } else {
            &[][..]
        };
        let url = build_passthrough_path_url(
            &transport.endpoint.base_url,
            path,
            params.request_query,
            blocked_keys,
        )?;
        return Some(maybe_add_gemini_stream_alt_sse(
            url,
            &provider_api_format,
            params.upstream_is_stream,
        ));
    }

    let url = match aether_ai_formats::normalize_api_format_alias(&provider_api_format).as_str() {
        "openai:chat" => Some(build_openai_chat_url(
            &transport.endpoint.base_url,
            params.request_query,
        )),
        "openai:responses" => Some(build_openai_responses_url(
            &transport.endpoint.base_url,
            params.request_query,
            false,
        )),
        "openai:responses:compact" => Some(build_openai_responses_url(
            &transport.endpoint.base_url,
            params.request_query,
            true,
        )),
        "claude:messages" => Some(build_claude_messages_url(
            &transport.endpoint.base_url,
            params.request_query,
        )),
        "gemini:generate_content" => build_gemini_content_url(
            &transport.endpoint.base_url,
            params.mapped_model?,
            params.upstream_is_stream,
            params.request_query,
        ),
        _ => None,
    }?;

    Some(maybe_add_gemini_stream_alt_sse(
        url,
        &provider_api_format,
        params.upstream_is_stream,
    ))
}

pub fn build_local_openai_chat_upstream_url(
    transport: &GatewayProviderTransportSnapshot,
    request_query: Option<&str>,
) -> Option<String> {
    build_transport_request_url(
        transport,
        TransportRequestUrlParams {
            provider_api_format: "openai:chat",
            mapped_model: None,
            upstream_is_stream: false,
            request_query,
            kiro_api_region: None,
        },
    )
}

pub fn build_cross_format_openai_chat_upstream_url(
    transport: &GatewayProviderTransportSnapshot,
    mapped_model: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
    request_query: Option<&str>,
) -> Option<String> {
    aether_ai_formats::request_conversion_kind("openai:chat", provider_api_format)?;
    build_transport_request_url(
        transport,
        TransportRequestUrlParams {
            provider_api_format,
            mapped_model: Some(mapped_model),
            upstream_is_stream,
            request_query,
            kiro_api_region: None,
        },
    )
}

pub fn build_local_openai_responses_upstream_url(
    transport: &GatewayProviderTransportSnapshot,
    compact: bool,
    request_query: Option<&str>,
) -> Option<String> {
    let provider_api_format = if compact {
        "openai:responses:compact"
    } else {
        "openai:responses"
    };
    build_transport_request_url(
        transport,
        TransportRequestUrlParams {
            provider_api_format,
            mapped_model: None,
            upstream_is_stream: false,
            request_query,
            kiro_api_region: None,
        },
    )
}

pub fn build_cross_format_openai_responses_upstream_url(
    transport: &GatewayProviderTransportSnapshot,
    mapped_model: &str,
    client_api_format: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
    request_query: Option<&str>,
) -> Option<String> {
    aether_ai_formats::request_conversion_kind(client_api_format, provider_api_format)?;
    build_transport_request_url(
        transport,
        TransportRequestUrlParams {
            provider_api_format,
            mapped_model: Some(mapped_model),
            upstream_is_stream,
            request_query,
            kiro_api_region: None,
        },
    )
}

pub fn build_kiro_cross_format_upstream_url(
    transport: &GatewayProviderTransportSnapshot,
    mapped_model: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
    request_query: Option<&str>,
    api_region: &str,
) -> Option<String> {
    build_transport_request_url(
        transport,
        TransportRequestUrlParams {
            provider_api_format,
            mapped_model: Some(mapped_model),
            upstream_is_stream,
            request_query,
            kiro_api_region: Some(api_region),
        },
    )
}

fn build_transport_hook_url(
    transport: &GatewayProviderTransportSnapshot,
    params: TransportRequestUrlParams<'_>,
) -> Option<String> {
    if let Some(api_region) = params.kiro_api_region {
        return crate::kiro::build_kiro_generate_assistant_response_url(
            &transport.endpoint.base_url,
            params.request_query,
            Some(api_region),
        );
    }

    if transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("claude_code")
    {
        return Some(build_claude_code_messages_url(
            &transport.endpoint.base_url,
            params.request_query,
        ));
    }

    if params
        .provider_api_format
        .trim()
        .to_ascii_lowercase()
        .starts_with("gemini:")
    {
        if let Some(auth) = resolve_local_vertex_api_key_query_auth(transport) {
            return build_vertex_api_key_gemini_content_url(
                params.mapped_model?,
                params.upstream_is_stream,
                &auth.value,
                params.request_query,
            );
        }
    }

    if is_antigravity_provider_transport(transport) {
        let query = params.request_query.map(|raw| {
            form_urlencoded::parse(raw.as_bytes())
                .into_owned()
                .collect::<BTreeMap<String, String>>()
        });
        return build_antigravity_v1internal_url(
            &transport.endpoint.base_url,
            if params.upstream_is_stream {
                AntigravityRequestUrlAction::StreamGenerateContent
            } else {
                AntigravityRequestUrlAction::GenerateContent
            },
            query.as_ref(),
        );
    }

    None
}

fn build_path_params(params: TransportRequestUrlParams<'_>) -> BTreeMap<&'static str, &str> {
    let mut path_params = BTreeMap::new();
    if let Some(model) = params
        .mapped_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        path_params.insert("model", model);
    }
    if params
        .provider_api_format
        .trim()
        .to_ascii_lowercase()
        .starts_with("gemini:")
    {
        path_params.insert(
            "action",
            if params.upstream_is_stream {
                "streamGenerateContent"
            } else {
                "generateContent"
            },
        );
    }
    path_params
}

fn expand_custom_path_template(path: &str, params: BTreeMap<&'static str, &str>) -> String {
    if params.is_empty() {
        return path.to_string();
    }

    let regex = custom_path_template_regex();
    let mut missing_key = false;
    let replaced = regex.replace_all(path, |captures: &regex::Captures<'_>| {
        let key = captures
            .get(1)
            .map(|value| value.as_str())
            .unwrap_or_default();
        match params.get(key).copied() {
            Some(value) => value.to_string(),
            None => {
                missing_key = true;
                captures
                    .get(0)
                    .map(|value| value.as_str().to_string())
                    .unwrap_or_default()
            }
        }
    });

    if missing_key {
        path.to_string()
    } else {
        replaced.into_owned()
    }
}

fn maybe_add_gemini_stream_alt_sse(
    upstream_url: String,
    provider_api_format: &str,
    upstream_is_stream: bool,
) -> String {
    if !provider_api_format.starts_with("gemini:") || !upstream_is_stream {
        return upstream_url;
    }

    let has_alt = upstream_url
        .split_once('?')
        .map(|(_, query)| {
            form_urlencoded::parse(query.as_bytes())
                .any(|(key, _)| key.as_ref().eq_ignore_ascii_case("alt"))
        })
        .unwrap_or(false);
    if has_alt {
        return upstream_url;
    }

    if upstream_url.contains('?') {
        format!("{upstream_url}&alt=sse")
    } else {
        format!("{upstream_url}?alt=sse")
    }
}

fn custom_path_template_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"\{([A-Za-z_][A-Za-z0-9_]*)\}")
            .expect("custom_path template regex should compile")
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_kiro_cross_format_upstream_url, build_transport_request_url,
        TransportRequestUrlParams,
    };
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport(
        provider_type: &str,
        api_format: &str,
        base_url: &str,
        custom_path: Option<&str>,
    ) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "provider".to_string(),
                provider_type: provider_type.to_string(),
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
                api_format: api_format.to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: base_url.to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: custom_path.map(ToOwned::to_owned),
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
                decrypted_api_key: "vertex-secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn uses_vertex_hook_before_custom_path_for_custom_aiplatform_transport() {
        let transport = sample_transport(
            "custom",
            "gemini:generate_content",
            "https://aiplatform.googleapis.com",
            Some("/custom/{model}:{action}"),
        );

        let url = build_transport_request_url(
            &transport,
            TransportRequestUrlParams {
                provider_api_format: "gemini:generate_content",
                mapped_model: Some("gemini-3.1-pro-preview"),
                upstream_is_stream: true,
                request_query: Some("foo=bar"),
                kiro_api_region: None,
            },
        )
        .expect("vertex hook url");

        assert_eq!(
            url,
            "https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-3.1-pro-preview:streamGenerateContent?alt=sse&foo=bar&key=vertex-secret"
        );
    }

    #[test]
    fn builds_openai_responses_url_for_formal_format_name() {
        let transport = sample_transport(
            "openai",
            "openai:responses",
            "https://api.openai.example/v1",
            None,
        );

        let url = build_transport_request_url(
            &transport,
            TransportRequestUrlParams {
                provider_api_format: "openai:responses",
                mapped_model: None,
                upstream_is_stream: false,
                request_query: Some("tenant=demo"),
                kiro_api_region: None,
            },
        )
        .expect("openai responses url");

        assert_eq!(url, "https://api.openai.example/v1/responses?tenant=demo");
    }

    #[test]
    fn expands_custom_path_templates_when_hook_does_not_apply() {
        let transport = sample_transport(
            "custom",
            "gemini:generate_content",
            "https://generativelanguage.googleapis.com",
            Some("/v1beta/models/{model}:{action}"),
        );

        let url = build_transport_request_url(
            &transport,
            TransportRequestUrlParams {
                provider_api_format: "gemini:generate_content",
                mapped_model: Some("gemini-2.5-pro"),
                upstream_is_stream: false,
                request_query: Some("key=client-key&foo=bar"),
                kiro_api_region: None,
            },
        )
        .expect("expanded custom path url");

        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent?foo=bar"
        );
    }

    #[test]
    fn keeps_original_custom_path_when_template_params_are_missing() {
        let transport = sample_transport(
            "custom",
            "claude:messages",
            "https://api.example.com",
            Some("/v1/messages/{model}"),
        );

        let url = build_transport_request_url(
            &transport,
            TransportRequestUrlParams {
                provider_api_format: "claude:messages",
                mapped_model: None,
                upstream_is_stream: false,
                request_query: None,
                kiro_api_region: None,
            },
        )
        .expect("fallback custom path url");

        assert_eq!(url, "https://api.example.com/v1/messages/{model}");
    }

    #[test]
    fn kiro_cross_format_helper_uses_region_specific_generate_assistant_url() {
        let transport = sample_transport(
            "kiro",
            "claude:messages",
            "https://codewhisperer.{region}.amazonaws.com/",
            None,
        );

        let url = build_kiro_cross_format_upstream_url(
            &transport,
            "claude-sonnet-4",
            "claude:messages",
            true,
            Some("conversationId=abc"),
            "us-west-2",
        )
        .expect("kiro url");

        assert!(url.starts_with(
            "https://codewhisperer.us-west-2.amazonaws.com/generateAssistantResponse"
        ));
        assert!(url.contains("conversationId=abc"));
    }
}
