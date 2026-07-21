use base64::Engine as _;
use std::collections::BTreeMap;

use aether_ai_formats::formats::conversion::response::{
    convert_claude_response_to_openai_responses, convert_gemini_chat_response_to_openai_chat,
    convert_openai_chat_response_to_claude_chat, convert_openai_chat_response_to_gemini_chat,
    convert_openai_chat_response_to_openai_responses,
    convert_openai_responses_response_to_openai_chat,
};
use aether_ai_formats::formats::openai::responses::response::ensure_modern_openai_responses_response_fields;
use aether_ai_formats::formats::registry::{convert_response, FormatContext, FormatError};
use aether_ai_formats::{
    canonical_response_unknown_block_count, canonical_to_claude_response,
    canonical_to_embedding_response, canonical_to_gemini_response,
    canonical_to_openai_chat_response, canonical_to_openai_responses_compact_response,
    canonical_to_openai_responses_response, from_claude_to_canonical_response,
    from_embedding_to_canonical_response, from_gemini_to_canonical_response,
    from_openai_chat_to_canonical_response, from_openai_responses_to_canonical_response,
    sync_chat_response_conversion_kind, sync_cli_response_conversion_kind,
};
use serde_json::{json, Map, Value};

use super::AiSurfaceFinalizeError;
use crate::formats::claude::messages::stream::ClaudeProviderState;
use crate::formats::gemini::generate_content::stream::GeminiProviderState;
use crate::formats::openai::chat::stream::{OpenAIChatProviderState, OpenAIResponsesProviderState};
use crate::formats::shared::model_directives::model_directive_display_model_from_report_context;
use crate::formats::shared::response::{
    remove_empty_pages_from_tool_arguments, remove_empty_pages_from_tool_input_value,
    sanitize_claude_read_tool_inputs,
};
use crate::formats::shared::stream_core::common::{
    content_part_from_openai_image_generation_item, gemini_usage_metadata_from_usage,
    map_openai_finish_reason_to_gemini, parse_json_arguments_value,
    unsupported_stream_event_message, CanonicalContentPart, CanonicalStreamEvent,
    CanonicalStreamFrame, CanonicalUsage,
};

#[derive(Clone, Debug, PartialEq)]
pub struct StandardCrossFormatSyncProduct {
    pub client_body_json: Value,
    pub provider_body_json: Value,
}

struct OpenAiCrossFormatProviderBody {
    body_json: Value,
    // Only validated stream aggregation may use a client projection that omits provider-only fields.
    aggregated_from_stream: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StandardSyncFinalizeNormalizedProduct {
    SuccessBody(Value),
    CrossFormat(StandardCrossFormatSyncProduct),
}

pub fn maybe_build_standard_cross_format_sync_product_from_normalized_payload(
    report_kind: &str,
    status_code: u16,
    report_context: Option<&Value>,
    body_json: Option<&Value>,
    body_base64: Option<&str>,
) -> Result<Option<StandardCrossFormatSyncProduct>, AiSurfaceFinalizeError> {
    if status_code >= 400 {
        return Ok(None);
    }

    let Some(report_context) = report_context else {
        return Ok(None);
    };
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let client_api_format = report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let (aggregated_stream_body, aggregated_stream_api_format) = match body_base64 {
        Some(body_base64) => {
            let body_bytes = base64::engine::general_purpose::STANDARD.decode(body_base64)?;
            let provider_stream_event_api_format =
                provider_stream_event_api_format_for_report_context(
                    report_context,
                    provider_api_format,
                );
            if is_standard_chat_finalize_kind(report_kind) {
                let body = try_aggregate_standard_chat_stream_sync_response(
                    &body_bytes,
                    &provider_stream_event_api_format,
                )?;
                let api_format = body
                    .as_ref()
                    .map(|_| provider_stream_event_api_format.clone());
                (body, api_format)
            } else if is_standard_cli_finalize_kind(report_kind) {
                let body = try_aggregate_standard_cli_stream_sync_response(
                    &body_bytes,
                    &provider_stream_event_api_format,
                )?;
                let api_format = body
                    .as_ref()
                    .map(|_| provider_stream_event_api_format.clone());
                (body, api_format)
            } else {
                return Ok(None);
            }
        }
        None => (None, None),
    };
    let provider_body_api_format = aggregated_stream_api_format
        .as_deref()
        .unwrap_or(provider_api_format);

    let Some(provider_body_json) = aggregated_stream_body.or_else(|| body_json.cloned()) else {
        return Ok(None);
    };

    Ok(maybe_build_standard_cross_format_sync_product(
        report_kind,
        provider_body_api_format,
        client_api_format,
        report_context,
        provider_body_json,
    ))
}

pub fn maybe_build_standard_same_format_sync_body_from_normalized_payload(
    report_kind: &str,
    status_code: u16,
    report_context: Option<&Value>,
    body_json: Option<&Value>,
    body_base64: Option<&str>,
) -> Result<Option<Value>, AiSurfaceFinalizeError> {
    let stream_body = maybe_build_standard_same_format_stream_sync_body(
        report_kind,
        status_code,
        report_context,
        body_base64,
    )?;
    Ok(stream_body.or_else(|| {
        maybe_build_standard_same_format_sync_body(
            report_kind,
            status_code,
            report_context,
            body_json,
        )
    }))
}

pub fn maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
    report_kind: &str,
    status_code: u16,
    report_context: Option<&Value>,
    body_json: Option<&Value>,
    body_base64: Option<&str>,
) -> Result<Option<Value>, AiSurfaceFinalizeError> {
    let stream_body = maybe_build_openai_responses_same_family_stream_sync_body(
        report_kind,
        status_code,
        report_context,
        body_base64,
    )?;
    Ok(stream_body.or_else(|| {
        maybe_build_openai_responses_same_family_sync_body(
            report_kind,
            status_code,
            report_context,
            body_json,
        )
    }))
}

pub fn maybe_build_openai_chat_cross_format_sync_product_from_normalized_payload(
    report_kind: &str,
    status_code: u16,
    report_context: Option<&Value>,
    body_json: Option<&Value>,
    body_base64: Option<&str>,
) -> Result<Option<StandardCrossFormatSyncProduct>, AiSurfaceFinalizeError> {
    if report_kind != "openai_chat_sync_finalize" || status_code >= 400 {
        return Ok(None);
    }

    let Some(report_context) = report_context else {
        return Ok(None);
    };
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let client_api_format = report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    if client_api_format != "openai:chat"
        || sync_chat_response_conversion_kind(&provider_api_format, &client_api_format).is_none()
    {
        return Ok(None);
    }

    let Some(provider_body) =
        maybe_build_openai_cross_format_provider_body_from_normalized_payload(
            body_json,
            body_base64,
            &provider_api_format,
        )?
    else {
        return Ok(None);
    };

    let provider_body_json = provider_body.body_json;
    let Some(client_body_json) = (match provider_api_format.as_str() {
        "claude:messages" => {
            convert_claude_canonical_response_to_openai_chat(&provider_body_json, report_context)
        }
        "gemini:generate_content" => {
            convert_gemini_canonical_response_to_openai_chat(&provider_body_json, report_context)
        }
        "openai:responses" => {
            convert_openai_responses_response_to_openai_chat(&provider_body_json, report_context)
                .or_else(|| {
                    provider_body
                        .aggregated_from_stream
                        .then(|| {
                            project_validated_openai_responses_stream_to_openai_chat(
                                &provider_body_json,
                                report_context,
                            )
                        })
                        .flatten()
                })
        }
        _ => None,
    }) else {
        return Ok(None);
    };

    Ok(Some(StandardCrossFormatSyncProduct {
        client_body_json,
        provider_body_json,
    }))
}

pub fn maybe_build_openai_responses_cross_format_sync_product_from_normalized_payload(
    report_kind: &str,
    status_code: u16,
    report_context: Option<&Value>,
    body_json: Option<&Value>,
    body_base64: Option<&str>,
) -> Result<Option<StandardCrossFormatSyncProduct>, AiSurfaceFinalizeError> {
    if !is_openai_responses_finalize_kind(report_kind) || status_code >= 400 {
        return Ok(None);
    }

    let Some(report_context) = report_context else {
        return Ok(None);
    };
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let client_api_format = report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let normalized_client_api_format =
        normalize_openai_responses_family_api_format(&client_api_format);

    if !matches!(
        normalized_client_api_format.as_str(),
        "openai:responses" | "openai:responses:compact"
    ) || sync_cli_response_conversion_kind(&provider_api_format, &normalized_client_api_format)
        .is_none()
    {
        return Ok(None);
    }

    let Some(provider_body) =
        maybe_build_openai_cross_format_provider_body_from_normalized_payload(
            body_json,
            body_base64,
            &provider_api_format,
        )?
    else {
        return Ok(None);
    };
    let provider_body_json = provider_body.body_json;

    let normalized_provider_api_format =
        normalize_openai_responses_family_api_format(&provider_api_format);
    let Some(client_body_json) = (match normalized_provider_api_format.as_str() {
        "openai:responses" | "openai:responses:compact" => Some(provider_body_json.clone()),
        "claude:messages" => {
            convert_claude_response_to_openai_responses(&provider_body_json, report_context)
        }
        "gemini:generate_content" => convert_gemini_canonical_response_to_openai_responses(
            &provider_body_json,
            report_context,
            normalized_client_api_format == "openai:responses:compact",
        ),
        _ => None,
    }) else {
        return Ok(None);
    };

    Ok(Some(StandardCrossFormatSyncProduct {
        client_body_json,
        provider_body_json,
    }))
}

fn maybe_encode_sync_capture_envelope_stream_body(
    body_json: Option<&Value>,
) -> Result<Option<String>, AiSurfaceFinalizeError> {
    let Some(body_json) = body_json else {
        return Ok(None);
    };
    let Some(body_object) = body_json.as_object() else {
        return Ok(None);
    };
    let Some(chunks) = body_object.get("chunks").and_then(Value::as_array) else {
        return Ok(None);
    };
    if is_error_like_sync_body(body_json) {
        return Ok(None);
    }
    if chunks.is_empty() {
        return Err(AiSurfaceFinalizeError::new(
            "Provider stream capture envelope contains no events",
        ));
    }

    if body_object
        .get("raw_response")
        .is_some_and(|value| !value.is_null())
    {
        return Err(AiSurfaceFinalizeError::new(
            "Provider stream capture envelope contains an unparsed raw response",
        ));
    }
    if let Some(metadata_value) = body_object.get("metadata") {
        let Some(metadata) = metadata_value.as_object() else {
            return Err(AiSurfaceFinalizeError::new(
                "Provider stream capture envelope metadata must be an object",
            ));
        };
        if metadata
            .get("parse_error")
            .is_some_and(|value| !value.is_null())
        {
            return Err(AiSurfaceFinalizeError::new(
                "Provider stream capture envelope reports a stream parse error",
            ));
        }
        if let Some(stream) = metadata.get("stream") {
            if stream.as_bool() != Some(true) {
                return Err(AiSurfaceFinalizeError::new(
                    "Provider stream capture envelope metadata.stream must be true",
                ));
            }
        }

        let dropped_chunks = sync_capture_metadata_u64(metadata, "dropped_chunks")?;
        let stored_chunks = sync_capture_metadata_u64(metadata, "stored_chunks")?;
        let total_chunks = sync_capture_metadata_u64(metadata, "total_chunks")?;
        if dropped_chunks.is_some_and(|dropped| dropped > 0) {
            return Err(incomplete_sync_capture_envelope_error());
        }
        match (stored_chunks, total_chunks) {
            (None, None) => {}
            (Some(stored), Some(total)) if stored == total && stored == chunks.len() as u64 => {}
            _ => return Err(incomplete_sync_capture_envelope_error()),
        }
    }

    let mut stream_body = Vec::new();
    for chunk in chunks {
        if !chunk.is_object() {
            return Err(AiSurfaceFinalizeError::new(
                "Provider stream capture envelope contains a non-object event",
            ));
        }
        stream_body.extend_from_slice(b"data: ");
        serde_json::to_writer(&mut stream_body, chunk)?;
        stream_body.extend_from_slice(b"\n\n");
    }

    Ok(Some(
        base64::engine::general_purpose::STANDARD.encode(stream_body),
    ))
}

fn sync_capture_metadata_u64(
    metadata: &Map<String, Value>,
    field: &str,
) -> Result<Option<u64>, AiSurfaceFinalizeError> {
    let Some(value) = metadata.get(field) else {
        return Ok(None);
    };
    value.as_u64().map(Some).ok_or_else(|| {
        AiSurfaceFinalizeError::new(format!(
            "Provider stream capture envelope metadata.{field} must be an unsigned integer"
        ))
    })
}

fn incomplete_sync_capture_envelope_error() -> AiSurfaceFinalizeError {
    AiSurfaceFinalizeError::new(
        "Provider stream capture envelope is incomplete and cannot be finalized losslessly",
    )
}

fn sync_finalize_supports_capture_envelope(
    report_kind: &str,
    report_context: Option<&Value>,
) -> bool {
    if !is_standard_chat_finalize_kind(report_kind) && !is_standard_cli_finalize_kind(report_kind) {
        return false;
    }
    let Some(report_context) = report_context else {
        return false;
    };
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Gemini private capture envelopes are normalized by provider_compat before this boundary.
    matches!(
        aether_ai_formats::normalize_api_format_alias(
            &provider_stream_event_api_format_for_report_context(
                report_context,
                provider_api_format,
            )
        )
        .as_str(),
        "openai:chat" | "openai:responses" | "openai:responses:compact" | "claude:messages"
    )
}

pub fn maybe_build_standard_sync_finalize_product_from_normalized_payload(
    report_kind: &str,
    status_code: u16,
    report_context: Option<&Value>,
    body_json: Option<&Value>,
    body_base64: Option<&str>,
) -> Result<Option<StandardSyncFinalizeNormalizedProduct>, AiSurfaceFinalizeError> {
    let capture_stream_body_base64 = if status_code < 400
        && body_base64.is_none()
        && sync_finalize_supports_capture_envelope(report_kind, report_context)
    {
        maybe_encode_sync_capture_envelope_stream_body(body_json)?
    } else {
        None
    };
    let capture_envelope_used = capture_stream_body_base64.is_some();
    let body_json = if capture_envelope_used {
        None
    } else {
        body_json
    };
    let body_base64 = body_base64.or(capture_stream_body_base64.as_deref());

    if let Some(body_json) = maybe_build_standard_same_format_sync_body_from_normalized_payload(
        report_kind,
        status_code,
        report_context,
        body_json,
        body_base64,
    )? {
        return Ok(Some(StandardSyncFinalizeNormalizedProduct::SuccessBody(
            body_json,
        )));
    }

    if let Some(body_json) =
        maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
            report_kind,
            status_code,
            report_context,
            body_json,
            body_base64,
        )?
    {
        return Ok(Some(StandardSyncFinalizeNormalizedProduct::SuccessBody(
            body_json,
        )));
    }

    if let Some(product) =
        maybe_build_openai_chat_cross_format_sync_product_from_normalized_payload(
            report_kind,
            status_code,
            report_context,
            body_json,
            body_base64,
        )?
    {
        return Ok(Some(StandardSyncFinalizeNormalizedProduct::CrossFormat(
            product,
        )));
    }

    if let Some(product) =
        maybe_build_openai_responses_cross_format_sync_product_from_normalized_payload(
            report_kind,
            status_code,
            report_context,
            body_json,
            body_base64,
        )?
    {
        return Ok(Some(StandardSyncFinalizeNormalizedProduct::CrossFormat(
            product,
        )));
    }

    if let Some(product) = maybe_build_embedding_cross_format_sync_product_from_normalized_payload(
        report_kind,
        status_code,
        report_context,
        body_json,
        body_base64,
    )? {
        return Ok(Some(StandardSyncFinalizeNormalizedProduct::CrossFormat(
            product,
        )));
    }

    let product = maybe_build_standard_cross_format_sync_product_from_normalized_payload(
        report_kind,
        status_code,
        report_context,
        body_json,
        body_base64,
    )?
    .map(StandardSyncFinalizeNormalizedProduct::CrossFormat);
    if capture_envelope_used && product.is_none() {
        return Err(AiSurfaceFinalizeError::new(
            "Provider stream capture envelope cannot be finalized losslessly",
        ));
    }
    Ok(product)
}

pub fn maybe_build_embedding_cross_format_sync_product_from_normalized_payload(
    report_kind: &str,
    status_code: u16,
    report_context: Option<&Value>,
    body_json: Option<&Value>,
    body_base64: Option<&str>,
) -> Result<Option<StandardCrossFormatSyncProduct>, AiSurfaceFinalizeError> {
    if report_kind != crate::contracts::OPENAI_EMBEDDING_SYNC_FINALIZE_REPORT_KIND
        || status_code >= 400
    {
        return Ok(None);
    }

    let Some(report_context) = report_context else {
        return Ok(None);
    };
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let client_api_format = report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    if provider_api_format == client_api_format {
        return Ok(None);
    }

    let Some(provider_namespace) =
        embedding_response_namespace_for_api_format(&provider_api_format)
    else {
        return Ok(None);
    };
    let Some(client_namespace) = embedding_response_namespace_for_api_format(&client_api_format)
    else {
        return Ok(None);
    };

    let provider_body_json = match body_base64 {
        Some(body_base64) => {
            let body_bytes = base64::engine::general_purpose::STANDARD.decode(body_base64)?;
            serde_json::from_slice::<Value>(&body_bytes).ok()
        }
        None => body_json.cloned(),
    };
    let Some(provider_body_json) =
        provider_body_json.filter(|value| !is_error_like_sync_body(value))
    else {
        return Ok(None);
    };

    let mut canonical =
        match from_embedding_to_canonical_response(&provider_body_json, provider_namespace) {
            Some(canonical) => canonical,
            None => return Ok(None),
        };
    apply_report_context_model_fallback(&mut canonical.model, report_context);
    let Some(client_body_json) = canonical_to_embedding_response(&canonical, client_namespace)
    else {
        return Ok(None);
    };
    let client_body_json =
        client_body_with_report_context_model(client_body_json, report_context, &client_api_format);

    Ok(Some(StandardCrossFormatSyncProduct {
        client_body_json,
        provider_body_json,
    }))
}

fn embedding_response_namespace_for_api_format(api_format: &str) -> Option<&'static str> {
    match aether_ai_formats::normalize_api_format_alias(api_format).as_str() {
        "openai:embedding" => Some("openai"),
        "jina:embedding" => Some("jina"),
        "gemini:embedding" => Some("gemini"),
        "aliyun:multimodal_embedding" => Some("aliyun"),
        _ => None,
    }
}

fn maybe_build_standard_same_format_sync_body(
    report_kind: &str,
    status_code: u16,
    report_context: Option<&Value>,
    body_json: Option<&Value>,
) -> Option<Value> {
    if status_code >= 400 {
        return None;
    }

    let report_context = report_context?;
    let expected_api_format = standard_same_format_api_format(report_kind)?;
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let client_api_format = report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let needs_conversion = report_context
        .get("needs_conversion")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if provider_api_format != expected_api_format
        || client_api_format != expected_api_format
        || needs_conversion
    {
        return None;
    }

    let body_json = body_json?;
    if is_error_like_sync_body(body_json) {
        return None;
    }
    if api_format_is_gemini_generate_content(expected_api_format)
        && !gemini_generate_content_body_has_visible_output(body_json)
    {
        return None;
    }

    let mut body_json = body_json.clone();
    if expected_api_format == "claude:messages" {
        sanitize_claude_read_tool_inputs(&mut body_json);
    }

    Some(client_body_with_report_context_model(
        body_json,
        report_context,
        &client_api_format,
    ))
}

fn maybe_build_standard_same_format_stream_sync_body(
    report_kind: &str,
    status_code: u16,
    report_context: Option<&Value>,
    body_base64: Option<&str>,
) -> Result<Option<Value>, AiSurfaceFinalizeError> {
    if status_code >= 400 {
        return Ok(None);
    }

    let report_context = match report_context {
        Some(report_context) => report_context,
        None => return Ok(None),
    };
    let expected_api_format = match standard_same_format_api_format(report_kind) {
        Some(expected_api_format) => expected_api_format,
        None => return Ok(None),
    };
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let client_api_format = report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let needs_conversion = report_context
        .get("needs_conversion")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if provider_api_format != expected_api_format
        || client_api_format != expected_api_format
        || needs_conversion
    {
        return Ok(None);
    }

    let Some(body_base64) = body_base64 else {
        return Ok(None);
    };
    let body_bytes = base64::engine::general_purpose::STANDARD.decode(body_base64)?;
    let provider_stream_event_api_format =
        provider_stream_event_api_format_for_report_context(report_context, &provider_api_format);
    let Some(body) = try_aggregate_standard_chat_stream_sync_response(
        &body_bytes,
        &provider_stream_event_api_format,
    )?
    else {
        return Ok(None);
    };
    if api_format_is_gemini_generate_content(&provider_stream_event_api_format)
        && !gemini_generate_content_body_has_visible_output(&body)
    {
        return Ok(None);
    }
    let Some(client_body) = convert_aggregated_stream_body_to_client_sync_response(
        report_kind,
        body,
        &provider_stream_event_api_format,
        &client_api_format,
        report_context,
    ) else {
        return Ok(None);
    };
    Ok(Some(client_body_with_report_context_model(
        client_body,
        report_context,
        &client_api_format,
    )))
}

fn maybe_build_openai_responses_same_family_sync_body(
    report_kind: &str,
    status_code: u16,
    report_context: Option<&Value>,
    body_json: Option<&Value>,
) -> Option<Value> {
    if status_code >= 400 || !is_openai_responses_finalize_kind(report_kind) {
        return None;
    }

    let report_context = report_context?;
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let client_api_format = report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let needs_conversion = report_context
        .get("needs_conversion")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let normalized_provider_api_format =
        normalize_openai_responses_family_api_format(&provider_api_format);
    let normalized_client_api_format =
        normalize_openai_responses_family_api_format(&client_api_format);
    if !is_openai_responses_family_api_format(&provider_api_format)
        || !is_openai_responses_family_api_format(&client_api_format)
        || (normalized_provider_api_format == normalized_client_api_format && needs_conversion)
    {
        return None;
    }

    let body_json = body_json?;
    if is_error_like_sync_body(body_json) {
        return None;
    }

    Some(client_body_with_report_context_model(
        body_json.clone(),
        report_context,
        &client_api_format,
    ))
}

fn maybe_build_openai_responses_same_family_stream_sync_body(
    report_kind: &str,
    status_code: u16,
    report_context: Option<&Value>,
    body_base64: Option<&str>,
) -> Result<Option<Value>, AiSurfaceFinalizeError> {
    if status_code >= 400 || !is_openai_responses_finalize_kind(report_kind) {
        return Ok(None);
    }

    let report_context = match report_context {
        Some(report_context) => report_context,
        None => return Ok(None),
    };
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let client_api_format = report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let needs_conversion = report_context
        .get("needs_conversion")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let provider_stream_event_api_format =
        provider_stream_event_api_format_for_report_context(report_context, &provider_api_format);
    let normalized_provider_api_format =
        normalize_openai_responses_family_api_format(&provider_stream_event_api_format);
    let normalized_client_api_format =
        normalize_openai_responses_family_api_format(&client_api_format);
    if !is_openai_responses_family_api_format(&provider_stream_event_api_format)
        || !is_openai_responses_family_api_format(&client_api_format)
        || (is_openai_responses_family_api_format(&provider_api_format)
            && normalized_provider_api_format == normalized_client_api_format
            && needs_conversion)
    {
        return Ok(None);
    }

    let Some(body_base64) = body_base64 else {
        return Ok(None);
    };
    let body_bytes = base64::engine::general_purpose::STANDARD.decode(body_base64)?;
    // Same-family clients retain the authoritative terminal body verbatim, including future
    // output item fields, but unknown intermediate event types still fail closed.
    ensure_no_unknown_openai_responses_stream_events(&body_bytes, true)?;
    let terminal_body = validated_terminal_openai_responses_stream_response(&body_bytes)?;
    if terminal_body.get("status").and_then(Value::as_str) != Some("completed")
        || terminal_body
            .get("output")
            .and_then(Value::as_array)
            .is_some_and(|output| !output.is_empty())
    {
        return Ok(Some(client_body_with_report_context_model(
            terminal_body,
            report_context,
            &client_api_format,
        )));
    }
    Ok(
        aggregate_openai_responses_stream_sync_response_from_validated_terminal(
            &body_bytes,
            terminal_body,
        )
        .map(|body| {
            client_body_with_report_context_model(body, report_context, &client_api_format)
        }),
    )
}

