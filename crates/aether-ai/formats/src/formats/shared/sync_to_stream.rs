use std::borrow::Cow;

use aether_ai_formats::formats::conversion::response::{
    convert_claude_response_to_openai_responses, convert_gemini_response_to_openai_responses,
    convert_openai_chat_response_to_openai_responses,
};
use aether_contracts::{ExecutionStreamTerminalSummary, StandardizedUsage};
use serde_json::{json, Map, Value};

use crate::formats::claude::messages::stream::ClaudeClientEmitter;
use crate::formats::gemini::generate_content::stream::GeminiClientEmitter;
use crate::formats::openai::chat::stream::{
    OpenAIChatClientEmitter, OpenAIResponsesClientEmitter, OpenAIResponsesProviderState,
};
use crate::formats::shared::sse::{encode_done_sse, encode_json_sse};
use crate::formats::shared::stream_core::common::{
    build_openai_chat_chunk, build_openai_chat_finish_chunk,
    build_openai_chat_usage_chunk_with_cache,
};
use crate::formats::shared::stream_core::{
    CanonicalStreamEvent, CanonicalStreamFrame, StreamingStandardFormatMatrix,
    StreamingStandardTerminalObserver,
};
use crate::formats::shared::stream_rewrite::maybe_build_ai_surface_stream_rewriter;
use crate::formats::shared::AiSurfaceFinalizeError;

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
    if let Some(outcome) = maybe_bridge_aether_sse_response_capture_to_stream(
        provider_body_json,
        provider_api_format.as_str(),
        client_api_format.as_str(),
        report_context,
    )? {
        return Ok(Some(outcome));
    }
    if provider_api_format == "openai:image" {
        return match client_api_format.as_str() {
            "openai:image" => {
                maybe_bridge_openai_image_sync_json_to_stream(provider_body_json, report_context)
            }
            "openai:chat" => maybe_bridge_openai_image_sync_json_to_chat_stream(
                provider_body_json,
                report_context,
            ),
            "openai:responses" | "openai:responses:compact" => {
                maybe_bridge_openai_image_sync_json_to_responses_stream(
                    provider_body_json,
                    report_context,
                )
            }
            _ => Ok(None),
        };
    }
    if client_api_format == "openai:image" && provider_api_format == "gemini:generate_content" {
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
    if is_openai_responses_family_api_format(provider_api_format.as_str())
        && is_openai_responses_family_api_format(client_api_format.as_str())
    {
        return bridge_openai_responses_same_family_sync_json_to_stream(
            provider_body_json,
            provider_api_format.as_str(),
            &bridge_context,
        );
    }

    let Some(openai_responses_response) = convert_provider_sync_response_to_openai_responses(
        provider_body_json,
        provider_api_format.as_str(),
        &bridge_context,
    ) else {
        return Ok(None);
    };
    let provider_actual_service_tier = provider_actual_service_tier_from_sync_response(
        provider_body_json,
        provider_api_format.as_str(),
    );
    let terminal_summary = build_terminal_summary_from_openai_responses_response(
        &openai_responses_response,
        provider_actual_service_tier.clone(),
    );
    let canonical_frames = build_canonical_frames_from_openai_responses_response(
        &openai_responses_response,
        &bridge_context,
    )?;
    let sse_body = if is_openai_responses_family_api_format(client_api_format.as_str()) {
        emit_openai_responses_stream_with_authoritative_terminal(
            canonical_frames,
            &openai_responses_response,
            openai_responses_terminal_event_type(&openai_responses_response)
                .unwrap_or("response.completed"),
            provider_actual_service_tier.as_deref(),
        )?
    } else {
        emit_client_stream_from_canonical_frames(
            canonical_frames,
            client_api_format.as_str(),
            provider_actual_service_tier.as_deref(),
        )?
    };

    Ok(Some(SyncToStreamBridgeOutcome {
        sse_body,
        terminal_summary,
    }))
}

fn bridge_openai_responses_same_family_sync_json_to_stream(
    response: &Value,
    provider_api_format: &str,
    report_context: &Value,
) -> Result<Option<SyncToStreamBridgeOutcome>, AiSurfaceFinalizeError> {
    let bridge_response = if openai_responses_terminal_event_type(response).is_some() {
        Cow::Borrowed(response)
    } else if provider_api_format == "openai:responses:compact"
        && response.get("output").is_some_and(Value::is_array)
    {
        let mut response = response.clone();
        let object = response
            .as_object_mut()
            .expect("Compact response with output should be an object");
        object.insert("status".to_string(), Value::String("completed".to_string()));
        object
            .entry("object".to_string())
            .or_insert_with(|| Value::String("response.compaction".to_string()));
        Cow::Owned(response)
    } else {
        return Ok(None);
    };
    let canonical_frames = build_canonical_frames_from_openai_responses_response(
        bridge_response.as_ref(),
        report_context,
    )?;
    let terminal_event_type = openai_responses_terminal_event_type(bridge_response.as_ref())
        .expect("bridge response should have a terminal status");
    let sse_body = emit_openai_responses_stream_with_authoritative_terminal(
        canonical_frames,
        response,
        terminal_event_type,
        provider_actual_service_tier_from_sync_response(response, provider_api_format).as_deref(),
    )?;

    Ok(Some(SyncToStreamBridgeOutcome {
        sse_body,
        terminal_summary: build_terminal_summary_from_openai_responses_response(
            response,
            provider_actual_service_tier_from_sync_response(response, provider_api_format),
        ),
    }))
}

