#[cfg(test)]
use crate::ai_serving::api::core_success_background_report_kind;
use crate::ai_serving::api::{
    build_core_error_body_for_client_format, core_error_background_report_kind,
    core_error_default_client_api_format, is_core_error_finalize_kind,
    maybe_compile_sync_finalize_response,
    normalize_provider_private_response_value as unwrap_local_finalize_response_value,
    LocalCoreSyncErrorKind,
};
use crate::api::response::build_client_response_from_parts;
use crate::control::GatewayControlDecision;
use crate::usage::spawn_sync_report;
use crate::{usage::GatewaySyncReportRequest, AppState, GatewayError};
use axum::body::Body;
use axum::http::Response;
use base64::Engine as _;
use tracing::warn;

#[derive(Clone, Debug)]
struct LocalSyncErrorDetails {
    message: String,
    code: Option<String>,
    kind: LocalCoreSyncErrorKind,
}

pub(super) fn maybe_build_local_core_error_response(
    trace_id: &str,
    decision: &GatewayControlDecision,
    payload: &GatewaySyncReportRequest,
) -> Result<Option<Response<Body>>, GatewayError> {
    if !is_core_error_finalize_kind(payload.report_kind.as_str()) {
        return Ok(None);
    }

    let Some(response_body_json) = resolve_local_core_error_response_body_json(payload)? else {
        return Ok(None);
    };
    let status_source_json = resolve_local_sync_source_body_json(payload)?;

    let mut response_headers = payload.headers.clone();
    response_headers.remove("content-encoding");
    response_headers.remove("content-length");
    response_headers.insert("content-type".to_string(), "application/json".to_string());

    let body_bytes = serde_json::to_vec(&response_body_json)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    response_headers.insert("content-length".to_string(), body_bytes.len().to_string());

    Ok(Some(build_client_response_from_parts(
        status_source_json
            .as_ref()
            .map_or(payload.status_code, |body_json| {
                resolve_local_sync_error_status_code(payload.status_code, body_json)
            }),
        &response_headers,
        Body::from(body_bytes),
        trace_id,
        Some(decision),
    )?))
}

fn maybe_resolve_local_sync_response_body_json(
    payload: &GatewaySyncReportRequest,
) -> Result<Option<serde_json::Value>, GatewayError> {
    if let Some(client_body_json) = payload.client_body_json.clone() {
        return Ok(Some(client_body_json));
    }

    if is_core_error_finalize_kind(payload.report_kind.as_str()) {
        if let Some(converted) = resolve_local_core_error_response_body_json(payload)? {
            return Ok(Some(converted));
        }
    }

    resolve_local_sync_source_body_json(payload)
}

fn build_local_sync_response_from_json(
    trace_id: &str,
    decision: &GatewayControlDecision,
    payload: &GatewaySyncReportRequest,
    body_json: serde_json::Value,
) -> Result<Response<Body>, GatewayError> {
    let status_code = if is_core_error_finalize_kind(payload.report_kind.as_str())
        || has_nested_error(&body_json)
    {
        resolve_local_sync_error_status_code(payload.status_code, &body_json)
    } else {
        payload.status_code
    };

    let mut response_headers = payload.headers.clone();
    response_headers.remove("content-encoding");
    response_headers.remove("content-length");
    response_headers.insert("content-type".to_string(), "application/json".to_string());

    let body_bytes =
        serde_json::to_vec(&body_json).map_err(|err| GatewayError::Internal(err.to_string()))?;
    response_headers.insert("content-length".to_string(), body_bytes.len().to_string());

    build_client_response_from_parts(
        status_code,
        &response_headers,
        Body::from(body_bytes),
        trace_id,
        Some(decision),
    )
}

fn build_local_sync_response_from_bytes(
    trace_id: &str,
    decision: &GatewayControlDecision,
    payload: &GatewaySyncReportRequest,
    body_bytes: Vec<u8>,
) -> Result<Response<Body>, GatewayError> {
    let mut response_headers = payload.headers.clone();
    response_headers.remove("content-length");
    if body_bytes.is_empty() {
        response_headers.remove("content-encoding");
    }
    response_headers.insert("content-length".to_string(), body_bytes.len().to_string());

    build_client_response_from_parts(
        payload.status_code,
        &response_headers,
        Body::from(body_bytes),
        trace_id,
        Some(decision),
    )
}