fn validated_terminal_openai_responses_stream_response(
    body: &[u8],
) -> Result<Value, AiSurfaceFinalizeError> {
    let events = parse_stream_json_events(body).ok_or_else(|| {
        AiSurfaceFinalizeError::new("OpenAI Responses stream contains invalid event framing")
    })?;
    let mut terminal_response = None;

    for event in events {
        let event = event.as_object().ok_or_else(|| {
            AiSurfaceFinalizeError::new("OpenAI Responses stream contains a non-object event")
        })?;
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            continue;
        };
        let expected_status = match event_type {
            "response.completed" | "response.done" => "completed",
            "response.incomplete" => "incomplete",
            "response.failed" => "failed",
            _ => continue,
        };
        if terminal_response.is_some() {
            return Err(AiSurfaceFinalizeError::new(
                "OpenAI Responses stream contains multiple authoritative terminal events",
            ));
        }
        let mut response = event
            .get("response")
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| {
                AiSurfaceFinalizeError::new(
                    "OpenAI Responses terminal event is missing its response object",
                )
            })?;
        if response
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| status != expected_status)
        {
            return Err(AiSurfaceFinalizeError::new(
                "OpenAI Responses terminal event conflicts with response.status",
            ));
        }
        response
            .entry("status".to_string())
            .or_insert_with(|| Value::String(expected_status.to_string()));
        terminal_response = Some(Value::Object(response));
    }

    terminal_response.ok_or_else(|| {
        AiSurfaceFinalizeError::new(
            "OpenAI Responses stream is missing an authoritative terminal event",
        )
    })
}

fn maybe_build_openai_cross_format_provider_body_from_normalized_payload(
    body_json: Option<&Value>,
    body_base64: Option<&str>,
    provider_api_format: &str,
) -> Result<Option<OpenAiCrossFormatProviderBody>, AiSurfaceFinalizeError> {
    let aggregated_stream_body = match body_base64 {
        Some(body_base64) => {
            let body_bytes = base64::engine::general_purpose::STANDARD.decode(body_base64)?;
            let normalized_provider_api_format =
                normalize_openai_responses_family_api_format(provider_api_format);
            match normalized_provider_api_format.as_str() {
                "claude:messages" => try_aggregate_claude_stream_sync_response(&body_bytes)?,
                "gemini:generate_content" => {
                    try_aggregate_gemini_stream_sync_response(&body_bytes)?
                }
                "openai:responses" | "openai:responses:compact" => {
                    try_aggregate_openai_responses_stream_sync_response(&body_bytes)?
                }
                _ => None,
            }
        }
        None => None,
    };

    let aggregated_from_stream = aggregated_stream_body.is_some();
    let provider_body_json = aggregated_stream_body.or_else(|| body_json.cloned());
    Ok(provider_body_json
        .filter(|value| !is_error_like_sync_body(value))
        .map(|body_json| OpenAiCrossFormatProviderBody {
            body_json,
            aggregated_from_stream,
        }))
}

fn is_error_like_sync_body(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };

    object.get("error").is_some_and(|error| !error.is_null())
        || object.get("status").and_then(Value::as_str) == Some("failed")
        || object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "error")
        || object
            .get("chunks")
            .and_then(Value::as_array)
            .is_some_and(|chunks| {
                chunks.iter().any(|chunk| {
                    chunk.as_object().is_some_and(|chunk_object| {
                        chunk_object
                            .get("error")
                            .is_some_and(|error| !error.is_null())
                            || chunk_object
                                .get("type")
                                .and_then(Value::as_str)
                                .is_some_and(|value| value == "error")
                    })
                })
            })
}

fn gemini_generate_content_body_has_visible_output(value: &Value) -> bool {
    crate::formats::gemini::generate_content::response::from_raw(value).is_some()
}

fn api_format_is_gemini_generate_content(api_format: &str) -> bool {
    aether_ai_formats::normalize_api_format_alias(api_format) == "gemini:generate_content"
}

pub fn maybe_build_standard_cross_format_sync_product(
    report_kind: &str,
    provider_api_format: &str,
    client_api_format: &str,
    report_context: &Value,
    provider_body_json: Value,
) -> Option<StandardCrossFormatSyncProduct> {
    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();
    let client_api_format = client_api_format.trim().to_ascii_lowercase();

    if provider_api_format == "openai:image" && client_api_format == "gemini:generate_content" {
        let client_body_json = match (
            provider_body_json.get("data").and_then(Value::as_array),
            provider_body_json.get("output").and_then(Value::as_array),
        ) {
            (Some(_), None) => crate::formats::shared::image_bridge::build_gemini_image_response_from_openai_image_response(
                &provider_body_json,
                Some(report_context),
            )?,
            (None, Some(_)) => crate::formats::shared::image_bridge::build_gemini_image_response_from_openai_responses_image_response(
                &provider_body_json,
                Some(report_context),
            )?,
            _ => return None,
        };
        return Some(StandardCrossFormatSyncProduct {
            client_body_json,
            provider_body_json,
        });
    }

    let client_body_json = if is_standard_chat_finalize_kind(report_kind) {
        sync_chat_response_conversion_kind(&provider_api_format, &client_api_format)?;
        convert_standard_chat_response(
            &provider_body_json,
            &provider_api_format,
            &client_api_format,
            report_context,
        )?
    } else if is_standard_cli_finalize_kind(report_kind) {
        sync_cli_response_conversion_kind(&provider_api_format, &client_api_format)?;
        convert_standard_cli_response(
            &provider_body_json,
            &provider_api_format,
            &client_api_format,
            report_context,
        )?
    } else {
        return None;
    };
    let client_body_json =
        client_body_with_report_context_model(client_body_json, report_context, &client_api_format);

    Some(StandardCrossFormatSyncProduct {
        client_body_json,
        provider_body_json,
    })
}

pub fn aggregate_standard_chat_stream_sync_response(
    body: &[u8],
    provider_api_format: &str,
) -> Option<Value> {
    try_aggregate_standard_chat_stream_sync_response(body, provider_api_format)
        .ok()
        .flatten()
}

fn try_aggregate_standard_chat_stream_sync_response(
    body: &[u8],
    provider_api_format: &str,
) -> Result<Option<Value>, AiSurfaceFinalizeError> {
    match aether_ai_formats::normalize_api_format_alias(provider_api_format).as_str() {
        "openai:chat" => try_aggregate_openai_chat_stream_sync_response(body),
        "openai:responses" | "openai:responses:compact" => {
            try_aggregate_openai_responses_stream_sync_response(body)
        }
        "claude:messages" => try_aggregate_claude_stream_sync_response(body),
        "gemini:generate_content" => try_aggregate_gemini_stream_sync_response(body),
        _ => Ok(None),
    }
}

pub fn convert_standard_chat_response(
    body_json: &Value,
    provider_api_format: &str,
    client_api_format: &str,
    report_context: &Value,
) -> Option<Value> {
    match convert_response(
        provider_api_format,
        client_api_format,
        body_json,
        &format_context_from_report_context(report_context),
    ) {
        Ok(converted) => return Some(converted),
        Err(error) if response_conversion_error_requires_fail_closed(&error) => return None,
        Err(_) => {}
    }

    if matches!(
        provider_api_format.trim().to_ascii_lowercase().as_str(),
        "openai:chat"
    ) {
        return convert_openai_chat_canonical_chat_response(
            body_json,
            client_api_format,
            report_context,
        );
    }
    if matches!(
        provider_api_format.trim().to_ascii_lowercase().as_str(),
        "claude:messages"
    ) {
        return convert_claude_canonical_chat_response(
            body_json,
            client_api_format,
            report_context,
        );
    }
    if matches!(
        provider_api_format.trim().to_ascii_lowercase().as_str(),
        "gemini:generate_content"
    ) {
        return convert_gemini_canonical_chat_response(
            body_json,
            client_api_format,
            report_context,
        );
    }

    match normalize_openai_responses_family_api_format(provider_api_format).as_str() {
        "openai:responses" | "openai:responses:compact" => {
            convert_openai_responses_canonical_chat_response(
                body_json,
                client_api_format,
                report_context,
            )
        }
        _ => None,
    }
}

pub fn aggregate_standard_cli_stream_sync_response(
    body: &[u8],
    provider_api_format: &str,
) -> Option<Value> {
    aggregate_standard_chat_stream_sync_response(body, provider_api_format)
}

fn try_aggregate_standard_cli_stream_sync_response(
    body: &[u8],
    provider_api_format: &str,
) -> Result<Option<Value>, AiSurfaceFinalizeError> {
    try_aggregate_standard_chat_stream_sync_response(body, provider_api_format)
}

pub fn convert_standard_cli_response(
    body_json: &Value,
    provider_api_format: &str,
    client_api_format: &str,
    report_context: &Value,
) -> Option<Value> {
    match convert_response(
        provider_api_format,
        client_api_format,
        body_json,
        &format_context_from_report_context(report_context),
    ) {
        Ok(converted) => return Some(converted),
        Err(error) if response_conversion_error_requires_fail_closed(&error) => return None,
        Err(_) => {}
    }

    if matches!(
        provider_api_format.trim().to_ascii_lowercase().as_str(),
        "openai:chat"
    ) {
        return convert_openai_chat_canonical_cli_response(
            body_json,
            client_api_format,
            report_context,
        );
    }
    if matches!(
        provider_api_format.trim().to_ascii_lowercase().as_str(),
        "claude:messages"
    ) {
        return convert_claude_canonical_cli_response(body_json, client_api_format, report_context);
    }
    if matches!(
        provider_api_format.trim().to_ascii_lowercase().as_str(),
        "gemini:generate_content"
    ) {
        return convert_gemini_canonical_cli_response(body_json, client_api_format, report_context);
    }

    let canonical = match normalize_openai_responses_family_api_format(provider_api_format).as_str()
    {
        "openai:responses" | "openai:responses:compact" => {
            return convert_openai_responses_canonical_responses_response(
                body_json,
                client_api_format,
                report_context,
            )
        }
        _ => convert_standard_chat_response(
            body_json,
            provider_api_format,
            "openai:chat",
            report_context,
        )?,
    };

    match normalize_openai_responses_family_api_format(client_api_format).as_str() {
        "openai:responses" => {
            convert_openai_chat_response_to_openai_responses(&canonical, report_context, false)
        }
        "openai:responses:compact" => {
            convert_openai_chat_response_to_openai_responses(&canonical, report_context, true)
        }
        "claude:messages" => {
            convert_openai_chat_response_to_claude_chat(&canonical, report_context)
        }
        "gemini:generate_content" => {
            convert_openai_chat_response_to_gemini_chat(&canonical, report_context)
        }
        _ => None,
    }
}

fn response_conversion_error_requires_fail_closed(error: &FormatError) -> bool {
    matches!(
        error,
        FormatError::UnsupportedField { .. }
            | FormatError::UnauditedField { .. }
            | FormatError::InvalidEnumValue { .. }
            | FormatError::LossyConversionBlocked { .. }
            | FormatError::InvalidTargetField { .. }
    )
}

fn format_context_from_report_context(report_context: &Value) -> FormatContext {
    let mut context = FormatContext::default().with_report_context(report_context.clone());
    if let Some(mapped_model) = report_context
        .get("mapped_model")
        .and_then(Value::as_str)
        .or_else(|| report_context.get("model").and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
    {
        context = context.with_mapped_model(mapped_model);
    }
    context
}

fn client_body_with_report_context_model(
    mut body_json: Value,
    report_context: &Value,
    client_api_format: &str,
) -> Value {
    let Some(display_model) = model_directive_display_model_from_report_context(report_context)
    else {
        return body_json;
    };
    let Some(object) = body_json.as_object_mut() else {
        return body_json;
    };
    match normalize_openai_responses_family_api_format(client_api_format).as_str() {
        "openai:chat" | "openai:responses" | "openai:responses:compact" | "claude:messages" => {
            object.insert("model".to_string(), Value::String(display_model));
        }
        "openai:embedding" | "jina:embedding" => {
            object.insert("model".to_string(), Value::String(display_model));
        }
        "gemini:generate_content" => {
            object.insert("modelVersion".to_string(), Value::String(display_model));
        }
        _ => {}
    }
    body_json
}

fn convert_openai_chat_canonical_chat_response(
    body_json: &Value,
    client_api_format: &str,
    report_context: &Value,
) -> Option<Value> {
    if !openai_chat_response_can_use_single_response_canonical(body_json) {
        return match client_api_format.trim().to_ascii_lowercase().as_str() {
            "openai:chat" => Some(body_json.clone()),
            "claude:messages" => {
                convert_openai_chat_response_to_claude_chat(body_json, report_context)
            }
            "gemini:generate_content" => {
                convert_openai_chat_response_to_gemini_chat(body_json, report_context)
            }
            _ => None,
        };
    }
    match client_api_format.trim().to_ascii_lowercase().as_str() {
        "openai:chat" => Some(body_json.clone()),
        "claude:messages" => {
            let openai_chat =
                convert_openai_chat_canonical_response_to_openai_chat(body_json, report_context)?;
            convert_openai_chat_response_to_claude_chat(&openai_chat, report_context)
        }
        "gemini:generate_content" => {
            let openai_chat =
                convert_openai_chat_canonical_response_to_openai_chat(body_json, report_context)?;
            convert_openai_chat_response_to_gemini_chat(&openai_chat, report_context)
        }
        _ => None,
    }
}

fn convert_openai_chat_canonical_cli_response(
    body_json: &Value,
    client_api_format: &str,
    report_context: &Value,
) -> Option<Value> {
    if !openai_chat_response_can_use_single_response_canonical(body_json) {
        return match normalize_openai_responses_family_api_format(client_api_format).as_str() {
            "openai:responses" => {
                convert_openai_chat_response_to_openai_responses(body_json, report_context, false)
            }
            "openai:responses:compact" => {
                convert_openai_chat_response_to_openai_responses(body_json, report_context, true)
            }
            "claude:messages" => {
                convert_openai_chat_response_to_claude_chat(body_json, report_context)
            }
            "gemini:generate_content" => {
                convert_openai_chat_response_to_gemini_chat(body_json, report_context)
            }
            _ => None,
        };
    }
    let openai_chat =
        convert_openai_chat_canonical_response_to_openai_chat(body_json, report_context)?;
    match normalize_openai_responses_family_api_format(client_api_format).as_str() {
        "openai:responses" => {
            convert_openai_chat_response_to_openai_responses(&openai_chat, report_context, false)
        }
        "openai:responses:compact" => {
            convert_openai_chat_response_to_openai_responses(&openai_chat, report_context, true)
        }
        "claude:messages" => {
            convert_openai_chat_response_to_claude_chat(&openai_chat, report_context)
        }
        "gemini:generate_content" => {
            convert_openai_chat_response_to_gemini_chat(&openai_chat, report_context)
        }
        _ => None,
    }
}

fn convert_openai_chat_canonical_response_to_openai_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    let mut canonical = from_openai_chat_to_canonical_response(body_json)?;
    apply_report_context_model_fallback(&mut canonical.model, report_context);
    Some(canonical_to_openai_chat_response(&canonical))
}

fn project_validated_openai_responses_stream_to_openai_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    // The caller retains body_json as provider_body_json. This projection is therefore allowed
    // to omit provider-only response metadata, but never unknown canonical output blocks.
    if !matches!(
        body_json.get("status").and_then(Value::as_str),
        Some("completed" | "incomplete")
    ) {
        return None;
    }

    let mut canonical = from_openai_responses_to_canonical_response(body_json)?;
    if canonical_response_unknown_block_count(&canonical) > 0 {
        return None;
    }

    apply_report_context_model_fallback(&mut canonical.model, report_context);
    Some(canonical_to_openai_chat_response(&canonical))
}

fn openai_chat_response_can_use_single_response_canonical(body_json: &Value) -> bool {
    body_json
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| choices.len() <= 1)
}

fn convert_openai_responses_canonical_chat_response(
    body_json: &Value,
    client_api_format: &str,
    report_context: &Value,
) -> Option<Value> {
    let canonical = from_openai_responses_to_canonical_response(body_json).map(|mut canonical| {
        apply_report_context_model_fallback(&mut canonical.model, report_context);
        canonical
    });
    match client_api_format.trim().to_ascii_lowercase().as_str() {
        "openai:chat" => canonical
            .as_ref()
            .map(canonical_to_openai_chat_response)
            .or_else(|| {
                convert_openai_responses_response_to_openai_chat(body_json, report_context)
            }),
        "claude:messages" => canonical
            .as_ref()
            .map(canonical_to_claude_response)
            .or_else(|| {
                let openai_chat =
                    convert_openai_responses_response_to_openai_chat(body_json, report_context)?;
                convert_openai_chat_response_to_claude_chat(&openai_chat, report_context)
            }),
        "gemini:generate_content" => canonical
            .as_ref()
            .and_then(|canonical| canonical_to_gemini_response(canonical, report_context))
            .or_else(|| {
                let openai_chat =
                    convert_openai_responses_response_to_openai_chat(body_json, report_context)?;
                convert_openai_chat_response_to_gemini_chat(&openai_chat, report_context)
            }),
        _ => None,
    }
}

fn convert_openai_responses_canonical_responses_response(
    body_json: &Value,
    client_api_format: &str,
    report_context: &Value,
) -> Option<Value> {
    match normalize_openai_responses_family_api_format(client_api_format).as_str() {
        "openai:responses" => convert_openai_responses_canonical_response_to_openai_responses(
            body_json,
            report_context,
            false,
        )
        .or_else(|| {
            let openai_chat =
                convert_openai_responses_response_to_openai_chat(body_json, report_context)?;
            convert_openai_chat_response_to_openai_responses(&openai_chat, report_context, false)
        }),
        "openai:responses:compact" => {
            convert_openai_responses_canonical_response_to_openai_responses(
                body_json,
                report_context,
                true,
            )
            .or_else(|| {
                let openai_chat =
                    convert_openai_responses_response_to_openai_chat(body_json, report_context)?;
                convert_openai_chat_response_to_openai_responses(&openai_chat, report_context, true)
            })
        }
        "claude:messages" => {
            let canonical =
                from_openai_responses_to_canonical_response(body_json).map(|mut canonical| {
                    apply_report_context_model_fallback(&mut canonical.model, report_context);
                    canonical
                });
            canonical
                .as_ref()
                .map(canonical_to_claude_response)
                .or_else(|| {
                    let openai_chat = convert_openai_responses_response_to_openai_chat(
                        body_json,
                        report_context,
                    )?;
                    convert_openai_chat_response_to_claude_chat(&openai_chat, report_context)
                })
        }
        "gemini:generate_content" => {
            let canonical =
                from_openai_responses_to_canonical_response(body_json).map(|mut canonical| {
                    apply_report_context_model_fallback(&mut canonical.model, report_context);
                    canonical
                });
            canonical
                .as_ref()
                .and_then(|canonical| canonical_to_gemini_response(canonical, report_context))
                .or_else(|| {
                    let openai_chat = convert_openai_responses_response_to_openai_chat(
                        body_json,
                        report_context,
                    )?;
                    convert_openai_chat_response_to_gemini_chat(&openai_chat, report_context)
                })
        }
        _ => None,
    }
}

fn convert_openai_responses_canonical_response_to_openai_responses(
    body_json: &Value,
    report_context: &Value,
    compact: bool,
) -> Option<Value> {
    let mut canonical = from_openai_responses_to_canonical_response(body_json)?;
    apply_report_context_model_fallback(&mut canonical.model, report_context);
    Some(if compact {
        canonical_to_openai_responses_compact_response(&canonical, report_context)
    } else {
        canonical_to_openai_responses_response(&canonical, report_context)
    })
}

fn convert_claude_canonical_chat_response(
    body_json: &Value,
    client_api_format: &str,
    report_context: &Value,
) -> Option<Value> {
    match client_api_format.trim().to_ascii_lowercase().as_str() {
        "openai:chat" => {
            convert_claude_canonical_response_to_openai_chat(body_json, report_context)
        }
        "claude:messages" => convert_claude_canonical_response_to_claude(body_json, report_context),
        "gemini:generate_content" => {
            let openai_chat =
                convert_claude_canonical_response_to_openai_chat(body_json, report_context)?;
            convert_openai_chat_response_to_gemini_chat(&openai_chat, report_context)
        }
        _ => None,
    }
}

fn convert_claude_canonical_cli_response(
    body_json: &Value,
    client_api_format: &str,
    report_context: &Value,
) -> Option<Value> {
    match normalize_openai_responses_family_api_format(client_api_format).as_str() {
        "openai:responses" => {
            let openai_chat =
                convert_claude_canonical_response_to_openai_chat(body_json, report_context)?;
            convert_openai_chat_response_to_openai_responses(&openai_chat, report_context, false)
        }
        "openai:responses:compact" => {
            let openai_chat =
                convert_claude_canonical_response_to_openai_chat(body_json, report_context)?;
            convert_openai_chat_response_to_openai_responses(&openai_chat, report_context, true)
        }
        "claude:messages" => convert_claude_canonical_response_to_claude(body_json, report_context),
        "gemini:generate_content" => {
            let openai_chat =
                convert_claude_canonical_response_to_openai_chat(body_json, report_context)?;
            convert_openai_chat_response_to_gemini_chat(&openai_chat, report_context)
        }
        _ => None,
    }
}

fn convert_claude_canonical_response_to_openai_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    let mut canonical = from_claude_to_canonical_response(body_json)?;
    apply_report_context_model_fallback(&mut canonical.model, report_context);
    let mut response = canonical_to_openai_chat_response(&canonical);
    if let Some(service_tier) = report_context
        .get("original_request_body")
        .and_then(Value::as_object)
        .and_then(|request| request.get("service_tier"))
        .cloned()
    {
        response["service_tier"] = service_tier;
    }
    Some(response)
}

fn convert_claude_canonical_response_to_claude(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    let mut canonical = from_claude_to_canonical_response(body_json)?;
    apply_report_context_model_fallback(&mut canonical.model, report_context);
    Some(canonical_to_claude_response(&canonical))
}

fn convert_gemini_canonical_chat_response(
    body_json: &Value,
    client_api_format: &str,
    report_context: &Value,
) -> Option<Value> {
    if !gemini_response_can_use_single_response_canonical(body_json) {
        let openai_chat = convert_gemini_chat_response_to_openai_chat(body_json, report_context)?;
        return match client_api_format.trim().to_ascii_lowercase().as_str() {
            "openai:chat" => Some(openai_chat),
            "claude:messages" => {
                convert_openai_chat_response_to_claude_chat(&openai_chat, report_context)
            }
            "gemini:generate_content" => {
                convert_openai_chat_response_to_gemini_chat(&openai_chat, report_context)
            }
            _ => None,
        };
    }
    match client_api_format.trim().to_ascii_lowercase().as_str() {
        "openai:chat" => {
            convert_gemini_canonical_response_to_openai_chat(body_json, report_context)
        }
        "claude:messages" => {
            let openai_chat =
                convert_gemini_canonical_response_to_openai_chat(body_json, report_context)?;
            convert_openai_chat_response_to_claude_chat(&openai_chat, report_context)
        }
        "gemini:generate_content" => {
            convert_gemini_canonical_response_to_gemini(body_json, report_context)
        }
        _ => None,
    }
}

fn convert_gemini_canonical_cli_response(
    body_json: &Value,
    client_api_format: &str,
    report_context: &Value,
) -> Option<Value> {
    if !gemini_response_can_use_single_response_canonical(body_json) {
        let openai_chat = convert_gemini_chat_response_to_openai_chat(body_json, report_context)?;
        return match normalize_openai_responses_family_api_format(client_api_format).as_str() {
            "openai:responses" => convert_openai_chat_response_to_openai_responses(
                &openai_chat,
                report_context,
                false,
            ),
            "openai:responses:compact" => {
                convert_openai_chat_response_to_openai_responses(&openai_chat, report_context, true)
            }
            "claude:messages" => {
                convert_openai_chat_response_to_claude_chat(&openai_chat, report_context)
            }
            "gemini:generate_content" => {
                convert_openai_chat_response_to_gemini_chat(&openai_chat, report_context)
            }
            _ => None,
        };
    }
    match normalize_openai_responses_family_api_format(client_api_format).as_str() {
        "openai:responses" => {
            convert_gemini_canonical_response_to_openai_responses(body_json, report_context, false)
        }
        "openai:responses:compact" => {
            convert_gemini_canonical_response_to_openai_responses(body_json, report_context, true)
        }
        "claude:messages" => {
            let openai_chat =
                convert_gemini_canonical_response_to_openai_chat(body_json, report_context)?;
            convert_openai_chat_response_to_claude_chat(&openai_chat, report_context)
        }
        "gemini:generate_content" => {
            convert_gemini_canonical_response_to_gemini(body_json, report_context)
        }
        _ => None,
    }
}

