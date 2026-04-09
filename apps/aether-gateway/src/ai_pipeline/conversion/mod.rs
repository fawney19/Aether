#[cfg(test)]
pub(crate) use crate::ai_pipeline::core_success_background_report_kind;
pub(crate) use crate::ai_pipeline::{
    build_core_error_body_for_client_format, core_error_background_report_kind,
    core_error_default_client_api_format, is_core_error_finalize_kind, LocalCoreSyncErrorKind,
};
pub(crate) use crate::ai_pipeline::{
    request_conversion_direct_auth, request_conversion_kind,
    request_conversion_transport_supported, sync_chat_response_conversion_kind,
    sync_cli_response_conversion_kind, RequestConversionKind, SyncChatResponseConversionKind,
    SyncCliResponseConversionKind,
};

#[cfg(test)]
mod tests {
    use super::{
        request_conversion_kind, sync_chat_response_conversion_kind,
        sync_cli_response_conversion_kind, RequestConversionKind, SyncChatResponseConversionKind,
        SyncCliResponseConversionKind,
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
}
