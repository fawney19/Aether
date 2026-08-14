use std::collections::BTreeMap;
use std::sync::OnceLock;

use aether_ai_formats::ApiOperation;
use regex::Regex;
use serde_json::Value;
use url::form_urlencoded;

use crate::antigravity::{
    build_antigravity_v1internal_url, is_antigravity_provider_transport,
    AntigravityRequestUrlAction,
};
use crate::claude_code::build_claude_code_messages_url;
use crate::gemini_cli::{
    build_gemini_cli_v1internal_url, is_gemini_cli_provider_transport, GeminiCliRequestUrlAction,
};
use crate::snapshot::GatewayProviderTransportSnapshot;
use crate::url::{
    build_claude_count_tokens_url as build_default_claude_count_tokens_url,
    build_claude_messages_url, build_gemini_content_url, build_openai_chat_url,
    build_openai_responses_url, build_openai_search_url, build_passthrough_path_url,
    normalize_gemini_content_action_path, strip_gateway_credential_query_parameters,
    GATEWAY_CREDENTIAL_QUERY_KEYS,
};
use crate::vertex::{
    build_vertex_api_key_gemini_content_url, build_vertex_api_key_gemini_embedding_url,
    build_vertex_service_account_gemini_content_url,
    build_vertex_service_account_gemini_embedding_url, resolve_local_vertex_api_key_query_auth,
    resolve_local_vertex_service_account_auth_config,
};
#[derive(Debug, Clone, Copy)]
pub struct TransportRequestUrlParams<'a> {
    pub provider_api_format: &'a str,
    pub mapped_model: Option<&'a str>,
    pub upstream_is_stream: bool,
    pub request_query: Option<&'a str>,
    pub kiro_api_region: Option<&'a str>,
    pub api_operation: Option<aether_ai_formats::ApiOperation>,
}

pub fn build_transport_request_url(
    transport: &GatewayProviderTransportSnapshot,
    params: TransportRequestUrlParams<'_>,
) -> Option<String> {
    build_transport_request_url_inner(transport, params, false)
}

pub fn build_transport_request_url_for_request_body(
    transport: &GatewayProviderTransportSnapshot,
    params: TransportRequestUrlParams<'_>,
    provider_request_body: Option<&Value>,
) -> Option<String> {
    let gemini_embedding_batch =
        gemini_embedding_request_body_uses_batch(params.provider_api_format, provider_request_body);
    build_transport_request_url_inner(transport, params, gemini_embedding_batch)
}

pub fn gemini_embedding_request_body_uses_batch(
    provider_api_format: &str,
    provider_request_body: Option<&Value>,
) -> bool {
    aether_ai_formats::normalize_api_format_alias(provider_api_format) == "gemini:embedding"
        && provider_request_body
            .and_then(|body| body.get("requests"))
            .and_then(Value::as_array)
            .is_some_and(|requests| !requests.is_empty())
}

fn build_transport_request_url_inner(
    transport: &GatewayProviderTransportSnapshot,
    params: TransportRequestUrlParams<'_>,
    gemini_embedding_batch: bool,
) -> Option<String> {
    let provider_api_format = params.provider_api_format.trim().to_ascii_lowercase();
    let normalized_provider_api_format =
        aether_ai_formats::normalize_api_format_alias(&provider_api_format);
    let sanitized_claude_request_query = (normalized_provider_api_format == "claude:messages")
        .then(|| strip_gateway_credential_query_parameters(params.request_query))
        .flatten();
    let params = if normalized_provider_api_format == "claude:messages" {
        TransportRequestUrlParams {
            request_query: sanitized_claude_request_query.as_deref(),
            ..params
        }
    } else {
        params
    };
    let is_claude_count_tokens = normalized_provider_api_format == "claude:messages"
        && params.api_operation == Some(ApiOperation::ClaudeCountTokens);
    if !transport_supports_api_operation(
        transport,
        normalized_provider_api_format.as_str(),
        params.api_operation,
    ) {
        return None;
    }
    if is_claude_count_tokens {
        if let Some(url) = build_configured_claude_count_tokens_url(transport, params.request_query)
        {
            return Some(url);
        }
    }
    if let Some(url) = build_transport_hook_url(transport, params) {
        return Some(url);
    }

    let custom_path_template = transport
        .endpoint
        .custom_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let custom_path_handles_operation =
        custom_path_template.is_some_and(|path| path.contains("{operation}"));
    let custom_path = custom_path_template.map(|path| {
        expand_custom_path_template(path, build_path_params(params, gemini_embedding_batch))
    });

    if let Some(path) = custom_path.as_deref() {
        let custom_path_is_complete_claude_count_tokens = normalized_provider_api_format
            == "claude:messages"
            && !custom_path_handles_operation
            && path
                .split_once('?')
                .map(|(path, _)| path)
                .unwrap_or(path)
                .trim_end_matches('/')
                .ends_with("/messages/count_tokens");
        let blocked_keys = if normalized_provider_api_format.starts_with("gemini:")
            || normalized_provider_api_format == "claude:messages"
        {
            GATEWAY_CREDENTIAL_QUERY_KEYS
        } else {
            &[][..]
        };
        let normalized_path = if normalized_provider_api_format == "gemini:generate_content" {
            normalize_gemini_content_action_path(path, params.upstream_is_stream)
        } else if normalized_provider_api_format == "gemini:embedding" {
            normalize_gemini_embedding_action_path(path, gemini_embedding_batch)
        } else {
            path.to_string()
        };
        let mut url = build_passthrough_path_url(
            &transport.endpoint.base_url,
            normalized_path.as_str(),
            params.request_query,
            blocked_keys,
        )?;
        if is_claude_count_tokens && !custom_path_handles_operation {
            url = build_default_claude_count_tokens_url(&url, None);
        } else if params.api_operation == Some(ApiOperation::ClaudeMessagesCreate)
            && custom_path_is_complete_claude_count_tokens
        {
            url = build_claude_messages_url(&url, None);
        }
        return Some(maybe_add_gemini_stream_alt_sse(
            url,
            &provider_api_format,
            params.upstream_is_stream,
        ));
    }

    let url = match normalized_provider_api_format.as_str() {
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
        "openai:search" => Some(build_openai_search_url(
            &transport.endpoint.base_url,
            params.request_query,
        )),
        "openai:embedding" | "jina:embedding" => {
            build_provider_embedding_v1_url(&transport.endpoint.base_url, params.request_query)
        }
        "aliyun:multimodal_embedding" => build_aliyun_multimodal_embedding_url(
            &transport.endpoint.base_url,
            params.request_query,
        ),
        "openai:rerank" | "jina:rerank" => {
            build_provider_rerank_v1_url(&transport.endpoint.base_url, params.request_query)
        }
        "claude:messages" => Some(if is_claude_count_tokens {
            build_default_claude_count_tokens_url(
                &transport.endpoint.base_url,
                params.request_query,
            )
        } else {
            build_claude_messages_url(&transport.endpoint.base_url, params.request_query)
        }),
        "gemini:generate_content" => build_gemini_content_url(
            &transport.endpoint.base_url,
            params.mapped_model?,
            params.upstream_is_stream,
            params.request_query,
        ),
        "gemini:embedding" => build_gemini_embedding_url(
            &transport.endpoint.base_url,
            params.mapped_model?,
            params.request_query,
            gemini_embedding_batch,
        ),
        "gemini:interactions" => {
            build_gemini_interactions_url(&transport.endpoint.base_url, params.request_query)
        }
        "doubao:embedding" => build_passthrough_path_url(
            &transport.endpoint.base_url,
            "/embeddings",
            params.request_query,
            &[],
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
            api_operation: None,
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
            api_operation: None,
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
            api_operation: None,
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
            api_operation: None,
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
            api_operation: None,
        },
    )
}