fn maybe_bridge_openai_image_sync_json_to_stream(
    provider_body_json: &Value,
    report_context: Option<&Value>,
) -> Result<Option<SyncToStreamBridgeOutcome>, AiSurfaceFinalizeError> {
    let Some(provider_body_json) =
        normalize_openai_image_sync_response(provider_body_json, report_context)?
    else {
        return Ok(None);
    };
    let Some(response) = provider_body_json.as_ref().as_object() else {
        return Ok(None);
    };
    let outputs = collect_openai_image_outputs(response, report_context);
    let Some(image) = outputs.iter().find_map(OpenAiImageOutput::b64_json) else {
        return Ok(None);
    };
    let image_count = openai_image_response_image_count(response).max(outputs.len() as u64);
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
        terminal_summary: Some(openai_image_terminal_summary(
            response,
            report_context,
            image_count,
        )),
    }))
}

fn maybe_bridge_openai_image_sync_json_to_chat_stream(
    provider_body_json: &Value,
    report_context: Option<&Value>,
) -> Result<Option<SyncToStreamBridgeOutcome>, AiSurfaceFinalizeError> {
    let Some(provider_body_json) =
        normalize_openai_image_sync_response(provider_body_json, report_context)?
    else {
        return Ok(None);
    };
    let Some(response) = provider_body_json.as_ref().as_object() else {
        return Ok(None);
    };
    let outputs = collect_openai_image_outputs(response, report_context);
    if outputs.is_empty() {
        return Ok(None);
    }

    let image_count = openai_image_response_image_count(response).max(outputs.len() as u64);
    let summary = openai_image_terminal_summary(response, report_context, image_count);
    let response_id = openai_image_bridge_response_id(response, report_context, "chatcmpl-image");
    let model = openai_image_bridge_response_model(response, report_context);
    let content = outputs
        .iter()
        .enumerate()
        .map(|(index, output)| output.markdown(index))
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut sse_body = Vec::new();
    sse_body.extend(encode_json_sse(
        None,
        &build_openai_chat_chunk(&response_id, &model, content, None, None),
    )?);
    sse_body.extend(encode_json_sse(
        None,
        &build_openai_chat_finish_chunk(&response_id, &model, Some("stop")),
    )?);
    if let Some((
        input_tokens,
        output_tokens,
        total_tokens,
        reasoning_tokens,
        cache_creation_tokens,
        cache_read_tokens,
    )) = summary
        .standardized_usage
        .as_ref()
        .and_then(openai_chat_usage_counts)
    {
        sse_body.extend(encode_json_sse(
            None,
            &build_openai_chat_usage_chunk_with_cache(
                &response_id,
                &model,
                input_tokens,
                output_tokens,
                total_tokens,
                reasoning_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            ),
        )?);
    }
    sse_body.extend(encode_done_sse());

    Ok(Some(SyncToStreamBridgeOutcome {
        sse_body,
        terminal_summary: Some(summary),
    }))
}

fn maybe_bridge_openai_image_sync_json_to_responses_stream(
    provider_body_json: &Value,
    report_context: Option<&Value>,
) -> Result<Option<SyncToStreamBridgeOutcome>, AiSurfaceFinalizeError> {
    let Some(provider_body_json) =
        normalize_openai_image_sync_response(provider_body_json, report_context)?
    else {
        return Ok(None);
    };
    let Some(response) = provider_body_json.as_ref().as_object() else {
        return Ok(None);
    };
    let outputs = collect_openai_image_outputs(response, report_context);
    if outputs.is_empty() {
        return Ok(None);
    }

    let response_id = openai_image_bridge_response_id(response, report_context, "resp-image");
    let model = openai_image_bridge_response_model(response, report_context);
    let mut response_output = Vec::new();
    for (index, output) in outputs.iter().enumerate() {
        response_output.push(output.responses_image_generation_item(&response_id, index));
    }

    let mut response_object = Map::new();
    response_object.insert("id".to_string(), Value::String(response_id.clone()));
    response_object.insert("object".to_string(), Value::String("response".to_string()));
    response_object.insert("model".to_string(), Value::String(model));
    response_object.insert("status".to_string(), Value::String("completed".to_string()));
    response_object.insert("output".to_string(), Value::Array(response_output.clone()));
    if let Some(created) = response.get("created").and_then(Value::as_i64) {
        response_object.insert("created_at".to_string(), json!(created));
    }
    if let Some(usage) = response.get("usage").filter(|value| value.is_object()) {
        response_object.insert("usage".to_string(), usage.clone());
    }

    let mut sse_body = Vec::new();
    for (index, item) in response_output.iter().enumerate() {
        sse_body.extend(encode_json_sse(
            Some("response.output_item.done"),
            &json!({
                "type": "response.output_item.done",
                "output_index": index,
                "item": item,
            }),
        )?);
    }
    sse_body.extend(encode_json_sse(
        Some("response.completed"),
        &json!({
            "type": "response.completed",
            "response": Value::Object(response_object),
        }),
    )?);

    let image_count = openai_image_response_image_count(response).max(outputs.len() as u64);
    Ok(Some(SyncToStreamBridgeOutcome {
        sse_body,
        terminal_summary: Some(openai_image_terminal_summary(
            response,
            report_context,
            image_count,
        )),
    }))
}

