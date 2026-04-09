use aether_contracts::{ExecutionPlan, RequestBody};

use super::super::{
    augment_sync_report_context, generic_decision_missing_exact_provider_request,
    LocalSyncPlanAndReport,
};
use crate::ai_pipeline::transport::auth::{
    build_openai_passthrough_headers, ensure_upstream_auth_header,
};
use crate::ai_pipeline::transport::url::{build_openai_chat_url, build_openai_cli_url};
use crate::{GatewayControlSyncDecisionResponse, GatewayError};

pub(crate) fn build_openai_chat_sync_plan_from_decision(
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    payload: GatewayControlSyncDecisionResponse,
) -> Result<Option<LocalSyncPlanAndReport>, GatewayError> {
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
    let Some(auth_header) = payload
        .auth_header
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(auth_value) = payload
        .auth_value
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
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
    let url = if let Some(upstream_url) = payload
        .upstream_url
        .clone()
        .filter(|value| !value.trim().is_empty())
    {
        upstream_url
    } else {
        let Some(upstream_base_url) = payload
            .upstream_base_url
            .clone()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        build_openai_chat_url(&upstream_base_url, parts.uri.query())
    };
    let provider_request_body_value = if let Some(body) = payload.provider_request_body.clone() {
        body
    } else {
        let Some(request_body_object) = body_json.as_object() else {
            return Ok(None);
        };
        let mut provider_request_body = serde_json::Map::from_iter(
            request_body_object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        if let Some(mapped_model) = payload
            .mapped_model
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            provider_request_body.insert(
                "model".to_string(),
                serde_json::Value::String(mapped_model.clone()),
            );
        }
        if payload.upstream_is_stream {
            provider_request_body.insert("stream".to_string(), serde_json::Value::Bool(true));
        }
        if let Some(prompt_cache_key) = payload
            .prompt_cache_key
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            let existing = provider_request_body
                .get("prompt_cache_key")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .unwrap_or_default();
            if existing.is_empty() {
                provider_request_body.insert(
                    "prompt_cache_key".to_string(),
                    serde_json::Value::String(prompt_cache_key.clone()),
                );
            }
        }
        serde_json::Value::Object(provider_request_body)
    };

    let mut provider_request_headers = if payload.provider_request_headers.is_empty() {
        build_openai_passthrough_headers(
            &parts.headers,
            &auth_header,
            &auth_value,
            &payload.extra_headers,
            payload.content_type.as_deref(),
        )
    } else {
        payload.provider_request_headers.clone()
    };
    ensure_upstream_auth_header(&mut provider_request_headers, &auth_header, &auth_value);
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

pub(crate) fn build_openai_cli_sync_plan_from_decision(
    parts: &http::request::Parts,
    _body_json: &serde_json::Value,
    payload: GatewayControlSyncDecisionResponse,
    compact: bool,
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
    let url = if let Some(upstream_url) = payload
        .upstream_url
        .clone()
        .filter(|value| !value.trim().is_empty())
    {
        upstream_url
    } else {
        let Some(upstream_base_url) = payload
            .upstream_base_url
            .clone()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        build_openai_cli_url(&upstream_base_url, parts.uri.query(), compact)
    };
    let Some(provider_request_body_value) = payload.provider_request_body.clone() else {
        return Ok(None);
    };

    let mut provider_request_headers = payload.provider_request_headers.clone();
    if let (Some(auth_header), Some(auth_value)) = (auth_header.as_deref(), auth_value.as_deref()) {
        ensure_upstream_auth_header(&mut provider_request_headers, auth_header, auth_value);
    }
    if payload.upstream_is_stream && !provider_request_headers.contains_key("accept") {
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