fn build_transport_hook_url(
    transport: &GatewayProviderTransportSnapshot,
    params: TransportRequestUrlParams<'_>,
) -> Option<String> {
    if let Some(api_region) = params.kiro_api_region {
        return crate::kiro::build_kiro_generate_assistant_response_url(
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
        let messages_url =
            build_claude_code_messages_url(&transport.endpoint.base_url, params.request_query);
        return Some(
            if params.api_operation == Some(aether_ai_formats::ApiOperation::ClaudeCountTokens) {
                build_default_claude_count_tokens_url(&messages_url, None)
            } else {
                messages_url
            },
        );
    }

    let normalized_provider_api_format =
        aether_ai_formats::normalize_api_format_alias(params.provider_api_format);
    match normalized_provider_api_format.as_str() {
        "gemini:generate_content" => {
            if is_gemini_cli_provider_transport(transport) {
                let query = params.request_query.map(|raw| {
                    form_urlencoded::parse(raw.as_bytes())
                        .into_owned()
                        .collect::<BTreeMap<String, String>>()
                });
                return build_gemini_cli_v1internal_url(
                    &transport.endpoint.base_url,
                    if params.upstream_is_stream {
                        GeminiCliRequestUrlAction::StreamGenerateContent
                    } else {
                        GeminiCliRequestUrlAction::GenerateContent
                    },
                    query.as_ref(),
                );
            }
            if let Some(auth) = resolve_local_vertex_api_key_query_auth(transport) {
                return build_vertex_api_key_gemini_content_url(
                    params.mapped_model?,
                    params.upstream_is_stream,
                    &auth.value,
                    params.request_query,
                );
            }
            if let Some(auth_config) = resolve_local_vertex_service_account_auth_config(transport) {
                return build_vertex_service_account_gemini_content_url(
                    params.mapped_model?,
                    params.upstream_is_stream,
                    &auth_config,
                    params.request_query,
                );
            }
        }
        "gemini:embedding" => {
            if let Some(auth) = resolve_local_vertex_api_key_query_auth(transport) {
                return build_vertex_api_key_gemini_embedding_url(
                    params.mapped_model?,
                    &auth.value,
                    params.request_query,
                );
            }
            if let Some(auth_config) = resolve_local_vertex_service_account_auth_config(transport) {
                return build_vertex_service_account_gemini_embedding_url(
                    params.mapped_model?,
                    &auth_config,
                    params.request_query,
                );
            }
        }
        _ => {}
    }

    if is_antigravity_provider_transport(transport)
        && normalized_provider_api_format == "gemini:generate_content"
    {
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

    if is_gemini_cli_provider_transport(transport)
        && normalized_provider_api_format == "gemini:generate_content"
    {
        let query = params.request_query.map(|raw| {
            form_urlencoded::parse(raw.as_bytes())
                .into_owned()
                .collect::<BTreeMap<String, String>>()
        });
        return build_gemini_cli_v1internal_url(
            &transport.endpoint.base_url,
            if params.upstream_is_stream {
                GeminiCliRequestUrlAction::StreamGenerateContent
            } else {
                GeminiCliRequestUrlAction::GenerateContent
            },
            query.as_ref(),
        );
    }

    None
}

fn build_path_params(
    params: TransportRequestUrlParams<'_>,
    gemini_embedding_batch: bool,
) -> BTreeMap<&'static str, &str> {
    let mut path_params = BTreeMap::new();
    if let Some(model) = params
        .mapped_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        path_params.insert("model", model);
    }
    let provider_api_format =
        aether_ai_formats::normalize_api_format_alias(params.provider_api_format);
    if let Some(operation) = params.api_operation {
        path_params.insert("operation", operation.as_str());
    }
    if provider_api_format == "gemini:generate_content" || provider_api_format == "gemini:embedding"
    {
        path_params.insert(
            "action",
            if provider_api_format == "gemini:embedding" {
                if gemini_embedding_batch {
                    "batchEmbedContents"
                } else {
                    "embedContent"
                }
            } else if params.upstream_is_stream {
                "streamGenerateContent"
            } else {
                "generateContent"
            },
        );
    }
    path_params
}

pub fn transport_supports_api_operation(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
    operation: Option<ApiOperation>,
) -> bool {
    if operation != Some(ApiOperation::ClaudeCountTokens) {
        return true;
    }
    if aether_ai_formats::normalize_api_format_alias(provider_api_format) != "claude:messages" {
        return false;
    }

    anthropic_count_tokens_supported(transport)
}

