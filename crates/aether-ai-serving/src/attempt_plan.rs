use std::collections::BTreeMap;

use aether_ai_formats::api::ExecutionRuntimeAuthContext;
use aether_contracts::{ExecutionPlan, RequestBody};
use url::Url;

use crate::dto::AiExecutionDecision;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiDecisionPlanCore {
    pub request_id: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub key_id: String,
    pub provider_api_format: String,
    pub client_api_format: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiUpstreamAuthPair {
    pub header: String,
    pub value: String,
}

#[derive(Debug)]
pub struct AiExecutionPlanFromDecisionParts {
    pub core: AiDecisionPlanCore,
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub content_type: Option<String>,
    pub body: RequestBody,
    pub stream: bool,
}

#[derive(Debug)]
pub struct AiExecutionDecisionFromPlanParts {
    pub action: String,
    pub decision_kind: Option<String>,
    pub request_id: Option<String>,
    pub upstream_base_url: Option<String>,
    pub include_auth_pair: bool,
    pub plan: ExecutionPlan,
    pub report_kind: Option<String>,
    pub report_context: Option<serde_json::Value>,
    pub auth_context: Option<ExecutionRuntimeAuthContext>,
}

pub fn take_ai_non_empty_string(value: &mut Option<String>) -> Option<String> {
    value.take().filter(|value| !value.trim().is_empty())
}

pub fn trim_ai_owned_non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() == value.len() {
        return Some(value);
    }
    Some(trimmed.to_owned())
}

pub fn take_ai_decision_plan_core(payload: &mut AiExecutionDecision) -> Option<AiDecisionPlanCore> {
    Some(AiDecisionPlanCore {
        request_id: take_ai_non_empty_string(&mut payload.request_id)?,
        provider_id: take_ai_non_empty_string(&mut payload.provider_id)?,
        endpoint_id: take_ai_non_empty_string(&mut payload.endpoint_id)?,
        key_id: take_ai_non_empty_string(&mut payload.key_id)?,
        provider_api_format: take_ai_non_empty_string(&mut payload.provider_api_format)?,
        client_api_format: take_ai_non_empty_string(&mut payload.client_api_format)?,
    })
}

pub fn take_ai_upstream_auth_pair(
    payload: &mut AiExecutionDecision,
) -> Option<Option<AiUpstreamAuthPair>> {
    let header = take_ai_non_empty_string(&mut payload.auth_header);
    let value = take_ai_non_empty_string(&mut payload.auth_value);
    match (header, value) {
        (Some(header), Some(value)) => Some(Some(AiUpstreamAuthPair { header, value })),
        (None, None) => Some(None),
        _ => None,
    }
}

pub fn resolve_ai_passthrough_sync_request_body(
    provider_request_body: Option<serde_json::Value>,
    provider_request_body_base64: Option<String>,
) -> RequestBody {
    if let Some(body_bytes_b64) =
        provider_request_body_base64.and_then(trim_ai_owned_non_empty_string)
    {
        return RequestBody {
            json_body: None,
            body_bytes_b64: Some(body_bytes_b64),
            body_ref: None,
        };
    }

    match provider_request_body.unwrap_or(serde_json::Value::Null) {
        serde_json::Value::Null => RequestBody {
            json_body: None,
            body_bytes_b64: None,
            body_ref: None,
        },
        other => RequestBody::from_json(other),
    }
}

pub fn build_ai_execution_plan_from_decision(
    payload: &mut AiExecutionDecision,
    parts: AiExecutionPlanFromDecisionParts,
) -> ExecutionPlan {
    ExecutionPlan {
        request_id: parts.core.request_id,
        candidate_id: payload.candidate_id.take(),
        provider_name: payload.provider_name.take(),
        provider_id: parts.core.provider_id,
        endpoint_id: parts.core.endpoint_id,
        key_id: parts.core.key_id,
        method: parts.method,
        url: parts.url,
        headers: parts.headers,
        content_type: parts.content_type,
        content_encoding: None,
        body: parts.body,
        stream: parts.stream,
        client_api_format: parts.core.client_api_format,
        provider_api_format: parts.core.provider_api_format,
        model_name: payload.model_name.take(),
        proxy: payload.proxy.take(),
        tls_profile: payload.tls_profile.take(),
        timeouts: payload.timeouts.take(),
    }
}

