use aether_contracts::{ExecutionPlan, RequestBody};
use tracing::debug;

use super::super::{
    augment_sync_report_context, generic_decision_missing_exact_provider_request,
    take_non_empty_string, LocalSyncPlanAndReport,
};
use crate::ai_pipeline::transport::auth::{
    build_claude_passthrough_headers, build_complete_passthrough_headers_with_auth,
    build_openai_passthrough_headers, ensure_upstream_auth_header,
};
use crate::ai_pipeline::transport::url::{build_openai_chat_url, build_openai_responses_url};
use crate::{GatewayControlSyncDecisionResponse, GatewayError};

pub(crate) fn build_openai_chat_sync_plan_from_decision(
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    payload: GatewayControlSyncDecisionResponse,
) -> Result<Option<LocalSyncPlanAndReport>, GatewayError> {
    let mut payload = payload;
    let Some(request_id) = take_non_empty_string(&mut payload.request_id) else {
        return Ok(None);
    };
    let Some(provider_id) = take_non_empty_string(&mut payload.provider_id) else {
        return Ok(None);
    };
    let Some(endpoint_id) = take_non_empty_string(&mut payload.endpoint_id) else {
        return Ok(None);
    };
    let Some(key_id) = take_non_empty_string(&mut payload.key_id) else {
        return Ok(None);
    };
    let auth_header = take_non_empty_string(&mut payload.auth_header);
    let auth_value = take_non_empty_string(&mut payload.auth_value);
    if auth_header.is_some() != auth_value.is_some() {
        return Ok(None);
    }
    let Some(provider_api_format) = take_non_empty_string(&mut payload.provider_api_format) else {
        return Ok(None);
    };
    let Some(client_api_format) = take_non_empty_string(&mut payload.client_api_format) else {
        return Ok(None);
    };
    let url = if let Some(upstream_url) = take_non_empty_string(&mut payload.upstream_url) {
        upstream_url
    } else {
        let Some(upstream_base_url) = take_non_empty_string(&mut payload.upstream_base_url) else {
            return Ok(None);
        };
        build_openai_chat_url(&upstream_base_url, parts.uri.query())
    };
    let provider_request_body_value = if let Some(body) = payload.provider_request_body.take() {
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
        if let Some(mapped_model) = take_non_empty_string(&mut payload.mapped_model) {
            provider_request_body
                .insert("model".to_string(), serde_json::Value::String(mapped_model));
        }
        if payload.upstream_is_stream {
            provider_request_body.insert("stream".to_string(), serde_json::Value::Bool(true));
        }
        if let Some(prompt_cache_key) = take_non_empty_string(&mut payload.prompt_cache_key) {
            let existing = provider_request_body
                .get("prompt_cache_key")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .unwrap_or_default();
            if existing.is_empty() {
                provider_request_body.insert(
                    "prompt_cache_key".to_string(),
                    serde_json::Value::String(prompt_cache_key),
                );
            }
        }
        serde_json::Value::Object(provider_request_body)
    };
    let existing_provider_request_headers = std::mem::take(&mut payload.provider_request_headers);
    let extra_headers = std::mem::take(&mut payload.extra_headers);
    let mut provider_request_headers = if existing_provider_request_headers.is_empty() {
        if let (Some(auth_header), Some(auth_value)) =
            (auth_header.as_deref(), auth_value.as_deref())
        {
            if provider_api_format == client_api_format {
                build_complete_passthrough_headers_with_auth(
                    &parts.headers,
                    auth_header,
                    auth_value,
                    &extra_headers,
                    payload.content_type.as_deref(),
                )
            } else if provider_api_format.starts_with("claude:") {
                build_claude_passthrough_headers(
                    &parts.headers,
                    auth_header,
                    auth_value,
                    &extra_headers,
                    payload.content_type.as_deref(),
                )
            } else {
                build_openai_passthrough_headers(
                    &parts.headers,
                    auth_header,
                    auth_value,
                    &extra_headers,
                    payload.content_type.as_deref(),
                )
            }
        } else {
            crate::ai_pipeline::transport::auth::build_passthrough_headers(
                &parts.headers,
                &extra_headers,
                payload.content_type.as_deref(),
            )
        }
    } else {
        existing_provider_request_headers
    };
    if let (Some(auth_header), Some(auth_value)) = (auth_header.as_deref(), auth_value.as_deref()) {
        ensure_upstream_auth_header(&mut provider_request_headers, auth_header, auth_value);
    }
    if payload.upstream_is_stream {
        provider_request_headers
            .entry("accept".to_string())
            .or_insert_with(|| "text/event-stream".to_string());
    }
    let content_type = payload
        .content_type
        .take()
        .or_else(|| Some("application/json".to_string()));
    let report_context = augment_sync_report_context(
        payload.report_context.take(),
        &provider_request_headers,
        &provider_request_body_value,
    )?;
    let plan = ExecutionPlan {
        request_id,
        candidate_id: payload.candidate_id.take(),
        provider_name: payload.provider_name.take(),
        provider_id,
        endpoint_id,
        key_id,
        method: "POST".to_string(),
        url,
        headers: std::mem::take(&mut provider_request_headers),
        content_type,
        content_encoding: None,
        body: RequestBody::from_json(provider_request_body_value),
        stream: payload.upstream_is_stream,
        client_api_format,
        provider_api_format,
        model_name: payload.model_name.take(),
        proxy: payload.proxy.take(),
        tls_profile: payload.tls_profile.take(),
        timeouts: payload.timeouts.take(),
    };

    Ok(Some(LocalSyncPlanAndReport {
        plan,
        report_kind: payload.report_kind,
        report_context,
    }))
}