fn anthropic_count_tokens_supported(transport: &GatewayProviderTransportSnapshot) -> bool {
    // Private message adapters do not implement Anthropic's token-counting
    // operation. A config flag cannot make their request envelopes compatible.
    if crate::kiro::is_kiro_provider_transport(transport)
        || crate::grok::is_grok_provider_transport(transport)
    {
        return false;
    }

    let Some(operations) = anthropic_transport_config_field(transport, "supported_operations")
    else {
        return true;
    };
    operations.as_array().is_some_and(|operations| {
        operations.iter().any(|operation| {
            operation
                .as_str()
                .is_some_and(|value| value.eq_ignore_ascii_case("count_tokens"))
        })
    })
}

fn build_configured_claude_count_tokens_url(
    transport: &GatewayProviderTransportSnapshot,
    request_query: Option<&str>,
) -> Option<String> {
    let path = anthropic_transport_config_field(transport, "count_tokens_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let path = path?;
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    build_passthrough_path_url(
        &transport.endpoint.base_url,
        path.as_str(),
        request_query,
        GATEWAY_CREDENTIAL_QUERY_KEYS,
    )
}

fn anthropic_transport_config_field<'a>(
    transport: &'a GatewayProviderTransportSnapshot,
    field: &str,
) -> Option<&'a Value> {
    let from_config = |config: Option<&'a Value>| {
        config
            .and_then(Value::as_object)
            .and_then(|config| config.get("anthropic"))
            .and_then(Value::as_object)
            .and_then(|anthropic| anthropic.get(field))
    };
    from_config(transport.endpoint.config.as_ref())
        .or_else(|| from_config(transport.provider.config.as_ref()))
}

fn normalize_gemini_embedding_action_path(path: &str, batch: bool) -> String {
    if batch {
        path.replace(":embedContent", ":batchEmbedContents")
    } else {
        path.replace(":batchEmbedContents", ":embedContent")
    }
}

fn build_provider_embedding_v1_url(upstream_base_url: &str, query: Option<&str>) -> Option<String> {
    build_provider_api_root_url(upstream_base_url, "/embeddings", query)
}

fn build_aliyun_multimodal_embedding_url(
    upstream_base_url: &str,
    query: Option<&str>,
) -> Option<String> {
    build_passthrough_path_url(
        upstream_base_url,
        "/api/v1/services/embeddings/multimodal-embedding/multimodal-embedding",
        query,
        &[],
    )
}

fn build_provider_rerank_v1_url(upstream_base_url: &str, query: Option<&str>) -> Option<String> {
    build_provider_api_root_url(upstream_base_url, "/rerank", query)
}

fn build_gemini_interactions_url(upstream_base_url: &str, query: Option<&str>) -> Option<String> {
    build_passthrough_path_url(upstream_base_url, "/v1/interactions", query, &["key"])
}

fn build_provider_api_root_url(
    upstream_base_url: &str,
    path: &str,
    query: Option<&str>,
) -> Option<String> {
    build_passthrough_path_url(upstream_base_url, path, query, &[])
}