pub fn build_ai_execution_decision_from_plan(
    parts: AiExecutionDecisionFromPlanParts,
) -> AiExecutionDecision {
    let ExecutionPlan {
        request_id,
        candidate_id,
        provider_name,
        provider_id,
        endpoint_id,
        key_id,
        method,
        url,
        headers,
        content_type,
        content_encoding: _content_encoding,
        body,
        stream,
        client_api_format,
        provider_api_format,
        model_name,
        proxy,
        tls_profile,
        timeouts,
    } = parts.plan;
    let auth_pair = parts
        .include_auth_pair
        .then(|| extract_ai_auth_header_pair(&headers))
        .flatten();
    let provider_contract = provider_api_format.clone();
    let client_contract = client_api_format.clone();
    let request_id = parts.request_id.unwrap_or(request_id);
    let auth_header = auth_pair.map(|(name, _)| name.to_string());
    let auth_value = auth_pair.map(|(_, value)| value.to_string());
    let RequestBody {
        json_body,
        body_bytes_b64,
        body_ref: _body_ref,
    } = body;

    AiExecutionDecision {
        action: parts.action,
        decision_kind: parts.decision_kind,
        execution_strategy: Some(ai_execution_strategy_for_formats(
            provider_api_format.as_str(),
            client_api_format.as_str(),
        )),
        conversion_mode: Some(ai_conversion_mode_for_formats(
            provider_api_format.as_str(),
            client_api_format.as_str(),
        )),
        request_id: Some(request_id),
        candidate_id,
        provider_name,
        provider_id: Some(provider_id),
        endpoint_id: Some(endpoint_id),
        key_id: Some(key_id),
        upstream_base_url: parts.upstream_base_url,
        upstream_url: Some(url),
        provider_request_method: Some(method),
        auth_header,
        auth_value,
        provider_api_format: Some(provider_api_format),
        client_api_format: Some(client_api_format),
        provider_contract: Some(provider_contract),
        client_contract: Some(client_contract),
        model_name,
        mapped_model: None,
        prompt_cache_key: None,
        extra_headers: BTreeMap::new(),
        provider_request_headers: headers,
        provider_request_body: json_body,
        provider_request_body_base64: body_bytes_b64,
        content_type,
        proxy,
        tls_profile,
        timeouts,
        upstream_is_stream: stream,
        report_kind: parts.report_kind,
        report_context: parts.report_context,
        auth_context: parts.auth_context,
    }
}

pub fn extract_ai_auth_header_pair(headers: &BTreeMap<String, String>) -> Option<(&str, &str)> {
    [
        "authorization",
        "x-api-key",
        "api-key",
        "x-goog-api-key",
        "proxy-authorization",
    ]
    .into_iter()
    .find_map(|name| {
        headers
            .iter()
            .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(header_name, value)| (header_name.as_str(), value.as_str()))
    })
}

pub fn infer_ai_upstream_base_url(upstream_url: &str) -> Option<String> {
    let parsed = Url::parse(upstream_url).ok()?;
    let host = parsed.host_str()?;
    let mut base = format!("{}://{}", parsed.scheme(), host);
    if let Some(port) = parsed.port() {
        base.push(':');
        base.push_str(port.to_string().as_str());
    }
    let base_path = infer_ai_upstream_base_path(parsed.path());
    if !base_path.is_empty() {
        base.push_str(base_path);
    }
    Some(base)
}

fn infer_ai_upstream_base_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return "";
    }

    for suffix in [
        "/responses/compact",
        "/responses",
        "/chat/completions",
        "/messages",
    ] {
        if let Some(prefix) = trimmed.strip_suffix(suffix) {
            return normalize_inferred_ai_base_path(prefix);
        }
    }

    for marker in ["/v1/videos", "/v1beta/"] {
        if let Some((prefix, _)) = trimmed.split_once(marker) {
            return normalize_inferred_ai_base_path(prefix);
        }
    }

    normalize_inferred_ai_base_path(trimmed)
}

