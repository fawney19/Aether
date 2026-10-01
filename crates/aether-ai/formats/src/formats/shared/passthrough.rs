use crate::contracts::{
    ApiOperation, CLAUDE_CHAT_STREAM_PLAN_KIND, CLAUDE_CHAT_SYNC_PLAN_KIND,
    CLAUDE_CLI_STREAM_PLAN_KIND, CLAUDE_CLI_SYNC_PLAN_KIND, CLAUDE_COUNT_TOKENS_SYNC_PLAN_KIND,
    CLAUDE_COUNT_TOKENS_SYNC_SUCCESS_REPORT_KIND, CODEX_LIVE_STREAM_PLAN_KIND,
    GEMINI_CHAT_STREAM_PLAN_KIND, GEMINI_CHAT_SYNC_PLAN_KIND, GEMINI_CLI_STREAM_PLAN_KIND,
    GEMINI_CLI_SYNC_PLAN_KIND, GEMINI_EMBEDDING_SYNC_PLAN_KIND,
    GEMINI_EMBEDDING_SYNC_SUCCESS_REPORT_KIND, GEMINI_INTERACTIONS_STREAM_PLAN_KIND,
    GEMINI_INTERACTIONS_STREAM_SUCCESS_REPORT_KIND, GEMINI_INTERACTIONS_SYNC_PLAN_KIND,
    GEMINI_INTERACTIONS_SYNC_SUCCESS_REPORT_KIND, OPENAI_EMBEDDING_SYNC_PLAN_KIND,
    OPENAI_MEMORIES_SYNC_PLAN_KIND, OPENAI_MEMORIES_SYNC_SUCCESS_REPORT_KIND,
    OPENAI_REALTIME_STREAM_PLAN_KIND, OPENAI_RERANK_SYNC_PLAN_KIND, OPENAI_SEARCH_SYNC_PLAN_KIND,
    OPENAI_SEARCH_SYNC_SUCCESS_REPORT_KIND,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalSameFormatProviderFamily {
    Standard,
    Gemini,
}

#[derive(Debug, Clone, Copy)]
pub struct LocalSameFormatProviderSpec {
    pub api_format: &'static str,
    pub decision_kind: &'static str,
    pub report_kind: &'static str,
    pub family: LocalSameFormatProviderFamily,
    pub require_streaming: bool,
    pub operation: Option<ApiOperation>,
}

pub fn resolve_sync_spec(plan_kind: &str) -> Option<LocalSameFormatProviderSpec> {
    match plan_kind {
        OPENAI_MEMORIES_SYNC_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "openai:responses",
            decision_kind: OPENAI_MEMORIES_SYNC_PLAN_KIND,
            report_kind: OPENAI_MEMORIES_SYNC_SUCCESS_REPORT_KIND,
            family: LocalSameFormatProviderFamily::Standard,
            require_streaming: false,
            operation: Some(ApiOperation::OpenAiMemoriesSummarize),
        }),
        CLAUDE_CHAT_SYNC_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "claude:messages",
            decision_kind: CLAUDE_CHAT_SYNC_PLAN_KIND,
            report_kind: "claude_chat_sync_success",
            family: LocalSameFormatProviderFamily::Standard,
            require_streaming: false,
            operation: Some(ApiOperation::ClaudeMessagesCreate),
        }),
        CLAUDE_CLI_SYNC_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "claude:messages",
            decision_kind: CLAUDE_CLI_SYNC_PLAN_KIND,
            report_kind: "claude_cli_sync_success",
            family: LocalSameFormatProviderFamily::Standard,
            require_streaming: false,
            operation: Some(ApiOperation::ClaudeMessagesCreate),
        }),
        CLAUDE_COUNT_TOKENS_SYNC_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "claude:messages",
            decision_kind: CLAUDE_COUNT_TOKENS_SYNC_PLAN_KIND,
            report_kind: CLAUDE_COUNT_TOKENS_SYNC_SUCCESS_REPORT_KIND,
            family: LocalSameFormatProviderFamily::Standard,
            require_streaming: false,
            operation: Some(ApiOperation::ClaudeCountTokens),
        }),
        GEMINI_CHAT_SYNC_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "gemini:generate_content",
            decision_kind: GEMINI_CHAT_SYNC_PLAN_KIND,
            report_kind: "gemini_chat_sync_success",
            family: LocalSameFormatProviderFamily::Gemini,
            require_streaming: false,
            operation: None,
        }),
        GEMINI_CLI_SYNC_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "gemini:generate_content",
            decision_kind: GEMINI_CLI_SYNC_PLAN_KIND,
            report_kind: "gemini_cli_sync_success",
            family: LocalSameFormatProviderFamily::Gemini,
            require_streaming: false,
            operation: None,
        }),
        GEMINI_EMBEDDING_SYNC_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "gemini:embedding",
            decision_kind: GEMINI_EMBEDDING_SYNC_PLAN_KIND,
            report_kind: GEMINI_EMBEDDING_SYNC_SUCCESS_REPORT_KIND,
            family: LocalSameFormatProviderFamily::Gemini,
            require_streaming: false,
            operation: None,
        }),
        GEMINI_INTERACTIONS_SYNC_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "gemini:interactions",
            decision_kind: GEMINI_INTERACTIONS_SYNC_PLAN_KIND,
            report_kind: GEMINI_INTERACTIONS_SYNC_SUCCESS_REPORT_KIND,
            family: LocalSameFormatProviderFamily::Gemini,
            require_streaming: false,
            operation: None,
        }),
        OPENAI_EMBEDDING_SYNC_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "openai:embedding",
            decision_kind: OPENAI_EMBEDDING_SYNC_PLAN_KIND,
            report_kind: "openai_embedding_sync_success",
            family: LocalSameFormatProviderFamily::Standard,
            require_streaming: false,
            operation: None,
        }),
        OPENAI_RERANK_SYNC_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "openai:rerank",
            decision_kind: OPENAI_RERANK_SYNC_PLAN_KIND,
            report_kind: "openai_rerank_sync_success",
            family: LocalSameFormatProviderFamily::Standard,
            require_streaming: false,
            operation: None,
        }),
        OPENAI_SEARCH_SYNC_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "openai:search",
            decision_kind: OPENAI_SEARCH_SYNC_PLAN_KIND,
            report_kind: OPENAI_SEARCH_SYNC_SUCCESS_REPORT_KIND,
            family: LocalSameFormatProviderFamily::Standard,
            require_streaming: false,
            operation: None,
        }),
        _ => None,
    }
}

