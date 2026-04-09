use aether_contracts::{ExecutionPlan, RequestBody};

use super::{augment_sync_report_context, LocalStreamPlanAndReport, LocalSyncPlanAndReport};
use crate::ai_pipeline::contracts::generic_decision_missing_exact_provider_request;
use crate::ai_pipeline::provider_adaptation_requires_eventstream_accept;
use crate::ai_pipeline::transport::ensure_upstream_auth_header;
use crate::{GatewayControlSyncDecisionResponse, GatewayError};

pub(crate) fn build_standard_sync_plan_from_decision(
    _parts: &http::request::Parts,
    _body_json: &serde_json::Value,
    payload: GatewayControlSyncDecisionResponse,
) -> Result<Option<LocalSyncPlanAndReport>, GatewayError> {
    if generic_decision_missing_exact_provider_request(&payload) {
        return Ok(None);
    }
    let Some(request_id) = payload
        .request_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(provider_id) = payload
        .provider_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(endpoint_id) = payload
        .endpoint_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(key_id) = payload
        .key_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(url) = payload
        .upstream_url
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let auth_header = payload
        .auth_header
        .clone()
        .filter(|value| !value.trim().is_empty());
    let auth_value = payload
        .auth_value
        .clone()
        .filter(|value| !value.trim().is_empty());
    if auth_header.is_some() != auth_value.is_some() {
        return Ok(None);
    }
    let Some(provider_api_format) = payload
        .provider_api_format
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(client_api_format) = payload
        .client_api_format
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(provider_request_body_value) = payload.provider_request_body.clone() else {
        return Ok(None);
    };

    let mut provider_request_headers = payload.provider_request_headers.clone();
    if let (Some(auth_header), Some(auth_value)) = (auth_header.as_deref(), auth_value.as_deref()) {
        ensure_upstream_auth_header(&mut provider_request_headers, auth_header, auth_value);
    }
    if payload.upstream_is_stream {
        provider_request_headers
            .entry("accept".to_string())
            .or_insert_with(|| "text/event-stream".to_string());
    }
    let plan = ExecutionPlan {
        request_id,
        candidate_id: payload.candidate_id.clone(),
        provider_name: payload.provider_name.clone(),
        provider_id,
        endpoint_id,
        key_id,
        method: "POST".to_string(),
        url,
        headers: std::mem::take(&mut provider_request_headers),
        content_type: payload
            .content_type
            .clone()
            .or_else(|| Some("application/json".to_string())),
        content_encoding: None,
        body: RequestBody::from_json(provider_request_body_value.clone()),
        stream: payload.upstream_is_stream,
        client_api_format,
        provider_api_format,
        model_name: payload.model_name.clone(),
        proxy: payload.proxy.clone(),
        tls_profile: payload.tls_profile.clone(),
        timeouts: payload.timeouts.clone(),
    };

    let report_context = augment_sync_report_context(
        payload.report_context,
        &plan.headers,
        &provider_request_body_value,
    )?;

    Ok(Some(LocalSyncPlanAndReport {
        plan,
        report_kind: payload.report_kind,
        report_context,
    }))
}

pub(crate) fn build_standard_stream_plan_from_decision(
    _parts: &http::request::Parts,
    _body_json: &serde_json::Value,
    payload: GatewayControlSyncDecisionResponse,
    _inject_stream_flag: bool,
) -> Result<Option<LocalStreamPlanAndReport>, GatewayError> {
    if generic_decision_missing_exact_provider_request(&payload) {
        return Ok(None);
    }
    let Some(request_id) = payload
        .request_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(provider_id) = payload
        .provider_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(endpoint_id) = payload
        .endpoint_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(key_id) = payload
        .key_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(url) = payload
        .upstream_url
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let auth_header = payload
        .auth_header
        .clone()
        .filter(|value| !value.trim().is_empty());
    let auth_value = payload
        .auth_value
        .clone()
        .filter(|value| !value.trim().is_empty());
    if auth_header.is_some() != auth_value.is_some() {
        return Ok(None);
    }
    let Some(provider_api_format) = payload
        .provider_api_format
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(client_api_format) = payload
        .client_api_format
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(provider_request_body_value) = payload.provider_request_body.clone() else {
        return Ok(None);
    };

    let envelope_name = payload
        .report_context
        .as_ref()
        .and_then(|context| context.get("envelope_name"))
        .and_then(serde_json::Value::as_str);
    let mut provider_request_headers = payload.provider_request_headers.clone();
    if let (Some(auth_header), Some(auth_value)) = (auth_header.as_deref(), auth_value.as_deref()) {
        ensure_upstream_auth_header(&mut provider_request_headers, auth_header, auth_value);
    }
    if provider_adaptation_requires_eventstream_accept(envelope_name, provider_api_format.as_str())
    {
        provider_request_headers
            .entry("accept".to_string())
            .or_insert_with(|| "application/vnd.amazon.eventstream".to_string());
    } else {
        provider_request_headers.insert("accept".to_string(), "text/event-stream".to_string());
    }
    let plan = ExecutionPlan {
        request_id,
        candidate_id: payload.candidate_id.clone(),
        provider_name: payload.provider_name.clone(),
        provider_id,
        endpoint_id,
        key_id,
        method: "POST".to_string(),
        url,
        headers: std::mem::take(&mut provider_request_headers),
        content_type: payload
            .content_type
            .clone()
            .or_else(|| Some("application/json".to_string())),
        content_encoding: None,
        body: RequestBody::from_json(provider_request_body_value.clone()),
        stream: true,
        client_api_format,
        provider_api_format,
        model_name: payload.model_name.clone(),
        proxy: payload.proxy.clone(),
        tls_profile: payload.tls_profile.clone(),
        timeouts: payload.timeouts.clone(),
    };

    let report_context = augment_sync_report_context(
        payload.report_context,
        &plan.headers,
        &provider_request_body_value,
    )?;

    Ok(Some(LocalStreamPlanAndReport {
        plan,
        report_kind: payload.report_kind,
        report_context,
    }))
}
