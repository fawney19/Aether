#![allow(dead_code)]

use aether_provider_transport::auth::{
    resolve_local_gemini_auth, resolve_local_openai_bearer_auth, resolve_local_standard_auth,
};
use aether_provider_transport::policy::{
    local_gemini_transport_unsupported_reason_with_network,
    local_openai_chat_transport_unsupported_reason,
    local_standard_transport_unsupported_reason_with_network,
};
use aether_provider_transport::GatewayProviderTransportSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestConversionKind {
    ToOpenAIChat,
    ToOpenAIFamilyCli,
    ToClaudeStandard,
    ToGeminiStandard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncChatResponseConversionKind {
    ToOpenAIChat,
    ToClaudeChat,
    ToGeminiChat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncCliResponseConversionKind {
    ToOpenAIFamilyCli,
    ToClaudeCli,
    ToGeminiCli,
}

const NON_COMPACT_STANDARD_CANDIDATE_API_FORMATS: &[&str] = &[
    "openai:chat",
    "openai:cli",
    "claude:chat",
    "claude:cli",
    "gemini:chat",
    "gemini:cli",
];
const STANDARD_API_FAMILY_ORDER: &[&str] = &["openai", "claude", "gemini"];

pub fn request_candidate_api_format_preference(
    client_api_format: &str,
    provider_api_format: &str,
) -> Option<(u8, u8)> {
    let client_api_format = client_api_format.trim().to_ascii_lowercase();
    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();

    if client_api_format == "openai:compact" {
        return (provider_api_format == "openai:compact").then_some((0, 0));
    }

    let (client_family, client_kind) =
        parse_non_compact_standard_api_format(client_api_format.as_str())?;
    let (provider_family, provider_kind) =
        parse_non_compact_standard_api_format(provider_api_format.as_str())?;
    let preference_bucket = if client_family == provider_family && client_kind == provider_kind {
        0
    } else if client_kind == provider_kind {
        1
    } else if client_family == provider_family {
        2
    } else {
        3
    };

    Some((
        preference_bucket,
        standard_api_family_priority(provider_family),
    ))
}

pub fn request_candidate_api_formats(
    client_api_format: &str,
    _require_streaming: bool,
) -> Vec<&'static str> {
    let client_api_format = client_api_format.trim().to_ascii_lowercase();
    if client_api_format == "openai:compact" {
        return vec!["openai:compact"];
    }
    if parse_non_compact_standard_api_format(client_api_format.as_str()).is_none() {
        return Vec::new();
    }

    let mut candidate_api_formats = NON_COMPACT_STANDARD_CANDIDATE_API_FORMATS.to_vec();
    candidate_api_formats.sort_by_key(|provider_api_format| {
        request_candidate_api_format_preference(client_api_format.as_str(), provider_api_format)
            .unwrap_or((u8::MAX, u8::MAX))
    });
    candidate_api_formats
}

pub fn request_conversion_kind(
    client_api_format: &str,
    provider_api_format: &str,
) -> Option<RequestConversionKind> {
    let client_api_format = client_api_format.trim().to_ascii_lowercase();
    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();
    if client_api_format == provider_api_format {
        return None;
    }
    if !is_standard_api_format(client_api_format.as_str())
        || !is_standard_api_format(provider_api_format.as_str())
    {
        return None;
    }
    if client_api_format == "openai:compact" || provider_api_format == "openai:compact" {
        return None;
    }

    match provider_api_format.as_str() {
        "openai:chat" => Some(RequestConversionKind::ToOpenAIChat),
        "openai:cli" => Some(RequestConversionKind::ToOpenAIFamilyCli),
        "claude:chat" | "claude:cli" => Some(RequestConversionKind::ToClaudeStandard),
        "gemini:chat" | "gemini:cli" => Some(RequestConversionKind::ToGeminiStandard),
        _ => None,
    }
}

pub fn sync_chat_response_conversion_kind(
    provider_api_format: &str,
    client_api_format: &str,
) -> Option<SyncChatResponseConversionKind> {
    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();
    let client_api_format = client_api_format.trim().to_ascii_lowercase();
    if provider_api_format == client_api_format {
        return None;
    }
    if !is_standard_api_format(provider_api_format.as_str()) {
        return None;
    }
    request_conversion_kind(client_api_format.as_str(), provider_api_format.as_str())?;
    match client_api_format.as_str() {
        "openai:chat" => Some(SyncChatResponseConversionKind::ToOpenAIChat),
        "claude:chat" => Some(SyncChatResponseConversionKind::ToClaudeChat),
        "gemini:chat" => Some(SyncChatResponseConversionKind::ToGeminiChat),
        _ => None,
    }
}

pub fn sync_cli_response_conversion_kind(
    provider_api_format: &str,
    client_api_format: &str,
) -> Option<SyncCliResponseConversionKind> {
    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();
    let client_api_format = client_api_format.trim().to_ascii_lowercase();
    if provider_api_format == client_api_format {
        return None;
    }
    if !is_standard_api_format(provider_api_format.as_str()) {
        return None;
    }
    if client_api_format != "openai:compact" {
        request_conversion_kind(client_api_format.as_str(), provider_api_format.as_str())?;
    }
    match client_api_format.as_str() {
        "openai:cli" | "openai:compact" => Some(SyncCliResponseConversionKind::ToOpenAIFamilyCli),
        "claude:cli" => Some(SyncCliResponseConversionKind::ToClaudeCli),
        "gemini:cli" => Some(SyncCliResponseConversionKind::ToGeminiCli),
        _ => None,
    }
}

pub fn request_conversion_requires_enable_flag(
    client_api_format: &str,
    provider_api_format: &str,
) -> bool {
    let client_api_format = client_api_format.trim().to_ascii_lowercase();
    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();
    match (
        api_data_format_id(client_api_format.as_str()),
        api_data_format_id(provider_api_format.as_str()),
    ) {
        (Some(client_data_format), Some(provider_data_format)) => {
            client_data_format != provider_data_format
        }
        _ => true,
    }
}

pub fn request_conversion_enabled_for_transport(
    transport: &GatewayProviderTransportSnapshot,
    client_api_format: &str,
    provider_api_format: &str,
) -> bool {
    let client_api_format = client_api_format.trim().to_ascii_lowercase();
    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();
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
    let client_api_format = client_api_format.trim().to_ascii_lowercase();
    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();
    if client_api_format == provider_api_format {
        return true;
    }
    if request_conversion_kind(client_api_format.as_str(), provider_api_format.as_str()).is_none() {
        return false;
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
    match transport
        .endpoint
        .api_format
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "openai:chat" => local_openai_chat_transport_unsupported_reason(transport),
        "openai:cli" => {
            local_standard_transport_unsupported_reason_with_network(transport, "openai:cli")
        }
        "openai:compact" => {
            local_standard_transport_unsupported_reason_with_network(transport, "openai:compact")
        }
        "claude:chat" => {
            local_standard_transport_unsupported_reason_with_network(transport, "claude:chat")
        }
        "claude:cli" => {
            local_standard_transport_unsupported_reason_with_network(transport, "claude:cli")
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
    match transport
        .endpoint
        .api_format
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "openai:chat" | "openai:cli" | "openai:compact" => {
            resolve_local_openai_bearer_auth(transport)
        }
        "gemini:chat" | "gemini:cli" => resolve_local_gemini_auth(transport),
        "claude:chat" | "claude:cli" => resolve_local_standard_auth(transport),
        _ => None,
    }
}

fn is_standard_api_format(api_format: &str) -> bool {
    matches!(
        api_format,
        "openai:chat"
            | "openai:cli"
            | "openai:compact"
            | "claude:chat"
            | "claude:cli"
            | "gemini:chat"
            | "gemini:cli"
    )
}

fn parse_non_compact_standard_api_format(api_format: &str) -> Option<(&str, &str)> {
    let (family, kind) = api_format.split_once(':')?;
    if !STANDARD_API_FAMILY_ORDER.contains(&family) || !matches!(kind, "chat" | "cli") {
        return None;
    }
    Some((family, kind))
}

fn standard_api_family_priority(family: &str) -> u8 {
    STANDARD_API_FAMILY_ORDER
        .iter()
        .position(|candidate| *candidate == family)
        .unwrap_or(STANDARD_API_FAMILY_ORDER.len()) as u8
}

fn api_data_format_id(api_format: &str) -> Option<&'static str> {
    match api_format {
        "claude:chat" | "claude:cli" => Some("claude"),
        "gemini:chat" | "gemini:cli" => Some("gemini"),
        "openai:chat" => Some("openai_chat"),
        "openai:cli" | "openai:compact" => Some("openai_responses"),
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

    #[test]
    fn request_conversion_registry_supports_bidirectional_standard_matrix() {
        assert_eq!(
            request_conversion_kind("openai:chat", "openai:cli"),
            Some(RequestConversionKind::ToOpenAIFamilyCli)
        );
        assert_eq!(
            request_conversion_kind("openai:chat", "claude:cli"),
            Some(RequestConversionKind::ToClaudeStandard)
        );
        assert_eq!(
            request_conversion_kind("openai:cli", "openai:chat"),
            Some(RequestConversionKind::ToOpenAIChat)
        );
        assert_eq!(
            request_conversion_kind("gemini:chat", "claude:chat"),
            Some(RequestConversionKind::ToClaudeStandard)
        );
        assert_eq!(
            request_conversion_kind("openai:compact", "gemini:cli"),
            None
        );
        assert_eq!(
            request_conversion_kind("gemini:cli", "openai:compact"),
            None
        );
        assert_eq!(
            request_conversion_kind("openai:chat", "openai:compact"),
            None
        );
        assert_eq!(
            request_conversion_kind("claude:chat", "claude:cli"),
            Some(RequestConversionKind::ToClaudeStandard)
        );
        assert_eq!(request_conversion_kind("claude:chat", "claude:chat"), None);
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
            sync_cli_response_conversion_kind("openai:cli", "gemini:cli"),
            Some(SyncCliResponseConversionKind::ToGeminiCli)
        );
        assert_eq!(
            sync_cli_response_conversion_kind("claude:chat", "openai:cli"),
            Some(SyncCliResponseConversionKind::ToOpenAIFamilyCli)
        );
        assert_eq!(
            sync_cli_response_conversion_kind("claude:cli", "openai:compact"),
            Some(SyncCliResponseConversionKind::ToOpenAIFamilyCli)
        );
        assert_eq!(
            sync_cli_response_conversion_kind("openai:compact", "claude:cli"),
            None
        );
        assert_eq!(
            sync_cli_response_conversion_kind("gemini:cli", "claude:cli"),
            Some(SyncCliResponseConversionKind::ToClaudeCli)
        );
    }

    #[test]
    fn request_candidate_registry_excludes_compact_as_cross_format_target() {
        assert_eq!(
            request_candidate_api_formats("openai:chat", false),
            vec![
                "openai:chat",
                "claude:chat",
                "gemini:chat",
                "openai:cli",
                "claude:cli",
                "gemini:cli",
            ]
        );
        assert_eq!(
            request_candidate_api_formats("openai:cli", false),
            vec![
                "openai:cli",
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
                "openai:cli",
                "gemini:cli",
                "claude:chat",
                "openai:chat",
                "gemini:chat",
            ]
        );
        assert_eq!(
            request_candidate_api_formats("openai:compact", false),
            vec!["openai:compact"]
        );
    }

    #[test]
    fn request_candidate_registry_prefers_same_kind_before_same_family_fallbacks() {
        assert_eq!(
            request_candidate_api_format_preference("claude:cli", "openai:cli"),
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
            "openai:cli"
        ));
        assert!(request_conversion_requires_enable_flag(
            "openai:cli",
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
                api_format: "openai:cli".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("cli".to_string()),
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
                api_formats: Some(vec!["openai:cli".to_string()]),
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
            "openai:cli"
        ));
        assert!(request_pair_allowed_for_transport(
            &transport,
            "claude:cli",
            "openai:cli"
        ));
        assert!(!request_pair_allowed_for_transport(
            &transport,
            "gemini:cli",
            "openai:cli"
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
                api_format: "openai:cli".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("cli".to_string()),
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
                api_formats: Some(vec!["openai:cli".to_string()]),
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
            "openai:cli"
        ));
    }
}
