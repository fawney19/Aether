use crate::{
    api_format_alias_matches,
    protocol::formats::{is_openai_responses_compact_format, normalize_api_format_alias},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestConversionKind {
    ToOpenAIChat,
    ToOpenAiResponses,
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
    ToOpenAiResponses,
    ToClaudeCli,
    ToGeminiCli,
}

const NON_COMPACT_STANDARD_CANDIDATE_API_FORMATS: &[&str] = &[
    "openai:chat",
    "openai:responses",
    "claude:messages",
    "gemini:generate_content",
];
const STANDARD_API_FORMAT_ORDER: &[&str] = &[
    "openai:chat",
    "openai:responses",
    "claude:messages",
    "gemini:generate_content",
];

pub fn request_candidate_api_format_preference(
    client_api_format: &str,
    provider_api_format: &str,
) -> Option<(u8, u8)> {
    let client_api_format = normalize_api_format_alias(client_api_format);
    let provider_api_format = normalize_api_format_alias(provider_api_format);

    if client_api_format == "openai:responses:compact" {
        return (provider_api_format == "openai:responses:compact").then_some((0, 0));
    }

    let (client_family, client_kind) =
        parse_non_compact_standard_api_format(client_api_format.as_str())?;
    let (provider_family, provider_kind) =
        parse_non_compact_standard_api_format(provider_api_format.as_str())?;
    let preference_bucket = if client_api_format == provider_api_format {
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
        standard_api_format_priority(provider_api_format.as_str()),
    ))
}

pub fn request_candidate_api_formats(
    client_api_format: &str,
    _require_streaming: bool,
) -> Vec<&'static str> {
    let client_api_format = normalize_api_format_alias(client_api_format);
    if client_api_format == "openai:responses:compact" {
        return vec!["openai:responses:compact"];
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
    let client_api_format = normalize_api_format_alias(client_api_format);
    let provider_api_format = normalize_api_format_alias(provider_api_format);
    if client_api_format == provider_api_format {
        return None;
    }
    if !is_standard_api_format(client_api_format.as_str())
        || !is_standard_api_format(provider_api_format.as_str())
    {
        return None;
    }
    if is_openai_responses_compact_format(client_api_format.as_str())
        || is_openai_responses_compact_format(provider_api_format.as_str())
    {
        return None;
    }

    match provider_api_format.as_str() {
        "openai:chat" => Some(RequestConversionKind::ToOpenAIChat),
        "openai:responses" => Some(RequestConversionKind::ToOpenAiResponses),
        "claude:messages" => Some(RequestConversionKind::ToClaudeStandard),
        "gemini:generate_content" => Some(RequestConversionKind::ToGeminiStandard),
        _ => None,
    }
}

pub fn sync_chat_response_conversion_kind(
    provider_api_format: &str,
    client_api_format: &str,
) -> Option<SyncChatResponseConversionKind> {
    let provider_api_format = normalize_api_format_alias(provider_api_format);
    let client_api_format = normalize_api_format_alias(client_api_format);
    if provider_api_format == client_api_format {
        return None;
    }
    if !is_standard_api_format(provider_api_format.as_str()) {
        return None;
    }
    request_conversion_kind(client_api_format.as_str(), provider_api_format.as_str())?;
    match client_api_format.as_str() {
        "openai:chat" => Some(SyncChatResponseConversionKind::ToOpenAIChat),
        "claude:messages" => Some(SyncChatResponseConversionKind::ToClaudeChat),
        "gemini:generate_content" => Some(SyncChatResponseConversionKind::ToGeminiChat),
        _ => None,
    }
}

pub fn sync_cli_response_conversion_kind(
    provider_api_format: &str,
    client_api_format: &str,
) -> Option<SyncCliResponseConversionKind> {
    let provider_api_format = normalize_api_format_alias(provider_api_format);
    let client_api_format = normalize_api_format_alias(client_api_format);
    if provider_api_format == client_api_format {
        return None;
    }
    if !is_standard_api_format(provider_api_format.as_str()) {
        return None;
    }
    if !is_openai_responses_compact_format(client_api_format.as_str()) {
        request_conversion_kind(client_api_format.as_str(), provider_api_format.as_str())?;
    }
    match client_api_format.as_str() {
        "openai:responses" | "openai:responses:compact" => {
            Some(SyncCliResponseConversionKind::ToOpenAiResponses)
        }
        "claude:messages" => Some(SyncCliResponseConversionKind::ToClaudeCli),
        "gemini:generate_content" => Some(SyncCliResponseConversionKind::ToGeminiCli),
        _ => None,
    }
}

