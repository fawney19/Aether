use axum::body::Bytes;

use crate::ai_serving::is_json_request;
use crate::ai_serving::{
    endpoint_config_forces_upstream_stream_policy as endpoint_config_forces_upstream_stream_policy_impl,
    enforce_request_body_stream_field as enforce_request_body_stream_field_impl,
    force_upstream_streaming_for_provider as force_upstream_streaming_for_provider_impl,
    parse_direct_request_body as parse_direct_request_body_impl,
    resolve_format_upstream_is_stream_for_provider as resolve_upstream_is_stream_for_provider_impl,
};
pub(crate) use crate::ai_serving::{
    CLAUDE_CHAT_STREAM_PLAN_KIND, CLAUDE_CHAT_SYNC_PLAN_KIND, CLAUDE_CLI_STREAM_PLAN_KIND,
    CLAUDE_CLI_SYNC_PLAN_KIND, EXECUTION_RUNTIME_STREAM_ACTION,
    EXECUTION_RUNTIME_STREAM_DECISION_ACTION, EXECUTION_RUNTIME_SYNC_ACTION,
    EXECUTION_RUNTIME_SYNC_DECISION_ACTION, GEMINI_CHAT_STREAM_PLAN_KIND,
    GEMINI_CHAT_SYNC_PLAN_KIND, GEMINI_CLI_STREAM_PLAN_KIND, GEMINI_CLI_SYNC_PLAN_KIND,
    GEMINI_EMBEDDING_SYNC_PLAN_KIND, GEMINI_FILES_DELETE_PLAN_KIND,
    GEMINI_FILES_DOWNLOAD_PLAN_KIND, GEMINI_FILES_GET_PLAN_KIND, GEMINI_FILES_LIST_PLAN_KIND,
    GEMINI_FILES_UPLOAD_PLAN_KIND, GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND,
    GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND, OPENAI_CHAT_STREAM_PLAN_KIND, OPENAI_CHAT_SYNC_PLAN_KIND,
    OPENAI_EMBEDDING_SYNC_PLAN_KIND, OPENAI_IMAGE_STREAM_PLAN_KIND, OPENAI_IMAGE_SYNC_PLAN_KIND,
    OPENAI_RERANK_SYNC_PLAN_KIND, OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND,
    OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND, OPENAI_RESPONSES_STREAM_PLAN_KIND,
    OPENAI_RESPONSES_SYNC_PLAN_KIND, OPENAI_SEARCH_SYNC_PLAN_KIND,
    OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND, OPENAI_VIDEO_CONTENT_PLAN_KIND,
    OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND, OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND,
    OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND,
};

pub(crate) use aether_ai_serving::AiRequestedModelFamily as RequestedModelFamily;

pub(crate) fn parse_direct_request_body(
    parts: &http::request::Parts,
    body_bytes: &Bytes,
) -> Option<(serde_json::Value, Option<String>)> {
    let is_json_request = is_json_request(&parts.headers);
    let body_bytes = if is_json_request {
        crate::ai_serving::decoded_request_body_bytes(&parts.headers, body_bytes.as_ref()).ok()?
    } else {
        std::borrow::Cow::Borrowed(body_bytes.as_ref())
    };
    parse_direct_request_body_impl(is_json_request, body_bytes.as_ref())
}

pub(crate) fn force_upstream_streaming_for_provider(
    provider_type: &str,
    provider_api_format: &str,
) -> bool {
    force_upstream_streaming_for_provider_impl(provider_type, provider_api_format)
}

pub(crate) fn resolve_upstream_is_stream_for_provider(
    endpoint_config: Option<&serde_json::Value>,
    provider_type: &str,
    provider_api_format: &str,
    client_is_stream: bool,
    hard_requires_streaming: bool,
) -> bool {
    resolve_upstream_is_stream_for_provider_impl(
        endpoint_config,
        provider_type,
        provider_api_format,
        client_is_stream,
        hard_requires_streaming,
    )
}