#[derive(Clone, Debug)]
struct OpenAiImageOutput {
    b64_json: Option<String>,
    url: Option<String>,
    mime_type: String,
    output_format: Option<String>,
    revised_prompt: Option<String>,
}

impl OpenAiImageOutput {
    fn b64_json(&self) -> Option<String> {
        self.b64_json
            .clone()
            .or_else(|| self.url.as_deref().and_then(extract_base64_from_data_url))
    }

    fn source_url(&self) -> Option<String> {
        self.url.clone().or_else(|| {
            self.b64_json
                .as_ref()
                .map(|value| format!("data:{};base64,{value}", self.mime_type))
        })
    }

    fn markdown(&self, index: usize) -> String {
        let alt = if index == 0 {
            "generated image".to_string()
        } else {
            format!("generated image {}", index + 1)
        };
        match self.source_url() {
            Some(url) => format!("![{alt}]({url})"),
            None => String::new(),
        }
    }

    fn responses_image_generation_item(&self, response_id: &str, index: usize) -> Value {
        let mut item = Map::new();
        item.insert(
            "id".to_string(),
            Value::String(format!("{response_id}_img_{index}")),
        );
        item.insert(
            "type".to_string(),
            Value::String("image_generation_call".to_string()),
        );
        item.insert("status".to_string(), Value::String("completed".to_string()));
        if let Some(result) = self.b64_json().or_else(|| self.url.clone()) {
            item.insert("result".to_string(), Value::String(result));
        }
        if let Some(output_format) = self.output_format.as_ref() {
            item.insert(
                "output_format".to_string(),
                Value::String(output_format.clone()),
            );
        }
        if let Some(revised_prompt) = self.revised_prompt.as_ref() {
            item.insert(
                "revised_prompt".to_string(),
                Value::String(revised_prompt.clone()),
            );
        }
        Value::Object(item)
    }
}

fn normalize_openai_image_sync_response<'a>(
    provider_body_json: &'a Value,
    report_context: Option<&Value>,
) -> Result<Option<Cow<'a, Value>>, AiSurfaceFinalizeError> {
    let provider_api_format = report_context
        .and_then(|value| value.get("provider_api_format"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("openai:image");
    if provider_api_format == "gemini:generate_content" {
        let Some(converted) =
            crate::formats::shared::image_bridge::build_openai_image_response_from_gemini_response(
                provider_body_json,
                report_context,
            )
        else {
            return Ok(None);
        };
        return Ok(Some(Cow::Owned(converted)));
    }
    if provider_body_json.get("output").is_some() && provider_body_json.get("data").is_none() {
        let Some(converted) = crate::formats::shared::image_bridge::build_openai_image_response_from_response_stream_sync_body(
            provider_body_json,
            report_context,
        ) else {
            return Ok(None);
        };
        return Ok(Some(Cow::Owned(converted)));
    }
    Ok(Some(Cow::Borrowed(provider_body_json)))
}

fn collect_openai_image_outputs(
    response: &Map<String, Value>,
    report_context: Option<&Value>,
) -> Vec<OpenAiImageOutput> {
    response
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|item| openai_image_output_from_item(item, report_context))
        .collect()
}

fn openai_image_output_from_item(
    item: &Map<String, Value>,
    report_context: Option<&Value>,
) -> Option<OpenAiImageOutput> {
    let b64_json = extract_openai_image_sync_b64_json(item);
    let url = item
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if b64_json.is_none() && url.is_none() {
        return None;
    }
    let output_format = item
        .get("output_format")
        .or_else(|| item.get("format"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| image_request_output_format(report_context));
    let mime_type = url
        .as_deref()
        .and_then(extract_mime_type_from_data_url)
        .or_else(|| {
            output_format
                .as_deref()
                .map(mime_type_from_image_output_format)
        })
        .unwrap_or_else(|| "image/png".to_string());
    let revised_prompt = item
        .get("revised_prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    Some(OpenAiImageOutput {
        b64_json,
        url,
        mime_type,
        output_format,
        revised_prompt,
    })
}

fn openai_image_response_image_count(response: &Map<String, Value>) -> u64 {
    response
        .get("data")
        .and_then(Value::as_array)
        .map(|items| items.len() as u64)
        .unwrap_or(0)
}

fn openai_image_terminal_summary(
    response: &Map<String, Value>,
    report_context: Option<&Value>,
    image_count: u64,
) -> ExecutionStreamTerminalSummary {
    ExecutionStreamTerminalSummary {
        standardized_usage: openai_image_standardized_usage(
            response.get("usage"),
            report_context,
            image_count,
        ),
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
        provider_actual_service_tier: None,
        observed_finish: true,
        unknown_event_count: 0,
        parser_error: None,
    }
}