pub fn request_conversion_requires_enable_flag(
    client_api_format: &str,
    provider_api_format: &str,
) -> bool {
    let client_api_format = normalize_api_format_alias(client_api_format);
    let provider_api_format = normalize_api_format_alias(provider_api_format);
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

pub fn is_standard_api_format(api_format: &str) -> bool {
    matches!(
        normalize_api_format_alias(api_format).as_str(),
        "openai:chat"
            | "openai:responses"
            | "openai:responses:compact"
            | "claude:messages"
            | "gemini:generate_content"
    )
}

pub fn parse_non_compact_standard_api_format(
    api_format: &str,
) -> Option<(&'static str, &'static str)> {
    match normalize_api_format_alias(api_format).as_str() {
        "openai:chat" => Some(("openai", "chat")),
        "openai:responses" => Some(("openai", "responses")),
        "claude:messages" => Some(("claude", "messages")),
        "gemini:generate_content" => Some(("gemini", "generate_content")),
        _ => None,
    }
}

pub fn api_data_format_id(api_format: &str) -> Option<&'static str> {
    match normalize_api_format_alias(api_format).as_str() {
        "claude:messages" => Some("claude"),
        "gemini:generate_content" => Some("gemini"),
        "openai:chat" => Some("openai_chat"),
        "openai:responses" | "openai:responses:compact" => Some("openai_responses"),
        _ => None,
    }
}

pub fn normalized_same_standard_api_format(left: &str, right: &str) -> bool {
    api_format_alias_matches(left, right)
}

fn standard_api_format_priority(api_format: &str) -> u8 {
    let api_format = normalize_api_format_alias(api_format);
    STANDARD_API_FORMAT_ORDER
        .iter()
        .position(|candidate| *candidate == api_format)
        .unwrap_or(STANDARD_API_FORMAT_ORDER.len()) as u8
}

#[cfg(test)]
mod tests {
    use super::{
        request_candidate_api_format_preference, request_candidate_api_formats,
        request_conversion_kind, request_conversion_requires_enable_flag,
        sync_chat_response_conversion_kind, sync_cli_response_conversion_kind,
        RequestConversionKind, SyncChatResponseConversionKind, SyncCliResponseConversionKind,
    };

    fn expected_request_conversion_kind(provider_api_format: &str) -> RequestConversionKind {
        match provider_api_format {
            "openai:chat" => RequestConversionKind::ToOpenAIChat,
            "openai:responses" => RequestConversionKind::ToOpenAiResponses,
            "claude:messages" => RequestConversionKind::ToClaudeStandard,
            "gemini:generate_content" => RequestConversionKind::ToGeminiStandard,
            other => panic!("unexpected provider format {other}"),
        }
    }

    #[test]
    fn request_conversion_registry_supports_bidirectional_standard_matrix() {
        assert_eq!(
            request_conversion_kind("openai:chat", "openai:responses"),
            Some(RequestConversionKind::ToOpenAiResponses)
        );
        assert_eq!(
            request_conversion_kind("openai:chat", "claude:messages"),
            Some(RequestConversionKind::ToClaudeStandard)
        );
        assert_eq!(
            request_conversion_kind("openai:responses", "openai:chat"),
            Some(RequestConversionKind::ToOpenAIChat)
        );
        assert_eq!(
            request_conversion_kind("openai:responses:compact", "gemini:generate_content"),
            None
        );
        assert_eq!(
            request_conversion_kind("gemini:generate_content", "openai:responses:compact"),
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
            request_conversion_kind("gemini:generate_content", "claude:messages"),
            Some(RequestConversionKind::ToClaudeStandard)
        );
        assert_eq!(request_conversion_kind("claude:chat", "claude:cli"), None);
        assert_eq!(
            request_conversion_kind("claude:messages", "claude:messages"),
            None
        );

        let formats = [
            "openai:chat",
            "openai:responses",
            "claude:messages",
            "gemini:generate_content",
        ];
        for client_api_format in formats {
            for provider_api_format in formats {
                let actual = request_conversion_kind(client_api_format, provider_api_format);
                if client_api_format == provider_api_format {
                    assert_eq!(actual, None, "{client_api_format} -> {provider_api_format}");
                } else {
                    assert_eq!(
                        actual,
                        Some(expected_request_conversion_kind(provider_api_format)),
                        "{client_api_format} -> {provider_api_format}"
                    );
                }
            }
        }
    }