fn build_local_core_sync_finalize_fallback_response(
    trace_id: &str,
    decision: &GatewayControlDecision,
    payload: &GatewaySyncReportRequest,
) -> Result<Response<Body>, GatewayError> {
    if let Some(body_json) = maybe_resolve_local_sync_response_body_json(payload)? {
        return build_local_sync_response_from_json(trace_id, decision, payload, body_json);
    }

    if let Some(body_base64) = payload.body_base64.as_ref() {
        let body_bytes = base64::engine::general_purpose::STANDARD
            .decode(body_base64)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return build_local_sync_response_from_bytes(trace_id, decision, payload, body_bytes);
    }

    build_local_sync_response_from_bytes(trace_id, decision, payload, Vec::new())
}

pub(crate) fn build_best_effort_local_core_error_body(
    payload: &GatewaySyncReportRequest,
    body_json: &serde_json::Value,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let default_api_format = core_error_default_client_api_format(payload.report_kind.as_str())
        .unwrap_or_default()
        .to_string();
    let client_api_format = payload
        .report_context
        .as_ref()
        .and_then(|value| value.get("client_api_format"))
        .and_then(|value| value.as_str())
        .unwrap_or(default_api_format.as_str())
        .trim()
        .to_ascii_lowercase();
    let provider_api_format = payload
        .report_context
        .as_ref()
        .and_then(|value| value.get("provider_api_format"))
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_else(|| client_api_format.clone());

    if client_api_format.is_empty() {
        return Ok(None);
    }
    if client_api_format == provider_api_format {
        return Ok(Some(body_json.clone()));
    }

    let details = extract_local_sync_error_details(payload.status_code, body_json);
    Ok(build_core_error_body_for_client_format(
        &client_api_format,
        &details.message,
        details.code.as_deref(),
        details.kind,
    ))
}

pub(crate) fn resolve_local_core_error_response_body_json(
    payload: &GatewaySyncReportRequest,
) -> Result<Option<serde_json::Value>, GatewayError> {
    if !is_core_error_finalize_kind(payload.report_kind.as_str()) {
        return Ok(None);
    }

    if let Some(client_body_json) = payload.client_body_json.clone() {
        return Ok(Some(client_body_json));
    }

    if let Some(body_json) = resolve_local_sync_source_body_json(payload)? {
        if let Some(converted) = build_best_effort_local_core_error_body(payload, &body_json)? {
            return Ok(Some(converted));
        }
        return Ok(Some(body_json));
    }

    let Some(body_text) = decode_local_sync_body_text(payload)? else {
        return Ok(None);
    };
    let client_api_format = resolve_local_sync_client_api_format(payload);
    if client_api_format.is_empty() {
        return Ok(None);
    }

    let kind =
        classify_local_sync_error_kind(payload.status_code, None, None, None, body_text.as_str());
    Ok(build_core_error_body_for_client_format(
        &client_api_format,
        body_text.as_str(),
        None,
        kind,
    ))
}

fn resolve_local_sync_source_body_json(
    payload: &GatewaySyncReportRequest,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let body_json = if let Some(body_json) = payload.body_json.clone() {
        body_json
    } else if let Some(body_base64) = payload.body_base64.as_deref() {
        let body_bytes = base64::engine::general_purpose::STANDARD
            .decode(body_base64)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let stripped = strip_utf8_bom_and_ws(&body_bytes);
        let Ok(body_json) = serde_json::from_slice::<serde_json::Value>(stripped) else {
            return Ok(None);
        };
        body_json
    } else {
        return Ok(None);
    };

    if let Some(report_context) = payload.report_context.as_ref() {
        if let Some(unwrapped) =
            unwrap_local_finalize_response_value(body_json.clone(), report_context)
        {
            return Ok(Some(unwrapped));
        }
    }

    Ok(Some(body_json))
}

