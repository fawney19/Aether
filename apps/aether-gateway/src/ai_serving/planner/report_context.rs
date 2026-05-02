use std::collections::BTreeMap;

use aether_ai_serving::{
    build_ai_execution_report_context,
    insert_provider_stream_event_api_format as insert_ai_provider_stream_event_api_format,
    provider_stream_event_api_format_for_provider_type as ai_provider_stream_event_api_format_for_provider_type,
    AiExecutionReportContextParts, AiRequestOrigin,
};
use aether_scheduler_core::SchedulerRankingOutcome;
use serde_json::{Map, Value};

use crate::ai_serving::{request_origin_from_headers, ExecutionRuntimeAuthContext, RequestOrigin};
use crate::orchestration::ExecutionAttemptIdentity;

pub(crate) struct LocalExecutionReportContextParts<'a> {
    pub(crate) auth_context: &'a ExecutionRuntimeAuthContext,
    pub(crate) request_id: &'a str,
    pub(crate) candidate_id: &'a str,
    pub(crate) attempt_identity: ExecutionAttemptIdentity,
    pub(crate) model: &'a str,
    pub(crate) provider_name: &'a str,
    pub(crate) provider_id: &'a str,
    pub(crate) endpoint_id: &'a str,
    pub(crate) key_id: &'a str,
    pub(crate) key_name: Option<&'a str>,
    pub(crate) model_id: Option<&'a str>,
    pub(crate) global_model_id: Option<&'a str>,
    pub(crate) global_model_name: Option<&'a str>,
    pub(crate) provider_api_format: &'a str,
    pub(crate) client_api_format: &'a str,
    pub(crate) mapped_model: Option<&'a str>,
    pub(crate) candidate_group_id: Option<&'a str>,
    pub(crate) ranking: Option<&'a SchedulerRankingOutcome>,
    pub(crate) upstream_url: Option<&'a str>,
    pub(crate) header_rules: Option<&'a Value>,
    pub(crate) body_rules: Option<&'a Value>,
    pub(crate) provider_request_method: Option<Value>,
    pub(crate) provider_request_headers: Option<&'a BTreeMap<String, String>>,
    pub(crate) original_headers: &'a http::HeaderMap,
    pub(crate) request_origin: Option<RequestOrigin>,
    pub(crate) original_request_body_json: Option<&'a Value>,
    pub(crate) original_request_body_base64: Option<&'a str>,
    pub(crate) client_requested_stream: bool,
    pub(crate) upstream_is_stream: bool,
    pub(crate) has_envelope: bool,
    pub(crate) needs_conversion: bool,
    pub(crate) extra_fields: Map<String, Value>,
}

pub(crate) fn build_local_execution_report_context(
    parts: LocalExecutionReportContextParts<'_>,
) -> Value {
    let RequestOrigin {
        client_ip,
        user_agent,
    } = parts
        .request_origin
        .unwrap_or_else(|| request_origin_from_headers(parts.original_headers));
    let original_headers = crate::ai_serving::collect_control_headers(parts.original_headers);
    let original_request_body = crate::ai_serving::build_report_context_original_request_echo(
        parts.original_request_body_json,
        parts.original_request_body_base64,
    );

    build_ai_execution_report_context(AiExecutionReportContextParts {
        auth_context: parts.auth_context,
        request_id: parts.request_id,
        candidate_id: parts.candidate_id,
        candidate_index: parts.attempt_identity.candidate_index,
        retry_index: parts.attempt_identity.retry_index,
        pool_key_index: parts.attempt_identity.pool_key_index,
        model: parts.model,
        provider_name: parts.provider_name,
        provider_id: parts.provider_id,
        endpoint_id: parts.endpoint_id,
        key_id: parts.key_id,
        key_name: parts.key_name,
        model_id: parts.model_id,
        global_model_id: parts.global_model_id,
        global_model_name: parts.global_model_name,
        provider_api_format: parts.provider_api_format,
        client_api_format: parts.client_api_format,
        mapped_model: parts.mapped_model,
        candidate_group_id: parts.candidate_group_id,
        ranking: parts.ranking,
        upstream_url: parts.upstream_url,
        header_rules: parts.header_rules,
        body_rules: parts.body_rules,
        provider_request_method: parts.provider_request_method,
        provider_request_headers: parts.provider_request_headers,
        original_headers: &original_headers,
        original_request_body,
        request_origin: AiRequestOrigin {
            client_ip,
            user_agent,
        },
        client_requested_stream: parts.client_requested_stream,
        upstream_is_stream: parts.upstream_is_stream,
        has_envelope: parts.has_envelope,
        needs_conversion: parts.needs_conversion,
        extra_fields: parts.extra_fields,
    })
}

