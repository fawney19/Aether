#![allow(dead_code)]

use aether_ai_formats::normalize_legacy_openai_format_alias;
use aether_provider_transport::auth::{
    resolve_local_gemini_auth, resolve_local_openai_bearer_auth, resolve_local_standard_auth,
};
use aether_provider_transport::kiro::local_kiro_request_transport_unsupported_reason_with_network;
use aether_provider_transport::policy::{
    local_gemini_transport_unsupported_reason_with_network,
    local_openai_chat_transport_unsupported_reason,
    local_standard_transport_unsupported_reason_with_network,
};
use aether_provider_transport::vertex::{
    is_vertex_api_key_transport_context,
    local_vertex_api_key_gemini_transport_unsupported_reason_with_network,
    resolve_local_vertex_api_key_query_auth, VERTEX_API_KEY_QUERY_PARAM,
};
use aether_provider_transport::GatewayProviderTransportSnapshot;

pub use aether_ai_formats::matrix::{
    RequestConversionKind, SyncChatResponseConversionKind, SyncCliResponseConversionKind,
};

pub fn request_candidate_api_format_preference(
    client_api_format: &str,
    provider_api_format: &str,
) -> Option<(u8, u8)> {
    aether_ai_formats::matrix::request_candidate_api_format_preference(
        client_api_format,
        provider_api_format,
    )
}

pub fn request_candidate_api_formats(
    client_api_format: &str,
    require_streaming: bool,
) -> Vec<&'static str> {
    aether_ai_formats::matrix::request_candidate_api_formats(client_api_format, require_streaming)
}

pub fn request_conversion_kind(
    client_api_format: &str,
    provider_api_format: &str,
) -> Option<RequestConversionKind> {
    aether_ai_formats::matrix::request_conversion_kind(client_api_format, provider_api_format)
}

pub fn sync_chat_response_conversion_kind(
    provider_api_format: &str,
    client_api_format: &str,
) -> Option<SyncChatResponseConversionKind> {
    aether_ai_formats::matrix::sync_chat_response_conversion_kind(
        provider_api_format,
        client_api_format,
    )
}

pub fn sync_cli_response_conversion_kind(
    provider_api_format: &str,
    client_api_format: &str,
) -> Option<SyncCliResponseConversionKind> {
    aether_ai_formats::matrix::sync_cli_response_conversion_kind(
        provider_api_format,
        client_api_format,
    )
}

pub fn request_conversion_requires_enable_flag(
    client_api_format: &str,
    provider_api_format: &str,
) -> bool {
    aether_ai_formats::matrix::request_conversion_requires_enable_flag(
        client_api_format,
        provider_api_format,
    )
}

pub fn request_conversion_enabled_for_transport(
    transport: &GatewayProviderTransportSnapshot,
    client_api_format: &str,
    provider_api_format: &str,
) -> bool {
    let client_api_format = normalize_legacy_openai_format_alias(client_api_format);
    let provider_api_format = normalize_legacy_openai_format_alias(provider_api_format);
    if client_api_format == provider_api_format {
        return true;
    }
    if request_conversion_kind(client_api_format.as_str(), provider_api_format.as_str()).is_none() {
        return false;
    }
    if !request_conversion_requires_enable_flag(
        client_api_format.as_str(),
        provider_api_format.as_str(),
    ) {
        return true;
    }
    transport.provider.enable_format_conversion
        || endpoint_accepts_client_api_format(transport, client_api_format.as_str())
}

pub fn request_pair_allowed_for_transport(
    transport: &GatewayProviderTransportSnapshot,
    client_api_format: &str,
    provider_api_format: &str,
) -> bool {
    let client_api_format = normalize_legacy_openai_format_alias(client_api_format);
    let provider_api_format = normalize_legacy_openai_format_alias(provider_api_format);
    if client_api_format == provider_api_format {
        return true;
    }
    if request_conversion_kind(client_api_format.as_str(), provider_api_format.as_str()).is_none() {
        return false;
    }
    if transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("kiro")
        && provider_api_format.eq_ignore_ascii_case("claude:cli")
    {
        return request_conversion_enabled_for_transport(
            transport,
            client_api_format.as_str(),
            provider_api_format.as_str(),
        ) && local_kiro_request_transport_unsupported_reason_with_network(transport)
            .is_none();
    }
    request_conversion_enabled_for_transport(
        transport,
        client_api_format.as_str(),
        provider_api_format.as_str(),
    )
}

