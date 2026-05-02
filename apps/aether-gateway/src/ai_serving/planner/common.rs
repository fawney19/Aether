use axum::body::Bytes;

use crate::ai_serving::{
    force_upstream_streaming_for_provider as force_upstream_streaming_for_provider_impl,
    is_json_request, parse_direct_request_body as parse_direct_request_body_impl,
};
pub(crate) use crate::ai_serving::{
    CLAUDE_CHAT_STREAM_PLAN_KIND, CLAUDE_CHAT_SYNC_PLAN_KIND, CLAUDE_CLI_STREAM_PLAN_KIND,
    CLAUDE_CLI_SYNC_PLAN_KIND, EXECUTION_RUNTIME_STREAM_ACTION,
    EXECUTION_RUNTIME_STREAM_DECISION_ACTION, EXECUTION_RUNTIME_SYNC_ACTION,
    EXECUTION_RUNTIME_SYNC_DECISION_ACTION, GEMINI_CHAT_STREAM_PLAN_KIND,
    GEMINI_CHAT_SYNC_PLAN_KIND, GEMINI_CLI_STREAM_PLAN_KIND, GEMINI_CLI_SYNC_PLAN_KIND,
    GEMINI_FILES_DELETE_PLAN_KIND, GEMINI_FILES_DOWNLOAD_PLAN_KIND, GEMINI_FILES_GET_PLAN_KIND,
    GEMINI_FILES_LIST_PLAN_KIND, GEMINI_FILES_UPLOAD_PLAN_KIND, GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND,
    GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND, OPENAI_CHAT_STREAM_PLAN_KIND, OPENAI_CHAT_SYNC_PLAN_KIND,
    OPENAI_IMAGE_STREAM_PLAN_KIND, OPENAI_IMAGE_SYNC_PLAN_KIND,
    OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND, OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND,
    OPENAI_RESPONSES_STREAM_PLAN_KIND, OPENAI_RESPONSES_SYNC_PLAN_KIND,
    OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND, OPENAI_VIDEO_CONTENT_PLAN_KIND,
    OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND, OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND,
    OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND,
};

pub(crate) use aether_ai_serving::AiRequestedModelFamily as RequestedModelFamily;

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

pub(crate) fn extract_standard_requested_model(body_json: &serde_json::Value) -> Option<String> {
    aether_ai_serving::extract_ai_standard_requested_model(body_json)
}

pub(crate) fn extract_requested_model_from_request(
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    family: RequestedModelFamily,
) -> Option<String> {
    aether_ai_serving::extract_ai_requested_model_from_request_path(
        parts.uri.path(),
        body_json,
        family,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        extract_requested_model_from_request, extract_standard_requested_model,
        force_upstream_streaming_for_provider, RequestedModelFamily,
    };
    use axum::http::Request;
    use serde_json::json;

    #[test]
    fn forces_streaming_for_codex_openai_responses() {
        assert!(force_upstream_streaming_for_provider(
            "codex",
            "openai:responses"
        ));
        assert!(!force_upstream_streaming_for_provider(
            "codex",
            "openai:responses:compact"
        ));
    }

    #[test]
    fn does_not_force_streaming_for_compact_or_other_provider_types() {
        assert!(!force_upstream_streaming_for_provider(
            "codex",
            "openai:responses:compact"
        ));
        assert!(!force_upstream_streaming_for_provider(
            "codex",
            "openai:responses:compact"
        ));
        assert!(!force_upstream_streaming_for_provider(
            "openai",
            "openai:responses"
        ));
    }

    #[test]
    fn extracts_standard_requested_model_from_request_body() {
        let requested_model =
            extract_standard_requested_model(&json!({ "model": " claude-sonnet-4 " }));

        assert_eq!(requested_model.as_deref(), Some("claude-sonnet-4"));
    }

    #[test]
    fn request_family_helper_delegates_standard_model_extraction() {
        let request = Request::builder()
            .uri("https://example.test/v1/chat/completions")
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();

        let requested_model = extract_requested_model_from_request(
            &parts,
            &json!({ "model": " claude-sonnet-4 " }),
            RequestedModelFamily::Standard,
        );

        assert_eq!(requested_model.as_deref(), Some("claude-sonnet-4"));
    }

    #[test]
    fn extracts_gemini_requested_model_from_request_path() {
        let request = Request::builder()
            .uri("https://example.test/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse")
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();

        let requested_model =
            extract_requested_model_from_request(&parts, &json!({}), RequestedModelFamily::Gemini);

        assert_eq!(requested_model.as_deref(), Some("gemini-2.5-pro"));
    }
}