fn build_gemini_embedding_url(
    upstream_base_url: &str,
    model: &str,
    query: Option<&str>,
    batch: bool,
) -> Option<String> {
    let trimmed_base_url = upstream_base_url
        .trim()
        .split_once('?')
        .map(|(base, _)| base)
        .unwrap_or_else(|| upstream_base_url.trim())
        .trim_end_matches('/');
    let trimmed_model = model.trim();
    if trimmed_base_url.is_empty() || trimmed_model.is_empty() {
        return None;
    }

    let action = if batch {
        "batchEmbedContents"
    } else {
        "embedContent"
    };
    let path = if trimmed_base_url.ends_with("/v1beta") {
        format!("/models/{trimmed_model}:{action}")
    } else if trimmed_base_url.contains("/v1beta/models/") {
        format!(":{action}")
    } else {
        format!("/v1beta/models/{trimmed_model}:{action}")
    };
    build_passthrough_path_url(upstream_base_url, &path, query, &["key"])
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
    if aether_ai_formats::normalize_api_format_alias(provider_api_format)
        != "gemini:generate_content"
        || !upstream_is_stream
    {
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
    use aether_ai_formats::ApiOperation;

    use super::{
        build_kiro_cross_format_upstream_url, build_transport_request_url,
        build_transport_request_url_for_request_body, TransportRequestUrlParams,
    };
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use serde_json::json;

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
                upstream_metadata: None,
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
                api_operation: None,
            },
        )
        .expect("vertex hook url");

        assert_eq!(
            url,
            "https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-3.1-pro-preview:streamGenerateContent?alt=sse&foo=bar&key=vertex-secret"
        );
    }

    #[test]
    fn uses_vertex_service_account_hook_before_default_gemini_url() {
        let mut transport = sample_transport(
            "vertex_ai",
            "gemini:generate_content",
            "https://aiplatform.googleapis.com",
            None,
        );
        transport.key.auth_type = "service_account".to_string();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config = Some(
            r#"{
                "client_email":"svc@example.iam.gserviceaccount.com",
                "private_key":"TEST-PRIVATE-KEY",
                "project_id":"demo-project"
            }"#
            .to_string(),
        );

        let url = build_transport_request_url(
            &transport,
            TransportRequestUrlParams {
                provider_api_format: "gemini:generate_content",
                mapped_model: Some("gemini-3.1-pro-preview"),
                upstream_is_stream: false,
                request_query: Some("foo=bar&beta=1"),
                kiro_api_region: None,
                api_operation: None,
            },
        )
        .expect("vertex service account hook url");

        assert_eq!(
            url,
            "https://aiplatform.googleapis.com/v1/projects/demo-project/locations/global/publishers/google/models/gemini-3.1-pro-preview:generateContent?foo=bar"
        );
    }

    #[test]
    fn uses_vertex_service_account_hook_for_gemini_embedding_url() {
        let mut transport = sample_transport(
            "vertex_ai",
            "gemini:embedding",
            "https://aiplatform.googleapis.com",
            None,
        );
        transport.endpoint.endpoint_kind = Some("embedding".to_string());
        transport.key.auth_type = "service_account".to_string();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config = Some(
            r#"{
                "client_email":"svc@example.iam.gserviceaccount.com",
                "private_key":"TEST-PRIVATE-KEY",
                "project_id":"demo-project"
            }"#
            .to_string(),
        );

        let provider_request_body = json!({
            "content": {"parts": [{"text": "hello"}]}
        });
        let url = build_transport_request_url_for_request_body(
            &transport,
            TransportRequestUrlParams {
                provider_api_format: "gemini:embedding",
                mapped_model: Some("gemini-embedding-2"),
                upstream_is_stream: false,
                request_query: Some("foo=bar&beta=1"),
                kiro_api_region: None,
                api_operation: None,
            },
            Some(&provider_request_body),
        )
        .expect("vertex embedding service account hook url");

        assert_eq!(
            url,
            "https://aiplatform.googleapis.com/v1/projects/demo-project/locations/global/publishers/google/models/gemini-embedding-2:predict?foo=bar"
        );
    }

    #[test]
    fn gemini_cli_generate_content_uses_v1internal_code_assist_url() {
        let transport = sample_transport(
            "gemini_cli",
            "gemini:generate_content",
            "https://cloudcode-pa.googleapis.com",
            None,
        );

        assert_eq!(
            build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "gemini:generate_content",
                    mapped_model: Some("gemini-2.5-pro"),
                    upstream_is_stream: false,
                    request_query: Some("key=blocked&beta=true&foo=bar"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://cloudcode-pa.googleapis.com/v1internal:generateContent?foo=bar")
        );
    }

    #[test]
    fn gemini_cli_stream_generate_content_uses_v1internal_code_assist_url() {
        let transport = sample_transport(
            "gemini_cli",
            "gemini:generate_content",
            "https://cloudcode-pa.googleapis.com",
            None,
        );

        assert_eq!(
            build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "gemini:generate_content",
                    mapped_model: Some("gemini-2.5-pro"),
                    upstream_is_stream: true,
                    request_query: Some("foo=bar"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some(
                "https://cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse&foo=bar"
            )
        );
    }

    #[test]
    fn vertex_gemini_embedding_batch_request_uses_vertex_predict_endpoint() {
        let mut transport = sample_transport(
            "vertex_ai",
            "gemini:embedding",
            "https://aiplatform.googleapis.com",
            None,
        );
        transport.endpoint.endpoint_kind = Some("embedding".to_string());
        transport.key.auth_type = "service_account".to_string();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config = Some(
            r#"{
                "client_email":"svc@example.iam.gserviceaccount.com",
                "private_key":"TEST-PRIVATE-KEY",
                "project_id":"demo-project"
            }"#
            .to_string(),
        );

        let batch_body = json!({
            "requests": [
                {
                    "model": "models/gemini-embedding-2",
                    "content": {"parts": [{"text": "alpha"}]}
                }
            ]
        });

        assert_eq!(
            build_transport_request_url_for_request_body(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "gemini:embedding",
                    mapped_model: Some("gemini-embedding-2"),
                    upstream_is_stream: false,
                    request_query: None,
                    kiro_api_region: None,
                    api_operation: None,
                },
                Some(&batch_body),
            )
            .as_deref(),
            Some(
                "https://aiplatform.googleapis.com/v1/projects/demo-project/locations/global/publishers/google/models/gemini-embedding-2:predict"
            )
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
                api_operation: None,
            },
        )
        .expect("openai responses url");

        assert_eq!(url, "https://api.openai.example/v1/responses?tenant=demo");
    }

    #[test]
    fn builds_openai_search_url_for_codex_provider_root() {
        let transport = sample_transport(
            "codex",
            "openai:search",
            "https://chatgpt.com/backend-api/codex",
            None,
        );

        let url = build_transport_request_url(
            &transport,
            TransportRequestUrlParams {
                provider_api_format: "openai:search",
                mapped_model: Some("gpt-5.6-luna"),
                upstream_is_stream: false,
                request_query: Some("tenant=demo"),
                kiro_api_region: None,
                api_operation: None,
            },
        )
        .expect("openai search url");

        assert_eq!(
            url,
            "https://chatgpt.com/backend-api/codex/alpha/search?tenant=demo"
        );
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
                api_operation: None,
            },
        )
        .expect("expanded custom path url");

        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent?foo=bar"
        );
    }

    #[test]
    fn rewrites_hardcoded_gemini_custom_path_action_to_match_stream_mode() {
        let stream_transport = sample_transport(
            "custom",
            "gemini:generate_content",
            "https://generativelanguage.googleapis.com",
            Some("/v1beta/models/{model}:generateContent"),
        );

        let stream_url = build_transport_request_url(
            &stream_transport,
            TransportRequestUrlParams {
                provider_api_format: "gemini:generate_content",
                mapped_model: Some("gemini-2.5-pro"),
                upstream_is_stream: true,
                request_query: Some("key=client-key&foo=bar"),
                kiro_api_region: None,
                api_operation: None,
            },
        )
        .expect("stream custom path url");

        assert_eq!(
            stream_url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?foo=bar&alt=sse"
        );

        let sync_transport = sample_transport(
            "custom",
            "gemini:generate_content",
            "https://generativelanguage.googleapis.com",
            Some("/v1beta/models/{model}:streamGenerateContent"),
        );

        let sync_url = build_transport_request_url(
            &sync_transport,
            TransportRequestUrlParams {
                provider_api_format: "gemini:generate_content",
                mapped_model: Some("gemini-2.5-pro"),
                upstream_is_stream: false,
                request_query: Some("foo=bar"),
                kiro_api_region: None,
                api_operation: None,
            },
        )
        .expect("sync custom path url");

        assert_eq!(
            sync_url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent?foo=bar"
        );

        let v1_transport = sample_transport(
            "custom",
            "gemini:generate_content",
            "https://generativelanguage.googleapis.com",
            Some("/v1/models/{model}:generateContent"),
        );

        let v1_stream_url = build_transport_request_url(
            &v1_transport,
            TransportRequestUrlParams {
                provider_api_format: "gemini:generate_content",
                mapped_model: Some("gemini-2.5-pro"),
                upstream_is_stream: true,
                request_query: None,
                kiro_api_region: None,
                api_operation: None,
            },
        )
        .expect("v1 stream custom path url");

        assert_eq!(
            v1_stream_url,
            "https://generativelanguage.googleapis.com/v1/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
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
                api_operation: None,
            },
        )
        .expect("fallback custom path url");

        assert_eq!(url, "https://api.example.com/v1/messages/{model}");
    }

    #[test]
    fn strips_gateway_query_key_from_claude_messages_url() {
        let transport = sample_transport(
            "custom",
            "claude:messages",
            "https://api.anthropic.example/v1?region=us",
            None,
        );

        let url = build_transport_request_url(
            &transport,
            TransportRequestUrlParams {
                provider_api_format: "claude:messages",
                mapped_model: Some("claude-sonnet-4"),
                upstream_is_stream: false,
                request_query: Some("key=gateway-secret&trace=1"),
                kiro_api_region: None,
                api_operation: Some(aether_ai_formats::ApiOperation::ClaudeMessagesCreate),
            },
        )
        .expect("messages url");

        assert_eq!(
            url,
            "https://api.anthropic.example/v1/messages?region=us&trace=1"
        );
        assert!(!url.contains("gateway-secret"));
    }

    #[test]
    fn routes_claude_count_tokens_as_an_operation_on_messages_format() {
        let mut transport = sample_transport(
            "custom",
            "claude:messages",
            "https://api.anthropic.example/v1",
            None,
        );
        transport.endpoint.config = Some(json!({
            "anthropic": {
                "supported_operations": ["messages", "count_tokens"]
            }
        }));

        let url = build_transport_request_url(
            &transport,
            TransportRequestUrlParams {
                provider_api_format: "claude:messages",
                mapped_model: Some("claude-sonnet-4"),
                upstream_is_stream: false,
                request_query: Some("key=gateway-secret&trace=1"),
                kiro_api_region: None,
                api_operation: Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
            },
        )
        .expect("count_tokens url");

        assert_eq!(
            url,
            "https://api.anthropic.example/v1/messages/count_tokens?trace=1"
        );
    }

    #[test]
    fn count_tokens_default_preserves_custom_anthropic_prefix() {
        let transport = sample_transport(
            "custom",
            "claude:messages",
            "https://proxy.example/anthropic?key=base-secret&tenant=base",
            None,
        );

        assert_eq!(
            build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "claude:messages",
                    mapped_model: Some("claude-sonnet-4"),
                    upstream_is_stream: false,
                    request_query: Some("KEY=client-secret&trace=1"),
                    kiro_api_region: None,
                    api_operation: Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
                },
            )
            .as_deref(),
            Some(
                "https://proxy.example/anthropic/messages/count_tokens?key=base-secret&tenant=base&trace=1"
            )
        );
    }

    #[test]
    fn count_tokens_uses_operation_aware_custom_path() {
        let mut transport = sample_transport(
            "custom",
            "claude:messages",
            "https://proxy.example/anthropic",
            Some("/operations/{operation}"),
        );
        transport.endpoint.config = Some(json!({
            "anthropic": {"supported_operations": ["messages", "count_tokens"]}
        }));

        assert_eq!(
            build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "claude:messages",
                    mapped_model: Some("claude-sonnet-4"),
                    upstream_is_stream: false,
                    request_query: Some("key=client-secret&trace=1"),
                    kiro_api_region: None,
                    api_operation: Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
                },
            )
            .as_deref(),
            Some("https://proxy.example/anthropic/operations/count_tokens?trace=1")
        );
    }

    #[test]
    fn count_tokens_derives_from_messages_only_custom_path() {
        let transport = sample_transport(
            "custom",
            "claude:messages",
            "https://api.anthropic.example",
            Some("/custom/v1/messages"),
        );

        assert_eq!(
            build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "claude:messages",
                    mapped_model: Some("claude-sonnet-4"),
                    upstream_is_stream: false,
                    request_query: None,
                    kiro_api_region: None,
                    api_operation: Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
                },
            )
            .as_deref(),
            Some("https://api.anthropic.example/custom/v1/messages/count_tokens")
        );
    }

    #[test]
    fn count_tokens_keeps_complete_custom_count_tokens_path() {
        let transport = sample_transport(
            "custom",
            "claude:messages",
            "https://api.anthropic.example",
            Some("/custom/v1/messages/count_tokens?key=path-secret"),
        );

        assert_eq!(
            build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "claude:messages",
                    mapped_model: Some("claude-sonnet-4"),
                    upstream_is_stream: false,
                    request_query: Some("key=client-secret&trace=1"),
                    kiro_api_region: None,
                    api_operation: Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
                },
            )
            .as_deref(),
            Some(
                "https://api.anthropic.example/custom/v1/messages/count_tokens?key=path-secret&trace=1"
            )
        );
    }

    #[test]
    fn messages_create_normalizes_complete_custom_count_tokens_path() {
        let transport = sample_transport(
            "custom",
            "claude:messages",
            "https://api.anthropic.example",
            Some("/custom/v1/messages/count_tokens?key=path-secret"),
        );

        assert_eq!(
            build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "claude:messages",
                    mapped_model: Some("claude-sonnet-4"),
                    upstream_is_stream: false,
                    request_query: Some("key=client-secret&trace=1"),
                    kiro_api_region: None,
                    api_operation: Some(aether_ai_formats::ApiOperation::ClaudeMessagesCreate),
                },
            )
            .as_deref(),
            Some("https://api.anthropic.example/custom/v1/messages?key=path-secret&trace=1")
        );
    }

    #[test]
    fn private_anthropic_adapters_reject_count_tokens_even_when_config_claims_support() {
        for provider_type in ["kiro", "grok"] {
            let mut transport = sample_transport(
                provider_type,
                "claude:messages",
                "https://private.example",
                None,
            );
            transport.endpoint.config = Some(json!({
                "anthropic": {
                    "supported_operations": ["messages", "count_tokens"],
                    "count_tokens_path": "/v1/messages/count_tokens"
                }
            }));

            assert!(build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "claude:messages",
                    mapped_model: Some("claude-sonnet-4"),
                    upstream_is_stream: false,
                    request_query: None,
                    kiro_api_region: Some("us-east-1"),
                    api_operation: Some(ApiOperation::ClaudeCountTokens),
                },
            )
            .is_none());
        }
    }

    #[test]
    fn claude_code_custom_root_keeps_messages_and_count_tokens_on_v1_surface() {
        let mut transport = sample_transport(
            "claude_code",
            "claude:messages",
            "https://proxy.example?key=base-secret",
            None,
        );
        transport.endpoint.config = Some(json!({
            "anthropic": {"supported_operations": ["messages", "count_tokens"]}
        }));

        assert_eq!(
            build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "claude:messages",
                    mapped_model: Some("claude-sonnet-4"),
                    upstream_is_stream: false,
                    request_query: Some("key=client-secret&trace=messages"),
                    kiro_api_region: None,
                    api_operation: Some(aether_ai_formats::ApiOperation::ClaudeMessagesCreate),
                },
            )
            .as_deref(),
            Some("https://proxy.example/v1/messages?key=base-secret&trace=messages")
        );
        assert_eq!(
            build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "claude:messages",
                    mapped_model: Some("claude-sonnet-4"),
                    upstream_is_stream: false,
                    request_query: Some("key=client-secret&trace=count"),
                    kiro_api_region: None,
                    api_operation: Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
                },
            )
            .as_deref(),
            Some("https://proxy.example/v1/messages/count_tokens?key=base-secret&trace=count")
        );
    }

    #[test]
    fn claude_code_official_api_root_builds_documented_messages_urls() {
        let mut transport = sample_transport(
            "claude_code",
            "claude:messages",
            "https://api.anthropic.com/v1",
            None,
        );
        transport.endpoint.config = Some(json!({
            "anthropic": {"supported_operations": ["messages", "count_tokens"]}
        }));

        for (operation, expected_url) in [
            (
                ApiOperation::ClaudeMessagesCreate,
                "https://api.anthropic.com/v1/messages",
            ),
            (
                ApiOperation::ClaudeCountTokens,
                "https://api.anthropic.com/v1/messages/count_tokens",
            ),
        ] {
            assert_eq!(
                build_transport_request_url(
                    &transport,
                    TransportRequestUrlParams {
                        provider_api_format: "claude:messages",
                        mapped_model: Some("claude-opus-4-6"),
                        upstream_is_stream: false,
                        request_query: None,
                        kiro_api_region: None,
                        api_operation: Some(operation),
                    },
                )
                .as_deref(),
                Some(expected_url)
            );
        }
    }

    #[test]
    fn count_tokens_config_fields_fall_back_from_endpoint_to_provider() {
        let mut transport = sample_transport(
            "custom",
            "claude:messages",
            "https://api.anthropic.example/v1?key=base-secret",
            None,
        );
        transport.endpoint.config = Some(json!({
            "anthropic": {"profile": "native_transparent"}
        }));
        transport.provider.config = Some(json!({
            "anthropic": {
                "supported_operations": ["messages", "count_tokens"],
                "count_tokens_path": "/v1/messages/provider_count_tokens?key=path-secret"
            }
        }));

        assert_eq!(
            build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "claude:messages",
                    mapped_model: Some("claude-sonnet-4"),
                    upstream_is_stream: false,
                    request_query: Some("key=client-secret&trace=1"),
                    kiro_api_region: None,
                    api_operation: Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
                },
            )
            .as_deref(),
            Some(
                "https://api.anthropic.example/v1/messages/provider_count_tokens?key=path-secret&trace=1"
            )
        );
    }

    #[test]
    fn rejects_count_tokens_when_provider_capability_excludes_it() {
        let mut transport = sample_transport(
            "custom",
            "claude:messages",
            "https://api.anthropic.example/v1",
            None,
        );
        transport.endpoint.config = Some(json!({
            "anthropic": {"profile": "native_transparent"}
        }));
        transport.provider.config = Some(json!({
            "anthropic": {"supported_operations": ["messages"]}
        }));

        assert!(build_transport_request_url(
            &transport,
            TransportRequestUrlParams {
                provider_api_format: "claude:messages",
                mapped_model: Some("claude-sonnet-4"),
                upstream_is_stream: false,
                request_query: None,
                kiro_api_region: None,
                api_operation: Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
            },
        )
        .is_none());
    }

    #[test]
    fn preserves_configured_query_credentials_and_strips_client_credentials() {
        let transport = sample_transport(
            "custom",
            "claude:messages",
            "https://proxy.example/anthropic?key=base-secret&tenant=base",
            Some("/messages?key=path-secret&variant=custom"),
        );

        assert_eq!(
            build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "claude:messages",
                    mapped_model: Some("claude-sonnet-4"),
                    upstream_is_stream: false,
                    request_query: Some("KEY=client-secret&trace=1"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some(
                "https://proxy.example/anthropic/messages?key=path-secret&tenant=base&trace=1&variant=custom"
            )
        );
    }

    #[test]
    fn kiro_cross_format_helper_uses_region_specific_generate_assistant_url() {
        let transport = sample_transport(
            "kiro",
            "claude:messages",
            "https://q.{region}.amazonaws.com/",
            None,
        );

        let url = build_kiro_cross_format_upstream_url(
            &transport,
            "claude-sonnet-4",
            "claude:messages",
            true,
            Some("key=gateway-secret&conversationId=abc"),
            "us-west-2",
        )
        .expect("kiro url");

        assert!(url.starts_with("https://runtime.us-west-2.kiro.dev/"));
        assert!(url.contains("conversationId=abc"));
        assert!(!url.contains("gateway-secret"));
    }

    #[test]
    fn embedding_request_url_builds_provider_default_paths() {
        let openai = sample_transport(
            "openai",
            "openai:embedding",
            "https://api.openai.example/v1",
            None,
        );
        let jina = sample_transport(
            "jina",
            "jina:embedding",
            "https://api.jina.example/v1",
            None,
        );
        let gemini = sample_transport(
            "gemini",
            "gemini:embedding",
            "https://generativelanguage.googleapis.com/v1beta",
            None,
        );
        let doubao = sample_transport(
            "doubao",
            "doubao:embedding",
            "https://ark.volces.example/api/v3",
            None,
        );
        let aliyun = sample_transport(
            "aliyun",
            "aliyun:multimodal_embedding",
            "https://dashscope.aliyuncs.com",
            None,
        );

        assert_eq!(
            build_transport_request_url(
                &openai,
                TransportRequestUrlParams {
                    provider_api_format: "openai:embedding",
                    mapped_model: Some("text-embedding-3-small"),
                    upstream_is_stream: false,
                    request_query: Some("tenant=demo"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://api.openai.example/v1/embeddings?tenant=demo")
        );
        assert_eq!(
            build_transport_request_url(
                &jina,
                TransportRequestUrlParams {
                    provider_api_format: "jina:embedding",
                    mapped_model: Some("jina-embeddings-v3"),
                    upstream_is_stream: false,
                    request_query: None,
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://api.jina.example/v1/embeddings")
        );
        assert_eq!(
            build_transport_request_url(
                &gemini,
                TransportRequestUrlParams {
                    provider_api_format: "gemini:embedding",
                    mapped_model: Some("gemini-embedding-001"),
                    upstream_is_stream: false,
                    request_query: Some("key=client-key&foo=bar"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some(
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-001:embedContent?foo=bar"
            )
        );
        assert_eq!(
            build_transport_request_url(
                &doubao,
                TransportRequestUrlParams {
                    provider_api_format: "doubao:embedding",
                    mapped_model: Some("doubao-embedding-text-240515"),
                    upstream_is_stream: false,
                    request_query: None,
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://ark.volces.example/api/v3/embeddings")
        );
        assert_eq!(
            build_transport_request_url(
                &aliyun,
                TransportRequestUrlParams {
                    provider_api_format: "aliyun:multimodal_embedding",
                    mapped_model: Some("qwen3-vl-embedding"),
                    upstream_is_stream: false,
                    request_query: None,
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://dashscope.aliyuncs.com/api/v1/services/embeddings/multimodal-embedding/multimodal-embedding")
        );
    }

    #[test]
    fn gemini_interactions_request_url_uses_stable_v1_endpoint() {
        let gemini = sample_transport(
            "gemini",
            "gemini:interactions",
            "https://generativelanguage.googleapis.com",
            None,
        );
        let versioned = sample_transport(
            "gemini",
            "gemini:interactions",
            "https://generativelanguage.googleapis.com/v1",
            None,
        );

        assert_eq!(
            build_transport_request_url(
                &gemini,
                TransportRequestUrlParams {
                    provider_api_format: "gemini:interactions",
                    mapped_model: Some("gemini-3.5-flash"),
                    upstream_is_stream: false,
                    request_query: Some("key=client-key&trace=1"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://generativelanguage.googleapis.com/v1/interactions?trace=1")
        );
        assert_eq!(
            build_transport_request_url(
                &versioned,
                TransportRequestUrlParams {
                    provider_api_format: "gemini:interactions",
                    mapped_model: Some("gemini-3.5-flash"),
                    upstream_is_stream: true,
                    request_query: Some("key=client-key&trace=2"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://generativelanguage.googleapis.com/v1/interactions?trace=2")
        );
    }

    #[test]
    fn antigravity_hook_is_limited_to_generate_content() {
        let transport = sample_transport(
            "antigravity",
            "gemini:interactions",
            "https://daily-cloudcode-pa.googleapis.com",
            None,
        );

        assert_eq!(
            build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "gemini:interactions",
                    mapped_model: Some("gemini-3.5-flash"),
                    upstream_is_stream: false,
                    request_query: Some("key=client-key"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://daily-cloudcode-pa.googleapis.com/v1/interactions")
        );
    }

    #[test]
    fn antigravity_generate_content_drops_client_api_key_query() {
        let transport = sample_transport(
            "antigravity",
            "gemini:generate_content",
            "https://daily-cloudcode-pa.googleapis.com",
            None,
        );

        assert_eq!(
            build_transport_request_url(
                &transport,
                TransportRequestUrlParams {
                    provider_api_format: "gemini:generate_content",
                    mapped_model: Some("gemini-3-flash-agent"),
                    upstream_is_stream: true,
                    request_query: Some("key=client-aether-key&trace=1&beta=true"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some(
                "https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse&trace=1"
            )
        );
    }

    #[test]
    fn embedding_request_url_preserves_google_openai_compat_roots() {
        let developer_api_openai = sample_transport(
            "custom",
            "openai:embedding",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            None,
        );
        let vertex_openai = sample_transport(
            "custom",
            "openai:embedding",
            "https://aiplatform.googleapis.com/v1/projects/project-1/locations/global/endpoints/openapi",
            None,
        );

        assert_eq!(
            build_transport_request_url(
                &developer_api_openai,
                TransportRequestUrlParams {
                    provider_api_format: "openai:embedding",
                    mapped_model: Some("gemini-embedding-001"),
                    upstream_is_stream: false,
                    request_query: Some("trace=1"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://generativelanguage.googleapis.com/v1beta/openai/embeddings?trace=1")
        );
        assert_eq!(
            build_transport_request_url(
                &vertex_openai,
                TransportRequestUrlParams {
                    provider_api_format: "openai:embedding",
                    mapped_model: Some("gemini-embedding-001"),
                    upstream_is_stream: false,
                    request_query: None,
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some(
                "https://aiplatform.googleapis.com/v1/projects/project-1/locations/global/endpoints/openapi/embeddings"
            )
        );
    }

    #[test]
    fn gemini_embedding_batch_body_uses_batch_endpoint() {
        let gemini = sample_transport(
            "gemini",
            "gemini:embedding",
            "https://generativelanguage.googleapis.com/v1beta",
            None,
        );
        let batch_body = json!({
            "requests": [
                {
                    "model": "models/gemini-embedding-001",
                    "content": {"parts": [{"text": "alpha"}]}
                },
                {
                    "model": "models/gemini-embedding-001",
                    "content": {"parts": [{"text": "beta"}]}
                }
            ]
        });

        assert_eq!(
            build_transport_request_url_for_request_body(
                &gemini,
                TransportRequestUrlParams {
                    provider_api_format: "gemini:embedding",
                    mapped_model: Some("gemini-embedding-001"),
                    upstream_is_stream: false,
                    request_query: Some("key=client-key&foo=bar"),
                    kiro_api_region: None,
                    api_operation: None,
                },
                Some(&batch_body),
            )
            .as_deref(),
            Some(
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-001:batchEmbedContents?foo=bar"
            )
        );
    }

    #[test]
    fn gemini_embedding_custom_action_template_follows_batch_body() {
        let gemini = sample_transport(
            "gemini",
            "gemini:embedding",
            "https://generativelanguage.googleapis.com",
            Some("/v1beta/models/{model}:{action}"),
        );
        let batch_body = json!({
            "requests": [
                {
                    "model": "models/gemini-embedding-001",
                    "content": {"parts": [{"text": "alpha"}]}
                }
            ]
        });

        assert_eq!(
            build_transport_request_url_for_request_body(
                &gemini,
                TransportRequestUrlParams {
                    provider_api_format: "gemini:embedding",
                    mapped_model: Some("gemini-embedding-001"),
                    upstream_is_stream: false,
                    request_query: None,
                    kiro_api_region: None,
                    api_operation: None,
                },
                Some(&batch_body),
            )
            .as_deref(),
            Some(
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-001:batchEmbedContents"
            )
        );
    }

    #[test]
    fn rerank_request_url_builds_provider_default_paths() {
        let openai = sample_transport(
            "openai",
            "openai:rerank",
            "https://api.openai.example/v1",
            None,
        );
        let jina = sample_transport("jina", "jina:rerank", "https://api.jina.example/v1", None);

        assert_eq!(
            build_transport_request_url(
                &openai,
                TransportRequestUrlParams {
                    provider_api_format: "openai:rerank",
                    mapped_model: Some("rerank-1"),
                    upstream_is_stream: false,
                    request_query: Some("tenant=demo"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://api.openai.example/v1/rerank?tenant=demo")
        );
        assert_eq!(
            build_transport_request_url(
                &jina,
                TransportRequestUrlParams {
                    provider_api_format: "jina:rerank",
                    mapped_model: Some("jina-reranker-v2-base-multilingual"),
                    upstream_is_stream: false,
                    request_query: None,
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://api.jina.example/v1/rerank")
        );
    }

    #[test]
    fn embedding_request_url_handles_base_variants_and_queries() {
        let openai_without_v1 = sample_transport(
            "openai",
            "openai:embedding",
            "https://api.openai.example/root?tenant=base",
            None,
        );
        let jina_with_v1 = sample_transport(
            "jina",
            "jina:embedding",
            "https://api.jina.example/v1/",
            None,
        );
        let gemini_model_base = sample_transport(
            "gemini",
            "gemini:embedding",
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-001",
            None,
        );
        let doubao_with_query = sample_transport(
            "doubao",
            "doubao:embedding",
            "https://ark.volces.example/api/v3?tenant=base",
            None,
        );

        assert_eq!(
            build_transport_request_url(
                &openai_without_v1,
                TransportRequestUrlParams {
                    provider_api_format: "OPENAI:EMBEDDING",
                    mapped_model: Some("text-embedding-3-small"),
                    upstream_is_stream: false,
                    request_query: Some("tenant=request&trace=1"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://api.openai.example/root/embeddings?tenant=request&trace=1")
        );
        assert_eq!(
            build_transport_request_url(
                &jina_with_v1,
                TransportRequestUrlParams {
                    provider_api_format: "jina:embedding",
                    mapped_model: Some("jina-embeddings-v3"),
                    upstream_is_stream: false,
                    request_query: Some("trace=2"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://api.jina.example/v1/embeddings?trace=2")
        );
        assert_eq!(
            build_transport_request_url(
                &gemini_model_base,
                TransportRequestUrlParams {
                    provider_api_format: "gemini:embedding",
                    mapped_model: Some("gemini-embedding-001"),
                    upstream_is_stream: false,
                    request_query: Some("key=client-key&trace=3"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-001:embedContent?trace=3")
        );
        assert_eq!(
            build_transport_request_url(
                &doubao_with_query,
                TransportRequestUrlParams {
                    provider_api_format: "doubao:embedding",
                    mapped_model: Some("doubao-embedding-text-240515"),
                    upstream_is_stream: false,
                    request_query: Some("trace=4"),
                    kiro_api_region: None,
                    api_operation: None,
                },
            )
            .as_deref(),
            Some("https://ark.volces.example/api/v3/embeddings?tenant=base&trace=4")
        );
    }

    #[test]
    fn gemini_embedding_request_url_requires_mapped_model_without_custom_path() {
        let transport = sample_transport(
            "gemini",
            "gemini:embedding",
            "https://generativelanguage.googleapis.com/v1beta",
            None,
        );

        assert!(build_transport_request_url(
            &transport,
            TransportRequestUrlParams {
                provider_api_format: "gemini:embedding",
                mapped_model: None,
                upstream_is_stream: false,
                request_query: None,
                kiro_api_region: None,
                api_operation: None,
            },
        )
        .is_none());
    }

    #[test]
    fn embedding_request_url_expands_custom_gemini_embed_action() {
        let transport = sample_transport(
            "custom",
            "gemini:embedding",
            "https://generativelanguage.googleapis.com",
            Some("/v1beta/models/{model}:{action}"),
        );

        let url = build_transport_request_url(
            &transport,
            TransportRequestUrlParams {
                provider_api_format: "gemini:embedding",
                mapped_model: Some("gemini-embedding-001"),
                upstream_is_stream: true,
                request_query: Some("key=client-key&foo=bar"),
                kiro_api_region: None,
                api_operation: None,
            },
        )
        .expect("expanded custom embedding path url");

        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-001:embedContent?foo=bar"
        );
    }
}
