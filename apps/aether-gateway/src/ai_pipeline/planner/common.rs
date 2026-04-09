use axum::body::Bytes;

pub(crate) use crate::ai_pipeline::contracts::{
    CLAUDE_CHAT_STREAM_PLAN_KIND, CLAUDE_CHAT_SYNC_PLAN_KIND, CLAUDE_CLI_STREAM_PLAN_KIND,
    CLAUDE_CLI_SYNC_PLAN_KIND, EXECUTION_RUNTIME_STREAM_ACTION,
    EXECUTION_RUNTIME_STREAM_DECISION_ACTION, EXECUTION_RUNTIME_SYNC_ACTION,
    EXECUTION_RUNTIME_SYNC_DECISION_ACTION, GEMINI_CHAT_STREAM_PLAN_KIND,
    GEMINI_CHAT_SYNC_PLAN_KIND, GEMINI_CLI_STREAM_PLAN_KIND, GEMINI_CLI_SYNC_PLAN_KIND,
    GEMINI_FILES_DELETE_PLAN_KIND, GEMINI_FILES_DOWNLOAD_PLAN_KIND, GEMINI_FILES_GET_PLAN_KIND,
    GEMINI_FILES_LIST_PLAN_KIND, GEMINI_FILES_UPLOAD_PLAN_KIND, GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND,
    GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND, OPENAI_CHAT_STREAM_PLAN_KIND, OPENAI_CHAT_SYNC_PLAN_KIND,
    OPENAI_CLI_STREAM_PLAN_KIND, OPENAI_CLI_SYNC_PLAN_KIND, OPENAI_COMPACT_STREAM_PLAN_KIND,
    OPENAI_COMPACT_SYNC_PLAN_KIND, OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND,
    OPENAI_VIDEO_CONTENT_PLAN_KIND, OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND,
    OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND, OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND,
};
use crate::ai_pipeline::{
    force_upstream_streaming_for_provider as force_upstream_streaming_for_provider_impl,
    is_json_request, parse_direct_request_body as parse_direct_request_body_impl,
};

pub(crate) fn parse_direct_request_body(
    parts: &http::request::Parts,
    body_bytes: &Bytes,
) -> Option<(serde_json::Value, Option<String>)> {
    parse_direct_request_body_impl(is_json_request(&parts.headers), body_bytes.as_ref())
}

pub(crate) fn force_upstream_streaming_for_provider(
    provider_type: &str,
    provider_api_format: &str,
) -> bool {
    force_upstream_streaming_for_provider_impl(provider_type, provider_api_format)
}

#[cfg(test)]
mod tests {
    use super::force_upstream_streaming_for_provider;

    #[test]
    fn forces_streaming_for_codex_openai_cli() {
        assert!(force_upstream_streaming_for_provider("codex", "openai:cli"));
    }

    #[test]
    fn does_not_force_streaming_for_compact_or_other_provider_types() {
        assert!(!force_upstream_streaming_for_provider(
            "codex",
            "openai:compact"
        ));
        assert!(!force_upstream_streaming_for_provider(
            "openai",
            "openai:cli"
        ));
    }
}