fn convert_gemini_canonical_response_to_openai_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    if !gemini_response_can_use_single_response_canonical(body_json) {
        return convert_gemini_chat_response_to_openai_chat(body_json, report_context);
    }
    let mut canonical = from_gemini_to_canonical_response(body_json)?;
    apply_report_context_model_fallback(&mut canonical.model, report_context);
    Some(canonical_to_openai_chat_response(&canonical))
}

fn convert_gemini_canonical_response_to_openai_responses(
    body_json: &Value,
    report_context: &Value,
    compact: bool,
) -> Option<Value> {
    let openai_chat = convert_gemini_canonical_response_to_openai_chat(body_json, report_context)?;
    convert_openai_chat_response_to_openai_responses(&openai_chat, report_context, compact)
}

fn convert_gemini_canonical_response_to_gemini(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    if !gemini_response_can_use_single_response_canonical(body_json) {
        let openai_chat = convert_gemini_chat_response_to_openai_chat(body_json, report_context)?;
        return convert_openai_chat_response_to_gemini_chat(&openai_chat, report_context);
    }
    let mut canonical = from_gemini_to_canonical_response(body_json)?;
    apply_report_context_model_fallback(&mut canonical.model, report_context);
    canonical_to_gemini_response(&canonical, report_context)
}

fn gemini_response_can_use_single_response_canonical(body_json: &Value) -> bool {
    body_json
        .get("candidates")
        .and_then(Value::as_array)
        .is_some_and(|candidates| candidates.len() <= 1)
}

fn apply_report_context_model_fallback(model: &mut String, report_context: &Value) {
    if let Some(display_model) = model_directive_display_model_from_report_context(report_context) {
        *model = display_model;
        return;
    }
    if model != "unknown" && !model.trim().is_empty() {
        return;
    }
    if let Some(fallback) = report_context
        .get("mapped_model")
        .and_then(Value::as_str)
        .or_else(|| report_context.get("model").and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
    {
        *model = fallback.to_string();
    }
}

#[derive(Debug, Default)]
struct OpenAIChatChoiceState {
    role: Option<String>,
    content: String,
    finish_reason: Option<String>,
    tool_calls: BTreeMap<usize, OpenAIChatToolCallState>,
}

#[derive(Debug, Default)]
struct OpenAIChatToolCallState {
    id: Option<String>,
    tool_type: Option<String>,
    function_name: Option<String>,
    function_arguments: String,
}

#[derive(Debug, Default)]
struct ClaudeContentBlockState {
    object: Map<String, Value>,
    text: String,
    signature: Option<String>,
    partial_json: String,
}

fn is_standard_chat_finalize_kind(report_kind: &str) -> bool {
    matches!(
        report_kind,
        "openai_chat_sync_finalize" | "claude_chat_sync_finalize" | "gemini_chat_sync_finalize"
    )
}

fn is_standard_cli_finalize_kind(report_kind: &str) -> bool {
    matches!(
        report_kind,
        "openai_responses_sync_finalize"
            | "openai_responses_compact_sync_finalize"
            | "openai_cli_sync_finalize"
            | "openai_compact_sync_finalize"
            | "claude_cli_sync_finalize"
            | "gemini_cli_sync_finalize"
    )
}

fn is_openai_responses_finalize_kind(report_kind: &str) -> bool {
    matches!(
        report_kind,
        "openai_responses_sync_finalize"
            | "openai_responses_compact_sync_finalize"
            | "openai_cli_sync_finalize"
            | "openai_compact_sync_finalize"
    )
}

fn standard_same_format_api_format(report_kind: &str) -> Option<&'static str> {
    match report_kind {
        "openai_chat_sync_finalize" => Some("openai:chat"),
        "claude_chat_sync_finalize" => Some("claude:messages"),
        "gemini_chat_sync_finalize" => Some("gemini:generate_content"),
        "openai_embedding_sync_finalize" => Some("openai:embedding"),
        "claude_cli_sync_finalize" => Some("claude:messages"),
        "gemini_cli_sync_finalize" => Some("gemini:generate_content"),
        _ => None,
    }
}

fn provider_stream_event_api_format_for_report_context(
    report_context: &Value,
    provider_api_format: &str,
) -> String {
    report_context_api_format_field(report_context, "provider_stream_event_api_format")
        .or_else(|| report_context_api_format_field(report_context, "provider_stream_api_format"))
        .unwrap_or_else(|| provider_api_format.trim().to_ascii_lowercase())
}

fn report_context_api_format_field(report_context: &Value, key: &str) -> Option<String> {
    let value = report_context.get(key)?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_ascii_lowercase())
}

fn api_formats_match(left: &str, right: &str) -> bool {
    aether_ai_formats::normalize_api_format_alias(left)
        == aether_ai_formats::normalize_api_format_alias(right)
}

fn convert_aggregated_stream_body_to_client_sync_response(
    report_kind: &str,
    body: Value,
    provider_stream_event_api_format: &str,
    client_api_format: &str,
    report_context: &Value,
) -> Option<Value> {
    if api_formats_match(provider_stream_event_api_format, client_api_format) {
        return Some(body);
    }
    if is_standard_chat_finalize_kind(report_kind) {
        return convert_standard_chat_response(
            &body,
            provider_stream_event_api_format,
            client_api_format,
            report_context,
        );
    }
    if is_standard_cli_finalize_kind(report_kind) {
        return convert_standard_cli_response(
            &body,
            provider_stream_event_api_format,
            client_api_format,
            report_context,
        );
    }
    None
}

fn is_openai_responses_family_api_format(api_format: &str) -> bool {
    matches!(
        normalize_openai_responses_family_api_format(api_format).as_str(),
        "openai:responses" | "openai:responses:compact"
    )
}

fn normalize_openai_responses_family_api_format(api_format: &str) -> String {
    aether_ai_formats::normalize_api_format_alias(api_format)
}

fn parse_stream_json_events(body: &[u8]) -> Option<Vec<Value>> {
    let text = std::str::from_utf8(body).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Some(Vec::new());
    }

    if trimmed.starts_with('[') {
        let array_value: Value = serde_json::from_str(trimmed).ok()?;
        let array = array_value.as_array()?;
        return Some(
            array
                .iter()
                .filter(|value| value.is_object())
                .cloned()
                .collect(),
        );
    }

    let mut events = Vec::new();
    let mut current_event_type: Option<String> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim_matches('\r').trim();
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(event_name) = line.strip_prefix("event:") {
            current_event_type = Some(event_name.trim().to_string());
            continue;
        }
        let data_line = if let Some(rest) = line.strip_prefix("data:") {
            rest.trim()
        } else {
            line
        };
        if data_line.is_empty() || data_line == "[DONE]" {
            continue;
        }

        let mut event: Value = serde_json::from_str(data_line).ok()?;
        if let Some(event_object) = event.as_object_mut() {
            if !event_object.contains_key("type") {
                if let Some(event_name) = current_event_type.take() {
                    event_object.insert("type".to_string(), Value::String(event_name));
                }
            }
        }
        events.push(event);
        current_event_type = None;
    }

    Some(events)
}

fn try_aggregate_openai_chat_stream_sync_response(
    body: &[u8],
) -> Result<Option<Value>, AiSurfaceFinalizeError> {
    let report_context = Value::Object(Map::new());
    let mut provider = OpenAIChatProviderState::default();
    ensure_no_unknown_provider_stream_events(body, |line| {
        provider.push_line(&report_context, line)
    })?;
    Ok(aggregate_openai_chat_stream_sync_response(body))
}

fn try_aggregate_openai_responses_stream_sync_response(
    body: &[u8],
) -> Result<Option<Value>, AiSurfaceFinalizeError> {
    ensure_no_unknown_openai_responses_stream_events(body, false)?;
    let terminal_response = validated_terminal_openai_responses_stream_response(body)?;
    Ok(
        aggregate_openai_responses_stream_sync_response_from_validated_terminal(
            body,
            terminal_response,
        ),
    )
}

fn ensure_no_unknown_openai_responses_stream_events(
    body: &[u8],
    allow_authoritative_terminal_output_extensions: bool,
) -> Result<(), AiSurfaceFinalizeError> {
    let Ok(text) = std::str::from_utf8(body) else {
        return Ok(());
    };
    let report_context = Value::Object(Map::new());
    let mut provider = OpenAIResponsesProviderState::default();
    let mut declared_event_type: Option<String> = None;
    for raw_line in text.lines() {
        let frames = provider.push_line(&report_context, raw_line.as_bytes().to_vec())?;
        let raw_payload = raw_line.trim();
        if let Some(event_type) = raw_payload.strip_prefix("event:") {
            declared_event_type = Some(event_type.trim().to_string());
            continue;
        }
        let payload = raw_payload
            .strip_prefix("data:")
            .map(str::trim)
            .unwrap_or(raw_payload);
        let declared_event_type = declared_event_type.take();
        let payload_event_type = serde_json::from_str::<Value>(payload)
            .ok()
            .and_then(|payload| {
                payload
                    .get("type")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            });
        if matches!(
            (declared_event_type.as_deref(), payload_event_type.as_deref()),
            (Some(declared), Some(payload)) if declared != payload
        ) {
            return Err(AiSurfaceFinalizeError::new(
                "OpenAI Responses SSE event name conflicts with payload.type",
            ));
        }
        let event_type = payload_event_type.or(declared_event_type);
        let terminal_output_extensions_are_lossless = allow_authoritative_terminal_output_extensions
            && event_type.as_deref().is_some_and(|event_type| {
                matches!(
                    event_type,
                    "response.completed"
                        | "response.done"
                        | "response.incomplete"
                        | "response.failed"
                )
            });
        if let Some(payload) = frames.iter().find_map(|frame| match &frame.event {
            CanonicalStreamEvent::UnknownEvent(payload)
                if payload.get("type").and_then(Value::as_str) != Some("response.failed")
                    && !terminal_output_extensions_are_lossless =>
            {
                Some(payload)
            }
            _ => None,
        }) {
            return Err(unsupported_stream_event_finalize_error(payload));
        }
    }
    Ok(())
}

fn try_aggregate_claude_stream_sync_response(
    body: &[u8],
) -> Result<Option<Value>, AiSurfaceFinalizeError> {
    let report_context = Value::Object(Map::new());
    let mut provider = ClaudeProviderState::default();
    ensure_no_unknown_provider_stream_events(body, |line| {
        provider.push_line(&report_context, line)
    })?;
    Ok(aggregate_claude_stream_sync_response(body))
}

fn ensure_no_unknown_provider_stream_events(
    body: &[u8],
    mut push_line: impl FnMut(Vec<u8>) -> Result<Vec<CanonicalStreamFrame>, AiSurfaceFinalizeError>,
) -> Result<(), AiSurfaceFinalizeError> {
    let Ok(text) = std::str::from_utf8(body) else {
        return Ok(());
    };
    for raw_line in text.lines() {
        let frames = push_line(raw_line.as_bytes().to_vec())?;
        if let Some(payload) = frames.iter().find_map(|frame| match &frame.event {
            CanonicalStreamEvent::UnknownEvent(payload) => Some(payload),
            _ => None,
        }) {
            return Err(unsupported_stream_event_finalize_error(payload));
        }
    }
    Ok(())
}

pub fn aggregate_openai_chat_stream_sync_response(body: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(body).ok()?;
    let mut response_id: Option<String> = None;
    let mut model: Option<String> = None;
    let mut created: Option<u64> = None;
    let mut usage: Option<Value> = None;
    let mut choices: BTreeMap<usize, OpenAIChatChoiceState> = BTreeMap::new();
    let mut saw_chunk = false;

    for raw_line in text.lines() {
        let line = raw_line.trim_matches('\r').trim();
        if line.is_empty() || line.starts_with(':') || line.starts_with("event:") {
            continue;
        }

        let Some(data_line) = line.strip_prefix("data:") else {
            continue;
        };
        let data_line = data_line.trim();
        if data_line.is_empty() || data_line == "[DONE]" {
            continue;
        }

        let chunk: Value = serde_json::from_str(data_line).ok()?;
        let chunk_object = chunk.as_object()?;
        saw_chunk = true;

        if response_id.is_none() {
            response_id = chunk_object
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
        }
        if model.is_none() {
            model = chunk_object
                .get("model")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
        }
        if created.is_none() {
            created = chunk_object.get("created").and_then(Value::as_u64);
        }
        if let Some(u) = chunk_object.get("usage") {
            usage = Some(u.clone());
        }

        let Some(chunk_choices) = chunk_object.get("choices").and_then(Value::as_array) else {
            continue;
        };
        for chunk_choice in chunk_choices {
            let Some(choice_object) = chunk_choice.as_object() else {
                continue;
            };
            let Some(index) = choice_object
                .get("index")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
            else {
                continue;
            };
            let state = choices.entry(index).or_default();
            if let Some(finish_reason) = choice_object.get("finish_reason").and_then(Value::as_str)
            {
                state.finish_reason = Some(finish_reason.to_string());
            }

            let Some(delta) = choice_object.get("delta").and_then(Value::as_object) else {
                continue;
            };
            if let Some(role) = delta.get("role").and_then(Value::as_str) {
                state.role = Some(role.to_string());
            }
            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                state.content.push_str(content);
            }
            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for tool_call in tool_calls {
                    let Some(tool_call_object) = tool_call.as_object() else {
                        continue;
                    };
                    let tool_index = tool_call_object
                        .get("index")
                        .and_then(Value::as_u64)
                        .map(|value| value as usize)
                        .unwrap_or(0);
                    let tool_state = state.tool_calls.entry(tool_index).or_default();
                    if let Some(id) = tool_call_object.get("id").and_then(Value::as_str) {
                        tool_state.id = Some(id.to_string());
                    }
                    if let Some(tool_type) = tool_call_object.get("type").and_then(Value::as_str) {
                        tool_state.tool_type = Some(tool_type.to_string());
                    }
                    if let Some(function) =
                        tool_call_object.get("function").and_then(Value::as_object)
                    {
                        if let Some(name) = function.get("name").and_then(Value::as_str) {
                            tool_state.function_name = Some(name.to_string());
                        }
                        if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                            tool_state.function_arguments.push_str(arguments);
                        }
                    }
                }
            }
        }
    }

    if !saw_chunk {
        return None;
    }

    let mut response_object = Map::new();
    response_object.insert(
        "id".to_string(),
        Value::String(response_id.unwrap_or_else(|| "chatcmpl-local-finalize".to_string())),
    );
    response_object.insert(
        "object".to_string(),
        Value::String("chat.completion".to_string()),
    );
    if let Some(created) = created {
        response_object.insert("created".to_string(), Value::Number(created.into()));
    }
    if let Some(model) = model {
        response_object.insert("model".to_string(), Value::String(model));
    }

    let mut response_choices = Vec::with_capacity(choices.len());
    for (index, state) in choices {
        let mut message = Map::new();
        message.insert(
            "role".to_string(),
            Value::String(state.role.unwrap_or_else(|| "assistant".to_string())),
        );
        if state.tool_calls.is_empty() {
            message.insert("content".to_string(), Value::String(state.content));
        } else {
            if state.content.is_empty() {
                message.insert("content".to_string(), Value::Null);
            } else {
                message.insert("content".to_string(), Value::String(state.content));
            }
            let tool_calls = state
                .tool_calls
                .into_iter()
                .map(|(tool_index, tool_state)| {
                    json!({
                        "index": tool_index,
                        "id": tool_state.id,
                        "type": tool_state.tool_type.unwrap_or_else(|| "function".to_string()),
                        "function": {
                            "name": tool_state.function_name,
                            "arguments": tool_state.function_arguments,
                        },
                    })
                })
                .collect::<Vec<_>>();
            message.insert("tool_calls".to_string(), Value::Array(tool_calls));
        }

        response_choices.push(json!({
            "index": index,
            "message": Value::Object(message),
            "finish_reason": state.finish_reason,
        }));
    }
    response_object.insert("choices".to_string(), Value::Array(response_choices));
    if let Some(usage) = usage {
        response_object.insert("usage".to_string(), usage);
    }

    Some(Value::Object(response_object))
}

pub fn aggregate_openai_responses_stream_sync_response(body: &[u8]) -> Option<Value> {
    try_aggregate_openai_responses_stream_sync_response(body)
        .ok()
        .flatten()
}