pub(crate) fn endpoint_config_forces_body_stream_field(
    endpoint_config: Option<&serde_json::Value>,
) -> bool {
    endpoint_config_forces_upstream_stream_policy_impl(endpoint_config)
}

pub(crate) fn request_requires_body_stream_field(
    body_json: &serde_json::Value,
    force_body_stream_field: bool,
) -> bool {
    force_body_stream_field
        || body_json
            .as_object()
            .is_some_and(|object| object.contains_key("stream"))
}

pub(crate) fn enforce_provider_body_stream_policy(
    provider_request_body: &mut serde_json::Value,
    provider_api_format: &str,
    upstream_is_stream: bool,
    require_body_stream_field: bool,
) {
    enforce_request_body_stream_field_impl(
        provider_request_body,
        provider_api_format,
        upstream_is_stream,
        require_body_stream_field,
    );
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
        endpoint_config_forces_body_stream_field, enforce_provider_body_stream_policy,
        extract_requested_model_from_request, extract_standard_requested_model,
        force_upstream_streaming_for_provider, resolve_upstream_is_stream_for_provider,
        RequestedModelFamily,
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
    fn resolves_endpoint_upstream_stream_policy_with_provider_hard_constraints() {
        assert!(resolve_upstream_is_stream_for_provider(
            Some(&json!({"upstream_stream_policy": "force_stream"})),
            "openai",
            "openai:chat",
            false,
            false,
        ));
        assert!(!resolve_upstream_is_stream_for_provider(
            Some(&json!({"upstream_stream_policy": "force_non_stream"})),
            "openai",
            "openai:chat",
            true,
            false,
        ));
        assert!(resolve_upstream_is_stream_for_provider(
            Some(&json!({"upstream_stream_policy": "auto"})),
            "openai",
            "openai:chat",
            true,
            false,
        ));
        assert!(resolve_upstream_is_stream_for_provider(
            Some(&json!({"upstream_stream_policy": "force_non_stream"})),
            "codex",
            "openai:responses",
            true,
            false,
        ));
        assert!(!resolve_upstream_is_stream_for_provider(
            Some(&json!({"upstream_stream_policy": "force_stream"})),
            "codex",
            "openai:image",
            true,
            true,
        ));
        assert!(!resolve_upstream_is_stream_for_provider(
            Some(&json!({"upstream_stream_policy": "force_stream"})),
            "codex",
            "openai:responses:compact",
            true,
            true,
        ));
        assert!(!resolve_upstream_is_stream_for_provider(
            Some(&json!({"upstream_stream_policy": "force_stream"})),
            "custom",
            "openai:responses:compact",
            true,
            true,
        ));
    }

    #[test]
    fn enforces_provider_body_stream_policy_for_body_and_streamless_formats() {
        let mut openai_chat = json!({"stream": true});
        enforce_provider_body_stream_policy(&mut openai_chat, "openai:chat", false, false);
        assert_eq!(openai_chat.get("stream"), Some(&json!(false)));

        let mut ordinary_sync = json!({"messages": []});
        enforce_provider_body_stream_policy(&mut ordinary_sync, "openai:chat", false, false);
        assert!(ordinary_sync.get("stream").is_none());

        let mut compact = json!({"stream": true});
        enforce_provider_body_stream_policy(&mut compact, "openai:responses:compact", true, true);
        assert!(compact.get("stream").is_none());
    }

    #[test]
    fn detects_endpoint_configs_that_force_body_stream_field() {
        assert!(endpoint_config_forces_body_stream_field(Some(
            &json!({"upstream_stream_policy": "force_stream"})
        )));
        assert!(endpoint_config_forces_body_stream_field(Some(
            &json!({"upstream_stream_policy": "force_non_stream"})
        )));
        assert!(!endpoint_config_forces_body_stream_field(Some(
            &json!({"upstream_stream_policy": "auto"})
        )));
        assert!(!endpoint_config_forces_body_stream_field(None));
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
