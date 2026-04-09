#![allow(dead_code)]

use aether_provider_transport::auth::{
    resolve_local_gemini_auth, resolve_local_openai_chat_auth, resolve_local_standard_auth,
};
use aether_provider_transport::policy::{
    supports_local_openai_chat_transport, supports_local_standard_transport_with_network,
};
use aether_provider_transport::{
    supports_local_gemini_transport_with_network, GatewayProviderTransportSnapshot,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestConversionKind {
    ToOpenAIChat,
    ToOpenAIFamilyCli,
    ToOpenAICompact,
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

    match provider_api_format.as_str() {
        "openai:chat" => Some(RequestConversionKind::ToOpenAIChat),
        "openai:cli" => Some(RequestConversionKind::ToOpenAIFamilyCli),
        "openai:compact" => Some(RequestConversionKind::ToOpenAICompact),
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
    match client_api_format.as_str() {
        "openai:cli" | "openai:compact" => Some(SyncCliResponseConversionKind::ToOpenAIFamilyCli),
        "claude:cli" => Some(SyncCliResponseConversionKind::ToClaudeCli),
        "gemini:cli" => Some(SyncCliResponseConversionKind::ToGeminiCli),
        _ => None,
    }
}

pub fn request_conversion_transport_supported(
    transport: &GatewayProviderTransportSnapshot,
    _kind: RequestConversionKind,
) -> bool {
    match transport
        .endpoint
        .api_format
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "openai:chat" => supports_local_openai_chat_transport(transport),
        "openai:cli" => supports_local_standard_transport_with_network(transport, "openai:cli"),
        "openai:compact" => {
            supports_local_standard_transport_with_network(transport, "openai:compact")
        }
        "claude:chat" => supports_local_standard_transport_with_network(transport, "claude:chat"),
        "claude:cli" => supports_local_standard_transport_with_network(transport, "claude:cli"),
        "gemini:chat" => supports_local_gemini_transport_with_network(transport, "gemini:chat"),
        "gemini:cli" => supports_local_gemini_transport_with_network(transport, "gemini:cli"),
        _ => false,
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
        "openai:chat" => resolve_local_openai_chat_auth(transport),
        "gemini:chat" | "gemini:cli" => resolve_local_gemini_auth(transport),
        "openai:cli" | "openai:compact" | "claude:chat" | "claude:cli" => {
            resolve_local_standard_auth(transport)
        }
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

#[cfg(test)]
mod tests {
    use super::{
        request_conversion_direct_auth, request_conversion_kind,
        request_conversion_transport_supported, sync_chat_response_conversion_kind,
        sync_cli_response_conversion_kind, RequestConversionKind, SyncChatResponseConversionKind,
        SyncCliResponseConversionKind,
    };
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    #[test]
    fn request_conversion_registry_supports_bidirectional_standard_matrix() {
        assert_eq!(
            request_conversion_kind("claude:chat", "openai:chat"),
            Some(RequestConversionKind::ToOpenAIChat)
        );
        assert_eq!(
            request_conversion_kind("gemini:chat", "claude:chat"),
            Some(RequestConversionKind::ToClaudeStandard)
        );
        assert_eq!(
            request_conversion_kind("gemini:cli", "openai:compact"),
            Some(RequestConversionKind::ToOpenAICompact)
        );
        assert_eq!(
            request_conversion_kind("openai:compact", "gemini:cli"),
            Some(RequestConversionKind::ToGeminiStandard)
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
            sync_cli_response_conversion_kind("claude:cli", "openai:compact"),
            Some(SyncCliResponseConversionKind::ToOpenAIFamilyCli)
        );
        assert_eq!(
            sync_cli_response_conversion_kind("gemini:cli", "claude:cli"),
            Some(SyncCliResponseConversionKind::ToClaudeCli)
        );
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
}
