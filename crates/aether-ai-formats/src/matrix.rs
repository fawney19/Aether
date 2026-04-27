use crate::{
    formats::{is_openai_responses_compact_format, normalize_legacy_openai_format_alias},
    legacy_openai_format_alias_matches,
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
    let client_api_format = normalize_legacy_openai_format_alias(client_api_format);
    let provider_api_format = normalize_legacy_openai_format_alias(provider_api_format);

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
        standard_api_family_priority(provider_family),
    ))
}

pub fn request_candidate_api_formats(
    client_api_format: &str,
    _require_streaming: bool,
) -> Vec<&'static str> {
    let client_api_format = normalize_legacy_openai_format_alias(client_api_format);
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
    let client_api_format = normalize_legacy_openai_format_alias(client_api_format);
    let provider_api_format = normalize_legacy_openai_format_alias(provider_api_format);
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
        "claude:chat" | "claude:cli" => Some(RequestConversionKind::ToClaudeStandard),
        "gemini:chat" | "gemini:cli" => Some(RequestConversionKind::ToGeminiStandard),
        _ => None,
    }
}

pub fn sync_chat_response_conversion_kind(
    provider_api_format: &str,
    client_api_format: &str,
) -> Option<SyncChatResponseConversionKind> {
    let provider_api_format = normalize_legacy_openai_format_alias(provider_api_format);
    let client_api_format = normalize_legacy_openai_format_alias(client_api_format);
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
    let provider_api_format = normalize_legacy_openai_format_alias(provider_api_format);
    let client_api_format = normalize_legacy_openai_format_alias(client_api_format);
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
        "claude:cli" => Some(SyncCliResponseConversionKind::ToClaudeCli),
        "gemini:cli" => Some(SyncCliResponseConversionKind::ToGeminiCli),
        _ => None,
    }
}

pub fn request_conversion_requires_enable_flag(
    client_api_format: &str,
    provider_api_format: &str,
) -> bool {
    let client_api_format = normalize_legacy_openai_format_alias(client_api_format);
    let provider_api_format = normalize_legacy_openai_format_alias(provider_api_format);
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
        normalize_legacy_openai_format_alias(api_format).as_str(),
        "openai:chat"
            | "openai:responses"
            | "openai:responses:compact"
            | "claude:chat"
            | "claude:cli"
            | "gemini:chat"
            | "gemini:cli"
    )
}

pub fn parse_non_compact_standard_api_format(
    api_format: &str,
) -> Option<(&'static str, &'static str)> {
    match normalize_legacy_openai_format_alias(api_format).as_str() {
        "openai:chat" => Some(("openai", "chat")),
        "openai:responses" => Some(("openai", "cli")),
        "claude:chat" => Some(("claude", "chat")),
        "claude:cli" => Some(("claude", "cli")),
        "gemini:chat" => Some(("gemini", "chat")),
        "gemini:cli" => Some(("gemini", "cli")),
        _ => None,
    }
}

pub fn api_data_format_id(api_format: &str) -> Option<&'static str> {
    match normalize_legacy_openai_format_alias(api_format).as_str() {
        "claude:chat" | "claude:cli" => Some("claude"),
        "gemini:chat" | "gemini:cli" => Some("gemini"),
        "openai:chat" => Some("openai_chat"),
        "openai:responses" | "openai:responses:compact" => Some("openai_responses"),
        _ => None,
    }
}

pub fn normalized_same_standard_api_format(left: &str, right: &str) -> bool {
    legacy_openai_format_alias_matches(left, right)
}

fn standard_api_family_priority(family: &str) -> u8 {
    STANDARD_API_FAMILY_ORDER
        .iter()
        .position(|candidate| *candidate == family)
        .unwrap_or(STANDARD_API_FAMILY_ORDER.len()) as u8
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
            "claude:chat" | "claude:cli" => RequestConversionKind::ToClaudeStandard,
            "gemini:chat" | "gemini:cli" => RequestConversionKind::ToGeminiStandard,
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

        let formats = [
            "openai:chat",
            "openai:responses",
            "claude:chat",
            "claude:cli",
            "gemini:chat",
            "gemini:cli",
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
    fn request_candidate_registry_prefers_same_kind_before_same_family_fallbacks() {
        assert_eq!(
            request_candidate_api_formats("openai:chat", false),
            vec![
                "openai:chat",
                "claude:chat",
                "gemini:chat",
                "openai:responses",
                "claude:cli",
                "gemini:cli"
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
                "gemini:chat"
            ]
        );
        assert_eq!(
            request_candidate_api_formats("openai:cli", false),
            request_candidate_api_formats("openai:responses", false)
        );
        assert_eq!(
            request_candidate_api_formats("claude:cli", false),
            vec![
                "claude:cli",
                "openai:responses",
                "gemini:cli",
                "claude:chat",
                "openai:chat",
                "gemini:chat"
            ]
        );
        assert_eq!(
            request_candidate_api_formats("openai:compact", false),
            vec!["openai:responses:compact"]
        );
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
            "claude:chat",
            "gemini:chat"
        ));
        assert!(request_conversion_requires_enable_flag(
            "openai:compact",
            "claude:cli"
        ));
    }
}