fn normalize_inferred_ai_base_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        ""
    } else {
        trimmed
    }
}

fn ai_execution_strategy_for_formats(provider_api_format: &str, client_api_format: &str) -> String {
    if provider_api_format == client_api_format {
        "local_same_format"
    } else {
        "local_cross_format"
    }
    .to_string()
}

fn ai_conversion_mode_for_formats(provider_api_format: &str, client_api_format: &str) -> String {
    if provider_api_format == client_api_format {
        "none"
    } else {
        "bidirectional"
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;

    #[test]
    fn take_ai_decision_plan_core_consumes_required_non_empty_fields() {
        let mut payload = test_decision();

        let core =
            take_ai_decision_plan_core(&mut payload).expect("core fields should be available");

        assert_eq!(core.request_id, "req_1");
        assert_eq!(core.provider_id, "provider_1");
        assert_eq!(core.endpoint_id, "endpoint_1");
        assert_eq!(core.key_id, "key_1");
        assert_eq!(core.provider_api_format, "openai:chat");
        assert_eq!(core.client_api_format, "openai:chat");
        assert!(payload.request_id.is_none());
        assert!(payload.provider_api_format.is_none());
    }

    #[test]
    fn take_ai_decision_plan_core_rejects_blank_required_fields() {
        let mut payload = test_decision();
        payload.endpoint_id = Some("  ".to_string());

        assert!(take_ai_decision_plan_core(&mut payload).is_none());
    }

    #[test]
    fn take_ai_upstream_auth_pair_rejects_incomplete_auth() {
        let mut payload = test_decision();
        payload.auth_header = Some("authorization".to_string());
        payload.auth_value = Some(" ".to_string());

        assert!(take_ai_upstream_auth_pair(&mut payload).is_none());
    }

    #[test]
    fn resolve_ai_passthrough_sync_request_body_prefers_trimmed_base64() {
        let body = resolve_ai_passthrough_sync_request_body(
            Some(json!({"ignored": true})),
            Some("  YWJj  ".to_string()),
        );

        assert_eq!(body.body_bytes_b64.as_deref(), Some("YWJj"));
        assert!(body.json_body.is_none());
    }

    #[test]
    fn resolve_ai_passthrough_sync_request_body_uses_json_when_no_base64() {
        let body = resolve_ai_passthrough_sync_request_body(Some(json!({"ok": true})), None);

        assert_eq!(body.json_body, Some(json!({"ok": true})));
        assert!(body.body_bytes_b64.is_none());
    }

    #[test]
    fn build_ai_execution_plan_from_decision_merges_core_and_remaining_payload_fields() {
        let mut payload = test_decision();
        let core =
            take_ai_decision_plan_core(&mut payload).expect("core fields should be available");
        let plan = build_ai_execution_plan_from_decision(
            &mut payload,
            AiExecutionPlanFromDecisionParts {
                core,
                method: "POST".to_string(),
                url: "https://example.com/v1/chat/completions".to_string(),
                headers: BTreeMap::from([(
                    "content-type".to_string(),
                    "application/json".to_string(),
                )]),
                content_type: Some("application/json".to_string()),
                body: RequestBody::from_json(json!({"model": "gpt-test"})),
                stream: true,
            },
        );

        assert_eq!(plan.request_id, "req_1");
        assert_eq!(plan.candidate_id.as_deref(), Some("candidate_1"));
        assert_eq!(plan.provider_id, "provider_1");
        assert_eq!(plan.endpoint_id, "endpoint_1");
        assert_eq!(plan.key_id, "key_1");
        assert!(plan.stream);
        assert_eq!(plan.provider_api_format, "openai:chat");
        assert_eq!(plan.client_api_format, "openai:chat");
        assert_eq!(plan.model_name.as_deref(), Some("gpt-test"));
        assert!(payload.candidate_id.is_none());
        assert!(payload.model_name.is_none());
    }

    #[test]
    fn infer_ai_upstream_base_url_preserves_codex_base_path() {
        assert_eq!(
            infer_ai_upstream_base_url("https://tiger.bookapi.cc/codex/responses").as_deref(),
            Some("https://tiger.bookapi.cc/codex")
        );
        assert_eq!(
            infer_ai_upstream_base_url("https://chatgpt.com/backend-api/codex/responses")
                .as_deref(),
            Some("https://chatgpt.com/backend-api/codex")
        );
    }

    #[test]
    fn infer_ai_upstream_base_url_preserves_nested_v1_prefix() {
        assert_eq!(
            infer_ai_upstream_base_url(
                "https://api.openai.example/custom/v1/chat/completions?mode=1"
            )
            .as_deref(),
            Some("https://api.openai.example/custom/v1")
        );
    }

    #[test]
    fn infer_ai_upstream_base_url_strips_video_operation_path() {
        assert_eq!(
            infer_ai_upstream_base_url("https://video.example/nested/v1/videos/task-123/content")
                .as_deref(),
            Some("https://video.example/nested")
        );
    }

    #[test]
    fn build_ai_execution_decision_from_plan_maps_plan_fields() {
        let plan = ExecutionPlan {
            request_id: "plan-request".to_string(),
            candidate_id: Some("candidate-1".to_string()),
            provider_name: Some("provider".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://api.example.com/v1/chat/completions".to_string(),
            headers: BTreeMap::from([("Authorization".to_string(), "Bearer secret".to_string())]),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "mapped"})),
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "claude:messages".to_string(),
            model_name: Some("mapped".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        };

        let decision = build_ai_execution_decision_from_plan(AiExecutionDecisionFromPlanParts {
            action: "execution_runtime.sync_decision".to_string(),
            decision_kind: Some("openai_chat_sync".to_string()),
            request_id: Some("trace-1".to_string()),
            upstream_base_url: Some("https://api.example.com".to_string()),
            include_auth_pair: true,
            plan,
            report_kind: Some("report".to_string()),
            report_context: Some(json!({"candidate_index": 0})),
            auth_context: None,
        });

        assert_eq!(decision.request_id.as_deref(), Some("trace-1"));
        assert_eq!(
            decision.execution_strategy.as_deref(),
            Some("local_cross_format")
        );
        assert_eq!(decision.conversion_mode.as_deref(), Some("bidirectional"));
        assert_eq!(decision.auth_header.as_deref(), Some("Authorization"));
        assert_eq!(decision.auth_value.as_deref(), Some("Bearer secret"));
        assert_eq!(
            decision.provider_request_body,
            Some(json!({"model": "mapped"}))
        );
        assert_eq!(decision.report_kind.as_deref(), Some("report"));
    }

    fn test_decision() -> AiExecutionDecision {
        AiExecutionDecision {
            action: "sync".to_string(),
            decision_kind: Some("test".to_string()),
            execution_strategy: None,
            conversion_mode: None,
            request_id: Some("req_1".to_string()),
            candidate_id: Some("candidate_1".to_string()),
            provider_name: Some("provider".to_string()),
            provider_id: Some("provider_1".to_string()),
            endpoint_id: Some("endpoint_1".to_string()),
            key_id: Some("key_1".to_string()),
            upstream_base_url: Some("https://example.com".to_string()),
            upstream_url: Some("https://example.com/v1/chat/completions".to_string()),
            provider_request_method: None,
            auth_header: Some("authorization".to_string()),
            auth_value: Some("Bearer token".to_string()),
            provider_api_format: Some("openai:chat".to_string()),
            client_api_format: Some("openai:chat".to_string()),
            provider_contract: Some("openai:chat".to_string()),
            client_contract: Some("openai:chat".to_string()),
            model_name: Some("gpt-test".to_string()),
            mapped_model: Some("gpt-test".to_string()),
            prompt_cache_key: None,
            extra_headers: BTreeMap::new(),
            provider_request_headers: BTreeMap::new(),
            provider_request_body: None,
            provider_request_body_base64: None,
            content_type: None,
            proxy: None,
            tls_profile: None,
            timeouts: None,
            upstream_is_stream: false,
            report_kind: None,
            report_context: None,
            auth_context: None,
        }
    }
}