fn openai_image_standardized_usage(
    usage: Option<&Value>,
    report_context: Option<&Value>,
    image_count: u64,
) -> Option<StandardizedUsage> {
    let mut standardized_usage = usage
        .and_then(standardized_usage_from_openai_usage)
        .unwrap_or_default();
    if image_count > 0 {
        standardized_usage.request_count = i64::try_from(image_count).unwrap_or(i64::MAX);
        standardized_usage
            .dimensions
            .insert("image_count".to_string(), json!(image_count));
    }
    if let Some(output_format) = image_request_output_format(report_context) {
        standardized_usage
            .dimensions
            .insert("image_output_format".to_string(), json!(output_format));
    }
    if let Some(size) = image_request_size(report_context) {
        standardized_usage
            .dimensions
            .insert("image_size".to_string(), json!(size));
    }
    if let Some(quality) = image_request_quality(report_context) {
        standardized_usage
            .dimensions
            .insert("image_quality".to_string(), json!(quality));
    }
    (standardized_usage.signal_score() > 0).then_some(standardized_usage)
}

fn openai_chat_usage_counts(usage: &StandardizedUsage) -> Option<(u64, u64, u64, u64, u64, u64)> {
    let input_tokens = usage.input_tokens.max(0) as u64;
    let output_tokens = usage.output_tokens.max(0) as u64;
    let reasoning_tokens = usage.reasoning_tokens.max(0) as u64;
    let cache_creation_tokens = usage.cache_creation_tokens.max(0) as u64;
    let cache_read_tokens = usage.cache_read_tokens.max(0) as u64;
    let total_tokens = usage
        .dimensions
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            input_tokens
                .saturating_add(output_tokens)
                .saturating_add(reasoning_tokens)
        });
    (total_tokens > 0).then_some((
        input_tokens,
        output_tokens,
        total_tokens,
        reasoning_tokens,
        cache_creation_tokens,
        cache_read_tokens,
    ))
}

fn openai_image_bridge_response_id(
    response: &Map<String, Value>,
    report_context: Option<&Value>,
    fallback_prefix: &str,
) -> String {
    response
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            report_context
                .and_then(|value| value.get("request_id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!("{fallback_prefix}-{value}"))
        })
        .unwrap_or_else(|| fallback_prefix.to_string())
}

fn openai_image_bridge_response_model(
    response: &Map<String, Value>,
    report_context: Option<&Value>,
) -> String {
    response
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| image_bridge_model(report_context))
        .unwrap_or_else(|| "gpt-image".to_string())
}

