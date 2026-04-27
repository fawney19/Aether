#[cfg(test)]
pub(crate) use crate::ai_pipeline::core_success_background_report_kind;
pub(crate) use crate::ai_pipeline::{
    build_core_error_body_for_client_format, core_error_background_report_kind,
    core_error_default_client_api_format, is_core_error_finalize_kind, LocalCoreSyncErrorKind,
};
pub(crate) use crate::ai_pipeline::{
    request_candidate_api_format_preference, request_candidate_api_formats,
    request_conversion_direct_auth, request_conversion_enabled_for_transport,
    request_conversion_kind, request_conversion_requires_enable_flag,
    request_conversion_transport_supported, request_conversion_transport_unsupported_reason,
    request_pair_allowed_for_transport, sync_chat_response_conversion_kind,
    sync_cli_response_conversion_kind, RequestConversionKind, SyncChatResponseConversionKind,
    SyncCliResponseConversionKind,
};

#[cfg(test)]
mod tests {
    use super::{
        request_candidate_api_format_preference, request_candidate_api_formats,
        request_conversion_kind, sync_chat_response_conversion_kind,
        sync_cli_response_conversion_kind, RequestConversionKind, SyncChatResponseConversionKind,
        SyncCliResponseConversionKind,
    };

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
            request_conversion_kind("gemini:chat", "claude:chat"),
            Some(RequestConversionKind::ToClaudeStandard)
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
}
