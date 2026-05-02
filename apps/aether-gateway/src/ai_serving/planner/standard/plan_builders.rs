use std::collections::BTreeMap;

use aether_contracts::RequestBody;

use super::{
    augment_sync_report_context, build_ai_execution_plan_from_decision, take_ai_decision_plan_core,
    take_ai_upstream_auth_pair, take_non_empty_string, AiExecutionPlanFromDecisionParts,
    AiStreamAttempt, AiSyncAttempt,
};
use crate::ai_serving::transport::{
    build_standard_plan_fallback_headers, StandardPlanFallbackAcceptPolicy,
    StandardPlanFallbackHeadersInput,
};
use crate::ai_serving::{
    generic_decision_missing_exact_provider_request,
    provider_adaptation_requires_eventstream_accept,
};
use crate::{AiExecutionDecision, GatewayError};

pub(crate) fn build_standard_sync_plan_from_decision(
    parts: &http::request::Parts,
    _body_json: &serde_json::Value,
    payload: AiExecutionDecision,
) -> Result<Option<AiSyncAttempt>, GatewayError> {
    let mut payload = payload;
    if generic_decision_missing_exact_provider_request(&payload) {
        return Ok(None);
    }
    let Some(core) = take_ai_decision_plan_core(&mut payload) else {
        return Ok(None);
    };
    let Some(url) = take_non_empty_string(&mut payload.upstream_url) else {
        return Ok(None);
    };
    let Some(auth_pair) = take_ai_upstream_auth_pair(&mut payload) else {
        return Ok(None);
    };
    let Some(provider_request_body_value) = payload.provider_request_body.take() else {
        return Ok(None);
    };
    let mut provider_request_headers =
        build_standard_plan_fallback_headers(StandardPlanFallbackHeadersInput {
            request_headers: &parts.headers,
            existing_provider_request_headers: std::mem::take(
                &mut payload.provider_request_headers,
            ),
            auth_header: auth_pair.as_ref().map(|pair| pair.header.as_str()),
            auth_value: auth_pair.as_ref().map(|pair| pair.value.as_str()),
            extra_headers: &BTreeMap::new(),
            content_type: payload.content_type.as_deref(),
            provider_api_format: core.provider_api_format.as_str(),
            client_api_format: core.client_api_format.as_str(),
            upstream_is_stream: payload.upstream_is_stream,
            build_from_request_when_empty: false,
            accept_policy: StandardPlanFallbackAcceptPolicy::TextEventStreamIfStreaming,
        });
    let content_type = payload
        .content_type
        .take()
        .or_else(|| Some("application/json".to_string()));
    let report_context = augment_sync_report_context(
        payload.report_context.take(),
        &provider_request_headers,
        &provider_request_body_value,
    )?;
    let stream = payload.upstream_is_stream;
    let plan = build_ai_execution_plan_from_decision(
        &mut payload,
        AiExecutionPlanFromDecisionParts {
            core,
            method: "POST".to_string(),
            url,
            headers: std::mem::take(&mut provider_request_headers),
            content_type,
            body: RequestBody::from_json(provider_request_body_value),
            stream,
        },
    );

    Ok(Some(AiSyncAttempt {
        plan,
        report_kind: payload.report_kind,
        report_context,
    }))
}

pub(crate) fn build_standard_stream_plan_from_decision(
    parts: &http::request::Parts,
    _body_json: &serde_json::Value,
    payload: AiExecutionDecision,
    _inject_stream_flag: bool,
) -> Result<Option<AiStreamAttempt>, GatewayError> {
    let mut payload = payload;
    if generic_decision_missing_exact_provider_request(&payload) {
        return Ok(None);
    }
    let Some(core) = take_ai_decision_plan_core(&mut payload) else {
        return Ok(None);
    };
    let Some(url) = take_non_empty_string(&mut payload.upstream_url) else {
        return Ok(None);
    };
    let Some(auth_pair) = take_ai_upstream_auth_pair(&mut payload) else {
        return Ok(None);
    };
    let Some(provider_request_body_value) = payload.provider_request_body.take() else {
        return Ok(None);
    };

    let envelope_name = payload
        .report_context
        .as_ref()
        .and_then(|context| context.get("envelope_name"))
        .and_then(serde_json::Value::as_str);
    let accept_policy = if provider_adaptation_requires_eventstream_accept(
        envelope_name,
        core.provider_api_format.as_str(),
    ) {
        StandardPlanFallbackAcceptPolicy::ProviderEventStreamIfMissing
    } else {
        StandardPlanFallbackAcceptPolicy::TextEventStreamRequired
    };
    let mut provider_request_headers =
        build_standard_plan_fallback_headers(StandardPlanFallbackHeadersInput {
            request_headers: &parts.headers,
            existing_provider_request_headers: std::mem::take(
                &mut payload.provider_request_headers,
            ),
            auth_header: auth_pair.as_ref().map(|pair| pair.header.as_str()),
            auth_value: auth_pair.as_ref().map(|pair| pair.value.as_str()),
            extra_headers: &BTreeMap::new(),
            content_type: payload.content_type.as_deref(),
            provider_api_format: core.provider_api_format.as_str(),
            client_api_format: core.client_api_format.as_str(),
            upstream_is_stream: true,
            build_from_request_when_empty: false,
            accept_policy,
        });
    let content_type = payload
        .content_type
        .take()
        .or_else(|| Some("application/json".to_string()));
    let report_context = augment_sync_report_context(
        payload.report_context.take(),
        &provider_request_headers,
        &provider_request_body_value,
    )?;
    let plan = build_ai_execution_plan_from_decision(
        &mut payload,
        AiExecutionPlanFromDecisionParts {
            core,
            method: "POST".to_string(),
            url,
            headers: std::mem::take(&mut provider_request_headers),
            content_type,
            body: RequestBody::from_json(provider_request_body_value),
            stream: true,
        },
    );

    Ok(Some(AiStreamAttempt {
        plan,
        report_kind: payload.report_kind,
        report_context,
    }))
}