fn aggregate_openai_responses_stream_sync_response_from_validated_terminal(
    body: &[u8],
    terminal_response: Value,
) -> Option<Value> {
    let events = parse_stream_json_events(body)?;
    if events.is_empty() {
        return None;
    }

    let mut response = terminal_response.as_object()?.clone();
    let mut response_id: Option<String> = None;
    let mut model: Option<String> = None;
    let mut message_states: BTreeMap<usize, OpenAIResponsesSyncMessageState> = BTreeMap::new();
    let mut reasoning_states: BTreeMap<usize, OpenAIResponsesSyncReasoningState> = BTreeMap::new();
    let mut tool_states: BTreeMap<usize, OpenAIResponsesSyncToolState> = BTreeMap::new();
    let mut image_items: BTreeMap<usize, Value> = BTreeMap::new();
    let mut opaque_items: BTreeMap<usize, Value> = BTreeMap::new();
    let mut item_output_indexes = BTreeMap::<String, usize>::new();
    let mut generic_output_indexes = BTreeMap::<String, usize>::new();
    let mut next_output_index = 0_usize;

    for event in events {
        let event_object = event.as_object()?;
        if let Some(response) = event_object.get("response").and_then(Value::as_object) {
            response_id = response_id.or_else(|| {
                response
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            });
            model = model.or_else(|| {
                response
                    .get("model")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            });
        }

        match event_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "response.created" | "response.in_progress" => {}
            "response.output_text.delta" | "response.outtext.delta" => {
                let output_index = openai_responses_event_output_index(event_object).unwrap_or(0);
                let content_index = openai_responses_event_content_index(event_object);
                match event_object.get("delta") {
                    Some(Value::String(delta)) => {
                        append_openai_responses_message_text_delta(
                            message_states.entry(output_index).or_default(),
                            content_index,
                            delta,
                        );
                    }
                    Some(Value::Object(delta)) => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            merge_openai_responses_message_text_delta_object(
                                message_states.entry(output_index).or_default(),
                                content_index,
                                text,
                            );
                        }
                    }
                    _ => {}
                }
            }
            "response.output_text.done" => {
                let output_index = openai_responses_event_output_index(event_object).unwrap_or(0);
                let content_index = openai_responses_event_content_index(event_object);
                let part = event_object.get("part").and_then(Value::as_object);
                let text = event_object
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        event_object
                            .get("part")
                            .and_then(Value::as_object)
                            .and_then(|part| part.get("text"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or_default();
                merge_openai_responses_message_text_part(
                    message_states.entry(output_index).or_default(),
                    content_index,
                    text,
                    part,
                );
            }
            "response.output_text.annotation.added" => {
                let output_index = openai_responses_event_output_index(event_object).unwrap_or(0);
                let content_index = openai_responses_event_content_index(event_object);
                merge_openai_responses_message_text_annotation(
                    message_states.entry(output_index).or_default(),
                    content_index,
                    event_object,
                );
            }
            "response.refusal.delta" => {
                let output_index = openai_responses_event_output_index(event_object).unwrap_or(0);
                let content_index = openai_responses_event_content_index(event_object);
                let delta = event_object
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                append_openai_responses_message_refusal_delta(
                    message_states.entry(output_index).or_default(),
                    content_index,
                    delta,
                );
            }
            "response.refusal.done" => {
                let output_index = openai_responses_event_output_index(event_object).unwrap_or(0);
                let content_index = openai_responses_event_content_index(event_object);
                let part = event_object.get("part").and_then(Value::as_object);
                let refusal = event_object
                    .get("refusal")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        event_object
                            .get("part")
                            .and_then(Value::as_object)
                            .and_then(|part| part.get("refusal"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or_default();
                merge_openai_responses_message_refusal_part(
                    message_states.entry(output_index).or_default(),
                    content_index,
                    refusal,
                    part,
                );
            }
            "response.content_part.added" | "response.content_part.done" => {
                let Some(part) = event_object.get("part").and_then(Value::as_object) else {
                    continue;
                };
                let output_index = openai_responses_event_output_index(event_object).unwrap_or(0);
                let content_index = openai_responses_event_content_index(event_object);
                merge_openai_responses_message_part(
                    message_states.entry(output_index).or_default(),
                    content_index,
                    part,
                );
            }
            "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                let output_index = openai_responses_event_output_index(event_object).unwrap_or(0);
                let delta = event_object
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if delta.is_empty() {
                    continue;
                }
                reasoning_states
                    .entry(output_index)
                    .or_default()
                    .summary_text
                    .push_str(delta);
            }
            "response.reasoning_text.done" | "response.reasoning_summary_text.done" => {
                let output_index = openai_responses_event_output_index(event_object).unwrap_or(0);
                let text = event_object
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        event_object
                            .get("part")
                            .and_then(Value::as_object)
                            .and_then(|part| part.get("text"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or_default();
                merge_openai_responses_reasoning_text(
                    reasoning_states.entry(output_index).or_default(),
                    text,
                );
            }
            "response.reasoning_summary_part.added" | "response.reasoning_summary_part.done" => {
                let Some(part) = event_object.get("part").and_then(Value::as_object) else {
                    continue;
                };
                if part.get("type").and_then(Value::as_str) != Some("summary_text") {
                    continue;
                }
                let output_index = openai_responses_event_output_index(event_object).unwrap_or(0);
                let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
                merge_openai_responses_reasoning_text(
                    reasoning_states.entry(output_index).or_default(),
                    text,
                );
            }
            "response.output_item.added" | "response.output_item.done" => {
                let Some(item) = event_object.get("item").and_then(Value::as_object) else {
                    continue;
                };
                let item_key = openai_responses_generic_output_item_key(item);
                let output_index =
                    if let Some(output_index) = openai_responses_event_output_index(event_object) {
                        next_output_index = next_output_index.max(output_index.saturating_add(1));
                        generic_output_indexes.insert(item_key, output_index);
                        output_index
                    } else if let Some(output_index) = generic_output_indexes.get(&item_key) {
                        *output_index
                    } else {
                        let output_index = next_output_index;
                        next_output_index = next_output_index.saturating_add(1);
                        generic_output_indexes.insert(item_key, output_index);
                        output_index
                    };
                match item.get("type").and_then(Value::as_str).unwrap_or_default() {
                    "message" => merge_openai_responses_message_item(
                        message_states.entry(output_index).or_default(),
                        item,
                    ),
                    "reasoning" => merge_openai_responses_reasoning_item(
                        reasoning_states.entry(output_index).or_default(),
                        item,
                    ),
                    "function_call" | "custom_tool_call" => {
                        merge_openai_responses_tool_item(
                            tool_states.entry(output_index).or_default(),
                            item,
                        );
                        register_openai_responses_tool_aliases(
                            &mut item_output_indexes,
                            output_index,
                            item,
                        );
                    }
                    "image_generation_call" => {
                        image_items.insert(output_index, Value::Object(item.clone()));
                    }
                    _ if event_object.get("type").and_then(Value::as_str)
                        == Some("response.output_item.done") =>
                    {
                        opaque_items.insert(output_index, Value::Object(item.clone()));
                    }
                    _ => {}
                }
            }
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
                let Some(output_index) =
                    resolve_openai_responses_tool_output_index(event_object, &item_output_indexes)
                else {
                    continue;
                };
                let delta = event_object
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if delta.is_empty() {
                    continue;
                }
                let state = tool_states.entry(output_index).or_default();
                if event_object
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.starts_with("response.custom_tool_call_input."))
                {
                    state
                        .item
                        .entry("type".to_string())
                        .or_insert_with(|| Value::String("custom_tool_call".to_string()));
                    if let Some(name) = event_object.get("name").and_then(Value::as_str) {
                        state
                            .item
                            .entry("name".to_string())
                            .or_insert_with(|| Value::String(name.to_string()));
                    }
                    if let Some(item_id) = event_object.get("item_id").and_then(Value::as_str) {
                        state
                            .item
                            .entry("id".to_string())
                            .or_insert_with(|| Value::String(item_id.to_string()));
                    }
                    if let Some(call_id) = event_object.get("call_id").and_then(Value::as_str) {
                        state
                            .item
                            .entry("call_id".to_string())
                            .or_insert_with(|| Value::String(call_id.to_string()));
                    }
                }
                state.arguments.push_str(delta);
                register_openai_responses_tool_event_aliases(
                    &mut item_output_indexes,
                    event_object,
                    output_index,
                );
            }
            "response.function_call_arguments.done" | "response.custom_tool_call_input.done" => {
                let Some(output_index) =
                    resolve_openai_responses_tool_output_index(event_object, &item_output_indexes)
                else {
                    continue;
                };
                if let Some(item) = event_object.get("item").and_then(Value::as_object) {
                    merge_openai_responses_tool_item(
                        tool_states.entry(output_index).or_default(),
                        item,
                    );
                    register_openai_responses_tool_aliases(
                        &mut item_output_indexes,
                        output_index,
                        item,
                    );
                }
                if event_object
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.starts_with("response.custom_tool_call_input."))
                {
                    let state = tool_states.entry(output_index).or_default();
                    state
                        .item
                        .entry("type".to_string())
                        .or_insert_with(|| Value::String("custom_tool_call".to_string()));
                    if let Some(name) = event_object.get("name").and_then(Value::as_str) {
                        state
                            .item
                            .entry("name".to_string())
                            .or_insert_with(|| Value::String(name.to_string()));
                    }
                    if let Some(item_id) = event_object.get("item_id").and_then(Value::as_str) {
                        state
                            .item
                            .entry("id".to_string())
                            .or_insert_with(|| Value::String(item_id.to_string()));
                    }
                    if let Some(call_id) = event_object.get("call_id").and_then(Value::as_str) {
                        state
                            .item
                            .entry("call_id".to_string())
                            .or_insert_with(|| Value::String(call_id.to_string()));
                    }
                }
                let arguments = event_object
                    .get("arguments")
                    .or_else(|| event_object.get("input"))
                    .and_then(Value::as_str)
                    .or_else(|| {
                        event_object
                            .get("item")
                            .and_then(Value::as_object)
                            .and_then(|item| item.get("arguments").or_else(|| item.get("input")))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or_default();
                merge_openai_responses_tool_arguments(
                    tool_states.entry(output_index).or_default(),
                    arguments,
                );
                register_openai_responses_tool_event_aliases(
                    &mut item_output_indexes,
                    event_object,
                    output_index,
                );
            }
            "response.completed" | "response.done" | "response.incomplete" | "response.failed" => {
                let Some(response) = event_object.get("response").and_then(Value::as_object) else {
                    continue;
                };
                if let Some(output) = response.get("output").and_then(Value::as_array) {
                    for (output_index, item) in output.iter().enumerate() {
                        let Some(item) = item.as_object() else {
                            continue;
                        };
                        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
                            "message" => merge_openai_responses_message_item(
                                message_states.entry(output_index).or_default(),
                                item,
                            ),
                            "reasoning" => merge_openai_responses_reasoning_item(
                                reasoning_states.entry(output_index).or_default(),
                                item,
                            ),
                            "function_call" | "custom_tool_call" => {
                                merge_openai_responses_tool_item(
                                    tool_states.entry(output_index).or_default(),
                                    item,
                                );
                                register_openai_responses_tool_aliases(
                                    &mut item_output_indexes,
                                    output_index,
                                    item,
                                );
                            }
                            "image_generation_call" => {
                                image_items.insert(output_index, Value::Object(item.clone()));
                            }
                            _ => {
                                opaque_items.insert(output_index, Value::Object(item.clone()));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    response
        .entry("object".to_string())
        .or_insert_with(|| Value::String("response".to_string()));
    if let Some(response_id) = response_id.as_ref() {
        response
            .entry("id".to_string())
            .or_insert_with(|| Value::String(response_id.clone()));
    }
    if let Some(model) = model.as_ref() {
        response
            .entry("model".to_string())
            .or_insert_with(|| Value::String(model.clone()));
    }

    let response_id = response
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or(response_id)
        .unwrap_or_else(|| "resp-local-stream".to_string());

    if response
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|output| !output.is_empty())
    {
        ensure_modern_openai_responses_response_fields(&mut response);
        return Some(Value::Object(response));
    }

    let mut output_indexes = message_states
        .keys()
        .chain(reasoning_states.keys())
        .chain(tool_states.keys())
        .chain(image_items.keys())
        .chain(opaque_items.keys())
        .copied()
        .collect::<Vec<_>>();
    output_indexes.sort_unstable();
    output_indexes.dedup();

    if !output_indexes.is_empty() {
        let mut output = Vec::with_capacity(output_indexes.len());
        for output_index in output_indexes {
            if let Some(state) = reasoning_states.remove(&output_index) {
                output.push(materialize_openai_responses_reasoning_item(
                    &response_id,
                    state,
                ));
            }
            if let Some(state) = message_states.remove(&output_index) {
                output.push(materialize_openai_responses_message_item(
                    &response_id,
                    state,
                ));
            }
            if let Some(state) = tool_states.remove(&output_index) {
                output.push(materialize_openai_responses_tool_item(output_index, state));
            }
            if let Some(item) = image_items.remove(&output_index) {
                output.push(item);
            }
            if let Some(item) = opaque_items.remove(&output_index) {
                output.push(item);
            }
        }
        response.insert("output".to_string(), Value::Array(output));
    }

    ensure_modern_openai_responses_response_fields(&mut response);

    Some(Value::Object(response))
}

#[derive(Default)]
struct OpenAIResponsesSyncMessageState {
    item: Map<String, Value>,
    parts: BTreeMap<usize, Value>,
}

#[derive(Default)]
struct OpenAIResponsesSyncReasoningState {
    item: Map<String, Value>,
    summary_text: String,
}

#[derive(Default)]
struct OpenAIResponsesSyncToolState {
    item: Map<String, Value>,
    arguments: String,
}

fn openai_responses_generic_output_item_key(item: &Map<String, Value>) -> String {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
    if let Some(item_id) = item.get("id").and_then(Value::as_str) {
        return format!("{item_type}:id:{item_id}");
    }
    if let Some(encrypted_content) = item.get("encrypted_content").and_then(Value::as_str) {
        return format!("{item_type}:encrypted_content:{encrypted_content}");
    }
    format!(
        "{item_type}:{}",
        serde_json::to_string(item).unwrap_or_default()
    )
}

fn openai_responses_event_output_index(event: &Map<String, Value>) -> Option<usize> {
    event
        .get("output_index")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
}

fn openai_responses_event_content_index(event: &Map<String, Value>) -> usize {
    event
        .get("content_index")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(0)
}

fn reconcile_openai_responses_authoritative_text(buffer: &mut String, authoritative: &str) {
    if authoritative.is_empty() {
        return;
    }
    if authoritative.starts_with(buffer.as_str()) {
        buffer.push_str(&authoritative[buffer.len()..]);
    } else if buffer != authoritative {
        *buffer = authoritative.to_string();
    }
}

fn default_openai_responses_output_text_part() -> Value {
    json!({
        "type": "output_text",
        "text": "",
        "annotations": [],
    })
}

fn default_openai_responses_refusal_part() -> Value {
    json!({
        "type": "refusal",
        "refusal": "",
    })
}

fn append_openai_responses_message_text_delta(
    state: &mut OpenAIResponsesSyncMessageState,
    content_index: usize,
    delta: &str,
) {
    if delta.is_empty() {
        return;
    }
    let part = state
        .parts
        .entry(content_index)
        .or_insert_with(default_openai_responses_output_text_part);
    let Some(part) = part.as_object_mut() else {
        return;
    };
    if !part
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| matches!(value, "output_text" | "text"))
    {
        return;
    }
    let current = part.get("text").and_then(Value::as_str).unwrap_or_default();
    let current = current.to_string();
    part.insert("type".to_string(), Value::String("output_text".to_string()));
    part.insert(
        "text".to_string(),
        Value::String(format!("{current}{delta}")),
    );
    part.entry("annotations".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
}

fn merge_openai_responses_message_text_delta_object(
    state: &mut OpenAIResponsesSyncMessageState,
    content_index: usize,
    text: &str,
) {
    if text.is_empty() {
        return;
    }
    let part = state
        .parts
        .entry(content_index)
        .or_insert_with(default_openai_responses_output_text_part);
    let Some(part) = part.as_object_mut() else {
        return;
    };
    if !part
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| matches!(value, "output_text" | "text"))
    {
        return;
    }
    let current = part
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let merged = if text.starts_with(current.as_str()) {
        text.to_string()
    } else if current == text || current.starts_with(text) {
        current
    } else {
        format!("{current}{text}")
    };
    part.insert("type".to_string(), Value::String("output_text".to_string()));
    part.insert("text".to_string(), Value::String(merged));
    part.entry("annotations".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
}

fn merge_openai_responses_message_text_part(
    state: &mut OpenAIResponsesSyncMessageState,
    content_index: usize,
    text: &str,
    template_part: Option<&Map<String, Value>>,
) {
    if text.is_empty() && template_part.is_none() {
        return;
    }
    let part = state.parts.entry(content_index).or_insert_with(|| {
        template_part
            .map(|part| Value::Object(part.clone()))
            .unwrap_or_else(default_openai_responses_output_text_part)
    });
    let Some(part) = part.as_object_mut() else {
        return;
    };
    if let Some(template_part) = template_part {
        for (key, value) in template_part {
            if key != "text" {
                part.insert(key.clone(), value.clone());
            }
        }
    }
    part.insert("type".to_string(), Value::String("output_text".to_string()));
    let mut current = part
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    reconcile_openai_responses_authoritative_text(&mut current, text);
    part.insert("text".to_string(), Value::String(current));
    part.entry("annotations".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
}

fn append_openai_responses_message_refusal_delta(
    state: &mut OpenAIResponsesSyncMessageState,
    content_index: usize,
    delta: &str,
) {
    if delta.is_empty() {
        return;
    }
    let part = state
        .parts
        .entry(content_index)
        .or_insert_with(default_openai_responses_refusal_part);
    let Some(part) = part.as_object_mut() else {
        return;
    };
    if part.get("type").and_then(Value::as_str) != Some("refusal") {
        return;
    }
    let current = part
        .get("refusal")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    part.insert("type".to_string(), Value::String("refusal".to_string()));
    part.insert(
        "refusal".to_string(),
        Value::String(format!("{current}{delta}")),
    );
}

fn merge_openai_responses_message_refusal_part(
    state: &mut OpenAIResponsesSyncMessageState,
    content_index: usize,
    refusal: &str,
    template_part: Option<&Map<String, Value>>,
) {
    if refusal.is_empty() && template_part.is_none() {
        return;
    }
    let part = state.parts.entry(content_index).or_insert_with(|| {
        template_part
            .map(|part| Value::Object(part.clone()))
            .unwrap_or_else(default_openai_responses_refusal_part)
    });
    let Some(part) = part.as_object_mut() else {
        return;
    };
    if let Some(template_part) = template_part {
        for (key, value) in template_part {
            if key != "refusal" {
                part.insert(key.clone(), value.clone());
            }
        }
    }
    part.insert("type".to_string(), Value::String("refusal".to_string()));
    let mut current = part
        .get("refusal")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    reconcile_openai_responses_authoritative_text(&mut current, refusal);
    part.insert("refusal".to_string(), Value::String(current));
}

fn merge_openai_responses_message_text_annotation(
    state: &mut OpenAIResponsesSyncMessageState,
    content_index: usize,
    event: &Map<String, Value>,
) {
    let Some(annotation) = event.get("annotation") else {
        return;
    };
    let annotation_index = event
        .get("annotation_index")
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    let part = state
        .parts
        .entry(content_index)
        .or_insert_with(default_openai_responses_output_text_part);
    let Some(part) = part.as_object_mut() else {
        return;
    };
    if !part
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| matches!(value, "output_text" | "text"))
    {
        return;
    }
    part.insert("type".to_string(), Value::String("output_text".to_string()));
    part.entry("text".to_string())
        .or_insert_with(|| Value::String(String::new()));
    let annotations = part
        .entry("annotations".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(annotations) = annotations.as_array_mut() else {
        return;
    };
    if let Some(annotation_index) = annotation_index {
        if annotations.len() <= annotation_index {
            annotations.resize(annotation_index + 1, Value::Null);
        }
        annotations[annotation_index] = annotation.clone();
    } else {
        annotations.push(annotation.clone());
    }
}

fn merge_openai_responses_message_part(
    state: &mut OpenAIResponsesSyncMessageState,
    content_index: usize,
    part: &Map<String, Value>,
) {
    if part
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| matches!(value, "output_text" | "text"))
    {
        let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
        merge_openai_responses_message_text_part(state, content_index, text, Some(part));
    } else if part.get("type").and_then(Value::as_str) == Some("refusal") {
        let refusal = part
            .get("refusal")
            .and_then(Value::as_str)
            .unwrap_or_default();
        merge_openai_responses_message_refusal_part(state, content_index, refusal, Some(part));
    } else {
        state
            .parts
            .insert(content_index, Value::Object(part.clone()));
    }
}

fn merge_openai_responses_reasoning_text(
    state: &mut OpenAIResponsesSyncReasoningState,
    text: &str,
) {
    if text.is_empty() {
        return;
    }
    if state.summary_text.is_empty() || text.len() >= state.summary_text.len() {
        state.summary_text = text.to_string();
    }
}

fn merge_openai_responses_tool_arguments(
    state: &mut OpenAIResponsesSyncToolState,
    arguments: &str,
) {
    if arguments.is_empty() {
        return;
    }
    if state.arguments.is_empty() || arguments.len() >= state.arguments.len() {
        state.arguments = arguments.to_string();
    }
}

fn extract_openai_responses_reasoning_text(item: &Map<String, Value>) -> Option<String> {
    item.get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|part| {
            let part = part.as_object()?;
            (part.get("type").and_then(Value::as_str) == Some("summary_text")).then(|| {
                part.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
        })
}

fn merge_openai_responses_message_item(
    state: &mut OpenAIResponsesSyncMessageState,
    item: &Map<String, Value>,
) {
    if let Some(content) = item.get("content").and_then(Value::as_array) {
        for (content_index, part) in content.iter().enumerate() {
            if let Some(part) = part.as_object() {
                merge_openai_responses_message_part(state, content_index, part);
            }
        }
    }
    state.item = item.clone();
}

fn merge_openai_responses_reasoning_item(
    state: &mut OpenAIResponsesSyncReasoningState,
    item: &Map<String, Value>,
) {
    if let Some(text) = extract_openai_responses_reasoning_text(item) {
        merge_openai_responses_reasoning_text(state, text.as_str());
    }
    state.item = item.clone();
}

fn merge_openai_responses_tool_item(
    state: &mut OpenAIResponsesSyncToolState,
    item: &Map<String, Value>,
) {
    if let Some(arguments) = item.get("arguments").and_then(Value::as_str) {
        merge_openai_responses_tool_arguments(state, arguments);
    }
    state.item = item.clone();
}

fn register_openai_responses_tool_aliases(
    aliases: &mut BTreeMap<String, usize>,
    output_index: usize,
    item: &Map<String, Value>,
) {
    if let Some(id) = item.get("id").and_then(Value::as_str).map(str::trim) {
        if !id.is_empty() {
            aliases.insert(id.to_string(), output_index);
        }
    }
    if let Some(call_id) = item.get("call_id").and_then(Value::as_str).map(str::trim) {
        if !call_id.is_empty() {
            aliases.insert(call_id.to_string(), output_index);
        }
    }
}

fn register_openai_responses_tool_event_aliases(
    aliases: &mut BTreeMap<String, usize>,
    event: &Map<String, Value>,
    output_index: usize,
) {
    for key in ["item_id", "call_id", "id"] {
        if let Some(value) = event.get(key).and_then(Value::as_str).map(str::trim) {
            if !value.is_empty() {
                aliases.insert(value.to_string(), output_index);
            }
        }
    }
}

fn resolve_openai_responses_tool_output_index(
    event: &Map<String, Value>,
    aliases: &BTreeMap<String, usize>,
) -> Option<usize> {
    openai_responses_event_output_index(event).or_else(|| {
        ["item_id", "call_id", "id"]
            .iter()
            .find_map(|key| event.get(*key).and_then(Value::as_str))
            .and_then(|value| aliases.get(value).copied())
    })
}

fn materialize_openai_responses_message_item(
    response_id: &str,
    state: OpenAIResponsesSyncMessageState,
) -> Value {
    let mut item = state.item;
    item.entry("type".to_string())
        .or_insert_with(|| Value::String("message".to_string()));
    item.entry("id".to_string())
        .or_insert_with(|| Value::String(format!("{response_id}_msg")));
    item.entry("role".to_string())
        .or_insert_with(|| Value::String("assistant".to_string()));
    item.entry("status".to_string())
        .or_insert_with(|| Value::String("completed".to_string()));

    let mut content = match item.remove("content") {
        Some(Value::Array(content)) => content,
        _ => Vec::new(),
    };
    for (content_index, part) in state.parts {
        if content_index < content.len() {
            content[content_index] = part;
        } else {
            content.push(part);
        }
    }
    item.insert("content".to_string(), Value::Array(content));
    Value::Object(item)
}

fn materialize_openai_responses_reasoning_item(
    response_id: &str,
    state: OpenAIResponsesSyncReasoningState,
) -> Value {
    let mut item = state.item;
    item.entry("type".to_string())
        .or_insert_with(|| Value::String("reasoning".to_string()));
    item.entry("id".to_string())
        .or_insert_with(|| Value::String(format!("{response_id}_rs_0")));
    item.entry("status".to_string())
        .or_insert_with(|| Value::String("completed".to_string()));
    if !state.summary_text.is_empty() {
        item.insert(
            "summary".to_string(),
            Value::Array(vec![json!({
                "type": "summary_text",
                "text": state.summary_text,
            })]),
        );
    }
    Value::Object(item)
}

fn materialize_openai_responses_tool_item(
    output_index: usize,
    state: OpenAIResponsesSyncToolState,
) -> Value {
    let mut item = state.item;
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .filter(|value| *value == "custom_tool_call")
        .unwrap_or("function_call")
        .to_string();
    let generated_id = format!("call_auto_{output_index}");
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            item.get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or(generated_id.clone());

    item.insert("type".to_string(), Value::String(item_type.clone()));
    item.entry("id".to_string())
        .or_insert_with(|| Value::String(call_id.clone()));
    item.insert("call_id".to_string(), Value::String(call_id));
    item.entry("name".to_string())
        .or_insert_with(|| Value::String("unknown".to_string()));
    item.entry("status".to_string())
        .or_insert_with(|| Value::String("completed".to_string()));
    if !state.arguments.is_empty() {
        let argument_key = if item_type == "custom_tool_call" {
            "input"
        } else {
            "arguments"
        };
        item.insert(argument_key.to_string(), Value::String(state.arguments));
    } else {
        let argument_key = if item_type == "custom_tool_call" {
            "input"
        } else {
            "arguments"
        };
        item.entry(argument_key.to_string())
            .or_insert_with(|| Value::String(String::new()));
    }
    Value::Object(item)
}

#[derive(Default)]
struct GeminiSyncToolState {
    call_id: String,
    name: String,
    arguments: String,
    part_index: Option<usize>,
}

pub fn aggregate_claude_stream_sync_response(body: &[u8]) -> Option<Value> {
    let events = parse_stream_json_events(body)?;
    if events.is_empty() {
        return None;
    }

    let mut message_object: Option<Map<String, Value>> = None;
    let mut content_blocks: BTreeMap<usize, ClaudeContentBlockState> = BTreeMap::new();
    let mut usage: Option<Value> = None;
    let mut saw_message_start = false;

    for event in events {
        let event_object = event.as_object()?;
        let event_type = event_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match event_type {
            "message_start" => {
                let mut message = event_object.get("message")?.as_object()?.clone();
                usage = message.remove("usage");
                message_object = Some(message);
                saw_message_start = true;
            }
            "content_block_start" => {
                let index = event_object
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize)
                    .unwrap_or(0);
                let object = event_object
                    .get("content_block")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                content_blocks.insert(
                    index,
                    ClaudeContentBlockState {
                        object,
                        ..Default::default()
                    },
                );
            }
            "content_block_delta" => {
                let index = event_object
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize)
                    .unwrap_or(0);
                let state = content_blocks.entry(index).or_default();
                let Some(delta) = event_object.get("delta").and_then(Value::as_object) else {
                    continue;
                };
                match delta
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            state.text.push_str(text);
                        }
                    }
                    "input_json_delta" => {
                        if let Some(partial_json) =
                            delta.get("partial_json").and_then(Value::as_str)
                        {
                            state.partial_json.push_str(partial_json);
                        }
                    }
                    "thinking_delta" => {
                        if let Some(thinking) = delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .or_else(|| delta.get("text").and_then(Value::as_str))
                        {
                            state.text.push_str(thinking);
                        }
                    }
                    "signature_delta" => {
                        if let Some(signature) = delta.get("signature").and_then(Value::as_str) {
                            if !signature.is_empty() {
                                state.signature = Some(signature.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(message) = message_object.as_mut() {
                    if let Some(delta) = event_object.get("delta").and_then(Value::as_object) {
                        if let Some(stop_reason) = delta.get("stop_reason") {
                            message.insert("stop_reason".to_string(), stop_reason.clone());
                        }
                        if let Some(stop_sequence) = delta.get("stop_sequence") {
                            message.insert("stop_sequence".to_string(), stop_sequence.clone());
                        }
                    }
                }
                if let Some(delta_usage) = event_object.get("usage") {
                    usage = Some(delta_usage.clone());
                }
            }
            "message_stop" => {}
            _ => {}
        }
    }

    if !saw_message_start {
        return None;
    }

    let mut message = message_object?;
    let mut content = Vec::with_capacity(content_blocks.len());
    for (_index, state) in content_blocks {
        let mut block = state.object;
        let block_type = block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("text")
            .to_string();
        match block_type.as_str() {
            "text" => {
                block.insert(
                    "text".to_string(),
                    Value::String(if state.text.is_empty() {
                        block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    } else {
                        state.text
                    }),
                );
            }
            "thinking" => {
                block.insert(
                    "thinking".to_string(),
                    Value::String(if state.text.is_empty() {
                        block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    } else {
                        state.text
                    }),
                );
                if let Some(signature) = state.signature {
                    block.insert("signature".to_string(), Value::String(signature));
                }
            }
            "tool_use" => {
                let tool_name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Some(input) = block.get("input") {
                    let sanitized = remove_empty_pages_from_tool_input_value(&tool_name, input);
                    if sanitized != *input {
                        block.insert("input".to_string(), sanitized);
                    }
                }
                if !state.partial_json.is_empty() {
                    let arguments =
                        remove_empty_pages_from_tool_arguments(&tool_name, &state.partial_json);
                    let input = serde_json::from_str::<Value>(&arguments)
                        .unwrap_or(Value::String(arguments));
                    block.insert("input".to_string(), input);
                }
            }
            _ => {
                if !state.text.is_empty() {
                    block.insert("text".to_string(), Value::String(state.text));
                }
                if let Some(signature) = state.signature {
                    block.insert("signature".to_string(), Value::String(signature));
                }
            }
        }
        content.push(Value::Object(block));
    }
    message.insert("content".to_string(), Value::Array(content));
    if let Some(usage_value) = usage {
        message.insert("usage".to_string(), usage_value);
    }

    Some(Value::Object(message))
}

pub fn aggregate_gemini_stream_sync_response(body: &[u8]) -> Option<Value> {
    try_aggregate_gemini_stream_sync_response(body)
        .ok()
        .flatten()
}

fn try_aggregate_gemini_stream_sync_response(
    body: &[u8],
) -> Result<Option<Value>, AiSurfaceFinalizeError> {
    let Some(events) = parse_stream_json_events(body) else {
        return Ok(None);
    };
    if events.is_empty() {
        return Ok(None);
    }

    let report_context = Value::Object(Map::new());
    let mut provider = GeminiProviderState::default();
    let mut response_id: Option<Value> = None;
    let mut private_response_id: Option<Value> = None;
    let mut model_version: Option<Value> = None;
    let mut usage_metadata: Option<Value> = None;
    let mut prompt_feedback: Option<Value> = None;
    let mut candidate: Map<String, Value> = Map::new();
    let mut role: Option<Value> = None;
    let mut saw_candidate = false;
    let mut parts: Vec<Value> = Vec::new();
    let mut tool_states: BTreeMap<usize, GeminiSyncToolState> = BTreeMap::new();
    let mut finish_reason: Option<String> = None;
    let mut usage_from_frames: Option<CanonicalUsage> = None;

    for event in &events {
        let Some(raw_event_object) = event.as_object() else {
            return Ok(None);
        };
        if let Some(id) = raw_event_object.get("responseId") {
            response_id = Some(id.clone());
        }
        if let Some(id) = raw_event_object.get("_v1internal_response_id") {
            private_response_id = Some(id.clone());
        }
        let event_object = if let Some(response) = raw_event_object
            .get("response")
            .and_then(Value::as_object)
            .filter(|response| response.contains_key("candidates"))
        {
            response
        } else {
            raw_event_object
        };
        if let Some(id) = event_object.get("responseId") {
            response_id = Some(id.clone());
        }
        if let Some(id) = event_object.get("_v1internal_response_id") {
            private_response_id = Some(id.clone());
        }
        if let Some(version) = event_object.get("modelVersion") {
            model_version = Some(version.clone());
        }
        if let Some(usage) = event_object.get("usageMetadata") {
            usage_metadata = Some(usage.clone());
        }
        if let Some(prompt) = event_object.get("promptFeedback") {
            prompt_feedback = Some(prompt.clone());
        }
        let Some(event_candidates) = event_object.get("candidates").and_then(Value::as_array)
        else {
            continue;
        };
        for event_candidate in event_candidates {
            let Some(candidate_object) = event_candidate.as_object() else {
                continue;
            };
            for (key, value) in candidate_object {
                if key != "content" {
                    candidate.insert(key.clone(), value.clone());
                }
            }
            if let Some(content) = candidate_object.get("content").and_then(Value::as_object) {
                if let Some(content_role) = content.get("role") {
                    role = Some(content_role.clone());
                }
            }
            saw_candidate = true;
        }

        let line = format!("data: {event}\n").into_bytes();
        let frames = provider.push_line(&report_context, line)?;
        for frame in frames {
            if response_id.is_none() && !frame.id.is_empty() {
                response_id = Some(Value::String(frame.id.clone()));
            }
            if model_version.is_none() && !frame.model.is_empty() {
                model_version = Some(Value::String(frame.model.clone()));
            }
            match frame.event {
                CanonicalStreamEvent::Start => {}
                CanonicalStreamEvent::TextDelta(text) => {
                    append_gemini_text_part(&mut parts, text, false);
                }
                CanonicalStreamEvent::ReasoningDelta(text) => {
                    append_gemini_text_part(&mut parts, text, true);
                }
                CanonicalStreamEvent::ReasoningSignature(signature) => {
                    attach_gemini_reasoning_signature(&mut parts, signature);
                }
                CanonicalStreamEvent::ContentPart(part) => {
                    parts.push(gemini_sync_part_from_canonical_content_part(part));
                }
                CanonicalStreamEvent::ImageGenerationCall { item, .. } => {
                    if let Some(part) = content_part_from_openai_image_generation_item(&item) {
                        parts.push(gemini_sync_part_from_canonical_content_part(part));
                    }
                }
                CanonicalStreamEvent::OpenAiResponsesOutputItem { raw_event, .. } => {
                    return Err(unsupported_stream_event_finalize_error(&raw_event))
                }
                CanonicalStreamEvent::ToolCallStart {
                    index,
                    call_id,
                    name,
                } => {
                    let generated_call_id = if call_id.trim().is_empty() {
                        format!("call_auto_{index}")
                    } else {
                        call_id
                    };
                    let generated_name = if name.trim().is_empty() {
                        "unknown".to_string()
                    } else {
                        name
                    };
                    let state = tool_states.entry(index).or_default();
                    state.call_id = generated_call_id;
                    state.name = generated_name;
                    if state.part_index.is_none() {
                        let part_index = parts.len();
                        parts.push(sync_gemini_function_call_part(state));
                        state.part_index = Some(part_index);
                    } else if let Some(part_index) = state.part_index {
                        parts[part_index] = sync_gemini_function_call_part(state);
                    }
                }
                CanonicalStreamEvent::ToolCallArgumentsDelta { index, arguments } => {
                    let state = tool_states.entry(index).or_default();
                    state.arguments.push_str(&arguments);
                    let part_index = if let Some(part_index) = state.part_index {
                        part_index
                    } else {
                        let part_index = parts.len();
                        parts.push(sync_gemini_function_call_part(state));
                        state.part_index = Some(part_index);
                        part_index
                    };
                    parts[part_index] = sync_gemini_function_call_part(state);
                }
                CanonicalStreamEvent::ToolResultDelta {
                    tool_use_id,
                    name,
                    content,
                    ..
                } => {
                    parts.push(sync_gemini_function_response_part(
                        tool_use_id,
                        name,
                        content,
                    ));
                }
                CanonicalStreamEvent::UnknownEvent(payload) => {
                    return Err(unsupported_stream_event_finalize_error(&payload))
                }
                CanonicalStreamEvent::ReasoningSummaryDone => {}
                CanonicalStreamEvent::Finish {
                    finish_reason: frame_finish_reason,
                    usage,
                } => {
                    finish_reason = frame_finish_reason
                        .map(|value| {
                            map_openai_finish_reason_to_gemini(Some(value.as_str())).to_string()
                        })
                        .or(finish_reason);
                    if usage.is_some() {
                        usage_from_frames = usage;
                    }
                }
            }
        }
    }

    let frames = provider.finish(&report_context)?;
    for frame in frames {
        if response_id.is_none() && !frame.id.is_empty() {
            response_id = Some(Value::String(frame.id.clone()));
        }
        if model_version.is_none() && !frame.model.is_empty() {
            model_version = Some(Value::String(frame.model.clone()));
        }
        match frame.event {
            CanonicalStreamEvent::UnknownEvent(payload) => {
                return Err(unsupported_stream_event_finalize_error(&payload))
            }
            CanonicalStreamEvent::Finish {
                finish_reason: frame_finish_reason,
                usage,
            } => {
                finish_reason = frame_finish_reason
                    .map(|value| {
                        map_openai_finish_reason_to_gemini(Some(value.as_str())).to_string()
                    })
                    .or(finish_reason);
                if usage.is_some() {
                    usage_from_frames = usage;
                }
            }
            _ => {}
        }
    }

    if !saw_candidate {
        return Ok(None);
    }

    candidate.insert(
        "content".to_string(),
        json!({
            "role": role.unwrap_or_else(|| Value::String("model".to_string())),
            "parts": parts,
        }),
    );
    candidate
        .entry("index".to_string())
        .or_insert_with(|| Value::from(0_u64));
    if let Some(finish_reason) = finish_reason {
        candidate.insert("finishReason".to_string(), Value::String(finish_reason));
    }

    let mut response = Map::new();
    if let Some(response_id) = response_id {
        response.insert("responseId".to_string(), response_id);
    }
    if let Some(private_response_id) = private_response_id {
        response.insert("_v1internal_response_id".to_string(), private_response_id);
    }
    response.insert(
        "candidates".to_string(),
        Value::Array(vec![Value::Object(candidate)]),
    );
    if let Some(version) = model_version {
        response.insert("modelVersion".to_string(), version);
    }
    if usage_metadata.is_none() {
        usage_metadata = usage_from_frames.map(gemini_usage_metadata_from_canonical);
    }
    if let Some(usage) = usage_metadata {
        response.insert("usageMetadata".to_string(), usage);
    }
    if let Some(prompt) = prompt_feedback {
        response.insert("promptFeedback".to_string(), prompt);
    }
    Ok(Some(Value::Object(response)))
}

fn unsupported_stream_event_finalize_error(payload: &Value) -> AiSurfaceFinalizeError {
    AiSurfaceFinalizeError::new(unsupported_stream_event_message(payload))
}

fn append_gemini_text_part(parts: &mut Vec<Value>, text: String, thought: bool) {
    if text.is_empty() {
        return;
    }
    let Some(existing) = parts
        .last_mut()
        .and_then(Value::as_object_mut)
        .filter(|part| is_mergeable_gemini_text_part(part, thought))
    else {
        let mut part = Map::new();
        part.insert("text".to_string(), Value::String(text));
        if thought {
            part.insert("thought".to_string(), Value::Bool(true));
        }
        parts.push(Value::Object(part));
        return;
    };
    let current = existing
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    existing.insert(
        "text".to_string(),
        Value::String(format!("{current}{text}")),
    );
}

fn attach_gemini_reasoning_signature(parts: &mut Vec<Value>, signature: String) {
    if signature.is_empty() {
        return;
    }
    for part in parts.iter_mut().rev() {
        let Some(part_object) = part.as_object_mut() else {
            continue;
        };
        if is_mergeable_gemini_text_part(part_object, true) {
            part_object.insert(
                "thoughtSignature".to_string(),
                Value::String(signature.clone()),
            );
            return;
        }
    }
    parts.push(json!({
        "text": "",
        "thought": true,
        "thoughtSignature": signature,
    }));
}

fn is_mergeable_gemini_text_part(part: &Map<String, Value>, thought: bool) -> bool {
    if !part.contains_key("text") {
        return false;
    }
    if part
        .get("thought")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        != thought
    {
        return false;
    }
    part.keys().all(|key| {
        matches!(
            key.as_str(),
            "text" | "thought" | "thoughtSignature" | "thought_signature"
        )
    })
}

fn sync_gemini_function_call_part(state: &GeminiSyncToolState) -> Value {
    json!({
        "functionCall": {
            "id": if state.call_id.trim().is_empty() {
                "call_auto_0".to_string()
            } else {
                state.call_id.clone()
            },
            "name": if state.name.trim().is_empty() {
                "unknown".to_string()
            } else {
                state.name.clone()
            },
            "args": sync_gemini_function_args_value(&state.arguments),
        }
    })
}

fn sync_gemini_function_response_part(
    tool_use_id: String,
    name: Option<String>,
    content: String,
) -> Value {
    json!({
        "functionResponse": {
            "id": if tool_use_id.trim().is_empty() {
                "call_auto_0".to_string()
            } else {
                tool_use_id
            },
            "name": name
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "unknown".to_string()),
            "response": sync_gemini_function_response_value(&content),
        }
    })
}

fn sync_gemini_function_response_value(content: &str) -> Value {
    if content.trim().is_empty() {
        return Value::Object(Map::new());
    }
    match serde_json::from_str::<Value>(content) {
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(value) => json!({ "output": value }),
        Err(_) => json!({ "output": content }),
    }
}

fn sync_gemini_function_args_value(arguments: &str) -> Value {
    match parse_json_arguments_value(arguments) {
        Some(Value::Object(map)) => Value::Object(map),
        Some(value) => json!({ "raw": value }),
        None if arguments.trim().is_empty() => Value::Object(Map::new()),
        None => json!({ "raw": arguments }),
    }
}

fn gemini_sync_part_from_canonical_content_part(part: CanonicalContentPart) -> Value {
    match part {
        CanonicalContentPart::ImageUrl(url) => {
            if let Some((mime_type, data)) = parse_data_url(url.as_str()) {
                json!({
                    "inlineData": {
                        "mimeType": mime_type,
                        "data": data,
                    }
                })
            } else {
                json!({
                    "fileData": {
                        "fileUri": url.clone(),
                        "mimeType": guess_media_type_from_reference(url.as_str(), "image/jpeg"),
                    }
                })
            }
        }
        CanonicalContentPart::File {
            file_data,
            reference,
            mime_type,
            ..
        } => {
            if let Some(file_data) = file_data {
                if let Some((mime_type, data)) = parse_data_url(file_data.as_str()) {
                    json!({
                        "inlineData": {
                            "mimeType": mime_type,
                            "data": data,
                        }
                    })
                } else {
                    json!({ "text": "[File]" })
                }
            } else if let Some(reference) = reference {
                json!({
                    "fileData": {
                        "fileUri": reference.clone(),
                        "mimeType": mime_type.unwrap_or_else(|| {
                            guess_media_type_from_reference(reference.as_str(), "application/octet-stream")
                        }),
                    }
                })
            } else {
                json!({ "text": "[File]" })
            }
        }
        CanonicalContentPart::Audio { data, format } => json!({
            "inlineData": {
                "mimeType": format!("audio/{format}"),
                "data": data,
            }
        }),
    }
}

fn gemini_usage_metadata_from_canonical(usage: CanonicalUsage) -> Value {
    gemini_usage_metadata_from_usage(&usage)
}

fn parse_data_url(value: &str) -> Option<(String, String)> {
    let rest = value.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let mime_type = meta.strip_suffix(";base64")?;
    if mime_type.trim().is_empty() || data.trim().is_empty() {
        return None;
    }
    Some((mime_type.to_string(), data.to_string()))
}

fn guess_media_type_from_reference(reference: &str, default_mime: &str) -> String {
    let normalized = reference
        .split('?')
        .next()
        .unwrap_or(reference)
        .to_ascii_lowercase();
    if normalized.ends_with(".png") {
        "image/png".to_string()
    } else if normalized.ends_with(".gif") {
        "image/gif".to_string()
    } else if normalized.ends_with(".webp") {
        "image/webp".to_string()
    } else if normalized.ends_with(".jpg") || normalized.ends_with(".jpeg") {
        "image/jpeg".to_string()
    } else if normalized.ends_with(".pdf") {
        "application/pdf".to_string()
    } else {
        default_mime.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        aggregate_claude_stream_sync_response, aggregate_gemini_stream_sync_response,
        aggregate_openai_chat_stream_sync_response,
        aggregate_openai_responses_stream_sync_response, convert_standard_chat_response,
        convert_standard_cli_response,
        maybe_build_openai_chat_cross_format_sync_product_from_normalized_payload,
        maybe_build_openai_responses_cross_format_sync_product_from_normalized_payload,
        maybe_build_openai_responses_same_family_sync_body_from_normalized_payload,
        maybe_build_standard_cross_format_sync_product,
        maybe_build_standard_cross_format_sync_product_from_normalized_payload,
        maybe_build_standard_same_format_sync_body_from_normalized_payload,
        maybe_build_standard_sync_finalize_product_from_normalized_payload,
        try_aggregate_openai_responses_stream_sync_response, StandardSyncFinalizeNormalizedProduct,
    };
    use aether_ai_formats::formats::conversion::response::{
        convert_claude_chat_response_to_openai_chat, convert_gemini_chat_response_to_openai_chat,
        convert_openai_chat_response_to_claude_chat, convert_openai_chat_response_to_gemini_chat,
        convert_openai_chat_response_to_openai_responses,
        convert_openai_responses_response_to_openai_chat,
    };
    use aether_ai_formats::{sync_cli_response_conversion_kind, SyncCliResponseConversionKind};
    use base64::Engine as _;
    use serde_json::json;

    #[test]
    fn converts_openai_images_sync_body_to_gemini_image_body() {
        let provider_body_json = json!({
            "created": 1776839946,
            "model": "gpt-image-2",
            "data": [{
                "revised_prompt": "revised prompt",
                "b64_json": "aGVsbG8="
            }],
            "usage": {
                "input_tokens": 3,
                "output_tokens": 4,
                "total_tokens": 7
            }
        });
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "gemini:generate_content",
        });

        let product = maybe_build_standard_cross_format_sync_product(
            "gemini_chat_sync_finalize",
            "openai:image",
            "gemini:generate_content",
            &report_context,
            provider_body_json,
        )
        .expect("OpenAI Images sync body should convert");

        assert_eq!(product.client_body_json["modelVersion"], "gpt-image-2");
        assert_eq!(
            product.client_body_json["candidates"][0]["content"]["parts"][0]["text"],
            "revised prompt"
        );
        assert_eq!(
            product.client_body_json["candidates"][0]["content"]["parts"][1]["inlineData"]["data"],
            "aGVsbG8="
        );
        assert_eq!(
            product.client_body_json["usageMetadata"]["totalTokenCount"],
            7
        );
    }

    #[test]
    fn converts_openai_responses_image_body_to_gemini_image_body() {
        let provider_body_json = json!({
            "model": "gpt-image-2",
            "output": [{
                "type": "image_generation_call",
                "status": "completed",
                "result": "aGVsbG8=",
                "output_format": "png"
            }]
        });
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "gemini:generate_content",
        });

        let product = maybe_build_standard_cross_format_sync_product(
            "gemini_chat_sync_finalize",
            "openai:image",
            "gemini:generate_content",
            &report_context,
            provider_body_json,
        )
        .expect("OpenAI Responses image body should convert");

        assert_eq!(product.client_body_json["modelVersion"], "gpt-image-2");
        assert_eq!(
            product.client_body_json["candidates"][0]["content"]["parts"][0]["inlineData"]
                ["mimeType"],
            "image/png"
        );
        assert_eq!(
            product.client_body_json["candidates"][0]["content"]["parts"][0]["inlineData"]["data"],
            "aGVsbG8="
        );
    }

    #[test]
    fn aggregates_openai_chat_stream_tool_usage_and_finish_into_sync_body() {
        let body = concat!(
            "data: {\"id\":\"chatcmpl_stream_123\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_stream_123\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello \"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_stream_123\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_123\",\"type\":\"function\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\"\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_stream_123\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":\\\"rust\\\"}\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_stream_123\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n",
        );

        let result = aggregate_openai_chat_stream_sync_response(body.as_bytes())
            .expect("openai chat stream should aggregate into a sync body");

        assert_eq!(result["id"], "chatcmpl_stream_123");
        assert_eq!(result["model"], "gpt-5");
        assert_eq!(result["choices"][0]["message"]["role"], "assistant");
        assert_eq!(result["choices"][0]["message"]["content"], "Hello ");
        assert_eq!(
            result["choices"][0]["message"]["tool_calls"][0]["id"],
            "call_123"
        );
        assert_eq!(
            result["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "lookup"
        );
        assert_eq!(
            result["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"q\":\"rust\"}"
        );
        assert_eq!(result["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(result["usage"]["prompt_tokens"], 1);
        assert_eq!(result["usage"]["completion_tokens"], 2);
        assert_eq!(result["usage"]["total_tokens"], 3);
    }

    #[test]
    fn aggregates_claude_stream_thinking_signatures_into_sync_body() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_123\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"step by step\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig_123\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let aggregated =
            aggregate_claude_stream_sync_response(body.as_bytes()).expect("body should aggregate");

        assert_eq!(aggregated["content"][0]["type"], "thinking");
        assert_eq!(aggregated["content"][0]["thinking"], "step by step");
        assert_eq!(aggregated["content"][0]["signature"], "sig_123");
        assert_eq!(aggregated["usage"]["output_tokens"], 2);
    }

    #[test]
    fn aggregates_claude_stream_removes_empty_pages_from_tool_input() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_123\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_read\",\"name\":\"Read\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"file_path\\\":\\\"/tmp/a.txt\\\",\\\"offset\\\":1,\\\"limit\\\":20,\\\"pages\\\":\\\"\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let aggregated =
            aggregate_claude_stream_sync_response(body.as_bytes()).expect("body should aggregate");

        assert_eq!(aggregated["content"][0]["type"], "tool_use");
        assert_eq!(
            aggregated["content"][0]["input"],
            json!({
                "file_path": "/tmp/a.txt",
                "offset": 1,
                "limit": 20,
            })
        );
    }

    #[test]
    fn aggregates_claude_stream_removes_empty_pages_from_start_tool_input() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_123\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_read\",\"name\":\"Read\",\"input\":{\"file_path\":\"/tmp/a.txt\",\"limit\":20,\"pages\":\"\"}}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let aggregated =
            aggregate_claude_stream_sync_response(body.as_bytes()).expect("body should aggregate");

        assert_eq!(
            aggregated["content"][0]["input"],
            json!({
                "file_path": "/tmp/a.txt",
                "limit": 20,
            })
        );
    }

    #[test]
    fn aggregates_claude_stream_preserves_empty_pages_for_non_read_tool_input() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_123\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_search\",\"name\":\"Search\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"\\\",\\\"pages\\\":\\\"\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let aggregated =
            aggregate_claude_stream_sync_response(body.as_bytes()).expect("body should aggregate");

        assert_eq!(aggregated["content"][0]["type"], "tool_use");
        assert_eq!(
            aggregated["content"][0]["input"],
            json!({
                "query": "",
                "pages": "",
            })
        );
    }

    #[test]
    fn aggregates_gemini_stream_deltas_media_and_signatures_into_sync_body() {
        let body = concat!(
            "data: {\"responseId\":\"resp_gem_stream_123\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"rea\",\"thought\":true}]}}]}\n\n",
            "data: {\"responseId\":\"resp_gem_stream_123\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"son\",\"thought\":true}]}}]}\n\n",
            "data: {\"responseId\":\"resp_gem_stream_123\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"\",\"thought\":true,\"thoughtSignature\":\"sig_123\"}]}}]}\n\n",
            "data: {\"responseId\":\"resp_gem_stream_123\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"inlineData\":{\"mimeType\":\"image/png\",\"data\":\"iVBORw0KGgo=\"}}]}}]}\n\n",
            "data: {\"responseId\":\"resp_gem_stream_123\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}}\n\n",
        );

        let aggregated =
            aggregate_gemini_stream_sync_response(body.as_bytes()).expect("body should aggregate");

        assert_eq!(aggregated["responseId"], "resp_gem_stream_123");
        assert_eq!(
            aggregated["candidates"][0]["content"]["parts"][0]["text"],
            "reason"
        );
        assert_eq!(
            aggregated["candidates"][0]["content"]["parts"][0]["thoughtSignature"],
            "sig_123"
        );
        assert_eq!(
            aggregated["candidates"][0]["content"]["parts"][1]["inlineData"]["mimeType"],
            "image/png"
        );
        assert_eq!(aggregated["candidates"][0]["finishReason"], "STOP");
        assert_eq!(aggregated["usageMetadata"]["totalTokenCount"], 5);
    }

    #[test]
    fn gemini_stream_aggregation_rejects_unknown_parts() {
        let body = "data: {\"responseId\":\"resp_gem_unknown_123\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"futurePart\":{\"kept\":true}}]}}]}\n\n";

        assert!(
            aggregate_gemini_stream_sync_response(body.as_bytes()).is_none(),
            "unknown Gemini stream parts must not be silently aggregated into a successful sync body"
        );
    }

    #[test]
    fn gemini_stream_finalize_rejects_unknown_parts_even_with_json_fallback() {
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "openai:chat",
        });
        let stream_body = "data: {\"responseId\":\"resp_gem_unknown_456\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"futurePart\":{\"kept\":true}}]}}]}\n\n";
        let provider_body_json = json!({
            "responseId": "resp_gem_unknown_456",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [{
                "index": 0,
                "content": {
                    "role": "model",
                    "parts": [{
                        "text": "fallback"
                    }]
                }
            }]
        });

        let result = maybe_build_standard_cross_format_sync_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            Some(&base64::engine::general_purpose::STANDARD.encode(stream_body)),
        );

        assert!(result.is_err());
        let error = result.expect_err("unknown Gemini stream parts should fail closed");
        assert!(error
            .to_string()
            .contains("Unsupported provider stream event cannot be converted losslessly"));
        assert!(error
            .to_string()
            .contains("field $.futurePart is unsupported"));
    }

    #[test]
    fn aggregates_gemini_stream_function_response_into_sync_body() {
        let body = concat!(
            "data: {\"responseId\":\"resp_gem_tool_result_123\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"functionResponse\":{\"id\":\"call_123\",\"name\":\"lookup\",\"response\":{\"ok\":true}}}]}}]}\n\n",
            "data: {\"responseId\":\"resp_gem_tool_result_123\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}}\n\n",
        );

        let aggregated =
            aggregate_gemini_stream_sync_response(body.as_bytes()).expect("body should aggregate");

        assert_eq!(
            aggregated["candidates"][0]["content"]["parts"][0]["functionResponse"]["id"],
            "call_123"
        );
        assert_eq!(
            aggregated["candidates"][0]["content"]["parts"][0]["functionResponse"]["response"],
            json!({"ok": true})
        );
    }

    #[test]
    fn builds_openai_chat_cross_format_sync_product_from_gemini_stream_with_media_and_signature() {
        let body = concat!(
            "data: {\"responseId\":\"resp_gem_stream_456\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"thinking\",\"thought\":true}]}}]}\n\n",
            "data: {\"responseId\":\"resp_gem_stream_456\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"\",\"thought\":true,\"thoughtSignature\":\"sig_456\"}]}}]}\n\n",
            "data: {\"responseId\":\"resp_gem_stream_456\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"inlineData\":{\"mimeType\":\"image/png\",\"data\":\"iVBORw0KGgo=\"}}]}}]}\n\n",
            "data: {\"responseId\":\"resp_gem_stream_456\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":4,\"candidatesTokenCount\":2,\"totalTokenCount\":6}}\n\n",
        );
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "openai:chat",
            "mapped_model": "gemini-2.5-pro",
        });

        let product = maybe_build_standard_cross_format_sync_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("product build should succeed")
        .expect("product should exist");

        assert_eq!(
            product.provider_body_json["candidates"][0]["content"]["parts"][0]["thoughtSignature"],
            "sig_456"
        );
        assert_eq!(
            product.client_body_json["choices"][0]["message"]["reasoning_parts"][0]["signature"],
            "sig_456"
        );
        assert_eq!(
            product.client_body_json["choices"][0]["message"]["content"][0]["type"],
            "image_url"
        );
        assert_eq!(
            product.client_body_json["choices"][0]["message"]["content"][0]["image_url"]["url"],
            "data:image/png;base64,iVBORw0KGgo="
        );
    }

    #[test]
    fn builds_standard_cross_format_sync_product_from_normalized_stream_payload() {
        let body = concat!(
            "data: {\"id\":\"chatcmpl_123\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_123\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5\",",
            "\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_123\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5\",",
            "\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],",
            "\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n",
            "data: [DONE]\n\n",
        );
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "claude:messages",
        });

        let product = maybe_build_standard_cross_format_sync_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("product build should succeed")
        .expect("product should exist");

        assert_eq!(
            product.provider_body_json,
            json!({
                "id": "chatcmpl_123",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-5",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hello",
                    },
                    "finish_reason": "stop",
                }],
                "usage": {
                    "prompt_tokens": 1,
                    "completion_tokens": 2,
                    "total_tokens": 3,
                },
            })
        );
        assert_eq!(
            product.client_body_json.get("type"),
            Some(&json!("message"))
        );
        assert_eq!(
            product.client_body_json.get("id"),
            Some(&json!("chatcmpl_123"))
        );
        assert_eq!(
            product.client_body_json.get("content"),
            Some(&json!([{ "type": "text", "text": "Hello" }]))
        );
        assert_eq!(
            product.client_body_json.get("stop_reason"),
            Some(&json!("end_turn"))
        );
    }

    #[test]
    fn falls_back_to_body_json_when_stream_aggregation_returns_none() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "claude:messages",
        });
        let provider_body_json = json!({
            "id": "chatcmpl_123",
            "object": "chat.completion",
            "created": 1,
            "model": "gpt-5",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello",
                },
                "finish_reason": "stop",
            }],
            "usage": {
                "prompt_tokens": 1,
                "completion_tokens": 2,
                "total_tokens": 3,
            },
        });

        let product = maybe_build_standard_cross_format_sync_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            Some(&base64::engine::general_purpose::STANDARD.encode("not an sse stream")),
        )
        .expect("fallback build should succeed");

        assert_eq!(
            product.expect("product should exist").provider_body_json,
            provider_body_json
        );
    }

    #[test]
    fn builds_standard_same_format_body_from_stream_payload() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-3-5-sonnet-latest\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":5,\"output_tokens\":7}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "claude:messages",
            "needs_conversion": false,
        });

        let body_json = maybe_build_standard_same_format_sync_body_from_normalized_payload(
            "claude_chat_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("same-format builder should succeed")
        .expect("body should exist");

        assert_eq!(body_json.get("type"), Some(&json!("message")));
        assert_eq!(body_json.get("role"), Some(&json!("assistant")));
        assert_eq!(
            body_json.get("content"),
            Some(&json!([{ "type": "text", "text": "hello" }]))
        );
    }

    #[test]
    fn falls_back_to_body_json_for_standard_same_format_sync_payload() {
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "gemini:generate_content",
            "needs_conversion": false,
        });
        let provider_body_json = json!({
            "candidates": [{
                "index": 0,
                "content": {
                    "parts": [{ "text": "hello" }],
                    "role": "model"
                },
                "finishReason": "STOP"
            }]
        });

        let body_json = maybe_build_standard_same_format_sync_body_from_normalized_payload(
            "gemini_cli_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("same-format sync body should succeed")
        .expect("body should exist");

        assert_eq!(body_json, provider_body_json);
    }

    #[test]
    fn same_format_claude_sync_body_sanitizes_read_tool_input() {
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "claude:messages",
            "needs_conversion": false,
        });
        let provider_body_json = json!({
            "id": "msg_read",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-6",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_read",
                    "name": "Read",
                    "input": {
                        "file_path": "/tmp/a.txt",
                        "limit": 20,
                        "pages": ""
                    }
                },
                {
                    "type": "tool_use",
                    "id": "toolu_search",
                    "name": "Search",
                    "input": {
                        "query": "",
                        "pages": ""
                    }
                }
            ]
        });

        let body_json = maybe_build_standard_same_format_sync_body_from_normalized_payload(
            "claude_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("same-format sync body should succeed")
        .expect("body should exist");

        assert_eq!(
            body_json["content"][0]["input"],
            json!({
                "file_path": "/tmp/a.txt",
                "limit": 20,
            })
        );
        assert_eq!(
            body_json["content"][1]["input"],
            json!({
                "query": "",
                "pages": "",
            })
        );
    }

    #[test]
    fn same_format_sync_response_restores_model_directive_display_model() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
            "model": "gpt-5.5-xhigh",
            "mapped_model": "gpt-5.5",
            "needs_conversion": false,
        });
        let provider_body_json = json!({
            "id": "resp_123",
            "object": "response",
            "model": "gpt-5.5",
            "status": "completed",
            "output": []
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_responses_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("same-format sync body should succeed")
        .expect("body should exist");
        let StandardSyncFinalizeNormalizedProduct::SuccessBody(body_json) = product else {
            panic!("same-format response should be a success body");
        };

        assert_eq!(body_json["model"], "gpt-5.5-xhigh");
    }

    #[test]
    fn cross_format_sync_response_restores_model_directive_display_model() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "claude:messages",
            "model": "gpt-5.5-xhigh",
            "mapped_model": "gpt-5.5",
            "needs_conversion": true,
        });
        let provider_body_json = json!({
            "id": "resp_123",
            "object": "response",
            "model": "gpt-5.5",
            "status": "completed",
            "output": [{
                "type": "message",
                "id": "msg_123",
                "role": "assistant",
                "status": "completed",
                "content": [{ "type": "output_text", "text": "done" }]
            }]
        });

        let product = maybe_build_standard_cross_format_sync_product(
            "claude_cli_sync_finalize",
            "openai:responses",
            "claude:messages",
            &report_context,
            provider_body_json,
        )
        .expect("cross-format product should exist");

        assert_eq!(product.client_body_json["model"], "gpt-5.5-xhigh");
        assert_eq!(product.provider_body_json["model"], "gpt-5.5");
    }

    #[test]
    fn rejects_standard_same_format_when_needs_conversion_is_true() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "openai:chat",
            "needs_conversion": true,
        });
        let provider_body_json = json!({ "id": "chatcmpl_123" });

        let body_json = maybe_build_standard_same_format_sync_body_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("same-format guard should not error");

        assert!(body_json.is_none());
    }

    #[test]
    fn rejects_standard_same_format_error_body_json() {
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "claude:messages",
            "needs_conversion": false,
        });
        let provider_body_json = json!({
            "type": "error",
            "error": {
                "type": "rate_limit_error",
                "message": "slow down"
            }
        });

        let body_json = maybe_build_standard_same_format_sync_body_from_normalized_payload(
            "claude_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("same-format error guard should not error");

        assert!(body_json.is_none());
    }

    #[test]
    fn builds_openai_responses_same_family_body_from_stream_payload() {
        let body = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
        );
        let report_context = json!({
            "provider_api_format": "openai:responses:compact",
            "client_api_format": "openai:responses:compact",
            "needs_conversion": false,
        });

        let body_json = maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
            "openai_responses_compact_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("openai-responses family stream should succeed")
        .expect("body should exist");

        assert_eq!(body_json.get("id"), Some(&json!("resp_123")));
        assert_eq!(body_json.get("status"), Some(&json!("completed")));
        assert_eq!(body_json["output"][0]["content"][0]["text"], json!("Hello"));
    }

    #[test]
    fn same_family_completed_metadata_uses_materialized_stream_output() {
        for terminal_response in [
            json!({
                "id": "resp_codex_terminal_metadata",
                "object": "response",
                "model": "gpt-5.6-sol",
                "status": "completed",
                "output": [],
                "usage": {"input_tokens": 1, "output_tokens": 2, "total_tokens": 3}
            }),
            json!({
                "id": "resp_codex_terminal_metadata",
                "object": "response",
                "model": "gpt-5.6-sol",
                "status": "completed",
                "usage": {"input_tokens": 1, "output_tokens": 2, "total_tokens": 3}
            }),
        ] {
            let body = format!(
                "event: response.created\ndata: {}\n\n\
                 event: response.output_text.delta\ndata: {}\n\n\
                 event: response.completed\ndata: {}\n\n",
                json!({
                    "type": "response.created",
                    "response": {
                        "id": "resp_codex_terminal_metadata",
                        "object": "response",
                        "model": "gpt-5.6-sol",
                        "status": "in_progress",
                        "output": []
                    }
                }),
                json!({
                    "type": "response.output_text.delta",
                    "output_index": 0,
                    "content_index": 0,
                    "delta": "Hello from Codex"
                }),
                json!({"type": "response.completed", "response": terminal_response})
            );
            let report_context = json!({
                "provider_api_format": "openai:responses",
                "client_api_format": "openai:responses",
                "needs_conversion": false,
            });

            let body_json =
                maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
                    "openai_responses_sync_finalize",
                    200,
                    Some(&report_context),
                    None,
                    Some(&base64::engine::general_purpose::STANDARD.encode(body)),
                )
                .expect("Responses stream should aggregate")
                .expect("sync response should exist");

            assert_eq!(
                body_json["output"][0]["content"][0]["text"],
                "Hello from Codex"
            );
        }
    }

    #[test]
    fn same_family_responses_stream_uses_terminal_response_as_authoritative_snapshot() {
        let terminal_response = json!({
            "id": "resp_gpt56_terminal",
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
                "input_tokens": 12,
                "output_tokens": 4,
                "total_tokens": 16,
                "input_tokens_details": {"cache_write_tokens": 3}
            }
        });
        let body = format!(
            "event: response.completed\ndata: {}\n\n",
            json!({"type": "response.completed", "response": terminal_response})
        );
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
            "needs_conversion": false,
        });

        let body_json = maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
            "openai_responses_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("same-family terminal snapshot extensions should remain lossless")
        .expect("terminal response should become the sync response");

        assert_eq!(body_json, terminal_response);
    }

    #[test]
    fn cross_format_responses_stream_still_rejects_unknown_gpt_5_6_events() {
        let body = concat!(
            "event: response.program.delta\n",
            "data: {\"type\":\"response.program.delta\",\"delta\":\"print\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_cross_unknown\",\"object\":\"response\",\"model\":\"gpt-5.6-sol\",\"status\":\"completed\",\"output\":[{\"type\":\"program\",\"id\":\"program-1\"}]}}\n\n",
        );
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "needs_conversion": true,
        });

        let result = maybe_build_openai_chat_cross_format_sync_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        );

        assert!(result.is_err());
    }

    #[test]
    fn same_family_responses_stream_accepts_authoritative_incomplete_snapshot() {
        let terminal_response = json!({
            "id": "resp_gpt56_incomplete",
            "object": "response",
            "model": "gpt-5.6-sol",
            "status": "incomplete",
            "output": [{
                "type": "message",
                "id": "msg_gpt56_incomplete",
                "role": "assistant",
                "status": "incomplete",
                "content": [{
                    "type": "output_text",
                    "text": "partial",
                    "annotations": []
                }]
            }],
            "incomplete_details": {"reason": "max_output_tokens"}
        });
        let body = format!(
            "event: response.output_text.delta\ndata: {{\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"partial\"}}\n\n\
             event: response.incomplete\ndata: {}\n\n",
            json!({"type": "response.incomplete", "response": terminal_response})
        );
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
            "needs_conversion": false,
        });

        let body_json = maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
            "openai_responses_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("incomplete terminal snapshot should remain a valid Responses result")
        .expect("terminal response should become the sync response");

        assert_eq!(body_json, terminal_response);
    }

    #[test]
    fn same_family_responses_stream_accepts_authoritative_failed_snapshot() {
        let terminal_response = json!({
            "id": "resp_gpt56_failed",
            "object": "response",
            "model": "gpt-5.6-sol",
            "status": "failed",
            "output": [{
                "type": "program",
                "call_id": "program-call-1",
                "fingerprint": "fp-program-1"
            }],
            "error": {"code": "server_error", "message": "upstream failed"}
        });
        let body = format!(
            "event: response.failed\ndata: {}\n\n",
            json!({
                "type": "response.failed",
                "sequence_number": 3,
                "response": terminal_response
            })
        );
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
            "needs_conversion": false,
        });

        let body_json = maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
            "openai_responses_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("failed terminal snapshot should remain an authoritative Responses result")
        .expect("failed terminal response should become the sync response");

        assert_eq!(body_json, terminal_response);
    }

    #[test]
    fn builds_openai_responses_same_family_body_from_legacy_outtext_delta_alias() {
        let body = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_legacy_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
            "event: response.outtext.delta\n",
            "data: {\"type\":\"response.outtext.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello from legacy alias\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_legacy_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello from legacy alias\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":4,\"total_tokens\":5}}}\n\n",
        );
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
            "needs_conversion": false,
        });

        let body_json = maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
            "openai_responses_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("openai-responses legacy alias aggregation should succeed")
        .expect("body should exist");

        assert_eq!(body_json["id"], "resp_legacy_123");
        assert_eq!(
            body_json["output"][0]["content"][0]["text"],
            json!("Hello from legacy alias")
        );
    }

    #[test]
    fn preserves_openai_responses_completed_output_when_already_present() {
        let body = concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"Ignored\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_preserve_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_preserve_123\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"Authoritative\",\"annotations\":[]}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
        );

        let result = aggregate_openai_responses_stream_sync_response(body.as_bytes())
            .expect("openai-responses stream should aggregate into a sync body");

        assert_eq!(result["output"][0]["content"][0]["text"], "Authoritative");
    }

    #[test]
    fn reconstructs_openai_responses_image_generation_call_from_output_item_done() {
        let body = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_image_123\",\"object\":\"response\",\"model\":\"gpt-5.4-mini\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_123\",\"type\":\"image_generation_call\",\"status\":\"completed\",\"output_format\":\"png\",\"result\":\"aGVsbG8=\"}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_image_123\",\"object\":\"response\",\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
        );

        let result = aggregate_openai_responses_stream_sync_response(body.as_bytes())
            .expect("openai-responses stream should aggregate into a sync body");

        assert_eq!(result["output"][0]["type"], "image_generation_call");
        assert_eq!(result["output"][0]["result"], "aGVsbG8=");
        assert_eq!(result["output"][0]["output_format"], "png");
    }

    #[test]
    fn reconstructs_openai_responses_compaction_without_output_index_or_terminal_output() {
        let body = concat!(
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"compaction\",\"encrypted_content\":\"ENCRYPTED_CONTEXT_COMPACTION_SUMMARY\"}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-compact\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0,\"total_tokens\":0}}}\n\n",
        );

        let result = aggregate_openai_responses_stream_sync_response(body.as_bytes())
            .expect("compaction stream should aggregate into a sync response");

        assert_eq!(result["output"].as_array().map(Vec::len), Some(1));
        assert_eq!(result["output"][0]["type"], "compaction");
        assert_eq!(
            result["output"][0]["encrypted_content"],
            "ENCRYPTED_CONTEXT_COMPACTION_SUMMARY"
        );
    }

    #[test]
    fn reconstructs_openai_responses_multi_part_message_content_order() {
        let body = concat!(
            "event: response.output_item.added\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_multi_123\",\"role\":\"assistant\",\"status\":\"in_progress\",\"content\":[]}}\n\n",
            "event: response.content_part.added\n",
            "data: {\"type\":\"response.content_part.added\",\"output_index\":0,\"content_index\":0,\"part\":{\"type\":\"output_text\",\"text\":\"Hello\",\"annotations\":[]}}\n\n",
            "event: response.content_part.added\n",
            "data: {\"type\":\"response.content_part.added\",\"output_index\":0,\"content_index\":1,\"part\":{\"type\":\"output_text\",\"text\":\" world\",\"annotations\":[]}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_multi_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[]}}\n\n",
        );

        let result = aggregate_openai_responses_stream_sync_response(body.as_bytes())
            .expect("openai-responses stream should aggregate into a sync body");

        assert_eq!(result["output"][0]["type"], "message");
        assert_eq!(result["output"][0]["content"][0]["text"], "Hello");
        assert_eq!(result["output"][0]["content"][1]["text"], " world");
    }

    #[test]
    fn aggregates_openai_responses_text_snapshot_deltas_without_duplicates() {
        let body = concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":{\"text\":\"Hello\"}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":{\"text\":\"Hello world\"}}\n\n",
            "event: response.output_text.done\n",
            "data: {\"type\":\"response.output_text.done\",\"output_index\":0,\"content_index\":0,\"text\":\"Hello world\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_snapshot_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[]}}\n\n",
        );

        let result = aggregate_openai_responses_stream_sync_response(body.as_bytes())
            .expect("openai-responses stream should aggregate into a sync body");

        assert_eq!(result["output"][0]["content"][0]["text"], "Hello world");
    }

    #[test]
    fn preserves_openai_responses_non_text_content_parts() {
        let body = concat!(
            "event: response.output_item.added\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_non_text_123\",\"role\":\"assistant\",\"status\":\"in_progress\",\"content\":[]}}\n\n",
            "event: response.content_part.done\n",
            "data: {\"type\":\"response.content_part.done\",\"output_index\":0,\"content_index\":0,\"part\":{\"type\":\"refusal\",\"refusal\":\"blocked\"}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_non_text_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[]}}\n\n",
        );

        let result = aggregate_openai_responses_stream_sync_response(body.as_bytes())
            .expect("openai-responses stream should aggregate into a sync body");

        assert_eq!(result["output"][0]["content"][0]["type"], "refusal");
        assert_eq!(result["output"][0]["content"][0]["refusal"], "blocked");
    }

    #[test]
    fn aggregates_official_refusal_stream_events() {
        let body = concat!(
            "event: response.output_item.added\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_refusal_123\",\"role\":\"assistant\",\"status\":\"in_progress\",\"content\":[]}}\n\n",
            "event: response.content_part.added\n",
            "data: {\"type\":\"response.content_part.added\",\"output_index\":0,\"content_index\":0,\"part\":{\"type\":\"refusal\",\"refusal\":\"\"}}\n\n",
            "event: response.refusal.delta\n",
            "data: {\"type\":\"response.refusal.delta\",\"output_index\":0,\"content_index\":0,\"item_id\":\"msg_refusal_123\",\"delta\":\"I can't\"}\n\n",
            "event: response.refusal.done\n",
            "data: {\"type\":\"response.refusal.done\",\"output_index\":0,\"content_index\":0,\"item_id\":\"msg_refusal_123\",\"refusal\":\"I can't help with that.\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_refusal_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[]}}\n\n",
        );

        let result = aggregate_openai_responses_stream_sync_response(body.as_bytes())
            .expect("openai-responses refusal stream should aggregate into a sync body");

        assert_eq!(result["output"][0]["content"][0]["type"], "refusal");
        assert_eq!(
            result["output"][0]["content"][0]["refusal"],
            "I can't help with that."
        );
        assert_eq!(result["output_text"], "");
    }

    #[test]
    fn aggregates_official_output_text_annotation_added_event() {
        let body = concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello annotated\"}\n\n",
            "event: response.output_text.annotation.added\n",
            "data: {\"type\":\"response.output_text.annotation.added\",\"output_index\":0,\"content_index\":0,\"annotation_index\":0,\"annotation\":{\"type\":\"text_annotation\",\"text\":\"annotated\",\"start\":6,\"end\":15}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_annotation_added_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[]}}\n\n",
        );

        let result = aggregate_openai_responses_stream_sync_response(body.as_bytes())
            .expect("openai-responses annotation stream should aggregate into a sync body");

        assert_eq!(result["output"][0]["content"][0]["text"], "Hello annotated");
        assert_eq!(
            result["output"][0]["content"][0]["annotations"][0]["type"],
            "text_annotation"
        );
        assert_eq!(
            result["output"][0]["content"][0]["annotations"][0]["start"],
            6
        );
        assert_eq!(result["output_text"], "Hello annotated");
    }

    #[test]
    fn authoritative_output_text_done_preserves_annotations() {
        let body = concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}\n\n",
            "event: response.output_text.done\n",
            "data: {\"type\":\"response.output_text.done\",\"output_index\":0,\"content_index\":0,\"part\":{\"type\":\"output_text\",\"text\":\"Hello\",\"annotations\":[{\"type\":\"url_citation\",\"url\":\"https://example.com\"}]}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_annotations_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[]}}\n\n",
        );

        let result = aggregate_openai_responses_stream_sync_response(body.as_bytes())
            .expect("openai-responses stream should aggregate into a sync body");

        assert_eq!(result["output"][0]["content"][0]["text"], "Hello");
        assert_eq!(
            result["output"][0]["content"][0]["annotations"][0]["type"],
            "url_citation"
        );
        assert_eq!(
            result["output"][0]["content"][0]["annotations"][0]["url"],
            "https://example.com"
        );
    }

    #[test]
    fn function_call_arguments_done_uses_item_metadata() {
        let body = concat!(
            "event: response.function_call_arguments.delta\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"item_id\":\"fc_done_123\",\"delta\":\"{\\\"location\\\":\"}\n\n",
            "event: response.function_call_arguments.done\n",
            "data: {\"type\":\"response.function_call_arguments.done\",\"output_index\":0,\"item_id\":\"fc_done_123\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_done_123\",\"call_id\":\"call_done_weather\",\"name\":\"get_weather\",\"arguments\":\"{\\\"location\\\": \\\"Tokyo\\\"}\"}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_done_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[]}}\n\n",
        );

        let result = aggregate_openai_responses_stream_sync_response(body.as_bytes())
            .expect("openai-responses stream should aggregate into a sync body");

        assert_eq!(result["output"][0]["type"], "function_call");
        assert_eq!(result["output"][0]["call_id"], "call_done_weather");
        assert_eq!(result["output"][0]["name"], "get_weather");
        assert_eq!(result["output"][0]["arguments"], r#"{"location": "Tokyo"}"#);
    }

    #[test]
    fn validates_and_preserves_chat_backed_function_call_metadata() {
        let body = concat!(
            "event: response.output_item.added\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_chat_metadata_123\",\"call_id\":\"call_chat_metadata_123\",\"status\":\"completed\",\"arguments\":\"{\\\"query\\\":\\\"aether\\\"}\",\"name\":\"lookup\",\"metadata\":{\"source\":\"chat\"},\"internal_chat_message_metadata_passthrough\":{\"turn_id\":\"turn_123\"}}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_chat_metadata_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
        );

        let result = try_aggregate_openai_responses_stream_sync_response(body.as_bytes())
            .expect("chat-backed function call metadata should pass stream validation")
            .expect("chat-backed function call stream should aggregate");

        assert_eq!(result["output"][0]["type"], "function_call");
        assert_eq!(result["output"][0]["call_id"], "call_chat_metadata_123");
        assert_eq!(result["output"][0]["metadata"]["source"], "chat");
        assert_eq!(
            result["output"][0]["internal_chat_message_metadata_passthrough"]["turn_id"],
            "turn_123"
        );
    }

    #[test]
    fn custom_tool_call_input_events_materialize_custom_tool_call() {
        let body = concat!(
            "event: response.custom_tool_call_input.delta\n",
            "data: {\"type\":\"response.custom_tool_call_input.delta\",\"output_index\":0,\"item_id\":\"ctc_123\",\"name\":\"code_exec\",\"delta\":\"print\"}\n\n",
            "event: response.custom_tool_call_input.done\n",
            "data: {\"type\":\"response.custom_tool_call_input.done\",\"output_index\":0,\"item_id\":\"ctc_123\",\"call_id\":\"call_custom_123\",\"name\":\"code_exec\",\"input\":\"print('hi')\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_custom_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[]}}\n\n",
        );

        let result = aggregate_openai_responses_stream_sync_response(body.as_bytes())
            .expect("custom tool stream should aggregate into a sync body");

        assert_eq!(result["output"][0]["type"], "custom_tool_call");
        assert_eq!(result["output"][0]["call_id"], "call_custom_123");
        assert_eq!(result["output"][0]["name"], "code_exec");
        assert_eq!(result["output"][0]["input"], "print('hi')");
        assert!(result["output"][0].get("arguments").is_none());
    }

    #[test]
    fn aggregates_modern_reasoning_text_and_response_done_alias() {
        let body = concat!(
            "event: response.reasoning_text.delta\n",
            "data: {\"type\":\"response.reasoning_text.delta\",\"output_index\":0,\"delta\":\"Need\"}\n\n",
            "event: response.reasoning_text.done\n",
            "data: {\"type\":\"response.reasoning_text.done\",\"output_index\":0,\"text\":\"Need care\"}\n\n",
            "event: response.done\n",
            "data: {\"type\":\"response.done\",\"response\":{\"id\":\"resp_done_alias_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\"}}\n\n",
        );

        let result = aggregate_openai_responses_stream_sync_response(body.as_bytes())
            .expect("modern response.done stream should aggregate");

        assert_eq!(result["output"][0]["type"], "reasoning");
        assert_eq!(result["output"][0]["summary"][0]["text"], "Need care");
        assert!(result["output"].as_array().is_some());
        assert_eq!(result["output_text"], "");
        assert!(result["completed_at"].as_i64().is_some());
    }

    #[test]
    fn accepts_openai_responses_same_family_stream_when_needs_conversion_is_true() {
        let body = concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[]}}\n\n",
        );
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses:compact",
            "needs_conversion": true,
        });

        let body_json = maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
            "openai_responses_compact_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("openai-responses same-family aggregation should not error")
        .expect("aggregated body should exist");

        assert_eq!(body_json["id"], "resp_123");
    }

    #[test]
    fn falls_back_to_body_json_for_openai_responses_same_family_sync_payload() {
        let report_context = json!({
            "provider_api_format": "openai:responses:compact",
            "client_api_format": "openai:responses:compact",
            "needs_conversion": false,
        });
        let provider_body_json = json!({
            "id": "resp_123",
            "object": "response",
            "status": "completed",
            "output": []
        });

        let body_json = maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
            "openai_responses_compact_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("openai-responses same-family sync should succeed")
        .expect("body should exist");

        assert_eq!(body_json, provider_body_json);
    }

    #[test]
    fn allows_openai_responses_same_family_cross_format_sync_when_conversion_is_flagged() {
        let report_context = json!({
            "provider_api_format": "openai:responses:compact",
            "client_api_format": "openai:responses",
            "needs_conversion": true,
        });
        let provider_body_json = json!({
            "id": "resp_family_123",
            "object": "response",
            "status": "completed",
            "output": []
        });

        let body_json = maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
            "openai_responses_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("openai-responses cross-family sync should succeed")
        .expect("body should exist");

        assert_eq!(body_json, provider_body_json);
    }

    #[test]
    fn rejects_openai_responses_same_family_error_body_json() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses:compact",
            "model": "gpt-5",
            "mapped_model": "gpt-5",
        });
        let provider_body_json = json!({
            "error": {
                "message": "quota reached",
                "type": "rate_limit_error"
            }
        });

        let body_json = maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
            "openai_responses_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("openai-responses same-family error guard should not error");

        assert!(body_json.is_none());
    }

    #[test]
    fn builds_openai_chat_cross_format_sync_product_from_claude_body_json() {
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "mapped_model": "claude-sonnet-4",
        });
        let provider_body_json = json!({
            "id": "msg_claude_direct_123",
            "type": "message",
            "model": "claude-sonnet-4",
            "role": "assistant",
            "content": [{"type": "text", "text": "Hello Claude"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 2,
                "output_tokens": 3
            }
        });

        let product = maybe_build_openai_chat_cross_format_sync_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("openai-chat cross-format should succeed")
        .expect("product should exist");

        assert_eq!(product.provider_body_json, provider_body_json);
        assert_eq!(
            product.client_body_json["choices"][0]["message"]["content"],
            "Hello Claude"
        );
    }

    #[test]
    fn claude_canonical_response_route_matches_legacy_outputs_for_existing_clients() {
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "mapped_model": "claude-sonnet-4",
            "original_request_body": {
                "service_tier": "default"
            }
        });
        let provider_body_json = json!({
            "id": "msg_claude_canonical_123",
            "type": "message",
            "model": "claude-sonnet-4",
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "plan", "signature": "sig_123"},
                {"type": "text", "text": "Hello Claude"},
                {
                    "type": "tool_use",
                    "id": "toolu_123",
                    "name": "lookup",
                    "input": {"q": "rust"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 2,
                "output_tokens": 3,
                "cache_read_input_tokens": 1,
                "cache_creation_input_tokens": 1
            }
        });

        let legacy_openai =
            convert_claude_chat_response_to_openai_chat(&provider_body_json, &report_context)
                .expect("legacy claude -> openai chat");
        let converted_openai = convert_standard_chat_response(
            &provider_body_json,
            "claude:messages",
            "openai:chat",
            &report_context,
        )
        .expect("canonical claude -> openai chat");
        assert_eq!(converted_openai, legacy_openai);

        let legacy_claude =
            convert_openai_chat_response_to_claude_chat(&legacy_openai, &report_context)
                .expect("legacy claude -> openai chat -> claude");
        let converted_claude = convert_standard_chat_response(
            &provider_body_json,
            "claude:messages",
            "claude:messages",
            &report_context,
        )
        .expect("canonical claude -> claude");
        assert_eq!(converted_claude, legacy_claude);

        let legacy_openai_responses = convert_openai_chat_response_to_openai_responses(
            &legacy_openai,
            &report_context,
            false,
        )
        .expect("legacy claude -> openai responses");
        let converted_openai_responses = convert_standard_cli_response(
            &provider_body_json,
            "claude:messages",
            "openai:responses",
            &report_context,
        )
        .expect("canonical claude -> openai responses");
        assert_eq!(converted_openai_responses, legacy_openai_responses);
    }

    #[test]
    fn gemini_canonical_response_route_matches_legacy_outputs_for_single_candidate() {
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "mapped_model": "gemini-2.5-pro",
        });
        let provider_body_json = json!({
            "responseId": "resp_gemini_canonical_123",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [{
                "index": 0,
                "finishReason": "STOP",
                "content": {
                    "role": "model",
                    "parts": [
                        {"text": "plan", "thought": true, "thoughtSignature": "sig_123"},
                        {"text": "Hello Gemini"},
                        {
                            "functionCall": {
                                "id": "call_123",
                                "name": "lookup",
                                "args": {"q": "rust"}
                            }
                        }
                    ]
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 2,
                "candidatesTokenCount": 3,
                "thoughtsTokenCount": 1,
                "totalTokenCount": 6
            }
        });

        let legacy_openai =
            convert_gemini_chat_response_to_openai_chat(&provider_body_json, &report_context)
                .expect("legacy gemini -> openai chat");
        let converted_openai = convert_standard_chat_response(
            &provider_body_json,
            "gemini:generate_content",
            "openai:chat",
            &report_context,
        )
        .expect("canonical gemini -> openai chat");
        assert_eq!(converted_openai, legacy_openai);

        let legacy_claude =
            convert_openai_chat_response_to_claude_chat(&legacy_openai, &report_context)
                .expect("legacy gemini -> openai chat -> claude");
        let converted_claude = convert_standard_chat_response(
            &provider_body_json,
            "gemini:generate_content",
            "claude:messages",
            &report_context,
        )
        .expect("canonical gemini -> claude");
        assert_eq!(converted_claude, legacy_claude);

        let legacy_openai_responses = convert_openai_chat_response_to_openai_responses(
            &legacy_openai,
            &report_context,
            false,
        )
        .expect("legacy gemini -> openai responses");
        let converted_openai_responses = convert_standard_cli_response(
            &provider_body_json,
            "gemini:generate_content",
            "openai:responses",
            &report_context,
        )
        .expect("canonical gemini -> openai responses");
        assert_eq!(converted_openai_responses, legacy_openai_responses);
    }

    #[test]
    fn openai_chat_canonical_response_route_matches_legacy_outputs_for_single_choice() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "claude:messages",
            "model": "gpt-5",
            "mapped_model": "gpt-5",
        });
        let provider_body_json = json!({
            "id": "chatcmpl_canonical_123",
            "object": "chat.completion",
            "model": "gpt-5",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello OpenAI",
                    "reasoning_parts": [{
                        "type": "thinking",
                        "thinking": "plan",
                        "signature": "sig_123"
                    }],
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "lookup",
                            "arguments": "{\"q\":\"rust\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 2,
                "completion_tokens": 4,
                "total_tokens": 6,
                "prompt_tokens_details": {
                    "cached_tokens": 1
                },
                "completion_tokens_details": {
                    "reasoning_tokens": 1
                }
            }
        });

        let legacy_claude =
            convert_openai_chat_response_to_claude_chat(&provider_body_json, &report_context)
                .expect("legacy openai chat -> claude");
        let converted_claude = convert_standard_chat_response(
            &provider_body_json,
            "openai:chat",
            "claude:messages",
            &report_context,
        )
        .expect("canonical openai chat -> claude");
        assert_eq!(converted_claude, legacy_claude);
        assert_eq!(converted_claude["usage"]["input_tokens"], 1);
        assert_eq!(converted_claude["usage"]["cache_read_input_tokens"], 1);
        assert_eq!(converted_claude["usage"]["output_tokens"], 4);

        let legacy_gemini =
            convert_openai_chat_response_to_gemini_chat(&provider_body_json, &report_context)
                .expect("legacy openai chat -> gemini");
        let converted_gemini = convert_standard_chat_response(
            &provider_body_json,
            "openai:chat",
            "gemini:generate_content",
            &report_context,
        )
        .expect("canonical openai chat -> gemini");
        assert_eq!(converted_gemini, legacy_gemini);

        let converted_openai_responses = convert_standard_cli_response(
            &provider_body_json,
            "openai:chat",
            "openai:responses",
            &report_context,
        )
        .expect("canonical openai chat -> openai responses");
        assert_eq!(converted_openai_responses["id"], "resp_canonical_123");
        assert!(
            converted_openai_responses["output"]
                .as_array()
                .is_some_and(|output| output.iter().any(|item| item["type"] == "reasoning")),
            "canonical OpenAI Chat -> Responses should preserve reasoning blocks"
        );
    }

    #[test]
    fn openai_responses_canonical_response_route_matches_legacy_chat_outputs() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "mapped_model": "gpt-5-upstream",
        });
        let provider_body_json = json!({
            "id": "resp_cli_canonical_123",
            "object": "response",
            "created_at": 1741476542i64,
            "status": "completed",
            "model": "gpt-5-upstream",
            "service_tier": "flex",
            "output": [{
                "type": "reasoning",
                "id": "rs_1",
                "summary": [{
                    "type": "summary_text",
                    "text": "because"
                }]
            }, {
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "Hello",
                    "annotations": [{
                        "type": "file_citation",
                        "start_index": 0,
                        "end_index": 5
                    }]
                }, {
                    "type": "refusal",
                    "refusal": "partial refusal"
                }]
            }, {
                "type": "function_call",
                "id": "call_1",
                "call_id": "call_1",
                "name": "lookup",
                "arguments": "{\"q\":\"rust\"}"
            }, {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": {"ok": true}
            }],
            "usage": {
                "input_tokens": 3,
                "input_tokens_details": {"cached_tokens": 2},
                "output_tokens": 5,
                "output_tokens_details": {"reasoning_tokens": 1},
                "total_tokens": 8
            }
        });

        let legacy_openai_chat =
            convert_openai_responses_response_to_openai_chat(&provider_body_json, &report_context)
                .expect("legacy openai responses -> openai chat");
        let converted_openai_chat = convert_standard_chat_response(
            &provider_body_json,
            "openai:responses",
            "openai:chat",
            &report_context,
        )
        .expect("canonical openai responses -> openai chat");
        assert_eq!(converted_openai_chat, legacy_openai_chat);

        let mut representable_body_json = provider_body_json.clone();
        representable_body_json
            .as_object_mut()
            .expect("response object")
            .remove("service_tier");
        representable_body_json["output"][1]["content"] = json!([{
            "type": "output_text",
            "text": "Hello",
            "annotations": []
        }]);

        let converted_claude = convert_standard_chat_response(
            &representable_body_json,
            "openai:responses",
            "claude:messages",
            &report_context,
        )
        .expect("canonical openai responses -> claude");
        assert_eq!(converted_claude["content"][3]["type"], "tool_result");
        assert_eq!(converted_claude["content"][3]["tool_use_id"], "call_1");
        assert_eq!(
            converted_claude["content"][3]["content"],
            json!({"ok": true})
        );
        assert_eq!(converted_claude["usage"]["input_tokens"], 1);
        assert_eq!(converted_claude["usage"]["cache_read_input_tokens"], 2);
        assert_eq!(converted_claude["usage"]["output_tokens"], 5);

        let converted_gemini = convert_standard_chat_response(
            &representable_body_json,
            "openai:responses",
            "gemini:generate_content",
            &report_context,
        )
        .expect("canonical openai responses -> gemini");
        assert_eq!(
            converted_gemini["candidates"][0]["content"]["parts"][3]["functionResponse"]["id"],
            "call_1"
        );
        assert_eq!(
            converted_gemini["candidates"][0]["content"]["parts"][3]["functionResponse"]
                ["response"],
            json!({"ok": true})
        );

        let converted_openai_responses = convert_standard_cli_response(
            &provider_body_json,
            "openai:responses",
            "openai:responses",
            &report_context,
        )
        .expect("canonical openai responses -> openai responses");
        assert_eq!(
            converted_openai_responses["output"][3]["type"],
            "function_call_output"
        );
        assert_eq!(
            converted_openai_responses["usage"]["output_tokens_details"]["reasoning_tokens"],
            1
        );
    }

    #[test]
    fn builds_openai_chat_cross_format_sync_product_from_gemini_body_json() {
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "mapped_model": "gemini-2.5-pro",
        });
        let provider_body_json = json!({
            "responseId": "resp_gemini_direct_123",
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello Gemini"}],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "modelVersion": "gemini-2.5-pro-upstream",
            "usageMetadata": {
                "promptTokenCount": 1,
                "candidatesTokenCount": 2,
                "totalTokenCount": 3
            }
        });

        let product = maybe_build_openai_chat_cross_format_sync_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("openai-chat cross-format should succeed")
        .expect("product should exist");

        assert_eq!(product.provider_body_json, provider_body_json);
        assert_eq!(
            product.client_body_json["choices"][0]["message"]["content"],
            "Hello Gemini"
        );
        assert_eq!(product.client_body_json["usage"]["completion_tokens"], 2);
    }

    #[test]
    fn builds_openai_chat_cross_format_sync_product_from_openai_responses_body_json() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "model": "gpt-5.4",
            "mapped_model": "gpt-5.4",
        });
        let provider_body_json = json!({
            "id": "resp_cli_direct_123",
            "object": "response",
            "status": "completed",
            "model": "gpt-5.4",
            "output": [{
                "type": "message",
                "id": "msg_cli_direct_123",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": "Hello CLI",
                    "annotations": []
                }]
            }]
        });

        let product = maybe_build_openai_chat_cross_format_sync_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("openai-chat cross-format should succeed")
        .expect("product should exist");

        assert_eq!(product.provider_body_json, provider_body_json);
        assert_eq!(
            product.client_body_json["choices"][0]["message"]["content"],
            "Hello CLI"
        );
    }

    #[test]
    fn builds_openai_responses_cross_format_sync_product_from_gemini_stream_payload() {
        let body = concat!(
            "data: {\"responseId\":\"resp_cli_stream_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello \"}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"gemini-2.5-pro-upstream\"}\n\n",
            "data: {\"responseId\":\"resp_cli_stream_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello Gemini CLI\"}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"gemini-2.5-pro-upstream\",\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}}\n\n",
        );
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "openai:responses",
            "model": "gpt-5",
            "mapped_model": "gemini-2.5-pro-upstream",
        });

        let product =
            maybe_build_openai_responses_cross_format_sync_product_from_normalized_payload(
                "openai_responses_sync_finalize",
                200,
                Some(&report_context),
                None,
                Some(&base64::engine::general_purpose::STANDARD.encode(body)),
            )
            .expect("openai-responses cross-format should succeed")
            .expect("product should exist");

        assert_eq!(
            product.provider_body_json["responseId"],
            "resp_cli_stream_123"
        );
        assert_eq!(product.client_body_json["object"], "response");
        assert_eq!(
            product.client_body_json["output"][0]["content"][0]["text"],
            "Hello Gemini CLI"
        );
    }

    #[test]
    fn rejects_openai_responses_cross_format_error_body_json() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses:compact",
            "model": "gpt-5",
            "mapped_model": "gpt-5",
        });
        let provider_body_json = json!({
            "error": {
                "message": "quota reached",
                "type": "rate_limit_error"
            }
        });

        let product =
            maybe_build_openai_responses_cross_format_sync_product_from_normalized_payload(
                "openai_responses_compact_sync_finalize",
                200,
                Some(&report_context),
                Some(&provider_body_json),
                None,
            )
            .expect("openai-responses cross-format error guard should not error");

        assert!(product.is_none());
    }

    #[test]
    fn rejects_openai_responses_cross_format_for_openai_family_provider() {
        let report_context = json!({
            "provider_api_format": "openai:responses:compact",
            "client_api_format": "openai:responses",
            "model": "gpt-5",
            "mapped_model": "gpt-5",
        });
        let provider_body_json = json!({
            "id": "resp_123",
            "object": "response",
            "status": "completed",
            "output": []
        });

        let product =
            maybe_build_openai_responses_cross_format_sync_product_from_normalized_payload(
                "openai_responses_sync_finalize",
                200,
                Some(&report_context),
                Some(&provider_body_json),
                None,
            )
            .expect("openai-responses cross-format openai-family guard should not error");

        assert!(product.is_none());
    }

    #[test]
    fn rejects_openai_chat_cross_format_for_unsupported_matrix() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "openai:chat",
        });
        let provider_body_json = json!({ "id": "chatcmpl_123" });

        let product = maybe_build_openai_chat_cross_format_sync_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("unsupported matrix should not error");

        assert!(product.is_none());
    }

    #[test]
    fn strict_response_conversion_errors_do_not_use_legacy_fallback() {
        let openai_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "claude:messages",
            "model": "claude-sonnet-4-5",
            "mapped_model": "gpt-5",
        });
        let openai_body = json!({
            "id": "chatcmpl_unknown_finish",
            "object": "chat.completion",
            "model": "gpt-5",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "done"
                },
                "finish_reason": "future_reason"
            }]
        });

        assert!(convert_standard_chat_response(
            &openai_body,
            "openai:chat",
            "claude:messages",
            &openai_context,
        )
        .is_none());

        let claude_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "mapped_model": "claude-sonnet-4-5",
        });
        let claude_body = json!({
            "id": "msg_unknown_stop",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-5",
            "content": [],
            "stop_reason": "future_reason",
            "usage": {
                "input_tokens": 1,
                "output_tokens": 2
            }
        });

        assert!(convert_standard_chat_response(
            &claude_body,
            "claude:messages",
            "openai:chat",
            &claude_context,
        )
        .is_none());
    }

    #[test]
    fn stream_finalize_rejects_unknown_openai_chat_events_without_body_fallback() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "claude:messages",
            "model": "claude-sonnet-4-5",
            "mapped_model": "gpt-5",
        });
        let stream_body = "data: {\"id\":\"chatcmpl_unknown_stream\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"future_delta\":\"x\"}}]}\n\n";
        let fallback_body = json!({
            "id": "chatcmpl_fallback",
            "object": "chat.completion",
            "model": "gpt-5",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "fallback"},
                "finish_reason": "stop"
            }]
        });

        let error = maybe_build_standard_cross_format_sync_product_from_normalized_payload(
            "claude_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&fallback_body),
            Some(&base64::engine::general_purpose::STANDARD.encode(stream_body)),
        )
        .expect_err("unknown stream event should fail closed before body fallback");

        assert!(error
            .to_string()
            .contains("Unsupported provider stream event cannot be converted losslessly"));
    }

    #[test]
    fn stream_finalize_rejects_unknown_claude_events_without_body_fallback() {
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "mapped_model": "claude-sonnet-4-5",
        });
        let stream_body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_unknown_stream\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null}}\n\n",
            "event: future_event\n",
            "data: {\"type\":\"future_event\",\"payload\":{\"kept\":true}}\n\n",
        );
        let fallback_body = json!({
            "id": "msg_fallback",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-5",
            "content": [{"type": "text", "text": "fallback"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 1,
                "output_tokens": 2
            }
        });

        let error = maybe_build_standard_cross_format_sync_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&fallback_body),
            Some(&base64::engine::general_purpose::STANDARD.encode(stream_body)),
        )
        .expect_err("unknown stream event should fail closed before body fallback");

        assert!(error
            .to_string()
            .contains("Unsupported provider stream event cannot be converted losslessly"));
        assert!(error
            .to_string()
            .contains("field $.type = \"future_event\""));
    }

    #[test]
    fn responses_same_family_stream_finalize_rejects_unknown_events_without_body_fallback() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
            "needs_conversion": false,
            "model": "gpt-5",
            "mapped_model": "gpt-5",
        });
        let stream_body = concat!(
            "event: response.future.delta\n",
            "data: {\"type\":\"response.future.delta\",\"response\":{\"id\":\"resp_unknown_stream\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"in_progress\"},\"payload\":{\"kept\":true}}\n\n",
        );
        let fallback_body = json!({
            "id": "resp_fallback",
            "object": "response",
            "model": "gpt-5",
            "status": "completed",
            "output": []
        });

        let error = maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
            "openai_responses_sync_finalize",
            200,
            Some(&report_context),
            Some(&fallback_body),
            Some(&base64::engine::general_purpose::STANDARD.encode(stream_body)),
        )
        .expect_err("unknown stream event should fail closed before body fallback");

        assert!(error
            .to_string()
            .contains("Unsupported provider stream event cannot be converted losslessly"));
        assert!(error
            .to_string()
            .contains("field $.type = \"response.future.delta\""));
    }

    #[test]
    fn responses_same_family_stream_rejects_unknown_event_before_valid_terminal_snapshot() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
            "needs_conversion": false,
            "model": "gpt-5",
            "mapped_model": "gpt-5",
        });
        let stream_body = concat!(
            "data: {\"type\":\"response.future.delta\",\"payload\":{\"must_not_drop\":true}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_unknown_before_terminal\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_unknown_before_terminal\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"must not pass\",\"annotations\":[]}]}]}}\n\n",
        );

        let error = maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(
            "openai_responses_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(stream_body)),
        )
        .expect_err("unknown event must fail closed even before a valid terminal snapshot");

        assert!(error
            .to_string()
            .contains("Unsupported provider stream event cannot be converted losslessly"));
    }

    #[test]
    fn responses_stream_rejects_conflicting_or_multiple_authoritative_terminals() {
        let cases = [
            (
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_conflicting_terminal\",\"object\":\"response\",\"status\":\"incomplete\",\"output\":[]}}\n\n",
                "conflicts with response.status",
            ),
            (
                concat!(
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_multiple_terminal\",\"object\":\"response\",\"status\":\"completed\",\"output\":[]}}\n\n",
                    "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_multiple_terminal\",\"object\":\"response\",\"status\":\"failed\",\"output\":[],\"error\":{\"code\":\"server_error\",\"message\":\"late failure\"}}}\n\n",
                ),
                "multiple authoritative terminal events",
            ),
            (
                concat!(
                    "event: response.future.delta\n",
                    "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"disguised\"}\n\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_disguised_unknown\",\"object\":\"response\",\"status\":\"completed\",\"output\":[]}}\n\n",
                ),
                "SSE event name conflicts with payload.type",
            ),
            (
                concat!(
                    "event: response.failed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_disguised_terminal\",\"object\":\"response\",\"status\":\"completed\",\"output\":[]}}\n\n",
                ),
                "SSE event name conflicts with payload.type",
            ),
        ];

        for (stream_body, expected_message) in cases {
            let error = try_aggregate_openai_responses_stream_sync_response(stream_body.as_bytes())
                .expect_err("invalid terminal lifecycle must fail closed");
            assert!(
                error.to_string().contains(expected_message),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn standard_sync_finalize_product_prefers_same_format_success_body() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "openai:chat",
            "needs_conversion": false,
        });
        let provider_body_json = json!({
            "id": "chatcmpl_123",
            "object": "chat.completion"
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("dispatch should succeed");

        assert_eq!(
            product,
            Some(StandardSyncFinalizeNormalizedProduct::SuccessBody(
                provider_body_json
            ))
        );
    }

    #[test]
    fn standard_sync_finalize_uses_explicit_stream_event_format_for_same_format_body() {
        let body = concat!(
            "event: keepalive\n",
            "data: {\"type\":\"keepalive\",\"sequence_number\":1}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"response_id\":\"resp_keepalive_sync_123\",\"output_index\":0,\"content_index\":0,\"delta\":\"pong\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_keepalive_sync_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[]}}\n\n",
        );
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "provider_stream_event_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "needs_conversion": false,
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("responses stream event format should aggregate")
        .expect("dispatch should produce a body");

        let StandardSyncFinalizeNormalizedProduct::SuccessBody(body_json) = product else {
            panic!("same-format response should be a success body");
        };
        assert_eq!(body_json["object"], "chat.completion");
        assert_eq!(body_json["choices"][0]["message"]["content"], "pong");
    }

    #[test]
    fn standard_sync_finalize_converts_chat_backed_function_call_metadata() {
        let body = concat!(
            "event: response.output_item.added\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_chat_metadata_123\",\"call_id\":\"call_chat_metadata_123\",\"status\":\"completed\",\"arguments\":\"{\\\"query\\\":\\\"aether\\\"}\",\"name\":\"lookup\",\"metadata\":{\"source\":\"chat\"},\"internal_chat_message_metadata_passthrough\":{\"turn_id\":\"turn_123\"}}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_chat_metadata_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
        );
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "needs_conversion": true,
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("chat-backed function call metadata should convert")
        .expect("dispatch should produce a body");

        let StandardSyncFinalizeNormalizedProduct::CrossFormat(product) = product else {
            panic!("Chat-to-Responses conversion should produce a cross-format body");
        };
        let body_json = product.client_body_json;
        let tool_call = &body_json["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(tool_call["id"], "call_chat_metadata_123");
        assert_eq!(tool_call["function"]["name"], "lookup");
        assert_eq!(tool_call["function"]["arguments"], "{\"query\":\"aether\"}");
        assert!(tool_call.get("metadata").is_none());
        assert!(tool_call
            .get("internal_chat_message_metadata_passthrough")
            .is_none());
    }

    #[test]
    fn standard_sync_finalize_preserves_responses_wire_metadata_for_responses_client() {
        let body = concat!(
            "event: response.output_item.added\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_chat_metadata_123\",\"call_id\":\"call_chat_metadata_123\",\"status\":\"completed\",\"arguments\":\"{\\\"query\\\":\\\"aether\\\"}\",\"name\":\"lookup\",\"metadata\":{\"source\":\"chat\"},\"internal_chat_message_metadata_passthrough\":{\"turn_id\":\"turn_123\"}}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_chat_metadata_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
        );
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "provider_stream_event_api_format": "openai:responses",
            "client_api_format": "openai:responses",
            "needs_conversion": true,
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_responses_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("Responses wire metadata should aggregate")
        .expect("dispatch should produce a body");

        let StandardSyncFinalizeNormalizedProduct::SuccessBody(body_json) = product else {
            panic!("matching Responses wire format should produce a success body");
        };
        assert_eq!(body_json["output"][0]["type"], "function_call");
        assert_eq!(body_json["output"][0]["metadata"]["source"], "chat");
        assert_eq!(
            body_json["output"][0]["internal_chat_message_metadata_passthrough"]["turn_id"],
            "turn_123"
        );
    }

    #[test]
    fn standard_same_format_gemini_finalize_rejects_usage_only_body() {
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "gemini:generate_content",
            "needs_conversion": false,
        });
        let provider_body_json = json!({
            "responseId": "resp-empty-visible-output",
            "modelVersion": "gemini-3-flash-preview",
            "usageMetadata": {
                "promptTokenCount": 8,
                "totalTokenCount": 8
            }
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "gemini_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("dispatch should succeed");

        assert_eq!(product, None);
    }

    #[test]
    fn standard_cross_format_finalize_uses_explicit_stream_event_format_for_provider_body() {
        let body = concat!(
            "event: keepalive\n",
            "data: {\"type\":\"keepalive\",\"sequence_number\":1}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"response_id\":\"resp_keepalive_cross_123\",\"output_index\":0,\"content_index\":0,\"delta\":\"pong\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_keepalive_cross_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[]}}\n\n",
        );
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "provider_stream_event_api_format": "openai:responses",
            "client_api_format": "claude:messages",
            "model": "claude-sonnet-4-5",
            "mapped_model": "gpt-5",
            "needs_conversion": true,
        });
        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "claude_chat_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("responses stream event format should aggregate")
        .expect("dispatch should produce a product");

        let StandardSyncFinalizeNormalizedProduct::CrossFormat(product) = product else {
            panic!("cross-format response should produce a conversion product");
        };
        assert_eq!(product.provider_body_json["object"], "response");
        assert_eq!(product.client_body_json["type"], "message");
        assert_eq!(product.client_body_json["content"][0]["text"], "pong");
    }

    #[test]
    fn standard_sync_finalize_product_handles_openai_responses_same_family_body() {
        let report_context = json!({
            "provider_api_format": "openai:responses:compact",
            "client_api_format": "openai:responses:compact",
            "needs_conversion": false,
        });
        let provider_body_json = json!({
            "id": "resp_123",
            "object": "response",
            "status": "completed",
            "output": []
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_responses_compact_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("dispatch should succeed");

        assert_eq!(
            product,
            Some(StandardSyncFinalizeNormalizedProduct::SuccessBody(
                provider_body_json
            ))
        );
    }

    #[test]
    fn standard_sync_finalize_aggregates_openai_responses_capture_envelope_for_cross_format() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "model": "gpt-5.6-luna",
            "mapped_model": "gpt-5.6-luna",
            "needs_conversion": true,
        });
        let provider_body_json = json!({
            "chunks": [
                {
                    "type": "response.output_text.delta",
                    "response_id": "resp_capture_123",
                    "output_index": 0,
                    "content_index": 0,
                    "delta": "Hello "
                },
                {
                    "type": "response.output_text.done",
                    "response_id": "resp_capture_123",
                    "output_index": 0,
                    "content_index": 0,
                    "text": "Hello capture"
                },
                {
                    "type": "response.output_item.done",
                    "output_index": 0,
                    "item": {
                        "id": "msg_capture_123",
                        "type": "message",
                        "status": "completed",
                        "role": "assistant",
                        "phase": "final_answer",
                        "content": [{
                            "type": "output_text",
                            "text": "Hello capture",
                            "annotations": [],
                            "logprobs": []
                        }]
                    }
                },
                {
                    "type": "response.completed",
                    "response": {
                        "id": "resp_capture_123",
                        "object": "response",
                        "status": "completed",
                        "model": "gpt-5.6-luna",
                        "background": false,
                        "max_output_tokens": null,
                        "output": [],
                        "usage": {
                            "input_tokens": 2,
                            "output_tokens": 3,
                            "total_tokens": 5
                        }
                    }
                }
            ],
            "metadata": {}
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("capture envelope aggregation should succeed")
        .expect("capture envelope should produce a cross-format product");

        let StandardSyncFinalizeNormalizedProduct::CrossFormat(product) = product else {
            panic!("responses capture envelope should be converted for an OpenAI Chat client")
        };
        assert_eq!(product.provider_body_json["id"], "resp_capture_123");
        assert_eq!(
            product.provider_body_json["output"][0]["content"][0]["text"],
            "Hello capture"
        );
        assert_eq!(
            product.client_body_json["choices"][0]["message"]["content"],
            "Hello capture"
        );
        assert_eq!(
            product.client_body_json["usage"],
            json!({
                "prompt_tokens": 2,
                "completion_tokens": 3,
                "total_tokens": 5
            })
        );
    }

    #[test]
    fn standard_sync_finalize_projects_validated_responses_stream_metadata_to_chat() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "provider_stream_event_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "model": "gpt-5.6-luna",
            "mapped_model": "gpt-5.6-luna",
            "needs_conversion": true,
        });
        let stream_body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"response_id\":\"resp_stream_metadata_123\",\"output_index\":0,\"content_index\":0,\"delta\":\"stream projection\"}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"response_id\":\"resp_stream_metadata_123\",\"output_index\":0,\"content_index\":0,\"text\":\"stream projection\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"msg_stream_metadata_123\",\"type\":\"message\",\"status\":\"completed\",\"role\":\"assistant\",\"phase\":\"final_answer\",\"content\":[{\"type\":\"output_text\",\"text\":\"stream projection\",\"annotations\":[],\"logprobs\":[]}]}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_stream_metadata_123\",\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-luna\",\"background\":false,\"max_output_tokens\":null,\"output\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":3,\"total_tokens\":5}}}\n\n",
        );

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(stream_body)),
        )
        .expect("validated Responses stream should aggregate")
        .expect("validated Responses stream should produce a Chat projection");

        let StandardSyncFinalizeNormalizedProduct::CrossFormat(product) = product else {
            panic!("Responses stream should retain a cross-format provider product")
        };
        assert_eq!(product.provider_body_json["background"], false);
        assert_eq!(
            product.provider_body_json["output"][0]["phase"],
            "final_answer"
        );
        assert_eq!(
            product.client_body_json["choices"][0]["message"]["content"],
            "stream projection"
        );
    }

    #[test]
    fn standard_sync_finalize_projects_authoritative_incomplete_responses_stream() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "provider_stream_event_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "model": "gpt-5.6-luna",
            "mapped_model": "gpt-5.6-luna",
            "needs_conversion": true,
        });
        let stream_body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"response_id\":\"resp_incomplete_stream_123\",\"output_index\":0,\"content_index\":0,\"delta\":\"partial output\"}\n\n",
            "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_incomplete_stream_123\",\"object\":\"response\",\"status\":\"incomplete\",\"model\":\"gpt-5.6-luna\",\"background\":false,\"output\":[],\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
        );

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(stream_body)),
        )
        .expect("authoritative incomplete stream should aggregate")
        .expect("authoritative incomplete stream should produce a Chat projection");

        let StandardSyncFinalizeNormalizedProduct::CrossFormat(product) = product else {
            panic!("incomplete Responses stream should retain a cross-format provider product")
        };
        assert_eq!(product.provider_body_json["status"], "incomplete");
        assert_eq!(
            product.client_body_json["choices"][0]["message"]["content"],
            "partial output"
        );
    }

    #[test]
    fn standard_sync_finalize_rejects_responses_stream_without_authoritative_terminal() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "provider_stream_event_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "model": "gpt-5.6-luna",
            "mapped_model": "gpt-5.6-luna",
            "needs_conversion": true,
        });
        let stream_body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"response_id\":\"resp_truncated_stream_123\",\"output_index\":0,\"content_index\":0,\"delta\":\"truncated\"}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"response_id\":\"resp_truncated_stream_123\",\"output_index\":0,\"content_index\":0,\"text\":\"truncated\"}\n\n",
        );

        assert!(aggregate_openai_responses_stream_sync_response(stream_body.as_bytes()).is_none());
        let error = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(stream_body)),
        )
        .expect_err("a truncated stream must fail closed");
        assert!(
            error
                .to_string()
                .contains("missing an authoritative terminal event"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn standard_sync_finalize_rejects_unknown_canonical_output_from_known_stream_event() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "provider_stream_event_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "model": "gpt-5.6-luna",
            "mapped_model": "gpt-5.6-luna",
            "needs_conversion": true,
        });
        let stream_body = concat!(
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"future_item_123\",\"type\":\"future_output\",\"payload\":\"must-not-drop\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_unknown_output_123\",\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-luna\",\"background\":false,\"output\":[]}}\n\n",
        );

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(stream_body)),
        )
        .expect("known event with unknown output should be inspected");
        assert!(
            product.is_none(),
            "an unknown canonical output block must not be dropped by the Chat projection"
        );
    }

    #[test]
    fn standard_sync_finalize_keeps_direct_responses_metadata_conversion_strict() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "model": "gpt-5.6-luna",
            "mapped_model": "gpt-5.6-luna",
            "needs_conversion": true,
        });
        let provider_body_json = json!({
            "id": "resp_direct_metadata_123",
            "object": "response",
            "status": "completed",
            "model": "gpt-5.6-luna",
            "background": false,
            "output": [{
                "id": "msg_direct_metadata_123",
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "direct body",
                    "annotations": []
                }]
            }]
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("direct response inspection should not fail");

        assert!(
            product.is_none(),
            "direct provider JSON must keep the strict lossless conversion boundary"
        );
    }

    #[test]
    fn standard_sync_finalize_never_projects_failed_responses_stream_as_chat_success() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "provider_stream_event_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "model": "gpt-5.6-luna",
            "mapped_model": "gpt-5.6-luna",
            "needs_conversion": true,
        });
        let stream_body = "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_failed_stream_123\",\"object\":\"response\",\"status\":\"failed\",\"model\":\"gpt-5.6-luna\",\"error\":null,\"output\":[{\"type\":\"message\",\"id\":\"msg_failed_stream_123\",\"role\":\"assistant\",\"status\":\"incomplete\",\"content\":[{\"type\":\"output_text\",\"text\":\"partial failure\",\"annotations\":[]}]}]}}\n\n";

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(stream_body)),
        )
        .expect("failed stream should remain available to the error boundary");

        assert!(
            product.is_none(),
            "a failed Responses stream must not become a Chat success body"
        );
    }

    #[test]
    fn standard_sync_finalize_aggregates_openai_responses_capture_envelope_for_same_family() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
            "model": "client-model",
            "mapped_model": "provider-model",
            "needs_conversion": false,
        });
        let provider_body_json = json!({
            "chunks": [
                {
                    "type": "response.output_text.delta",
                    "response_id": "resp_same_family_capture_123",
                    "output_index": 0,
                    "content_index": 0,
                    "delta": "same family"
                },
                {
                    "type": "response.completed",
                    "response": {
                        "id": "resp_same_family_capture_123",
                        "object": "response",
                        "status": "completed",
                        "model": "provider-model",
                        "output": []
                    }
                }
            ],
            "metadata": {}
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_responses_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("capture envelope aggregation should succeed")
        .expect("capture envelope should produce a same-family body");

        let StandardSyncFinalizeNormalizedProduct::SuccessBody(body_json) = product else {
            panic!("same-family responses capture should remain a success body")
        };
        assert_eq!(body_json["id"], "resp_same_family_capture_123");
        assert_eq!(body_json["output"][0]["content"][0]["text"], "same family");
        assert!(body_json.get("chunks").is_none());
    }

    #[test]
    fn standard_sync_finalize_rejects_unknown_capture_envelope_events() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "model": "client-model",
            "mapped_model": "provider-model",
            "needs_conversion": true,
        });
        let provider_body_json = json!({
            "chunks": [{
                "type": "response.future.delta",
                "response": {
                    "id": "resp_unknown_capture_123",
                    "object": "response",
                    "status": "in_progress",
                    "model": "provider-model"
                },
                "payload": {"kept": true}
            }],
            "metadata": {}
        });

        let error = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect_err("unknown capture event must fail closed instead of returning raw chunks");

        assert!(error
            .to_string()
            .contains("Unsupported provider stream event cannot be converted losslessly"));
        assert!(error
            .to_string()
            .contains("field $.type = \"response.future.delta\""));
    }

    #[test]
    fn standard_sync_finalize_leaves_capture_error_envelope_to_error_boundary() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "model": "client-model",
            "mapped_model": "provider-model",
            "needs_conversion": true,
        });
        let provider_body_json = json!({
            "chunks": [{
                "type": "error",
                "error": {
                    "type": "server_error",
                    "message": "provider failed"
                }
            }],
            "metadata": {}
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("known error envelope should remain available to the gateway error boundary");

        assert_eq!(product, None);
    }

    #[test]
    fn standard_sync_finalize_rejects_malformed_or_incomplete_capture_envelopes() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "model": "client-model",
            "mapped_model": "provider-model",
            "needs_conversion": true,
        });
        let cases = [
            (json!({"chunks": [], "metadata": {}}), "contains no events"),
            (
                json!({
                    "chunks": [{"type": "response.completed", "response": {}}],
                    "metadata": {
                        "stored_chunks": 1,
                        "total_chunks": 2,
                        "dropped_chunks": 1
                    }
                }),
                "is incomplete and cannot be finalized losslessly",
            ),
            (
                json!({
                    "chunks": [{"type": "response.completed", "response": {}}],
                    "metadata": {"total_chunks": 1}
                }),
                "is incomplete and cannot be finalized losslessly",
            ),
            (
                json!({
                    "chunks": [{"type": "response.completed", "response": {}}],
                    "metadata": {"dropped_chunks": "1"}
                }),
                "metadata.dropped_chunks must be an unsigned integer",
            ),
            (
                json!({
                    "chunks": [{"type": "response.completed", "response": {}}],
                    "metadata": false
                }),
                "metadata must be an object",
            ),
            (
                json!({
                    "chunks": [{"type": "response.completed", "response": {}}],
                    "metadata": {"stream": false}
                }),
                "metadata.stream must be true",
            ),
            (
                json!({
                    "chunks": [{"type": "response.completed", "response": {}}],
                    "metadata": {"parse_error": "invalid SSE frame"}
                }),
                "reports a stream parse error",
            ),
            (
                json!({
                    "chunks": [{"type": "response.completed", "response": {}}],
                    "raw_response": "unparsed",
                    "metadata": {}
                }),
                "contains an unparsed raw response",
            ),
            (
                json!({"chunks": ["not-an-event"], "metadata": {}}),
                "contains a non-object event",
            ),
        ];

        for (provider_body_json, expected_message) in cases {
            let error = maybe_build_standard_sync_finalize_product_from_normalized_payload(
                "openai_chat_sync_finalize",
                200,
                Some(&report_context),
                Some(&provider_body_json),
                None,
            )
            .expect_err("malformed capture envelope must fail closed");

            assert!(
                error.to_string().contains(expected_message),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn standard_sync_finalize_rejects_capture_envelope_when_target_format_is_unsupported() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "future:chat",
            "model": "client-model",
            "mapped_model": "provider-model",
            "needs_conversion": true,
        });
        let provider_body_json = json!({
            "chunks": [{
                "type": "response.completed",
                "response": {
                    "id": "resp_unsupported_target_123",
                    "object": "response",
                    "status": "completed",
                    "model": "provider-model",
                    "output": [{
                        "type": "message",
                        "id": "msg_unsupported_target_123",
                        "role": "assistant",
                        "status": "completed",
                        "content": [{
                            "type": "output_text",
                            "text": "must not leak as raw chunks",
                            "annotations": []
                        }]
                    }]
                }
            }],
            "metadata": {}
        });

        let error = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_cli_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect_err("unsupported target must not fall back to the raw capture envelope");

        assert!(error
            .to_string()
            .contains("capture envelope cannot be finalized losslessly"));
    }

    #[test]
    fn standard_sync_finalize_prefers_explicit_stream_body_over_capture_envelope() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "model": "client-model",
            "mapped_model": "provider-model",
            "needs_conversion": true,
        });
        let capture_envelope = json!({
            "chunks": [{"type": "response.future.delta"}],
            "metadata": {}
        });
        let stream_body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"response_id\":\"resp_explicit_stream_123\",\"output_index\":0,\"content_index\":0,\"delta\":\"explicit stream\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_explicit_stream_123\",\"object\":\"response\",\"status\":\"completed\",\"model\":\"provider-model\",\"output\":[]}}\n\n",
        );

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&capture_envelope),
            Some(&base64::engine::general_purpose::STANDARD.encode(stream_body)),
        )
        .expect("explicit stream body should take precedence")
        .expect("explicit stream body should produce a product");

        let StandardSyncFinalizeNormalizedProduct::CrossFormat(product) = product else {
            panic!("explicit Responses stream should convert to OpenAI Chat")
        };
        assert_eq!(
            product.client_body_json["choices"][0]["message"]["content"],
            "explicit stream"
        );
    }

    #[test]
    fn standard_sync_finalize_product_handles_openai_responses_same_family_cross_format_body() {
        let report_context = json!({
            "provider_api_format": "openai:responses:compact",
            "client_api_format": "openai:responses",
            "needs_conversion": true,
        });
        let provider_body_json = json!({
            "id": "resp_family_123",
            "object": "response",
            "status": "completed",
            "output": []
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_responses_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("dispatch should succeed");

        assert_eq!(
            product,
            Some(StandardSyncFinalizeNormalizedProduct::SuccessBody(
                provider_body_json
            ))
        );
    }

    #[test]
    fn standard_sync_finalize_product_handles_openai_chat_cross_format() {
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "mapped_model": "claude-sonnet-4",
        });
        let provider_body_json = json!({
            "id": "msg_claude_direct_123",
            "type": "message",
            "model": "claude-sonnet-4",
            "role": "assistant",
            "content": [{"type": "text", "text": "Hello Claude"}],
            "stop_reason": "end_turn"
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("dispatch should succeed")
        .expect("dispatch should produce a product");

        assert!(matches!(
            product,
            StandardSyncFinalizeNormalizedProduct::CrossFormat(_)
        ));
    }

    #[test]
    fn standard_sync_finalize_product_handles_openai_responses_cross_format() {
        let body = concat!(
            "data: {\"responseId\":\"resp_cli_stream_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello \"}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"gemini-2.5-pro-upstream\"}\n\n",
            "data: {\"responseId\":\"resp_cli_stream_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello Gemini CLI\"}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"gemini-2.5-pro-upstream\",\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}}\n\n",
        );
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "openai:responses",
            "model": "gpt-5",
            "mapped_model": "gemini-2.5-pro-upstream",
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "openai_responses_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&base64::engine::general_purpose::STANDARD.encode(body)),
        )
        .expect("dispatch should succeed")
        .expect("dispatch should produce a product");

        assert!(matches!(
            product,
            StandardSyncFinalizeNormalizedProduct::CrossFormat(_)
        ));
    }

    #[test]
    fn standard_sync_finalize_product_converts_openai_responses_null_error_to_claude_cli() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "claude:messages",
            "model": "claude-sonnet-4-5",
            "mapped_model": "gpt-5",
        });
        let provider_body_json = json!({
            "id": "resp_completed_cli_123",
            "object": "response",
            "model": "gpt-5",
            "status": "completed",
            "error": null,
            "output": [{
                "type": "message",
                "id": "msg_completed_cli_123",
                "role": "assistant",
                "status": "completed",
                "content": [{"type": "output_text", "text": "Done", "annotations": []}]
            }]
        });
        assert_eq!(
            sync_cli_response_conversion_kind("openai:responses", "claude:messages"),
            Some(SyncCliResponseConversionKind::ToClaudeCli)
        );
        assert!(convert_standard_cli_response(
            &provider_body_json,
            "openai:responses",
            "claude:messages",
            &report_context
        )
        .is_some());

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "claude_cli_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("dispatch should succeed")
        .expect("dispatch should produce a product");

        let StandardSyncFinalizeNormalizedProduct::CrossFormat(product) = product else {
            panic!("openai responses provider body should be converted for claude client")
        };
        assert_eq!(product.client_body_json["type"], "message");
        assert_eq!(product.client_body_json["content"][0]["text"], "Done");
        assert_eq!(product.client_body_json["stop_reason"], "end_turn");
    }

    #[test]
    fn standard_sync_finalize_product_falls_back_to_generic_standard_cross_format() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "claude:messages",
        });
        let provider_body_json = json!({
            "id": "chatcmpl_123",
            "object": "chat.completion",
            "created": 1,
            "model": "gpt-5",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello",
                },
                "finish_reason": "stop",
            }],
        });

        let product = maybe_build_standard_sync_finalize_product_from_normalized_payload(
            "claude_chat_sync_finalize",
            200,
            Some(&report_context),
            Some(&provider_body_json),
            None,
        )
        .expect("dispatch should succeed")
        .expect("dispatch should produce a product");

        assert!(matches!(
            product,
            StandardSyncFinalizeNormalizedProduct::CrossFormat(_)
        ));
    }

    #[test]
    fn multi_choice_openai_chat_response_keeps_legacy_fallback_route() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "claude:messages",
            "model": "gpt-5",
            "mapped_model": "gpt-5-upstream",
        });
        let provider_body_json = json!({
            "id": "chatcmpl_multi_123",
            "object": "chat.completion",
            "model": "gpt-5-upstream",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "first"},
                    "finish_reason": "stop"
                },
                {
                    "index": 1,
                    "message": {"role": "assistant", "content": "second"},
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 1,
                "completion_tokens": 2,
                "total_tokens": 3
            }
        });

        let legacy =
            convert_openai_chat_response_to_claude_chat(&provider_body_json, &report_context)
                .expect("legacy multi choice route");
        let converted = convert_standard_chat_response(
            &provider_body_json,
            "openai:chat",
            "claude:messages",
            &report_context,
        )
        .expect("fallback route");
        assert_eq!(converted, legacy);
    }

    #[test]
    fn multi_candidate_gemini_response_keeps_legacy_fallback_route() {
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "openai:chat",
            "model": "gemini-2.5-pro",
            "mapped_model": "gemini-upstream",
        });
        let provider_body_json = json!({
            "responseId": "resp_multi_123",
            "modelVersion": "gemini-upstream",
            "candidates": [
                {
                    "index": 0,
                    "finishReason": "STOP",
                    "content": {"role": "model", "parts": [{"text": "first"}]}
                },
                {
                    "index": 1,
                    "finishReason": "STOP",
                    "content": {"role": "model", "parts": [{"text": "second"}]}
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 1,
                "candidatesTokenCount": 2,
                "totalTokenCount": 3
            }
        });

        let legacy =
            convert_gemini_chat_response_to_openai_chat(&provider_body_json, &report_context)
                .expect("legacy multi candidate route");
        let converted = convert_standard_chat_response(
            &provider_body_json,
            "gemini:generate_content",
            "openai:chat",
            &report_context,
        )
        .expect("fallback route");
        assert_eq!(converted, legacy);
    }
}
