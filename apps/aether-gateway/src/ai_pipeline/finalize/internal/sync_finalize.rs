use crate::ai_pipeline::GatewayControlDecision;
use crate::ai_pipeline::CODEX_OPENAI_IMAGE_DEFAULT_OUTPUT_FORMAT;
use crate::ai_pipeline::{build_generated_tool_call_id, canonicalize_tool_arguments};
use crate::{usage::GatewaySyncReportRequest, GatewayError};
use base64::Engine as _;

pub(crate) use crate::ai_pipeline::finalize::common::{
    build_local_success_outcome, build_local_success_outcome_with_conversion_report,
    local_finalize_allows_envelope, unwrap_local_finalize_response_value,
    LocalCoreSyncFinalizeOutcome,
};
pub(crate) use crate::ai_pipeline::finalize::standard::{
    maybe_build_standard_sync_finalize_product_from_normalized_payload,
    StandardSyncFinalizeNormalizedProduct,
};
pub(crate) use crate::ai_pipeline::{
    aggregate_claude_stream_sync_response, aggregate_gemini_stream_sync_response,
    aggregate_openai_chat_stream_sync_response, aggregate_openai_cli_stream_sync_response,
};
pub(crate) use crate::ai_pipeline::{
    convert_claude_chat_response_to_openai_chat, convert_claude_cli_response_to_openai_cli,
    convert_gemini_chat_response_to_openai_chat, convert_gemini_cli_response_to_openai_cli,
};

pub(crate) fn maybe_build_local_core_sync_finalize_response(
    trace_id: &str,
    decision: &GatewayControlDecision,
    payload: &GatewaySyncReportRequest,
) -> Result<Option<LocalCoreSyncFinalizeOutcome>, GatewayError> {
    if let Some(outcome) =
        maybe_build_local_openai_image_sync_finalize_response(trace_id, decision, payload)?
    {
        return Ok(Some(outcome));
    }

    let Some(normalized_payload) =
        crate::ai_pipeline::adaptation::private_envelope::maybe_normalize_provider_private_sync_report_payload(payload)?
    else {
        return Ok(None);
    };
    let payload = &normalized_payload;
    let Some(report_context) = payload.report_context.as_ref() else {
        return Ok(None);
    };
    if !local_finalize_allows_envelope(report_context) {
        return Ok(None);
    }
    let Some(product) = maybe_build_standard_sync_finalize_product_from_normalized_payload(
        payload.report_kind.as_str(),
        payload.status_code,
        Some(report_context),
        payload.body_json.as_ref(),
        payload.body_base64.as_deref(),
    )
    .map_err(GatewayError::from)?
    else {
        return Ok(None);
    };

    match product {
        StandardSyncFinalizeNormalizedProduct::SuccessBody(body_json) => {
            let Some(body_json) = unwrap_local_finalize_response_value(body_json, report_context)
            else {
                return Ok(None);
            };
            Ok(Some(build_local_success_outcome(
                trace_id, decision, payload, body_json,
            )?))
        }
        StandardSyncFinalizeNormalizedProduct::CrossFormat(product) => {
            let Some(provider_body_json) =
                unwrap_local_finalize_response_value(product.provider_body_json, report_context)
            else {
                return Ok(None);
            };
            Ok(Some(build_local_success_outcome_with_conversion_report(
                trace_id,
                decision,
                payload,
                product.client_body_json,
                provider_body_json,
            )?))
        }
    }
}

