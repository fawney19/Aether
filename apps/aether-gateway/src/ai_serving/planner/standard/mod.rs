//! Standard contract planning surface.
//!
//! This groups the standard planning surface in one place:
//! request-side conversion, matrix registry, and decision payload builders.

use crate::ai_serving::GatewayControlDecision;
use crate::{AiExecutionDecision, AppState, GatewayError};

mod claude;
mod codex;
mod family;
mod gemini;
mod normalize;
mod openai;

pub(crate) use self::codex::apply_codex_openai_responses_special_headers;
pub(crate) use self::family::{
    build_local_stream_plan_and_reports, build_local_sync_plan_and_reports,
};
pub(crate) use self::normalize::{
    build_cross_format_openai_chat_request_body, build_cross_format_openai_chat_upstream_url,
    build_cross_format_openai_responses_request_body,
    build_cross_format_openai_responses_upstream_url, build_local_openai_chat_request_body,
    build_local_openai_chat_upstream_url, build_local_openai_responses_request_body,
    build_local_openai_responses_upstream_url,
};
pub(crate) use self::openai::{
    build_local_openai_chat_stream_plan_and_reports_for_kind,
    build_local_openai_chat_sync_plan_and_reports_for_kind,
    build_local_openai_responses_stream_plan_and_reports_for_kind,
    build_local_openai_responses_sync_plan_and_reports_for_kind, copy_request_number_field,
    copy_request_number_field_as, map_openai_reasoning_effort_to_claude_output,
    map_openai_reasoning_effort_to_gemini_budget, maybe_build_stream_local_decision_payload,
    maybe_build_stream_local_openai_responses_decision_payload,
    maybe_build_sync_local_decision_payload,
    maybe_build_sync_local_openai_responses_decision_payload, parse_openai_stop_sequences,
    resolve_openai_chat_max_tokens, set_local_openai_chat_execution_exhausted_diagnostic,
    value_as_u64,
};
pub(crate) use crate::ai_serving::normalize_standard_request_to_openai_chat_request;
pub(crate) use crate::ai_serving::{
    build_core_error_body_for_client_format, request_conversion_kind,
    request_conversion_transport_supported, sync_chat_response_conversion_kind,
    sync_cli_response_conversion_kind, RequestConversionKind, SyncChatResponseConversionKind,
    SyncCliResponseConversionKind,
};
pub(crate) use crate::ai_serving::{
    build_standard_request_body, convert_openai_chat_request_to_claude_request,
    convert_openai_chat_request_to_gemini_request,
    convert_openai_chat_request_to_openai_responses_request, extract_openai_text_content,
    normalize_openai_responses_request_to_openai_chat_request, parse_openai_tool_result_content,
};
pub(crate) use aether_ai_serving::{
    request_body_build_failure_extra_data, same_format_provider_request_body_failure_extra_data,
};

pub(crate) fn build_standard_upstream_url(
    parts: &http::request::Parts,
    transport: &crate::ai_serving::GatewayProviderTransportSnapshot,
    mapped_model: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
) -> Option<String> {
    crate::ai_serving::build_provider_transport_request_url(
        transport,
        provider_api_format,
        Some(mapped_model),
        upstream_is_stream,
        parts.uri.query(),
        None,
    )
}