fn decode_local_sync_body_text(
    payload: &GatewaySyncReportRequest,
) -> Result<Option<String>, GatewayError> {
    let Some(body_base64) = payload.body_base64.as_deref() else {
        return Ok(None);
    };
    let body_bytes = base64::engine::general_purpose::STANDARD
        .decode(body_base64)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let stripped = strip_utf8_bom_and_ws(&body_bytes);
    let body_text = String::from_utf8_lossy(stripped).trim().to_string();
    if body_text.is_empty() {
        return Ok(None);
    }
    Ok(Some(body_text))
}

fn resolve_local_sync_client_api_format(payload: &GatewaySyncReportRequest) -> String {
    let default_api_format = core_error_default_client_api_format(payload.report_kind.as_str())
        .unwrap_or_default()
        .to_string();
    payload
        .report_context
        .as_ref()
        .and_then(|value| value.get("client_api_format"))
        .and_then(|value| value.as_str())
        .unwrap_or(default_api_format.as_str())
        .trim()
        .to_ascii_lowercase()
}

pub(crate) fn resolve_core_error_background_report_kind(report_kind: &str) -> Option<String> {
    core_error_background_report_kind(report_kind).map(ToOwned::to_owned)
}

#[cfg(test)]
pub(crate) fn resolve_core_success_background_report_kind(report_kind: &str) -> Option<String> {
    core_success_background_report_kind(report_kind).map(ToOwned::to_owned)
}

fn resolve_local_sync_error_status_code(status_code: u16, body_json: &serde_json::Value) -> u16 {
    if (400..600).contains(&status_code) {
        return status_code;
    }

    let body_object = body_json.as_object();
    let error_object = body_object
        .and_then(|object| object.get("error"))
        .and_then(|value| value.as_object());

    let raw_code = first_non_empty_error_text(error_object, body_object, &["code"]);
    let raw_status = first_non_empty_error_text(error_object, body_object, &["status"]);
    for numeric_hint in [raw_code.as_deref(), raw_status.as_deref()]
        .into_iter()
        .flatten()
    {
        if let Ok(number) = numeric_hint.parse::<u16>() {
            if (400..600).contains(&number) {
                return number;
            }
        }
    }

    let raw_type = first_non_empty_error_text(error_object, body_object, &["type", "__type"]);
    let message = first_non_empty_error_text(
        error_object,
        body_object,
        &["message", "detail", "reason", "status", "type", "__type"],
    )
    .unwrap_or_else(|| "HTTP 400".to_string());
    let kind = classify_local_sync_error_kind(
        status_code,
        raw_type.as_deref(),
        raw_status.as_deref(),
        raw_code.as_deref(),
        message.as_str(),
    );
    default_status_code_for_local_sync_error_kind(kind)
}

fn extract_local_sync_error_details(
    status_code: u16,
    body_json: &serde_json::Value,
) -> LocalSyncErrorDetails {
    let resolved_status_code = resolve_local_sync_error_status_code(status_code, body_json);
    let body_object = body_json.as_object();
    let error_object = body_object
        .and_then(|object| object.get("error"))
        .and_then(|value| value.as_object());

    let message = first_non_empty_error_text(
        error_object,
        body_object,
        &["message", "detail", "reason", "status", "type", "__type"],
    )
    .unwrap_or_else(|| format!("HTTP {resolved_status_code}"));
    let code = first_non_empty_error_text(error_object, body_object, &["code", "status"]);
    let raw_type = first_non_empty_error_text(error_object, body_object, &["type", "__type"]);
    let raw_status = first_non_empty_error_text(error_object, body_object, &["status"]);
    let kind = classify_local_sync_error_kind(
        resolved_status_code,
        raw_type.as_deref(),
        raw_status.as_deref(),
        code.as_deref(),
        message.as_str(),
    );

    LocalSyncErrorDetails {
        message,
        code,
        kind,
    }
}

fn first_non_empty_error_text(
    error_object: Option<&serde_json::Map<String, serde_json::Value>>,
    body_object: Option<&serde_json::Map<String, serde_json::Value>>,
    keys: &[&str],
) -> Option<String> {
    for object in [error_object, body_object].into_iter().flatten() {
        for key in keys {
            let Some(value) = object.get(*key) else {
                continue;
            };
            match value {
                serde_json::Value::String(text) if !text.trim().is_empty() => {
                    return Some(text.trim().to_string());
                }
                serde_json::Value::Number(number) => return Some(number.to_string()),
                _ => {}
            }
        }
    }
    None
}

