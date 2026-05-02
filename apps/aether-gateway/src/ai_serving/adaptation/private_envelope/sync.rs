use base64::Engine as _;
use serde_json::Value;

use crate::{usage::GatewaySyncReportRequest, GatewayError};

use super::{
    maybe_build_provider_private_stream_normalizer, normalize_provider_private_report_context,
    normalize_provider_private_response_value, provider_private_response_allows_sync_finalize,
    stream_body_contains_error_event, ProviderPrivateStreamNormalizer,
};

pub(crate) fn maybe_normalize_provider_private_sync_report_payload(
    payload: &GatewaySyncReportRequest,
) -> Result<Option<GatewaySyncReportRequest>, GatewayError> {
    let Some(report_context) = payload.report_context.as_ref() else {
        return Ok(Some(payload.clone()));
    };
    if !report_context
        .get("has_envelope")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(Some(payload.clone()));
    }
    if !provider_private_response_allows_sync_finalize(report_context) {
        return Ok(None);
    }

    let mut normalized = payload.clone();
    normalized.report_context = normalize_provider_private_report_context(Some(report_context));

    if let Some(body_json) = payload.body_json.clone() {
        normalized.body_json = normalize_provider_private_response_value(body_json, report_context);
        if normalized.body_json.is_none() {
            return Ok(None);
        }
    }

    if let Some(body_base64) = payload.body_base64.as_deref() {
        let body_bytes = base64::engine::general_purpose::STANDARD
            .decode(body_base64)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let Some(normalized_bytes) =
            normalize_provider_private_stream_bytes(report_context, &body_bytes)?
        else {
            return Ok(None);
        };
        if stream_body_contains_error_event(&normalized_bytes) {
            return Ok(None);
        }
        normalized.body_base64 = (!normalized_bytes.is_empty())
            .then(|| base64::engine::general_purpose::STANDARD.encode(normalized_bytes));
    }

    Ok(Some(normalized))
}

fn normalize_provider_private_stream_bytes(
    report_context: &Value,
    body: &[u8],
) -> Result<Option<Vec<u8>>, GatewayError> {
    let Some(mut normalizer): Option<ProviderPrivateStreamNormalizer<'_>> =
        maybe_build_provider_private_stream_normalizer(Some(report_context))
    else {
        return Ok(Some(body.to_vec()));
    };
    let mut normalized = normalizer.push_chunk(body).map_err(GatewayError::from)?;
    normalized.extend(normalizer.finish().map_err(GatewayError::from)?);
    Ok(Some(normalized))
}