pub(crate) async fn maybe_build_sync_local_standard_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    if let Some(payload) = self::claude::maybe_build_sync_local_claude_decision_payload(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    self::gemini::maybe_build_sync_local_gemini_decision_payload(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await
}

pub(crate) async fn maybe_build_stream_local_standard_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    if let Some(payload) = self::claude::maybe_build_stream_local_claude_decision_payload(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    self::gemini::maybe_build_stream_local_gemini_decision_payload(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::build_standard_request_body;
    use serde_json::json;

    #[test]
    fn builds_openai_chat_request_from_claude_chat_source() {
        let request = json!({
            "model": "claude-3-7-sonnet",
            "system": "You are concise.",
            "messages": [
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "Hello from Claude"}]
                }
            ],
            "max_tokens": 128
        });

        let converted = build_standard_request_body(
            &request,
            "claude:messages",
            "gpt-5",
            "openai",
            "openai:chat",
            "/v1/messages",
            false,
            None,
            None,
        )
        .expect("claude chat should convert to openai chat");

        assert_eq!(converted["model"], "gpt-5");
        assert_eq!(converted["messages"][0]["role"], "system");
        assert_eq!(converted["messages"][0]["content"], "You are concise.");
        assert_eq!(converted["messages"][1]["role"], "user");
        assert_eq!(converted["messages"][1]["content"], "Hello from Claude");
    }

    #[test]
    fn builds_claude_chat_request_from_gemini_chat_source() {
        let request = json!({
            "systemInstruction": {
                "parts": [{"text": "Be brief."}]
            },
            "contents": [
                {
                    "role": "user",
                    "parts": [{"text": "Hello from Gemini"}]
                }
            ]
        });

        let converted = build_standard_request_body(
            &request,
            "gemini:generate_content",
            "claude-sonnet-4-5",
            "anthropic",
            "claude:messages",
            "/v1beta/models/gemini-2.5-pro:generateContent",
            false,
            None,
            None,
        )
        .expect("gemini chat should convert to claude chat");

        assert_eq!(converted["model"], "claude-sonnet-4-5");
        assert_eq!(converted["messages"][0]["role"], "user");
        assert!(
            converted["messages"]
                .to_string()
                .contains("Hello from Gemini"),
            "converted claude payload should retain the gemini user text: {converted}"
        );
    }

    #[test]
    fn builds_gemini_cli_request_from_claude_cli_source() {
        let request = json!({
            "model": "claude-sonnet-4-5",
            "messages": [
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "Need CLI output"}]
                }
            ],
            "max_tokens": 64
        });

        let converted = build_standard_request_body(
            &request,
            "claude:messages",
            "gemini-2.5-pro",
            "google",
            "gemini:generate_content",
            "/v1/messages",
            false,
            None,
            None,
        )
        .expect("claude cli should convert to gemini cli");

        assert_eq!(converted["contents"][0]["role"], "user");
        assert_eq!(
            converted["contents"][0]["parts"][0]["text"],
            "Need CLI output"
        );
    }

    #[test]
    fn builds_openai_responses_request_from_claude_cli_source_with_forced_stream() {
        let request = json!({
            "model": "claude-sonnet-4-5",
            "messages": [
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "Need OpenAI Responses output"}]
                }
            ],
            "max_tokens": 64
        });

        let converted = build_standard_request_body(
            &request,
            "claude:messages",
            "gpt-5",
            "openai",
            "openai:responses",
            "/v1/messages",
            true,
            None,
            None,
        )
        .expect("claude cli should convert to openai responses");

        assert_eq!(converted["model"], "gpt-5");
        assert_eq!(converted["input"][0]["role"], "user");
        assert_eq!(converted["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(
            converted["input"][0]["content"][0]["text"],
            "Need OpenAI Responses output"
        );
        assert_eq!(converted["stream"], true);
    }

    #[test]
    fn strips_metadata_for_codex_openai_responses_requests() {
        let request = json!({
            "model": "claude-sonnet-4-5",
            "metadata": {"trace_id": "abc"},
            "messages": [{
                "role": "user",
                "content": [{"type": "text", "text": "Need OpenAI Responses output"}]
            }],
            "max_tokens": 64
        });

        let converted = build_standard_request_body(
            &request,
            "claude:messages",
            "gpt-5",
            "codex",
            "openai:responses",
            "/v1/messages",
            false,
            None,
            None,
        )
        .expect("claude cli should convert to codex request");

        assert!(converted.get("metadata").is_none());
        assert_eq!(converted["store"], false);
        assert_eq!(converted["instructions"], "You are ChatGPT.");
    }

    #[test]
    fn strips_store_for_openai_responses_compact_requests() {
        let request = json!({
            "model": "gpt-5",
            "messages": [{
                "role": "user",
                "content": "Hello from OpenAI Chat"
            }],
            "store": true
        });

        let converted = build_standard_request_body(
            &request,
            "openai:chat",
            "gpt-5",
            "openai",
            "openai:responses:compact",
            "/v1/chat/completions",
            false,
            None,
            None,
        )
        .expect("openai chat should convert to openai responses compact");

        assert_eq!(converted["model"], "gpt-5");
        assert!(converted.get("store").is_none());
    }
}