pub(crate) fn provider_stream_event_api_format_for_provider_type(
    provider_type: &str,
) -> Option<&'static str> {
    ai_provider_stream_event_api_format_for_provider_type(provider_type)
}

pub(crate) fn insert_provider_stream_event_api_format(
    extra_fields: &mut Map<String, Value>,
    provider_type: &str,
) {
    insert_ai_provider_stream_event_api_format(extra_fields, provider_type);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{json, Map, Value};

    use super::{
        build_local_execution_report_context, provider_stream_event_api_format_for_provider_type,
        LocalExecutionReportContextParts,
    };
    use crate::ai_serving::ExecutionRuntimeAuthContext;
    use crate::ai_serving::RequestOrigin;
    use crate::orchestration::ExecutionAttemptIdentity;

    #[test]
    fn codex_provider_uses_openai_responses_stream_event_format() {
        assert_eq!(
            provider_stream_event_api_format_for_provider_type("codex"),
            Some("openai:responses")
        );
        assert_eq!(
            provider_stream_event_api_format_for_provider_type("CODEX"),
            Some("openai:responses")
        );
    }

    #[test]
    fn ordinary_providers_do_not_override_stream_event_format() {
        assert_eq!(
            provider_stream_event_api_format_for_provider_type("openai"),
            None
        );
        assert_eq!(
            provider_stream_event_api_format_for_provider_type("anthropic"),
            None
        );
    }

    #[test]
    fn local_execution_report_context_records_request_origin() {
        let auth_context = ExecutionRuntimeAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "api-key-1".to_string(),
            username: None,
            api_key_name: None,
            balance_remaining: None,
            access_allowed: true,
            api_key_is_standalone: false,
        };
        let original_headers = http::HeaderMap::new();
        let provider_request_headers = BTreeMap::new();

        let report_context =
            build_local_execution_report_context(LocalExecutionReportContextParts {
                auth_context: &auth_context,
                request_id: "trace-1",
                candidate_id: "candidate-1",
                attempt_identity: ExecutionAttemptIdentity::new(0, 0),
                model: "gpt-5",
                provider_name: "OpenAI",
                provider_id: "provider-1",
                endpoint_id: "endpoint-1",
                key_id: "key-1",
                key_name: None,
                model_id: None,
                global_model_id: None,
                global_model_name: None,
                provider_api_format: "openai:chat",
                client_api_format: "openai:chat",
                mapped_model: None,
                candidate_group_id: None,
                ranking: None,
                upstream_url: None,
                header_rules: None,
                body_rules: None,
                provider_request_method: None,
                provider_request_headers: Some(&provider_request_headers),
                original_headers: &original_headers,
                request_origin: Some(RequestOrigin {
                    client_ip: Some("203.0.113.8".to_string()),
                    user_agent: Some("Claude-Code/1.0".to_string()),
                }),
                original_request_body_json: Some(&json!({"model": "gpt-5"})),
                original_request_body_base64: None,
                client_requested_stream: false,
                upstream_is_stream: false,
                has_envelope: false,
                needs_conversion: false,
                extra_fields: Map::new(),
            });

        assert_eq!(
            report_context["client_ip"],
            Value::String("203.0.113.8".to_string())
        );
        assert_eq!(
            report_context["user_agent"],
            Value::String("Claude-Code/1.0".to_string())
        );
    }
}
