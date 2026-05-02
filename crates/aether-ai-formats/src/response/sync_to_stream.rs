use aether_ai_formats::protocol::conversion::response::{
    convert_claude_response_to_openai_responses, convert_gemini_response_to_openai_responses,
    convert_openai_chat_response_to_openai_responses,
};
use aether_contracts::{ExecutionStreamTerminalSummary, StandardizedUsage};
use serde_json::{json, Value};

use crate::response::sse::encode_json_sse;
use crate::response::standard::claude::stream::ClaudeClientEmitter;
use crate::response::standard::gemini::stream::GeminiClientEmitter;
use crate::response::standard::openai::stream::{
    OpenAIChatClientEmitter, OpenAIResponsesClientEmitter, OpenAIResponsesProviderState,
};
use crate::response::standard::stream_core::CanonicalStreamFrame;
use crate::response::AiSurfaceFinalizeError;

pub struct SyncToStreamBridgeOutcome {
    pub sse_body: Vec<u8>,
    pub terminal_summary: Option<ExecutionStreamTerminalSummary>,
}

pub fn maybe_bridge_standard_sync_json_to_stream(
    provider_body_json: &Value,
    provider_api_format: &str,
    client_api_format: &str,
    report_context: Option<&Value>,
) -> Result<Option<SyncToStreamBridgeOutcome>, AiSurfaceFinalizeError> {
    let provider_api_format = normalize_api_format(provider_api_format);
    let client_api_format = normalize_api_format(client_api_format);
    if provider_api_format == "openai:image" && client_api_format == "openai:image" {
        return maybe_bridge_openai_image_sync_json_to_stream(provider_body_json, report_context);
    }
    if !is_standard_api_format(provider_api_format.as_str())
        || !is_standard_api_format(client_api_format.as_str())
    {
        return Ok(None);
    }

    let bridge_context = build_bridge_report_context(
        report_context,
        provider_api_format.as_str(),
        client_api_format.as_str(),
    );
    let Some(openai_responses_response) = convert_provider_sync_response_to_openai_responses(
        provider_body_json,
        provider_api_format.as_str(),
        &bridge_context,
    ) else {
        return Ok(None);
    };
    let terminal_summary =
        build_terminal_summary_from_openai_responses_response(&openai_responses_response);
    let canonical_frames = build_canonical_frames_from_openai_responses_response(
        &openai_responses_response,
        &bridge_context,
    )?;
    let sse_body =
        emit_client_stream_from_canonical_frames(canonical_frames, client_api_format.as_str())?;

    Ok(Some(SyncToStreamBridgeOutcome {
        sse_body,
        terminal_summary,
    }))
}

fn maybe_bridge_openai_image_sync_json_to_stream(
    provider_body_json: &Value,
    report_context: Option<&Value>,
) -> Result<Option<SyncToStreamBridgeOutcome>, AiSurfaceFinalizeError> {
    let Some(response) = provider_body_json.as_object() else {
        return Ok(None);
    };
    let Some(image) = response
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .find_map(extract_openai_image_sync_b64_json)
    else {
        return Ok(None);
    };
    let usage = response.get("usage").cloned().unwrap_or(Value::Null);
    let event_name = openai_image_completed_event_name(report_context);
    let sse_body = encode_json_sse(
        Some(event_name),
        &json!({
            "type": event_name,
            "b64_json": image,
            "usage": usage,
        }),
    )?;

    Ok(Some(SyncToStreamBridgeOutcome {
        sse_body,
        terminal_summary: Some(ExecutionStreamTerminalSummary {
            standardized_usage: response
                .get("usage")
                .and_then(standardized_usage_from_openai_usage),
            finish_reason: Some("stop".to_string()),
            response_id: response
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            model: response
                .get("model")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| image_bridge_model(report_context)),
            observed_finish: true,
            unknown_event_count: 0,
            parser_error: None,
        }),
    }))
}

fn normalize_api_format(value: &str) -> String {
    aether_ai_formats::normalize_api_format_alias(value)
}

fn is_standard_api_format(value: &str) -> bool {
    matches!(
        value,
        "openai:chat"
            | "openai:responses"
            | "openai:responses:compact"
            | "claude:messages"
            | "gemini:generate_content"
    )
}

fn extract_openai_image_sync_b64_json(item: &serde_json::Map<String, Value>) -> Option<String> {
    item.get("b64_json")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            item.get("url")
                .and_then(Value::as_str)
                .and_then(extract_base64_from_data_url)
        })
}

fn extract_base64_from_data_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let (metadata, payload) = trimmed.split_once(',')?;
    if !metadata.starts_with("data:") || !metadata.ends_with(";base64") {
        return None;
    }
    (!payload.trim().is_empty()).then(|| payload.trim().to_string())
}

fn openai_image_completed_event_name(report_context: Option<&Value>) -> &'static str {
    if openai_image_request_operation(report_context) == Some("edit") {
        "image_edit.completed"
    } else {
        "image_generation.completed"
    }
}

