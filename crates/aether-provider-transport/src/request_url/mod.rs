use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;
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
    google_openai_compat_base_includes_api_root, normalize_gemini_content_action_path,
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
    if let Some(url) = build_transport_hook_url(transport, params) {
        return Some(url);
    }

    let custom_path = transport
        .endpoint
        .custom_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|path| {
            expand_custom_path_template(path, build_path_params(params, gemini_embedding_batch))
        });

    if let Some(path) = custom_path.as_deref() {
        let blocked_keys = if normalized_provider_api_format.starts_with("gemini:") {
            &["key"][..]
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
        let url = build_passthrough_path_url(
            &transport.endpoint.base_url,
            normalized_path.as_str(),
            params.request_query,
            blocked_keys,
        )?;
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
        "openai:embedding" | "jina:embedding" => {
            build_provider_embedding_v1_url(&transport.endpoint.base_url, params.request_query)
        }
        "openai:rerank" | "jina:rerank" => {
            build_provider_rerank_v1_url(&transport.endpoint.base_url, params.request_query)
        }
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
        "gemini:embedding" => build_gemini_embedding_url(
            &transport.endpoint.base_url,
            params.mapped_model?,
            params.request_query,
            gemini_embedding_batch,
        ),
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

    match aether_ai_formats::normalize_api_format_alias(params.provider_api_format).as_str() {
        "gemini:generate_content" => {
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
    if provider_api_format.starts_with("gemini:") {
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

fn normalize_gemini_embedding_action_path(path: &str, batch: bool) -> String {
    if batch {
        path.replace(":embedContent", ":batchEmbedContents")
    } else {
        path.replace(":batchEmbedContents", ":embedContent")
    }
}

fn build_provider_embedding_v1_url(upstream_base_url: &str, query: Option<&str>) -> Option<String> {
    build_provider_v1_url(upstream_base_url, "/embeddings", "/v1/embeddings", query)
}

fn build_provider_rerank_v1_url(upstream_base_url: &str, query: Option<&str>) -> Option<String> {
    build_provider_v1_url(upstream_base_url, "/rerank", "/v1/rerank", query)
}

fn build_provider_v1_url(
    upstream_base_url: &str,
    v1_path: &str,
    default_path: &str,
    query: Option<&str>,
) -> Option<String> {
    let base_without_query = upstream_base_url
        .trim()
        .split_once('?')
        .map(|(base, _)| base)
        .unwrap_or_else(|| upstream_base_url.trim())
        .trim_end_matches('/');
    let path = if base_without_query.ends_with("/v1")
        || google_openai_compat_base_includes_api_root(base_without_query)
    {
        v1_path
    } else {
        default_path
    };
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

    #[test]
    fn embedding_request_url_builds_provider_default_paths() {
        let openai = sample_transport(
            "openai",
            "openai:embedding",
            "https://api.openai.example/v1",
            None,
        );
        let jina = sample_transport("jina", "jina:embedding", "https://api.jina.example", None);
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

        assert_eq!(
            build_transport_request_url(
                &openai,
                TransportRequestUrlParams {
                    provider_api_format: "openai:embedding",
                    mapped_model: Some("text-embedding-3-small"),
                    upstream_is_stream: false,
                    request_query: Some("tenant=demo"),
                    kiro_api_region: None,
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
                },
            )
            .as_deref(),
            Some("https://ark.volces.example/api/v3/embeddings")
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
        let jina = sample_transport("jina", "jina:rerank", "https://api.jina.example", None);

        assert_eq!(
            build_transport_request_url(
                &openai,
                TransportRequestUrlParams {
                    provider_api_format: "openai:rerank",
                    mapped_model: Some("rerank-1"),
                    upstream_is_stream: false,
                    request_query: Some("tenant=demo"),
                    kiro_api_region: None,
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
                },
            )
            .as_deref(),
            Some("https://api.openai.example/root/v1/embeddings?tenant=request&trace=1")
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
            },
        )
        .expect("expanded custom embedding path url");

        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-001:embedContent?foo=bar"
        );
    }
}
