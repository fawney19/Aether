use crate::ai_serving::GatewayControlDecision;
use crate::ai_serving::{build_generated_tool_call_id, canonicalize_tool_arguments};
use crate::{usage::GatewaySyncReportRequest, GatewayError};

pub(crate) use crate::ai_serving::finalize::common::{
    build_local_success_outcome, build_local_success_outcome_with_conversion_report,
    local_finalize_allows_envelope, unwrap_local_finalize_response_value,
    LocalCoreSyncFinalizeOutcome,
};
pub(crate) use crate::ai_serving::finalize::standard::{
    maybe_build_standard_sync_finalize_product_from_normalized_payload,
    StandardSyncFinalizeNormalizedProduct,
};
pub(crate) use crate::ai_serving::{
    aggregate_claude_stream_sync_response, aggregate_gemini_stream_sync_response,
    aggregate_openai_chat_stream_sync_response, aggregate_openai_responses_stream_sync_response,
    maybe_build_openai_image_sync_finalize_product,
};
pub(crate) use crate::ai_serving::{
    convert_claude_chat_response_to_openai_chat, convert_claude_response_to_openai_responses,
    convert_gemini_chat_response_to_openai_chat, convert_gemini_response_to_openai_responses,
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
        crate::ai_serving::adaptation::private_envelope::maybe_normalize_provider_private_sync_report_payload(payload)?
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
    let Some(product) = maybe_build_openai_image_sync_finalize_product(
        payload.report_kind.as_str(),
        payload.status_code,
        payload.report_context.as_ref(),
        payload.body_base64.as_deref(),
    )
    .map_err(GatewayError::from)?
    else {
        return Ok(None);
    };

    Ok(Some(build_local_success_outcome_with_conversion_report(
        trace_id,
        decision,
        payload,
        product.client_body_json,
        product.provider_body_json,
    )?))
}

#[cfg(test)]
#[path = "../tests_sync.rs"]
mod tests;