    #[test]
    fn sync_response_conversion_registry_supports_bidirectional_standard_matrix() {
        assert_eq!(
            sync_chat_response_conversion_kind("openai:chat", "claude:messages"),
            Some(SyncChatResponseConversionKind::ToClaudeChat)
        );
        assert_eq!(
            sync_chat_response_conversion_kind("claude:messages", "gemini:generate_content"),
            Some(SyncChatResponseConversionKind::ToGeminiChat)
        );
        assert_eq!(
            sync_chat_response_conversion_kind("gemini:generate_content", "openai:chat"),
            Some(SyncChatResponseConversionKind::ToOpenAIChat)
        );
        assert_eq!(
            sync_cli_response_conversion_kind("openai:responses", "gemini:generate_content"),
            Some(SyncCliResponseConversionKind::ToGeminiCli)
        );
        assert_eq!(
            sync_cli_response_conversion_kind("claude:messages", "openai:responses"),
            Some(SyncCliResponseConversionKind::ToOpenAiResponses)
        );
        assert_eq!(
            sync_cli_response_conversion_kind("claude:messages", "openai:responses:compact"),
            Some(SyncCliResponseConversionKind::ToOpenAiResponses)
        );
        assert_eq!(
            sync_cli_response_conversion_kind("openai:responses:compact", "claude:messages"),
            None
        );
        assert_eq!(
            sync_cli_response_conversion_kind("gemini:generate_content", "claude:messages"),
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
    fn request_candidate_registry_prefers_same_kind_before_same_family_fallbacks() {
        assert_eq!(
            request_candidate_api_formats("openai:chat", false),
            vec![
                "openai:chat",
                "openai:responses",
                "claude:messages",
                "gemini:generate_content"
            ]
        );
        assert_eq!(
            request_candidate_api_formats("openai:responses", false),
            vec![
                "openai:responses",
                "openai:chat",
                "claude:messages",
                "gemini:generate_content"
            ]
        );
        assert_eq!(
            request_candidate_api_formats("claude:messages", false),
            vec![
                "claude:messages",
                "openai:chat",
                "openai:responses",
                "gemini:generate_content"
            ]
        );
        assert!(
            request_candidate_api_format_preference("claude:messages", "openai:chat")
                < request_candidate_api_format_preference("claude:messages", "openai:responses")
        );
        assert_eq!(
            request_candidate_api_formats("openai:cli", false),
            Vec::<&'static str>::new()
        );
        assert_eq!(
            request_candidate_api_formats("claude:cli", false),
            Vec::<&'static str>::new()
        );
        assert_eq!(
            request_candidate_api_formats("openai:compact", false),
            Vec::<&'static str>::new()
        );
        assert_eq!(
            request_candidate_api_format_preference("claude:cli", "openai:responses"),
            None
        );
        assert_eq!(
            request_candidate_api_format_preference("claude:cli", "claude:chat"),
            None
        );
        assert_eq!(
            request_candidate_api_format_preference("claude:cli", "openai:chat"),
            None
        );
    }

    #[test]
    fn request_conversion_enable_flag_only_applies_to_real_data_format_conversions() {
        assert!(!request_conversion_requires_enable_flag(
            "claude:messages",
            "claude:messages"
        ));
        assert!(request_conversion_requires_enable_flag(
            "claude:chat",
            "claude:cli"
        ));
        assert!(request_conversion_requires_enable_flag(
            "openai:chat",
            "openai:responses"
        ));
        assert!(request_conversion_requires_enable_flag(
            "claude:messages",
            "gemini:generate_content"
        ));
        assert!(request_conversion_requires_enable_flag(
            "openai:compact",
            "claude:cli"
        ));
    }
}
