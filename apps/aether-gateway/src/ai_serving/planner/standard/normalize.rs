#[path = "normalize/chat.rs"]
mod chat;
#[path = "normalize/responses.rs"]
mod responses;
#[cfg(test)]
#[path = "normalize/tests.rs"]
mod tests;

pub(crate) use self::chat::{
    build_cross_format_openai_chat_request_body, build_cross_format_openai_chat_upstream_url,
    build_local_openai_chat_request_body, build_local_openai_chat_upstream_url,
};
pub(crate) use self::responses::{
    build_cross_format_openai_responses_request_body,
    build_cross_format_openai_responses_request_body_with_codex_model_capabilities,
    build_cross_format_openai_responses_upstream_url, build_local_openai_responses_request_body,
    build_local_openai_responses_request_body_with_codex_model_capabilities,
    build_local_openai_responses_upstream_url,
};
pub(super) use crate::ai_serving::planner::common::{
    enforce_provider_body_stream_policy, request_requires_body_stream_field,
};

pub(crate) fn validate_final_openai_provider_request(
    provider_api_format: &str,
    mapped_model: &str,
    source_request_body: &serde_json::Value,
    provider_request_body: &serde_json::Value,
) -> Option<()> {
    let provider_model = provider_request_body
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(mapped_model);
    let source_model = source_request_body
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(mapped_model);
    crate::ai_serving::validate_openai_provider_request_contract(
        provider_api_format,
        provider_model,
        source_model,
        provider_request_body,
    )
    .ok()
}