fn classify_local_sync_error_kind(
    status_code: u16,
    raw_type: Option<&str>,
    raw_status: Option<&str>,
    raw_code: Option<&str>,
    message: &str,
) -> LocalCoreSyncErrorKind {
    let mut fingerprint = String::new();
    for segment in [raw_type, raw_status, raw_code, Some(message)] {
        if let Some(segment) = segment.map(str::trim).filter(|value| !value.is_empty()) {
            if !fingerprint.is_empty() {
                fingerprint.push(' ');
            }
            fingerprint.push_str(&segment.to_ascii_lowercase());
        }
    }

    if status_code == 429
        || fingerprint.contains("rate_limit")
        || fingerprint.contains("rate limited")
        || fingerprint.contains("resource_exhausted")
        || fingerprint.contains("throttl")
    {
        return LocalCoreSyncErrorKind::RateLimit;
    }
    if fingerprint.contains("contextlength")
        || fingerprint.contains("contentlengthexceeded")
        || fingerprint.contains("context window")
        || fingerprint.contains("context length")
        || fingerprint.contains("max_tokens")
        || (fingerprint.contains("context") && fingerprint.contains("token"))
    {
        return LocalCoreSyncErrorKind::ContextLengthExceeded;
    }
    if status_code == 401
        || fingerprint.contains("unauth")
        || fingerprint.contains("authentication")
    {
        return LocalCoreSyncErrorKind::Authentication;
    }
    if status_code == 403 || fingerprint.contains("permission") || fingerprint.contains("forbidden")
    {
        return LocalCoreSyncErrorKind::PermissionDenied;
    }
    if status_code == 404 || fingerprint.contains("not_found") || fingerprint.contains("not found")
    {
        return LocalCoreSyncErrorKind::NotFound;
    }
    if status_code == 503 || fingerprint.contains("overload") || fingerprint.contains("unavailable")
    {
        return LocalCoreSyncErrorKind::Overloaded;
    }
    if (500..600).contains(&status_code) {
        return LocalCoreSyncErrorKind::ServerError;
    }
    LocalCoreSyncErrorKind::InvalidRequest
}

fn default_status_code_for_local_sync_error_kind(kind: LocalCoreSyncErrorKind) -> u16 {
    match kind {
        LocalCoreSyncErrorKind::InvalidRequest | LocalCoreSyncErrorKind::ContextLengthExceeded => {
            400
        }
        LocalCoreSyncErrorKind::Authentication => 401,
        LocalCoreSyncErrorKind::PermissionDenied => 403,
        LocalCoreSyncErrorKind::NotFound => 404,
        LocalCoreSyncErrorKind::RateLimit => 429,
        LocalCoreSyncErrorKind::Overloaded => 503,
        LocalCoreSyncErrorKind::ServerError => 500,
    }
}

pub(crate) fn strip_utf8_bom_and_ws(mut body: &[u8]) -> &[u8] {
    loop {
        while let Some(first) = body.first() {
            if first.is_ascii_whitespace() {
                body = &body[1..];
            } else {
                break;
            }
        }
        if body.starts_with(&[0xEF, 0xBB, 0xBF]) {
            body = &body[3..];
        } else {
            break;
        }
    }
    body
}

pub(crate) fn has_nested_error(value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };

    if object.get("error").is_some_and(|error| !error.is_null()) {
        return true;
    }
    if object
        .get("type")
        .and_then(|value| value.as_str())
        .is_some_and(|value| value == "error")
    {
        return true;
    }

    object
        .get("chunks")
        .and_then(|value| value.as_array())
        .is_some_and(|chunks| {
            chunks.iter().any(|chunk| {
                chunk.as_object().is_some_and(|chunk_object| {
                    chunk_object
                        .get("error")
                        .is_some_and(|error| !error.is_null())
                        || chunk_object
                            .get("type")
                            .and_then(|value| value.as_str())
                            .is_some_and(|value| value == "error")
                })
            })
        })
}