pub fn resolve_stream_spec(plan_kind: &str) -> Option<LocalSameFormatProviderSpec> {
    match plan_kind {
        CODEX_LIVE_STREAM_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "codex:live",
            decision_kind: CODEX_LIVE_STREAM_PLAN_KIND,
            report_kind: "codex_live_websocket_success",
            family: LocalSameFormatProviderFamily::Standard,
            require_streaming: true,
            operation: None,
        }),
        OPENAI_REALTIME_STREAM_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "openai:realtime",
            decision_kind: OPENAI_REALTIME_STREAM_PLAN_KIND,
            report_kind: "openai_realtime_websocket_success",
            family: LocalSameFormatProviderFamily::Standard,
            require_streaming: true,
            operation: None,
        }),
        CLAUDE_CHAT_STREAM_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "claude:messages",
            decision_kind: CLAUDE_CHAT_STREAM_PLAN_KIND,
            report_kind: "claude_chat_stream_success",
            family: LocalSameFormatProviderFamily::Standard,
            require_streaming: true,
            operation: Some(ApiOperation::ClaudeMessagesCreate),
        }),
        CLAUDE_CLI_STREAM_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "claude:messages",
            decision_kind: CLAUDE_CLI_STREAM_PLAN_KIND,
            report_kind: "claude_cli_stream_success",
            family: LocalSameFormatProviderFamily::Standard,
            require_streaming: true,
            operation: Some(ApiOperation::ClaudeMessagesCreate),
        }),
        GEMINI_CHAT_STREAM_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "gemini:generate_content",
            decision_kind: GEMINI_CHAT_STREAM_PLAN_KIND,
            report_kind: "gemini_chat_stream_success",
            family: LocalSameFormatProviderFamily::Gemini,
            require_streaming: true,
            operation: None,
        }),
        GEMINI_CLI_STREAM_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "gemini:generate_content",
            decision_kind: GEMINI_CLI_STREAM_PLAN_KIND,
            report_kind: "gemini_cli_stream_success",
            family: LocalSameFormatProviderFamily::Gemini,
            require_streaming: true,
            operation: None,
        }),
        GEMINI_INTERACTIONS_STREAM_PLAN_KIND => Some(LocalSameFormatProviderSpec {
            api_format: "gemini:interactions",
            decision_kind: GEMINI_INTERACTIONS_STREAM_PLAN_KIND,
            report_kind: GEMINI_INTERACTIONS_STREAM_SUCCESS_REPORT_KIND,
            family: LocalSameFormatProviderFamily::Gemini,
            require_streaming: true,
            operation: None,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_stream_spec, resolve_sync_spec};

    #[test]
    fn resolves_claude_sync_same_format_spec() {
        let spec = resolve_sync_spec("claude_chat_sync").expect("spec");
        assert_eq!(spec.api_format, "claude:messages");
        assert_eq!(spec.report_kind, "claude_chat_sync_success");
        assert!(!spec.require_streaming);
    }

    #[test]
    fn resolves_gemini_stream_same_format_spec() {
        let spec = resolve_stream_spec("gemini_cli_stream").expect("spec");
        assert_eq!(spec.api_format, "gemini:generate_content");
        assert_eq!(spec.report_kind, "gemini_cli_stream_success");
        assert!(spec.require_streaming);
    }

    #[test]
    fn resolves_openai_realtime_websocket_same_format_spec() {
        let spec = resolve_stream_spec("openai_realtime_stream").expect("spec");
        assert_eq!(spec.api_format, "openai:realtime");
        assert_eq!(spec.report_kind, "openai_realtime_websocket_success");
        assert!(spec.require_streaming);
        assert_eq!(spec.family, super::LocalSameFormatProviderFamily::Standard);
    }

    #[test]
    fn resolves_codex_live_websocket_same_format_spec() {
        let spec = resolve_stream_spec("codex_live_stream").expect("spec");
        assert_eq!(spec.api_format, "codex:live");
        assert_eq!(spec.report_kind, "codex_live_websocket_success");
        assert!(spec.require_streaming);
        assert_eq!(spec.family, super::LocalSameFormatProviderFamily::Standard);
    }

    #[test]
    fn resolves_openai_embedding_sync_same_format_spec() {
        let spec = resolve_sync_spec("openai_embedding_sync").expect("spec");
        assert_eq!(spec.api_format, "openai:embedding");
        assert_eq!(spec.report_kind, "openai_embedding_sync_success");
        assert!(!spec.require_streaming);
    }

    #[test]
    fn resolves_gemini_embedding_sync_same_format_spec() {
        let spec = resolve_sync_spec("gemini_embedding_sync").expect("spec");
        assert_eq!(spec.api_format, "gemini:embedding");
        assert_eq!(spec.report_kind, "gemini_embedding_sync_success");
        assert_eq!(spec.family, super::LocalSameFormatProviderFamily::Gemini);
        assert!(!spec.require_streaming);
    }

    #[test]
    fn resolves_gemini_interactions_same_format_specs() {
        let sync = resolve_sync_spec("gemini_interactions_sync").expect("sync spec");
        assert_eq!(sync.api_format, "gemini:interactions");
        assert_eq!(sync.report_kind, "gemini_interactions_sync_success");
        assert_eq!(sync.family, super::LocalSameFormatProviderFamily::Gemini);
        assert!(!sync.require_streaming);

        let stream = resolve_stream_spec("gemini_interactions_stream").expect("stream spec");
        assert_eq!(stream.api_format, "gemini:interactions");
        assert_eq!(stream.report_kind, "gemini_interactions_stream_success");
        assert_eq!(stream.family, super::LocalSameFormatProviderFamily::Gemini);
        assert!(stream.require_streaming);
    }

    #[test]
    fn resolves_openai_rerank_sync_same_format_spec() {
        let spec = resolve_sync_spec("openai_rerank_sync").expect("spec");
        assert_eq!(spec.api_format, "openai:rerank");
        assert_eq!(spec.report_kind, "openai_rerank_sync_success");
        assert!(!spec.require_streaming);
    }

    #[test]
    fn resolves_openai_search_sync_spec() {
        let spec = resolve_sync_spec("openai_search_sync").expect("spec");
        assert_eq!(spec.api_format, "openai:search");
        assert_eq!(spec.report_kind, "openai_search_sync_success");
        assert!(!spec.require_streaming);
    }

    #[test]
    fn memories_uses_responses_permissions_and_a_native_sync_operation() {
        let spec = resolve_sync_spec("openai_memories_sync").expect("memory spec");
        assert_eq!(spec.api_format, "openai:responses");
        assert_eq!(
            spec.operation,
            Some(crate::ApiOperation::OpenAiMemoriesSummarize)
        );
        assert_eq!(spec.report_kind, "openai_memories_sync_success");
        assert!(!spec.require_streaming);
    }
}