pub(crate) fn build_openai_responses_sync_plan_from_decision(
    parts: &http::request::Parts,
    _body_json: &serde_json::Value,
    payload: GatewayControlSyncDecisionResponse,
    compact: bool,
) -> Result<Option<LocalSyncPlanAndReport>, GatewayError> {
    let mut payload = payload;
    if generic_decision_missing_exact_provider_request(&payload) {
        return Ok(None);
    }
    let Some(request_id) = take_non_empty_string(&mut payload.request_id) else {
        return Ok(None);
    };
    let Some(provider_id) = take_non_empty_string(&mut payload.provider_id) else {
        return Ok(None);
    };
    let Some(endpoint_id) = take_non_empty_string(&mut payload.endpoint_id) else {
        return Ok(None);
    };
    let Some(key_id) = take_non_empty_string(&mut payload.key_id) else {
        return Ok(None);
    };
    let auth_header = take_non_empty_string(&mut payload.auth_header);
    let auth_value = take_non_empty_string(&mut payload.auth_value);
    if auth_header.is_some() != auth_value.is_some() {
        return Ok(None);
    }
    let Some(provider_api_format) = take_non_empty_string(&mut payload.provider_api_format) else {
        return Ok(None);
    };
    let Some(client_api_format) = take_non_empty_string(&mut payload.client_api_format) else {
        return Ok(None);
    };
    let (url, url_source) = if let Some(upstream_url) =
        take_non_empty_string(&mut payload.upstream_url)
    {
        (upstream_url, "upstream_url")
    } else {
        let Some(upstream_base_url) = take_non_empty_string(&mut payload.upstream_base_url) else {
            return Ok(None);
        };
        (
            build_openai_responses_url(&upstream_base_url, parts.uri.query(), compact),
            "upstream_base_url",
        )
    };
    let Some(provider_request_body_value) = payload.provider_request_body.take() else {
        return Ok(None);
    };
    let mut provider_request_headers = std::mem::take(&mut payload.provider_request_headers);
    if let (Some(auth_header), Some(auth_value)) = (auth_header.as_deref(), auth_value.as_deref()) {
        ensure_upstream_auth_header(&mut provider_request_headers, auth_header, auth_value);
    }
    if payload.upstream_is_stream && !provider_request_headers.contains_key("accept") {
        provider_request_headers.insert("accept".to_string(), "text/event-stream".to_string());
    }
    let content_type = payload
        .content_type
        .take()
        .or_else(|| Some("application/json".to_string()));
    let report_context = augment_sync_report_context(
        payload.report_context.take(),
        &provider_request_headers,
        &provider_request_body_value,
    )?;
    let plan = ExecutionPlan {
        request_id,
        candidate_id: payload.candidate_id.take(),
        provider_name: payload.provider_name.take(),
        provider_id,
        endpoint_id,
        key_id,
        method: "POST".to_string(),
        url,
        headers: std::mem::take(&mut provider_request_headers),
        content_type,
        content_encoding: None,
        body: RequestBody::from_json(provider_request_body_value),
        stream: payload.upstream_is_stream,
        client_api_format,
        provider_api_format,
        model_name: payload.model_name.take(),
        proxy: payload.proxy.take(),
        tls_profile: payload.tls_profile.take(),
        timeouts: payload.timeouts.take(),
    };

    debug!(
        event_name = "local_openai_responses_sync_plan_built",
        log_type = "debug",
        request_id = %plan.request_id,
        candidate_id = ?plan.candidate_id,
        provider_id = %plan.provider_id,
        endpoint_id = %plan.endpoint_id,
        key_id = %plan.key_id,
        downstream_path = %parts.uri.path(),
        downstream_query = ?parts.uri.query(),
        url_source,
        decision_upstream_base_url = ?payload.upstream_base_url,
        decision_upstream_url = ?payload.upstream_url,
        plan_url = %plan.url,
        client_api_format = %plan.client_api_format,
        provider_api_format = %plan.provider_api_format,
        upstream_is_stream = payload.upstream_is_stream,
        compact,
        "gateway built local openai responses sync execution plan"
    );

    Ok(Some(LocalSyncPlanAndReport {
        plan,
        report_kind: payload.report_kind,
        report_context,
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{json, Value};

    use super::{
        build_openai_chat_sync_plan_from_decision, build_openai_responses_sync_plan_from_decision,
    };
    use crate::GatewayControlSyncDecisionResponse;

    fn object_keys(value: &Value) -> Vec<&str> {
        value
            .as_object()
            .expect("value should be an object")
            .keys()
            .map(String::as_str)
            .collect()
    }

    fn sample_responses_payload() -> GatewayControlSyncDecisionResponse {
        GatewayControlSyncDecisionResponse {
            action: "sync".to_string(),
            decision_kind: Some("openai_responses_sync".to_string()),
            execution_strategy: None,
            conversion_mode: None,
            request_id: Some("req_123".to_string()),
            candidate_id: Some("cand_123".to_string()),
            provider_name: Some("Codex".to_string()),
            provider_id: Some("prov_123".to_string()),
            endpoint_id: Some("ep_123".to_string()),
            key_id: Some("key_123".to_string()),
            upstream_base_url: Some("https://example.com".to_string()),
            upstream_url: Some("https://example.com/v1/responses".to_string()),
            provider_request_method: None,
            auth_header: Some("authorization".to_string()),
            auth_value: Some("Bearer test".to_string()),
            provider_api_format: Some("openai:responses".to_string()),
            client_api_format: Some("openai:responses".to_string()),
            provider_contract: Some("openai:responses".to_string()),
            client_contract: Some("openai:responses".to_string()),
            model_name: Some("gpt-5.4".to_string()),
            mapped_model: Some("gpt-5.4".to_string()),
            prompt_cache_key: Some("cache-key".to_string()),
            extra_headers: BTreeMap::new(),
            provider_request_headers: BTreeMap::from([(
                "content-type".to_string(),
                "application/json".to_string(),
            )]),
            provider_request_body: Some(json!({
                "text": {"verbosity": "low"},
                "input": [],
                "model": "gpt-5.4",
                "store": false,
                "tools": [],
                "stream": true,
                "include": ["reasoning.encrypted_content"],
                "reasoning": {"effort": "high"},
                "tool_choice": "auto",
                "instructions": "You are Codex.",
                "prompt_cache_key": "cache-key"
            })),
            provider_request_body_base64: None,
            content_type: Some("application/json".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
            upstream_is_stream: true,
            report_kind: Some("openai_responses_sync_success".to_string()),
            report_context: Some(json!({})),
            auth_context: None,
        }
    }

    #[test]
    fn build_openai_responses_sync_plan_preserves_provider_request_body_order_in_plan_and_report() {
        let parts = http::Request::builder()
            .uri("http://localhost/v1/responses")
            .body(())
            .expect("request should build")
            .into_parts()
            .0;
        let payload = sample_responses_payload();

        let built =
            build_openai_responses_sync_plan_from_decision(&parts, &json!({}), payload, false)
                .expect("plan build should succeed")
                .expect("plan should be produced");
        let plan_body = built
            .plan
            .body
            .json_body
            .as_ref()
            .expect("plan json body should exist");
        assert_eq!(
            object_keys(plan_body),
            vec![
                "text",
                "input",
                "model",
                "store",
                "tools",
                "stream",
                "include",
                "reasoning",
                "tool_choice",
                "instructions",
                "prompt_cache_key",
            ]
        );
        assert!(
            built
                .report_context
                .as_ref()
                .and_then(|value| value.get("provider_request_body"))
                .is_none(),
            "report context should not duplicate provider request body"
        );
    }

    #[test]
    fn build_openai_chat_sync_plan_fallback_preserves_complete_same_format_headers() {
        let parts = http::Request::builder()
            .uri("http://localhost/v1/chat/completions")
            .header(http::header::AUTHORIZATION, "Bearer client-token")
            .header("x-stainless-runtime-version", "v24.0.0")
            .header("x-app", "codex")
            .body(())
            .expect("request should build")
            .into_parts()
            .0;
        let payload = GatewayControlSyncDecisionResponse {
            action: "sync".to_string(),
            decision_kind: Some("openai_chat_sync".to_string()),
            execution_strategy: None,
            conversion_mode: None,
            request_id: Some("req_456".to_string()),
            candidate_id: Some("cand_456".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: Some("prov_456".to_string()),
            endpoint_id: Some("ep_456".to_string()),
            key_id: Some("key_456".to_string()),
            upstream_base_url: Some("https://example.com".to_string()),
            upstream_url: Some("https://example.com/v1/chat/completions".to_string()),
            provider_request_method: None,
            auth_header: Some("authorization".to_string()),
            auth_value: Some("Bearer upstream-token".to_string()),
            provider_api_format: Some("openai:chat".to_string()),
            client_api_format: Some("openai:chat".to_string()),
            provider_contract: Some("openai:chat".to_string()),
            client_contract: Some("openai:chat".to_string()),
            model_name: Some("gpt-5.4".to_string()),
            mapped_model: Some("gpt-5.4".to_string()),
            prompt_cache_key: None,
            extra_headers: BTreeMap::new(),
            provider_request_headers: BTreeMap::new(),
            provider_request_body: Some(json!({"model":"gpt-5.4","messages":[],"stream":false})),
            provider_request_body_base64: None,
            content_type: Some("application/json".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
            upstream_is_stream: false,
            report_kind: Some("openai_chat_sync_success".to_string()),
            report_context: Some(json!({})),
            auth_context: None,
        };

        let built = build_openai_chat_sync_plan_from_decision(&parts, &json!({}), payload)
            .expect("plan build should succeed")
            .expect("plan should be produced");

        assert_eq!(
            built.plan.headers.get("authorization").map(String::as_str),
            Some("Bearer upstream-token")
        );
        assert_eq!(
            built
                .plan
                .headers
                .get("x-stainless-runtime-version")
                .map(String::as_str),
            Some("v24.0.0")
        );
        assert_eq!(
            built.plan.headers.get("x-app").map(String::as_str),
            Some("codex")
        );
    }

    #[test]
    fn build_openai_chat_sync_plan_fallback_restores_claude_headers_for_cross_format() {
        let parts = http::Request::builder()
            .uri("http://localhost/v1/chat/completions")
            .header("anthropic-beta", "prompt-caching-2024-07-31")
            .header("x-stainless-runtime-version", "v24.0.0")
            .body(())
            .expect("request should build")
            .into_parts()
            .0;
        let payload = GatewayControlSyncDecisionResponse {
            action: "sync".to_string(),
            decision_kind: Some("openai_chat_sync".to_string()),
            execution_strategy: None,
            conversion_mode: Some("format_conversion".to_string()),
            request_id: Some("req_789".to_string()),
            candidate_id: Some("cand_789".to_string()),
            provider_name: Some("Claude".to_string()),
            provider_id: Some("prov_789".to_string()),
            endpoint_id: Some("ep_789".to_string()),
            key_id: Some("key_789".to_string()),
            upstream_base_url: Some("https://example.com".to_string()),
            upstream_url: Some("https://example.com/v1/messages".to_string()),
            provider_request_method: None,
            auth_header: Some("x-api-key".to_string()),
            auth_value: Some("sk-upstream-claude".to_string()),
            provider_api_format: Some("claude:chat".to_string()),
            client_api_format: Some("openai:chat".to_string()),
            provider_contract: Some("claude:chat".to_string()),
            client_contract: Some("openai:chat".to_string()),
            model_name: Some("claude-sonnet-4-5".to_string()),
            mapped_model: Some("claude-sonnet-4-5".to_string()),
            prompt_cache_key: None,
            extra_headers: BTreeMap::new(),
            provider_request_headers: BTreeMap::new(),
            provider_request_body: Some(
                json!({"model":"claude-sonnet-4-5","messages":[],"stream":false}),
            ),
            provider_request_body_base64: None,
            content_type: Some("application/json".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
            upstream_is_stream: false,
            report_kind: Some("openai_chat_sync_success".to_string()),
            report_context: Some(json!({})),
            auth_context: None,
        };

        let built = build_openai_chat_sync_plan_from_decision(&parts, &json!({}), payload)
            .expect("plan build should succeed")
            .expect("plan should be produced");

        assert_eq!(
            built.plan.headers.get("x-api-key").map(String::as_str),
            Some("sk-upstream-claude")
        );
        assert_eq!(
            built.plan.headers.get("anthropic-beta").map(String::as_str),
            Some("prompt-caching-2024-07-31")
        );
        assert_eq!(
            built
                .plan
                .headers
                .get("anthropic-version")
                .map(String::as_str),
            Some("2023-06-01")
        );
    }
}