pub fn request_conversion_transport_supported(
    transport: &GatewayProviderTransportSnapshot,
    kind: RequestConversionKind,
) -> bool {
    request_conversion_transport_unsupported_reason(transport, kind).is_none()
}

pub fn request_conversion_transport_unsupported_reason(
    transport: &GatewayProviderTransportSnapshot,
    _kind: RequestConversionKind,
) -> Option<&'static str> {
    if transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("kiro")
        && transport
            .endpoint
            .api_format
            .trim()
            .eq_ignore_ascii_case("claude:cli")
    {
        return local_kiro_request_transport_unsupported_reason_with_network(transport);
    }

    match normalize_legacy_openai_format_alias(&transport.endpoint.api_format).as_str() {
        "openai:chat" => local_openai_chat_transport_unsupported_reason(transport),
        "openai:responses" | "openai:responses:compact" => {
            local_standard_transport_unsupported_reason_with_network(
                transport,
                transport.endpoint.api_format.trim(),
            )
        }
        "claude:chat" => {
            local_standard_transport_unsupported_reason_with_network(transport, "claude:chat")
        }
        "claude:cli" => {
            local_standard_transport_unsupported_reason_with_network(transport, "claude:cli")
        }
        "gemini:chat" | "gemini:cli" if is_vertex_api_key_transport_context(transport) => {
            local_vertex_api_key_gemini_transport_unsupported_reason_with_network(transport)
        }
        "gemini:chat" => {
            local_gemini_transport_unsupported_reason_with_network(transport, "gemini:chat")
        }
        "gemini:cli" => {
            local_gemini_transport_unsupported_reason_with_network(transport, "gemini:cli")
        }
        _ => Some("transport_api_format_unsupported"),
    }
}

pub fn request_conversion_direct_auth(
    transport: &GatewayProviderTransportSnapshot,
    _kind: RequestConversionKind,
) -> Option<(String, String)> {
    match normalize_legacy_openai_format_alias(&transport.endpoint.api_format).as_str() {
        "openai:chat" | "openai:responses" | "openai:responses:compact" => {
            resolve_local_openai_bearer_auth(transport)
        }
        "gemini:chat" | "gemini:cli" => {
            if is_vertex_api_key_transport_context(transport) {
                resolve_local_vertex_api_key_query_auth(transport)
                    .map(|auth| (VERTEX_API_KEY_QUERY_PARAM.to_string(), auth.value))
            } else {
                resolve_local_gemini_auth(transport)
            }
        }
        "claude:chat" | "claude:cli" => resolve_local_standard_auth(transport),
        _ => None,
    }
}

fn endpoint_accepts_client_api_format(
    transport: &GatewayProviderTransportSnapshot,
    client_api_format: &str,
) -> bool {
    let Some(config) = transport
        .endpoint
        .format_acceptance_config
        .as_ref()
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    if !config
        .get("enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return false;
    }

    if config
        .get("reject_formats")
        .is_some_and(|value| json_format_list_contains(value, client_api_format))
    {
        return false;
    }

    match config.get("accept_formats") {
        Some(value) => json_format_list_contains(value, client_api_format),
        None => true,
    }
}