fn image_request_output_format(report_context: Option<&Value>) -> Option<String> {
    report_context
        .and_then(|value| value.get("image_request"))
        .and_then(|value| value.get("output_format"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn image_request_size(report_context: Option<&Value>) -> Option<String> {
    report_context
        .and_then(|value| value.get("image_request"))
        .and_then(|value| value.get("size"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn image_request_quality(report_context: Option<&Value>) -> Option<String> {
    report_context
        .and_then(|value| value.get("image_request"))
        .and_then(|value| value.get("quality"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn mime_type_from_image_output_format(output_format: &str) -> String {
    match output_format.trim().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "webp" => "image/webp".to_string(),
        "png" => "image/png".to_string(),
        value if !value.is_empty() => format!("image/{value}"),
        _ => "image/png".to_string(),
    }
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

fn is_openai_responses_family_api_format(value: &str) -> bool {
    matches!(value, "openai:responses" | "openai:responses:compact")
}

fn maybe_bridge_aether_sse_response_capture_to_stream(
    provider_body_json: &Value,
    provider_api_format: &str,
    client_api_format: &str,
    report_context: Option<&Value>,
) -> Result<Option<SyncToStreamBridgeOutcome>, AiSurfaceFinalizeError> {
    let Some(object) = provider_body_json.as_object() else {
        return Ok(None);
    };
    let status_code = object
        .get("status_code")
        .or_else(|| object.get("statusCode"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if !(200..300).contains(&status_code) {
        return Ok(None);
    }

    let Some(headers) = object.get("headers").and_then(Value::as_object) else {
        return Ok(None);
    };
    let content_type = response_capture_header(headers, "content-type").unwrap_or_default();
    if !content_type
        .to_ascii_lowercase()
        .contains("text/event-stream")
    {
        return Ok(None);
    }

    let Some(body_text) = object.get("body").and_then(Value::as_str) else {
        return Ok(None);
    };
    if body_text.trim().is_empty() || body_text.contains("...[truncated]") {
        return Ok(None);
    }

    let captured_api_format =
        response_capture_header(headers, "x-aether-control-endpoint-signature")
            .map(normalize_api_format)
            .or_else(|| infer_sse_body_api_format(body_text))
            .unwrap_or_else(|| provider_api_format.to_string());
    if !is_standard_api_format(captured_api_format.as_str())
        || !is_standard_api_format(client_api_format)
    {
        return Ok(None);
    }

    let bridge_context = build_bridge_report_context(
        report_context,
        captured_api_format.as_str(),
        client_api_format,
    );
    let sse_body = if captured_api_format == client_api_format {
        if captured_api_format == "claude:messages" {
            sanitize_same_format_claude_sse_body(body_text.as_bytes(), report_context)?
        } else {
            body_text.as_bytes().to_vec()
        }
    } else {
        rewrite_sse_body_between_formats(
            body_text.as_bytes(),
            captured_api_format.as_str(),
            client_api_format,
            &bridge_context,
        )?
    };
    let terminal_summary = observe_sse_terminal_summary(
        body_text.as_bytes(),
        captured_api_format.as_str(),
        &bridge_context,
    )?;

    Ok(Some(SyncToStreamBridgeOutcome {
        sse_body,
        terminal_summary,
    }))
}

fn sanitize_same_format_claude_sse_body(
    body: &[u8],
    report_context: Option<&Value>,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    let mut context = report_context
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let object = context
        .as_object_mut()
        .expect("same-format Claude context should stay object");
    object.insert(
        "provider_api_format".to_string(),
        Value::String("claude:messages".to_string()),
    );
    object.insert(
        "client_api_format".to_string(),
        Value::String("claude:messages".to_string()),
    );

    let Some(mut rewriter) = maybe_build_ai_surface_stream_rewriter(Some(&context)) else {
        return Ok(body.to_vec());
    };
    let mut out = rewriter.push_chunk(body)?;
    out.extend(rewriter.finish()?);
    Ok(out)
}

fn response_capture_header<'a>(headers: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn infer_sse_body_api_format(body_text: &str) -> Option<String> {
    if body_text.contains("event: message_start")
        || body_text.contains("\"type\":\"message_start\"")
    {
        return Some("claude:messages".to_string());
    }
    if body_text.contains("event: response.") || body_text.contains("\"type\":\"response.") {
        return Some("openai:responses".to_string());
    }
    if body_text.contains("data: [DONE]")
        || body_text.contains("\"object\":\"chat.completion.chunk\"")
    {
        return Some("openai:chat".to_string());
    }
    if body_text.contains("\"candidates\"") && body_text.contains("\"finishReason\"") {
        return Some("gemini:generate_content".to_string());
    }
    None
}

fn rewrite_sse_body_between_formats(
    body: &[u8],
    provider_api_format: &str,
    client_api_format: &str,
    report_context: &Value,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    let mut context =
        build_bridge_report_context(Some(report_context), provider_api_format, client_api_format);
    if let Some(object) = context.as_object_mut() {
        object.insert(
            "provider_stream_event_api_format".to_string(),
            Value::String(provider_api_format.to_string()),
        );
    }

    let mut matrix = StreamingStandardFormatMatrix::default();
    let mut out = Vec::new();
    for_each_sse_line(body, |line| {
        out.extend(matrix.transform_line(&context, line)?);
        Ok(())
    })?;
    out.extend(matrix.finish(&context)?);
    Ok(out)
}

fn observe_sse_terminal_summary(
    body: &[u8],
    provider_api_format: &str,
    report_context: &Value,
) -> Result<Option<ExecutionStreamTerminalSummary>, AiSurfaceFinalizeError> {
    let mut context = build_bridge_report_context(
        Some(report_context),
        provider_api_format,
        provider_api_format,
    );
    if let Some(object) = context.as_object_mut() {
        object.insert(
            "provider_stream_event_api_format".to_string(),
            Value::String(provider_api_format.to_string()),
        );
    }

    let mut observer = StreamingStandardTerminalObserver::default();
    for_each_sse_line(body, |line| observer.push_line(&context, line))?;
    observer.finish(&context)
}

fn for_each_sse_line<F>(body: &[u8], mut on_line: F) -> Result<(), AiSurfaceFinalizeError>
where
    F: FnMut(Vec<u8>) -> Result<(), AiSurfaceFinalizeError>,
{
    let mut start = 0usize;
    for (index, byte) in body.iter().enumerate() {
        if *byte == b'\n' {
            on_line(body[start..=index].to_vec())?;
            start = index + 1;
        }
    }
    if start < body.len() {
        on_line(body[start..].to_vec())?;
    }
    Ok(())
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

fn extract_mime_type_from_data_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let (metadata, _) = trimmed.split_once(',')?;
    let mime_type = metadata.strip_prefix("data:")?.strip_suffix(";base64")?;
    let mime_type = mime_type.trim();
    (!mime_type.is_empty()).then(|| mime_type.to_string())
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
    let event_type = openai_responses_terminal_event_type(openai_responses_response)
        .unwrap_or("response.completed");
    let line = format!(
        "data: {}\n",
        serde_json::to_string(&json!({
            "type": event_type,
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

fn openai_responses_terminal_event_type(response: &Value) -> Option<&'static str> {
    match response.get("status").and_then(Value::as_str) {
        Some("completed") => Some("response.completed"),
        Some("incomplete") => Some("response.incomplete"),
        Some("failed") => Some("response.failed"),
        _ => None,
    }
}

fn emit_client_stream_from_canonical_frames(
    canonical_frames: Vec<CanonicalStreamFrame>,
    client_api_format: &str,
    provider_actual_service_tier: Option<&str>,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    match client_api_format {
        "openai:chat" => {
            let mut emitter = OpenAIChatClientEmitter::default();
            emitter.set_actual_service_tier(provider_actual_service_tier);
            emit_with_openai_chat_emitter(&mut emitter, canonical_frames)
        }
        "openai:responses" | "openai:responses:compact" => {
            let mut emitter = OpenAIResponsesClientEmitter::default();
            emitter.set_actual_service_tier(provider_actual_service_tier);
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

fn emit_openai_responses_stream_with_authoritative_terminal(
    canonical_frames: Vec<CanonicalStreamFrame>,
    authoritative_response: &Value,
    terminal_event_type: &'static str,
    provider_actual_service_tier: Option<&str>,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    let mut emitter = OpenAIResponsesClientEmitter::default();
    emitter.set_actual_service_tier(provider_actual_service_tier);
    let mut output = Vec::new();
    for frame in canonical_frames {
        if matches!(
            &frame.event,
            CanonicalStreamEvent::Finish { .. } | CanonicalStreamEvent::UnknownEvent(_)
        ) {
            continue;
        }
        output.extend(
            emitter
                .emit(frame)
                .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?,
        );
    }
    output.extend(
        emitter
            .finish_with_authoritative_response_event(
                authoritative_response.clone(),
                terminal_event_type,
            )
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
    provider_actual_service_tier: Option<String>,
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
        provider_actual_service_tier,
        observed_finish: true,
        unknown_event_count: 0,
        parser_error: None,
    })
}

fn provider_actual_service_tier_from_sync_response(
    provider_response: &Value,
    provider_api_format: &str,
) -> Option<String> {
    if !matches!(
        normalize_api_format(provider_api_format).as_str(),
        "openai:chat" | "openai:responses" | "openai:responses:compact"
    ) {
        return None;
    }
    provider_response
        .get("response")
        .and_then(Value::as_object)
        .and_then(|response| response.get("service_tier"))
        .or_else(|| provider_response.get("service_tier"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 64)
        .map(str::to_ascii_lowercase)
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
                .and_then(|details| {
                    details
                        .get("cache_write_tokens")
                        .or_else(|| details.get("cached_creation_tokens"))
                        .or_else(|| details.get("cache_creation_tokens"))
                })
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
    use serde_json::{json, Value};

    use super::{maybe_bridge_standard_sync_json_to_stream, standardized_usage_from_openai_usage};

    fn utf8(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).expect("utf8 should decode")
    }

    fn json_sse_events(body: &str) -> Vec<Value> {
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter(|line| *line != "[DONE]")
            .map(|line| serde_json::from_str(line).expect("valid SSE JSON"))
            .collect()
    }

    fn assert_strictly_increasing_sequence_numbers(events: &[Value]) {
        let sequence_numbers = events
            .iter()
            .filter_map(|event| event.get("sequence_number").and_then(Value::as_u64))
            .collect::<Vec<_>>();
        assert_eq!(sequence_numbers.len(), events.len());
        assert!(
            sequence_numbers.windows(2).all(|pair| pair[0] < pair[1]),
            "sequence numbers must be strictly increasing: {sequence_numbers:?}"
        );
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
    fn bridges_same_family_responses_sync_with_standard_lifecycle_and_authoritative_terminal() {
        let response = json!({
            "id": "resp_gpt56_raw_bridge",
            "object": "response",
            "model": "gpt-5.6-sol",
            "status": "completed",
            "output": [
                {
                    "type": "program",
                    "call_id": "program-call-1",
                    "fingerprint": "fp-program-1"
                },
                {
                    "type": "program_output",
                    "call_id": "program-call-1",
                    "result": "hello",
                    "status": "completed"
                },
                {
                    "type": "multi_agent_call",
                    "call_id": "agent-call-1",
                    "action": "delegate",
                    "arguments": {"query": "release notes"},
                    "agent": "researcher"
                },
                {
                    "type": "agent_message",
                    "author": "researcher",
                    "recipient": "assistant",
                    "encrypted_content": "encrypted-agent-message"
                }
            ],
            "usage": {
                "input_tokens": 20,
                "input_tokens_details": {
                    "cached_tokens": 4,
                    "cache_write_tokens": 6
                },
                "output_tokens": 5,
                "total_tokens": 25
            }
        });

        let outcome = maybe_bridge_standard_sync_json_to_stream(
            &response,
            "openai:responses",
            "openai:responses:compact",
            None,
        )
        .expect("same-family raw bridge should succeed")
        .expect("same-family raw bridge should emit terminal SSE");

        let output = utf8(outcome.sse_body);
        assert!(output.contains("event: response.created"));
        assert!(output.contains("event: response.in_progress"));
        let events = json_sse_events(&output);
        assert_strictly_increasing_sequence_numbers(&events);
        let event = events
            .iter()
            .find(|event| event["type"] == "response.completed")
            .expect("terminal event data should exist");
        assert_eq!(events.last(), Some(event));
        assert_eq!(event["type"], "response.completed");
        assert_eq!(event["response"], response);
    }

    #[test]
    fn cross_format_bridge_keeps_provider_actual_service_tier_authoritative() {
        let provider_response = json!({
            "id": "chatcmpl_actual_tier",
            "object": "chat.completion",
            "model": "gpt-5.6-sol",
            "service_tier": "Default",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "done"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12}
        });
        let report_context = json!({
            "original_request_body": {
                "model": "gpt-5.6-sol",
                "service_tier": "priority"
            }
        });

        let outcome = maybe_bridge_standard_sync_json_to_stream(
            &provider_response,
            "openai:chat",
            "openai:responses",
            Some(&report_context),
        )
        .expect("cross-format bridge should succeed")
        .expect("cross-format bridge should emit terminal SSE");

        assert_eq!(
            outcome
                .terminal_summary
                .as_ref()
                .and_then(|summary| summary.provider_actual_service_tier.as_deref()),
            Some("default")
        );
        let events = json_sse_events(&utf8(outcome.sse_body));
        let terminal = events
            .iter()
            .find(|event| event["type"] == "response.completed")
            .expect("terminal event should exist");
        assert_eq!(terminal["response"]["service_tier"], "Default");
        let response_events = events
            .iter()
            .filter(|event| {
                matches!(
                    event.get("type").and_then(Value::as_str),
                    Some("response.created" | "response.in_progress" | "response.completed")
                )
            })
            .collect::<Vec<_>>();
        assert!(!response_events.is_empty());
        assert!(response_events.iter().all(|event| {
            event
                .pointer("/response/service_tier")
                .and_then(Value::as_str)
                .is_some_and(|tier| tier.eq_ignore_ascii_case("default"))
        }));
    }

    #[test]
    fn sync_bridge_does_not_echo_requested_service_tier_as_provider_actual() {
        let provider_response = json!({
            "id": "chatcmpl_without_actual_tier",
            "object": "chat.completion",
            "model": "gpt-5.6-sol",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "done"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12}
        });
        let report_context = json!({
            "original_request_body": {
                "model": "gpt-5.6-sol",
                "service_tier": "priority"
            }
        });

        let outcome = maybe_bridge_standard_sync_json_to_stream(
            &provider_response,
            "openai:chat",
            "openai:responses",
            Some(&report_context),
        )
        .expect("cross-format bridge should succeed")
        .expect("cross-format bridge should emit terminal SSE");

        assert_eq!(
            outcome
                .terminal_summary
                .as_ref()
                .and_then(|summary| summary.provider_actual_service_tier.as_deref()),
            None
        );
        let events = json_sse_events(&utf8(outcome.sse_body));
        assert!(events.iter().all(|event| {
            event
                .get("response")
                .and_then(|response| response.get("service_tier"))
                .is_none()
        }));
    }

    #[test]
    fn bridges_output_only_compact_json_without_reshaping_the_terminal_response() {
        let response = json!({
            "output": [{
                "type": "compaction",
                "id": "cmp_123",
                "encrypted_content": "encrypted-compact-history"
            }]
        });

        let outcome = maybe_bridge_standard_sync_json_to_stream(
            &response,
            "openai:responses:compact",
            "openai:responses:compact",
            None,
        )
        .expect("Compact bridge should succeed")
        .expect("Compact output should emit terminal SSE");

        let output = utf8(outcome.sse_body);
        assert!(output.contains("event: response.created"));
        assert!(output.contains("event: response.in_progress"));
        let events = json_sse_events(&output);
        let terminal = events
            .iter()
            .find(|event| event["type"] == "response.completed")
            .expect("Compact terminal event should exist");
        assert_eq!(terminal["response"], response);
        assert_eq!(events.last(), Some(terminal));
    }

    #[test]
    fn bridges_failed_same_family_responses_sync_with_authoritative_terminal() {
        let response = json!({
            "id": "resp_gpt56_failed",
            "object": "response",
            "model": "gpt-5.6-sol",
            "status": "failed",
            "error": {"code": "server_error", "message": "upstream failed"},
            "output": [{
                "type": "program",
                "call_id": "program-call-1",
                "fingerprint": "fp-program-1"
            }]
        });

        let outcome = maybe_bridge_standard_sync_json_to_stream(
            &response,
            "openai:responses",
            "openai:responses",
            None,
        )
        .expect("failed same-family raw bridge should succeed")
        .expect("failed response should emit terminal SSE");

        let output = utf8(outcome.sse_body);
        let events = json_sse_events(&output);
        assert_strictly_increasing_sequence_numbers(&events);
        let event = events
            .iter()
            .find(|event| event["type"] == "response.failed")
            .expect("terminal event data should exist");
        assert_eq!(events.last(), Some(event));
        assert_eq!(event["type"], "response.failed");
        assert_eq!(event["response"], response);
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
        assert_eq!(
            summary
                .standardized_usage
                .as_ref()
                .and_then(|usage| usage.dimensions.get("image_count"))
                .cloned(),
            Some(json!(1))
        );
    }

    #[test]
    fn bridges_openai_image_sync_json_to_openai_chat_sse() {
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:chat",
            "mapped_model": "gpt-image-2",
            "image_request": {
                "operation": "generate",
                "output_format": "png",
                "size": "1024x1024",
                "quality": "medium"
            }
        });
        let outcome = maybe_bridge_standard_sync_json_to_stream(
            &json!({
                "id": "img_123",
                "created": 1776971267,
                "model": "gpt-image-2",
                "data": [
                    {"b64_json": "aGVsbG8="},
                    {"b64_json": "d29ybGQ="}
                ],
                "usage": {
                    "total_tokens": 100,
                    "input_tokens": 50,
                    "output_tokens": 50,
                    "input_tokens_details": {
                        "cached_tokens": 20,
                        "cached_creation_tokens": 10
                    }
                }
            }),
            "openai:image",
            "openai:chat",
            Some(&report_context),
        )
        .expect("bridge should succeed")
        .expect("bridge should produce sse");

        let output = utf8(outcome.sse_body);
        assert!(output.contains("\"object\":\"chat.completion.chunk\""));
        assert!(output.contains("![generated image](data:image/png;base64,aGVsbG8=)"));
        assert!(output.contains("![generated image 2](data:image/png;base64,d29ybGQ=)"));
        assert!(output.contains("\"finish_reason\":\"stop\""));
        assert!(output.contains("\"cached_tokens\":20"));
        assert!(output.contains("\"cache_write_tokens\":10"));
        assert!(!output.contains("\"cached_creation_tokens\""));
        assert!(output.contains("data: [DONE]"));
        assert!(!output.contains("image_generation.completed"));

        let summary = outcome
            .terminal_summary
            .expect("terminal summary should exist");
        let usage = summary
            .standardized_usage
            .as_ref()
            .expect("standard usage should exist");
        assert_eq!(usage.request_count, 2);
        assert_eq!(usage.dimensions.get("image_count"), Some(&json!(2)));
        assert_eq!(
            usage.dimensions.get("image_size"),
            Some(&json!("1024x1024"))
        );
        assert_eq!(
            usage.dimensions.get("image_quality"),
            Some(&json!("medium"))
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

    #[test]
    fn bridges_aether_sse_response_capture_to_same_client_stream() {
        let captured_body = concat!(
            ": aether-keepalive\n\n",
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"gpt-5.5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let outcome = maybe_bridge_standard_sync_json_to_stream(
            &json!({
                "status_code": 200,
                "headers": {
                    "content-type": "text/event-stream",
                    "x-aether-control-endpoint-signature": "claude:messages"
                },
                "body": captured_body
            }),
            "openai:responses",
            "claude:messages",
            None,
        )
        .expect("bridge should succeed")
        .expect("capture should bridge");

        let output = utf8(outcome.sse_body);
        assert!(output.contains("event: message_start"));
        assert!(output.contains("event: message_stop"));
        assert!(output.contains("\"stop_reason\":\"end_turn\""));
        assert!(!output.contains("status_code"));
        assert_eq!(
            outcome
                .terminal_summary
                .as_ref()
                .and_then(|summary| summary.finish_reason.as_deref()),
            Some("stop")
        );
    }

    #[test]
    fn rewrites_same_format_claude_capture_to_sanitize_read_tool_input() {
        let captured_body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_read_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"gpt-5.5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_read_1\",\"name\":\"Read\",\"input\":{\"file_path\":\"D:/projects/UIAutoTest/docs/prd/msr.md\",\"offset\":0,\"limit\":2000,\"pages\":\"\"}}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srv_1\",\"name\":\"web_search\",\"input\":{\"query\":\"rust\"}}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let outcome = maybe_bridge_standard_sync_json_to_stream(
            &json!({
                "status_code": 200,
                "headers": {
                    "content-type": "text/event-stream",
                    "x-aether-control-endpoint-signature": "claude:messages"
                },
                "body": captured_body
            }),
            "openai:responses",
            "claude:messages",
            None,
        )
        .expect("bridge should succeed")
        .expect("capture should bridge");

        let output = utf8(outcome.sse_body);
        assert!(output.contains("\"name\":\"Read\""));
        assert!(output.contains("\"limit\":2000"));
        assert!(output.contains("\"type\":\"server_tool_use\""));
        assert!(output.contains("\"name\":\"web_search\""));
        assert!(!output.contains("\"pages\":\"\""));
        assert!(!output.contains("\\\"pages\\\":\\\"\\\""));
    }

    #[test]
    fn rewrites_aether_sse_response_capture_to_requested_client_stream() {
        let captured_body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"gpt-5.5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"Edit\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"file_path\\\":\\\"/tmp/a.txt\\\"}\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let outcome = maybe_bridge_standard_sync_json_to_stream(
            &json!({
                "status_code": 200,
                "headers": {
                    "content-type": "text/event-stream",
                    "x-aether-control-endpoint-signature": "claude:messages"
                },
                "body": captured_body
            }),
            "claude:messages",
            "openai:responses",
            None,
        )
        .expect("bridge should succeed")
        .expect("capture should bridge");

        let output = utf8(outcome.sse_body);
        assert!(output.contains("event: response.output_item.added"));
        assert!(output.contains("event: response.function_call_arguments.delta"));
        assert!(output.contains("event: response.completed"));
        assert!(!output.contains("status_code"));
    }
}