fn maybe_build_local_openai_image_sync_finalize_response(
    trace_id: &str,
    decision: &GatewayControlDecision,
    payload: &GatewaySyncReportRequest,
) -> Result<Option<LocalCoreSyncFinalizeOutcome>, GatewayError> {
    if payload.report_kind != "openai_image_sync_finalize" || payload.status_code >= 400 {
        return Ok(None);
    }
    let Some(report_context) = payload.report_context.as_ref() else {
        return Ok(None);
    };
    if report_context
        .get("client_api_format")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        != Some("openai:image")
    {
        return Ok(None);
    }
    let Some(body_base64) = payload.body_base64.as_deref() else {
        return Ok(None);
    };
    let default_output_format = report_context
        .get("image_request")
        .and_then(|value| value.get("output_format"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(CODEX_OPENAI_IMAGE_DEFAULT_OUTPUT_FORMAT);
    let body_bytes = base64::engine::general_purpose::STANDARD
        .decode(body_base64)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let text =
        std::str::from_utf8(&body_bytes).map_err(|err| GatewayError::Internal(err.to_string()))?;

    let mut created = None;
    let mut completed_response = None;
    let mut images = Vec::new();

    for raw_block in text.split("\n\n") {
        let block = raw_block.trim();
        if block.is_empty() {
            continue;
        }
        let data_line = block
            .lines()
            .find_map(|line| line.trim().strip_prefix("data:").map(str::trim));
        let Some(data_line) = data_line else {
            continue;
        };
        if data_line.is_empty() || data_line == "[DONE]" {
            continue;
        }
        let event: serde_json::Value = serde_json::from_str(data_line)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        match event
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
        {
            "response.created" => {
                created = event
                    .get("response")
                    .and_then(|value| value.get("created_at"))
                    .and_then(serde_json::Value::as_i64)
                    .or(created);
            }
            "response.output_item.done" => {
                let Some(item) = event.get("item").and_then(serde_json::Value::as_object) else {
                    continue;
                };
                if item.get("type").and_then(serde_json::Value::as_str)
                    != Some("image_generation_call")
                {
                    continue;
                }
                let Some(result) = item.get("result").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                images.push(serde_json::json!({
                    "b64_json": result,
                    "output_format": item.get("output_format").cloned().unwrap_or(serde_json::Value::String(default_output_format.to_string())),
                    "revised_prompt": item.get("revised_prompt").cloned().unwrap_or(serde_json::Value::Null),
                }));
            }
            "response.completed" => {
                completed_response = event
                    .get("response")
                    .and_then(serde_json::Value::as_object)
                    .cloned();
            }
            _ => {}
        }
    }

    if images.is_empty() {
        return Ok(None);
    }

    let completed_response = completed_response.unwrap_or_default();
    let provider_usage = completed_response
        .get("tool_usage")
        .and_then(|value| value.get("image_gen"))
        .cloned()
        .or_else(|| completed_response.get("usage").cloned());
    let provider_body_json = serde_json::json!({
        "id": completed_response.get("id").cloned().unwrap_or(serde_json::Value::Null),
        "object": "response",
        "model": completed_response.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "status": completed_response.get("status").cloned().unwrap_or(serde_json::Value::String("completed".to_string())),
        "usage": provider_usage,
        "tool_usage": completed_response.get("tool_usage").cloned().unwrap_or(serde_json::Value::Null),
        "output": images
            .iter()
            .map(|image| serde_json::json!({
                "type": "image_generation_call",
                "output_format": image.get("output_format").cloned().unwrap_or(serde_json::Value::Null),
                "revised_prompt": image.get("revised_prompt").cloned().unwrap_or(serde_json::Value::Null),
            }))
            .collect::<Vec<_>>(),
    });
    let client_images = images
        .iter()
        .map(|image| {
            let revised_prompt = image
                .get("revised_prompt")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let b64_json = image
                .get("b64_json")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let output_format = image
                .get("output_format")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(default_output_format);
            serde_json::json!({
                "b64_json": b64_json,
                "revised_prompt": revised_prompt,
            })
        })
        .collect::<Vec<_>>();
    let client_body_json = serde_json::json!({
        "created": created.unwrap_or_default(),
        "data": client_images,
        "usage": provider_body_json.get("usage").cloned().unwrap_or(serde_json::Value::Null),
    });

    Ok(Some(build_local_success_outcome_with_conversion_report(
        trace_id,
        decision,
        payload,
        client_body_json,
        provider_body_json,
    )?))
}

#[cfg(test)]
#[path = "../tests_sync.rs"]
mod tests;