fn json_format_list_contains(value: &serde_json::Value, api_format: &str) -> bool {
    let Some(items) = value.as_array() else {
        return false;
    };
    items.iter().any(|item| {
        item.as_str()
            .is_some_and(|candidate| candidate.trim().eq_ignore_ascii_case(api_format))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        request_candidate_api_format_preference, request_candidate_api_formats,
        request_conversion_direct_auth, request_conversion_enabled_for_transport,
        request_conversion_kind, request_conversion_requires_enable_flag,
        request_conversion_transport_supported, request_pair_allowed_for_transport,
        sync_chat_response_conversion_kind, sync_cli_response_conversion_kind,
        RequestConversionKind, SyncChatResponseConversionKind, SyncCliResponseConversionKind,
    };
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    const STANDARD_SURFACES: &[&str] = &[
        "openai:chat",
        "openai:responses",
        "claude:chat",
        "claude:cli",
        "gemini:chat",
        "gemini:cli",
    ];

    fn expected_request_conversion_kind(provider_api_format: &str) -> RequestConversionKind {
        match provider_api_format {
            "openai:chat" => RequestConversionKind::ToOpenAIChat,
            "openai:responses" => RequestConversionKind::ToOpenAiResponses,
            "claude:chat" | "claude:cli" => RequestConversionKind::ToClaudeStandard,
            "gemini:chat" | "gemini:cli" => RequestConversionKind::ToGeminiStandard,
            other => panic!("unexpected provider api format: {other}"),
        }
    }

    #[test]
    fn request_conversion_registry_supports_bidirectional_standard_matrix() {
        assert_eq!(
            request_conversion_kind("openai:chat", "openai:responses"),
            Some(RequestConversionKind::ToOpenAiResponses)
        );
        assert_eq!(
            request_conversion_kind("openai:chat", "claude:cli"),
            Some(RequestConversionKind::ToClaudeStandard)
        );
        assert_eq!(
            request_conversion_kind("openai:responses", "openai:chat"),
            Some(RequestConversionKind::ToOpenAIChat)
        );
        assert_eq!(
            request_conversion_kind("openai:responses:compact", "gemini:cli"),
            None
        );
        assert_eq!(
            request_conversion_kind("gemini:cli", "openai:responses:compact"),
            None
        );
        assert_eq!(
            request_conversion_kind("openai:chat", "openai:responses:compact"),
            None
        );
        assert_eq!(
            request_conversion_kind("openai:responses", "openai:cli"),
            None
        );
        assert_eq!(
            request_conversion_kind("openai:compact", "openai:responses:compact"),
            None
        );
        assert_eq!(
            request_conversion_kind("gemini:chat", "claude:chat"),
            Some(RequestConversionKind::ToClaudeStandard)
        );
        assert_eq!(
            request_conversion_kind("claude:chat", "claude:cli"),
            Some(RequestConversionKind::ToClaudeStandard)
        );
        assert_eq!(request_conversion_kind("claude:chat", "claude:chat"), None);
    }

    #[test]
    fn request_conversion_registry_covers_all_standard_surface_pairs() {
        for client_api_format in STANDARD_SURFACES {
            for provider_api_format in STANDARD_SURFACES {
                let actual = request_conversion_kind(client_api_format, provider_api_format);
                if client_api_format == provider_api_format {
                    assert_eq!(
                        actual, None,
                        "{client_api_format} -> {provider_api_format} should be same-format"
                    );
                } else {
                    assert_eq!(
                        actual,
                        Some(expected_request_conversion_kind(provider_api_format)),
                        "{client_api_format} -> {provider_api_format} should be routable"
                    );
                }
            }
        }
    }

    #[test]
    fn sync_response_conversion_registry_supports_bidirectional_standard_matrix() {
        assert_eq!(
            sync_chat_response_conversion_kind("openai:chat", "claude:chat"),
            Some(SyncChatResponseConversionKind::ToClaudeChat)
        );
        assert_eq!(
            sync_chat_response_conversion_kind("claude:chat", "gemini:chat"),
            Some(SyncChatResponseConversionKind::ToGeminiChat)
        );
        assert_eq!(
            sync_chat_response_conversion_kind("gemini:chat", "openai:chat"),
            Some(SyncChatResponseConversionKind::ToOpenAIChat)
        );
        assert_eq!(
            sync_cli_response_conversion_kind("openai:responses", "gemini:cli"),
            Some(SyncCliResponseConversionKind::ToGeminiCli)
        );
        assert_eq!(
            sync_cli_response_conversion_kind("claude:chat", "openai:responses"),
            Some(SyncCliResponseConversionKind::ToOpenAiResponses)
        );
        assert_eq!(
            sync_cli_response_conversion_kind("claude:cli", "openai:responses:compact"),
            Some(SyncCliResponseConversionKind::ToOpenAiResponses)
        );
        assert_eq!(
            sync_cli_response_conversion_kind("openai:responses:compact", "claude:cli"),
            None
        );
        assert_eq!(
            sync_cli_response_conversion_kind("gemini:cli", "claude:cli"),
            Some(SyncCliResponseConversionKind::ToClaudeCli)
        );
        assert_eq!(
            sync_cli_response_conversion_kind("openai:responses", "openai:cli"),
            None
        );
        assert_eq!(
            sync_cli_response_conversion_kind("openai:compact", "openai:responses:compact"),
            None
        );
    }

    #[test]
    fn sync_response_conversion_registry_covers_all_standard_surface_pairs() {
        for provider_api_format in STANDARD_SURFACES {
            for client_api_format in ["openai:chat", "claude:chat", "gemini:chat"] {
                let actual =
                    sync_chat_response_conversion_kind(provider_api_format, client_api_format);
                if *provider_api_format == client_api_format {
                    assert_eq!(
                        actual, None,
                        "{provider_api_format} -> {client_api_format} should be same-format"
                    );
                } else {
                    let expected = match client_api_format {
                        "openai:chat" => SyncChatResponseConversionKind::ToOpenAIChat,
                        "claude:chat" => SyncChatResponseConversionKind::ToClaudeChat,
                        "gemini:chat" => SyncChatResponseConversionKind::ToGeminiChat,
                        other => panic!("unexpected chat client api format: {other}"),
                    };
                    assert_eq!(
                        actual,
                        Some(expected),
                        "{provider_api_format} -> {client_api_format} should finalize to chat"
                    );
                }
            }

            for client_api_format in ["openai:responses", "claude:cli", "gemini:cli"] {
                let actual =
                    sync_cli_response_conversion_kind(provider_api_format, client_api_format);
                if *provider_api_format == client_api_format {
                    assert_eq!(
                        actual, None,
                        "{provider_api_format} -> {client_api_format} should be same-format"
                    );
                } else {
                    let expected = match client_api_format {
                        "openai:responses" => SyncCliResponseConversionKind::ToOpenAiResponses,
                        "claude:cli" => SyncCliResponseConversionKind::ToClaudeCli,
                        "gemini:cli" => SyncCliResponseConversionKind::ToGeminiCli,
                        other => panic!("unexpected cli client api format: {other}"),
                    };
                    assert_eq!(
                        actual,
                        Some(expected),
                        "{provider_api_format} -> {client_api_format} should finalize to cli"
                    );
                }
            }
        }
    }

    #[test]
    fn request_candidate_registry_excludes_compact_as_cross_format_target() {
        assert_eq!(
            request_candidate_api_formats("openai:chat", false),
            vec![
                "openai:chat",
                "claude:chat",
                "gemini:chat",
                "openai:responses",
                "claude:cli",
                "gemini:cli",
            ]
        );
        assert_eq!(
            request_candidate_api_formats("openai:responses", false),
            vec![
                "openai:responses",
                "claude:cli",
                "gemini:cli",
                "openai:chat",
                "claude:chat",
                "gemini:chat",
            ]
        );
        assert_eq!(
            request_candidate_api_formats("openai:cli", false),
            vec![
                "openai:responses",
                "claude:cli",
                "gemini:cli",
                "openai:chat",
                "claude:chat",
                "gemini:chat",
            ]
        );
        assert_eq!(
            request_candidate_api_formats("claude:cli", false),
            vec![
                "claude:cli",
                "openai:responses",
                "gemini:cli",
                "claude:chat",
                "openai:chat",
                "gemini:chat",
            ]
        );
        assert_eq!(
            request_candidate_api_formats("openai:compact", false),
            vec!["openai:responses:compact"]
        );
    }

    #[test]
    fn request_candidate_registry_prefers_same_kind_before_same_family_fallbacks() {
        assert_eq!(
            request_candidate_api_format_preference("claude:cli", "openai:responses"),
            Some((1, 0))
        );
        assert_eq!(
            request_candidate_api_format_preference("claude:cli", "claude:chat"),
            Some((2, 1))
        );
        assert_eq!(
            request_candidate_api_format_preference("claude:cli", "openai:chat"),
            Some((3, 0))
        );
    }

    #[test]
    fn request_conversion_enable_flag_only_applies_to_real_data_format_conversions() {
        assert!(!request_conversion_requires_enable_flag(
            "claude:chat",
            "claude:cli"
        ));
        assert!(request_conversion_requires_enable_flag(
            "openai:chat",
            "openai:responses"
        ));
        assert!(request_conversion_requires_enable_flag(
            "openai:responses",
            "openai:chat"
        ));
        assert!(request_conversion_requires_enable_flag(
            "openai:chat",
            "gemini:chat"
        ));
    }

    #[test]
    fn request_conversion_helpers_follow_transport_api_format() {
        let transport = GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "provider".to_string(),
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
                api_format: "openai:chat".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
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
        };

        assert!(request_conversion_transport_supported(
            &transport,
            RequestConversionKind::ToOpenAIChat
        ));
        assert_eq!(
            request_conversion_direct_auth(&transport, RequestConversionKind::ToOpenAIChat),
            Some(("authorization".to_string(), "Bearer secret".to_string()))
        );
    }

    #[test]
    fn endpoint_level_format_acceptance_enables_cross_format_pair_without_provider_flag() {
        let transport = GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "provider".to_string(),
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
                api_format: "openai:responses".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("responses".to_string()),
                is_active: true,
                base_url: "https://right.codes/codex".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: Some("/v1/messages".to_string()),
                config: None,
                format_acceptance_config: Some(serde_json::json!({
                    "enabled": true,
                    "accept_formats": ["claude:cli"],
                })),
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: "bearer".to_string(),
                is_active: true,
                api_formats: Some(vec!["openai:responses".to_string()]),
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
        };

        assert!(request_conversion_enabled_for_transport(
            &transport,
            "claude:cli",
            "openai:responses"
        ));
        assert!(request_pair_allowed_for_transport(
            &transport,
            "claude:cli",
            "openai:responses"
        ));
        assert!(!request_pair_allowed_for_transport(
            &transport,
            "gemini:cli",
            "openai:responses"
        ));
    }

    #[test]
    fn endpoint_reject_formats_override_endpoint_cross_format_enablement() {
        let transport = GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "provider".to_string(),
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
                api_format: "openai:responses".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("responses".to_string()),
                is_active: true,
                base_url: "https://right.codes/codex".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: Some("/v1/messages".to_string()),
                config: None,
                format_acceptance_config: Some(serde_json::json!({
                    "enabled": true,
                    "reject_formats": ["claude:cli"],
                })),
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: "bearer".to_string(),
                is_active: true,
                api_formats: Some(vec!["openai:responses".to_string()]),
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
        };

        assert!(!request_conversion_enabled_for_transport(
            &transport,
            "claude:cli",
            "openai:responses"
        ));
    }

    #[test]
    fn vertex_gemini_transport_supports_cross_format_conversion_with_query_auth() {
        let transport = GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-vertex".to_string(),
                name: "vertex".to_string(),
                provider_type: "vertex_ai".to_string(),
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
                id: "endpoint-vertex".to_string(),
                provider_id: "provider-vertex".to_string(),
                api_format: "gemini:chat".to_string(),
                api_family: Some("gemini".to_string()),
                endpoint_kind: Some("chat".to_string()),
                is_active: true,
                base_url: "https://aiplatform.googleapis.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-vertex".to_string(),
                provider_id: "provider-vertex".to_string(),
                name: "key".to_string(),
                auth_type: "api_key".to_string(),
                is_active: true,
                api_formats: Some(vec!["gemini:chat".to_string()]),
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
        };

        assert!(request_conversion_transport_supported(
            &transport,
            RequestConversionKind::ToGeminiStandard
        ));
        assert_eq!(
            request_conversion_direct_auth(&transport, RequestConversionKind::ToGeminiStandard),
            Some(("key".to_string(), "vertex-secret".to_string()))
        );
    }

    #[test]
    fn kiro_claude_cli_transport_supports_cross_format_conversion_via_envelope() {
        let transport = GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-kiro".to_string(),
                name: "kiro".to_string(),
                provider_type: "kiro".to_string(),
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
                id: "endpoint-kiro".to_string(),
                provider_id: "provider-kiro".to_string(),
                api_format: "claude:cli".to_string(),
                api_family: Some("claude".to_string()),
                endpoint_kind: Some("cli".to_string()),
                is_active: true,
                base_url: "https://q.{region}.amazonaws.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-kiro".to_string(),
                provider_id: "provider-kiro".to_string(),
                name: "key".to_string(),
                auth_type: "bearer".to_string(),
                is_active: true,
                api_formats: Some(vec!["claude:cli".to_string()]),
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "kiro-secret".to_string(),
                decrypted_auth_config: None,
            },
        };

        assert!(request_pair_allowed_for_transport(
            &transport,
            "openai:chat",
            "claude:cli"
        ));
        assert!(request_conversion_transport_supported(
            &transport,
            RequestConversionKind::ToClaudeStandard
        ));
    }
}