pub(crate) async fn submit_local_core_error_or_sync_finalize(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    payload: GatewaySyncReportRequest,
) -> Result<Response<Body>, GatewayError> {
    let response = if let Some(response) =
        maybe_compile_sync_finalize_response(trace_id, decision, &payload)?
    {
        response
    } else if let Some(response) =
        maybe_build_local_core_error_response(trace_id, decision, &payload)?
    {
        response
    } else {
        warn!(
            event_name = "local_core_finalize_fallback_raw_response_body",
            log_type = "event",
            trace_id = %trace_id,
            report_kind = %payload.report_kind,
            status_code = payload.status_code,
            client_api_format = payload.report_context.as_ref().and_then(|value| value.get("client_api_format")).and_then(|value| value.as_str()).unwrap_or(""),
            provider_api_format = payload.report_context.as_ref().and_then(|value| value.get("provider_api_format")).and_then(|value| value.as_str()).unwrap_or(""),
            envelope_name = payload.report_context.as_ref().and_then(|value| value.get("envelope_name")).and_then(|value| value.as_str()).unwrap_or(""),
            needs_conversion = payload.report_context.as_ref().and_then(|value| value.get("needs_conversion")).and_then(|value| value.as_bool()).unwrap_or(false),
            "gateway local core finalize fell back to raw response body"
        );
        build_local_core_sync_finalize_fallback_response(trace_id, decision, &payload)?
    };

    if let Some(error_report_kind) =
        resolve_core_error_background_report_kind(payload.report_kind.as_str())
    {
        let mut report_payload = payload.clone();
        report_payload.report_kind = error_report_kind;
        spawn_sync_report(state.clone(), report_payload);
    } else {
        warn!(
            event_name = "local_core_finalize_missing_error_report_mapping",
            log_type = "event",
            trace_id = %trace_id,
            report_kind = %payload.report_kind,
            "gateway built local core finalize response without background error report mapping"
        );
    }

    Ok(response)
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use serde_json::json;

    use super::maybe_build_local_core_error_response;
    use crate::control::GatewayControlDecision;
    use crate::usage::GatewaySyncReportRequest;

    fn test_decision() -> GatewayControlDecision {
        GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        )
        .with_execution_runtime_candidate(true)
    }

    fn core_finalize_payload(
        report_kind: &str,
        client_api_format: &str,
        provider_api_format: &str,
        status_code: u16,
        body_json: serde_json::Value,
    ) -> GatewaySyncReportRequest {
        GatewaySyncReportRequest {
            trace_id: "trace-core-error-status-123".to_string(),
            report_kind: report_kind.to_string(),
            report_context: Some(json!({
                "client_api_format": client_api_format,
                "provider_api_format": provider_api_format,
            })),
            status_code,
            headers: Default::default(),
            body_json: Some(body_json),
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        }
    }

    #[tokio::test]
    async fn maybe_build_local_core_error_response_infers_status_from_semantic_error_type() {
        let payload = core_finalize_payload(
            "openai_chat_sync_finalize",
            "openai:chat",
            "claude:messages",
            200,
            json!({
                "type": "error",
                "error": {
                    "type": "rate_limit_error",
                    "message": "slow down"
                }
            }),
        );

        let response = maybe_build_local_core_error_response(
            "trace-sync-status-type",
            &test_decision(),
            &payload,
        )
        .expect("response build should not error")
        .expect("response should exist");

        assert_eq!(response.status(), http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body should read"),
            )
            .expect("body should decode"),
            json!({
                "error": {
                    "message": "slow down",
                    "type": "rate_limit_error"
                }
            })
        );
    }

    #[tokio::test]
    async fn maybe_build_local_core_error_response_infers_status_from_gemini_status_text() {
        let payload = core_finalize_payload(
            "gemini_chat_sync_finalize",
            "gemini:generate_content",
            "gemini:generate_content",
            200,
            json!({
                "error": {
                    "message": "quota reached",
                    "status": "RESOURCE_EXHAUSTED"
                }
            }),
        );

        let response = maybe_build_local_core_error_response(
            "trace-sync-status-gemini",
            &test_decision(),
            &payload,
        )
        .expect("response build should not error")
        .expect("response should exist");

        assert_eq!(response.status(), http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body should read"),
            )
            .expect("body should decode"),
            json!({
                "error": {
                    "message": "quota reached",
                    "status": "RESOURCE_EXHAUSTED"
                }
            })
        );
    }
}