fn openai_image_request_operation(report_context: Option<&Value>) -> Option<&str> {
    report_context
        .and_then(|value| value.get("image_request"))
        .and_then(|value| value.get("operation"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn image_bridge_model(report_context: Option<&Value>) -> Option<String> {
    report_context.and_then(|context| {
        context
            .get("mapped_model")
            .or_else(|| context.get("model"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn build_bridge_report_context(
    report_context: Option<&Value>,
    provider_api_format: &str,
    client_api_format: &str,
) -> Value {
    let mut context = report_context
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let object = context
        .as_object_mut()
        .expect("bridge report context should stay object");
    object
        .entry("provider_api_format".to_string())
        .or_insert_with(|| Value::String(provider_api_format.to_string()));
    object
        .entry("client_api_format".to_string())
        .or_insert_with(|| Value::String(client_api_format.to_string()));
    context
}

fn convert_provider_sync_response_to_openai_responses(
    provider_body_json: &Value,
    provider_api_format: &str,
    report_context: &Value,
) -> Option<Value> {
    match provider_api_format {
        "openai:responses" | "openai:responses:compact" => Some(provider_body_json.clone()),
        "openai:chat" => convert_openai_chat_response_to_openai_responses(
            provider_body_json,
            report_context,
            false,
        ),
        "claude:messages" => {
            convert_claude_response_to_openai_responses(provider_body_json, report_context)
        }
        "gemini:generate_content" => {
            convert_gemini_response_to_openai_responses(provider_body_json, report_context)
        }
        _ => None,
    }
}

fn build_canonical_frames_from_openai_responses_response(
    openai_responses_response: &Value,
    report_context: &Value,
) -> Result<Vec<CanonicalStreamFrame>, AiSurfaceFinalizeError> {
    let mut state = OpenAIResponsesProviderState::default();
    let line = format!(
        "data: {}\n",
        serde_json::to_string(&json!({
            "type": "response.completed",
            "response": openai_responses_response,
        }))
        .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?
    );
    let mut frames = state
        .push_line(report_context, line.into_bytes())
        .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?;
    frames.extend(
        state
            .finish(report_context)
            .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?,
    );
    Ok(frames)
}

fn emit_client_stream_from_canonical_frames(
    canonical_frames: Vec<CanonicalStreamFrame>,
    client_api_format: &str,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    match client_api_format {
        "openai:chat" => {
            let mut emitter = OpenAIChatClientEmitter::default();
            emit_with_openai_chat_emitter(&mut emitter, canonical_frames)
        }
        "openai:responses" | "openai:responses:compact" => {
            let mut emitter = OpenAIResponsesClientEmitter::default();
            emit_with_openai_responses_emitter(&mut emitter, canonical_frames)
        }
        "claude:messages" => {
            let mut emitter = ClaudeClientEmitter::default();
            emit_with_claude_emitter(&mut emitter, canonical_frames)
        }
        "gemini:generate_content" => {
            let mut emitter = GeminiClientEmitter::default();
            emit_with_gemini_emitter(&mut emitter, canonical_frames)
        }
        _ => Ok(Vec::new()),
    }
}

fn emit_with_openai_chat_emitter(
    emitter: &mut OpenAIChatClientEmitter,
    canonical_frames: Vec<CanonicalStreamFrame>,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    let mut output = Vec::new();
    for frame in canonical_frames {
        output.extend(
            emitter
                .emit(frame)
                .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?,
        );
    }
    output.extend(
        emitter
            .finish()
            .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?,
    );
    Ok(output)
}

fn emit_with_openai_responses_emitter(
    emitter: &mut OpenAIResponsesClientEmitter,
    canonical_frames: Vec<CanonicalStreamFrame>,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    let mut output = Vec::new();
    for frame in canonical_frames {
        output.extend(
            emitter
                .emit(frame)
                .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?,
        );
    }
    output.extend(
        emitter
            .finish()
            .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?,
    );
    Ok(output)
}

fn emit_with_claude_emitter(
    emitter: &mut ClaudeClientEmitter,
    canonical_frames: Vec<CanonicalStreamFrame>,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    let mut output = Vec::new();
    for frame in canonical_frames {
        output.extend(
            emitter
                .emit(frame)
                .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?,
        );
    }
    output.extend(
        emitter
            .finish()
            .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?,
    );
    Ok(output)
}

fn emit_with_gemini_emitter(
    emitter: &mut GeminiClientEmitter,
    canonical_frames: Vec<CanonicalStreamFrame>,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    let mut output = Vec::new();
    for frame in canonical_frames {
        output.extend(
            emitter
                .emit(frame)
                .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?,
        );
    }
    output.extend(
        emitter
            .finish()
            .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?,
    );
    Ok(output)
}

fn build_terminal_summary_from_openai_responses_response(
    openai_responses_response: &Value,
) -> Option<ExecutionStreamTerminalSummary> {
    let response = openai_responses_response.as_object()?;
    let response_id = response
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let model = response
        .get("model")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let finish_reason = response
        .get("output")
        .and_then(Value::as_array)
        .map(|output| resolve_openai_responses_finish_reason(output))
        .filter(|value| !value.trim().is_empty());
    let standardized_usage = response
        .get("usage")
        .and_then(standardized_usage_from_openai_usage);
    Some(ExecutionStreamTerminalSummary {
        standardized_usage,
        finish_reason,
        response_id,
        model,
        observed_finish: true,
        unknown_event_count: 0,
        parser_error: None,
    })
}

fn resolve_openai_responses_finish_reason(output: &[Value]) -> String {
    let has_tool_calls = output.iter().filter_map(Value::as_object).any(|item| {
        item.get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "function_call")
    });
    if has_tool_calls {
        "tool_calls".to_string()
    } else {
        "stop".to_string()
    }
}

fn standardized_usage_from_openai_usage(value: &Value) -> Option<StandardizedUsage> {
    let usage = value.as_object()?;
    let mut input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cache_creation_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_i64)
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .or_else(|| usage.get("prompt_tokens_details"))
                .and_then(Value::as_object)
                .and_then(|details| details.get("cached_creation_tokens"))
                .and_then(Value::as_i64)
        })
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_i64)
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .or_else(|| usage.get("prompt_tokens_details"))
                .and_then(Value::as_object)
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_i64)
        })
        .unwrap_or(0);
    let total_tokens = usage.get("total_tokens").and_then(Value::as_i64).unwrap_or(
        input_tokens
            .saturating_add(output_tokens)
            .saturating_add(cache_creation_tokens)
            .saturating_add(cache_read_tokens),
    );
    if input_tokens == 0 && total_tokens > output_tokens {
        input_tokens = total_tokens.saturating_sub(output_tokens);
    }
    let mut standardized_usage = StandardizedUsage::new();
    standardized_usage.input_tokens = input_tokens;
    standardized_usage.output_tokens = output_tokens;
    standardized_usage.cache_creation_tokens = cache_creation_tokens;
    standardized_usage.cache_read_tokens = cache_read_tokens;
    standardized_usage
        .dimensions
        .insert("total_tokens".to_string(), json!(total_tokens));
    Some(standardized_usage.normalize_cache_creation_breakdown())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{maybe_bridge_standard_sync_json_to_stream, standardized_usage_from_openai_usage};

    fn utf8(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).expect("utf8 should decode")
    }

    #[test]
    fn openai_sync_usage_derives_missing_input_tokens_from_total() {
        let usage = standardized_usage_from_openai_usage(&json!({
            "output_tokens": 177,
            "total_tokens": 20_612,
            "input_tokens_details": {
                "cached_tokens": 19_840,
            },
        }))
        .expect("usage should parse");

        assert_eq!(usage.input_tokens, 20_435);
        assert_eq!(usage.output_tokens, 177);
        assert_eq!(usage.cache_read_tokens, 19_840);
    }

    #[test]
    fn bridges_openai_image_sync_json_to_generation_completed_sse() {
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:image",
            "mapped_model": "gpt-image-1",
            "image_request": {
                "operation": "generate"
            }
        });
        let outcome = maybe_bridge_standard_sync_json_to_stream(
            &json!({
                "created": 1776971267,
                "data": [{
                    "b64_json": "aGVsbG8="
                }],
                "usage": {
                    "total_tokens": 100,
                    "input_tokens": 50,
                    "output_tokens": 50,
                    "input_tokens_details": {
                        "text_tokens": 10,
                        "image_tokens": 40
                    }
                }
            }),
            "openai:image",
            "openai:image",
            Some(&report_context),
        )
        .expect("bridge should succeed")
        .expect("bridge should produce sse");

        let output = utf8(outcome.sse_body);
        assert!(output.contains("event: image_generation.completed"));
        assert!(output.contains("\"type\":\"image_generation.completed\""));
        assert!(output.contains("\"b64_json\":\"aGVsbG8=\""));
        assert!(output.contains("\"total_tokens\":100"));

        let summary = outcome
            .terminal_summary
            .expect("terminal summary should exist");
        assert_eq!(summary.model.as_deref(), Some("gpt-image-1"));
        assert_eq!(summary.finish_reason.as_deref(), Some("stop"));
        assert_eq!(
            summary
                .standardized_usage
                .as_ref()
                .and_then(|usage| usage.dimensions.get("total_tokens"))
                .cloned(),
            Some(json!(100))
        );
    }

    #[test]
    fn bridges_openai_image_sync_data_url_to_edit_completed_sse() {
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:image",
            "image_request": {
                "operation": "edit"
            }
        });
        let outcome = maybe_bridge_standard_sync_json_to_stream(
            &json!({
                "created": 1776971267,
                "data": [{
                    "url": "data:image/webp;base64,d29ybGQ="
                }],
                "usage": {
                    "total_tokens": 9,
                    "input_tokens": 4,
                    "output_tokens": 5
                }
            }),
            "openai:image",
            "openai:image",
            Some(&report_context),
        )
        .expect("bridge should succeed")
        .expect("bridge should produce sse");

        let output = utf8(outcome.sse_body);
        assert!(output.contains("event: image_edit.completed"));
        assert!(output.contains("\"type\":\"image_edit.completed\""));
        assert!(output.contains("\"b64_json\":\"d29ybGQ=\""));
        assert!(output.contains("\"total_tokens\":9"));
    }
}
