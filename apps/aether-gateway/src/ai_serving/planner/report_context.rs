use std::collections::BTreeMap;

use aether_ai_serving::{
    build_ai_execution_report_context,
    insert_provider_stream_event_api_format as insert_ai_provider_stream_event_api_format,
    provider_stream_event_api_format_for_provider_type as ai_provider_stream_event_api_format_for_provider_type,
    AiExecutionReportContextParts, AiRequestOrigin,
};
use aether_runtime_state::RuntimeLockLease;
use aether_scheduler_core::{ClientSessionAffinity, SchedulerRankingOutcome};
use serde_json::{Map, Value};

use crate::ai_serving::{
    request_origin_from_headers, request_path_implies_stream_request, sanitize_request_path,
    sanitize_request_path_and_query, sanitize_request_query_string, ExecutionRuntimeAuthContext,
    RequestOrigin,
};
use crate::client_session_affinity::{
    client_session_affinity_report_context_value, CLIENT_SESSION_AFFINITY_REPORT_CONTEXT_FIELD,
};
use crate::orchestration::{
    insert_pool_key_lease_report_context_fields, ExecutionAttemptIdentity,
    SCHEDULER_AFFINITY_EPOCH_REPORT_FIELD,
};

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
    pub(crate) pool_key_lease: Option<&'a RuntimeLockLease>,
    pub(crate) ranking: Option<&'a SchedulerRankingOutcome>,
    pub(crate) upstream_url: Option<&'a str>,
    pub(crate) header_rules: Option<&'a Value>,
    pub(crate) body_rules: Option<&'a Value>,
    pub(crate) provider_request_method: Option<Value>,
    pub(crate) provider_request_headers: Option<&'a BTreeMap<String, String>>,
    pub(crate) original_headers: &'a http::HeaderMap,
    pub(crate) request_path: Option<&'a str>,
    pub(crate) request_query_string: Option<&'a str>,
    pub(crate) request_origin: Option<RequestOrigin>,
    pub(crate) original_request_body_json: Option<&'a Value>,
    pub(crate) original_request_body_base64: Option<&'a str>,
    pub(crate) client_session_affinity: Option<&'a ClientSessionAffinity>,
    pub(crate) scheduler_affinity_epoch: Option<u64>,
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
    let mut extra_fields = parts.extra_fields;
    if let Some(value) = parts
        .client_session_affinity
        .and_then(client_session_affinity_report_context_value)
    {
        if let Some(client_family) = value
            .as_object()
            .and_then(|object| object.get("client_family"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|client_family| !client_family.is_empty())
        {
            extra_fields.insert(
                "client_family".to_string(),
                Value::String(client_family.to_ascii_lowercase()),
            );
        }
        extra_fields.insert(
            CLIENT_SESSION_AFFINITY_REPORT_CONTEXT_FIELD.to_string(),
            value,
        );
    }
    if let Some(incoming_tls) =
        crate::ai_serving::tls_fingerprint_from_headers(parts.original_headers)
    {
        merge_incoming_tls_fingerprint(&mut extra_fields, incoming_tls);
    }
    insert_pool_key_lease_report_context_fields(&mut extra_fields, parts.pool_key_lease);
    if let Some(epoch) = parts.scheduler_affinity_epoch {
        extra_fields.insert(
            SCHEDULER_AFFINITY_EPOCH_REPORT_FIELD.to_string(),
            Value::Number(epoch.into()),
        );
    }
    insert_request_path_fields(
        &mut extra_fields,
        parts.request_path,
        parts.request_query_string,
    );
    let client_requested_stream = parts.client_requested_stream
        || parts
            .request_path
            .is_some_and(request_path_implies_stream_request);

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
        client_requested_stream,
        upstream_is_stream: parts.upstream_is_stream,
        has_envelope: parts.has_envelope,
        needs_conversion: parts.needs_conversion,
        extra_fields,
    })
}

fn insert_request_path_fields(
    extra_fields: &mut Map<String, Value>,
    request_path: Option<&str>,
    request_query_string: Option<&str>,
) {
    let Some(path) = request_path.and_then(sanitize_request_path) else {
        return;
    };
    let query = request_query_string.and_then(sanitize_request_query_string);
    let path_and_query = sanitize_request_path_and_query(path.as_str(), query.as_deref())
        .unwrap_or_else(|| path.clone());
    extra_fields
        .entry("request_path".to_string())
        .or_insert_with(|| Value::String(path.clone()));
    if let Some(query) = query.clone() {
        extra_fields
            .entry("request_query_string".to_string())
            .or_insert_with(|| Value::String(query.to_string()));
    }
    extra_fields
        .entry("request_path_and_query".to_string())
        .or_insert_with(|| Value::String(path_and_query));
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

pub(crate) fn insert_native_client_envelope_name(
    extra_fields: &mut Map<String, Value>,
    envelope_name: &str,
    request_path: &str,
) {
    if envelope_name.eq_ignore_ascii_case("antigravity:v1internal")
        && request_path == "/v1internal:streamGenerateContent"
    {
        extra_fields.insert(
            "client_envelope_name".to_string(),
            Value::String(envelope_name.to_string()),
        );
    }
}

fn merge_incoming_tls_fingerprint(extra_fields: &mut Map<String, Value>, incoming_tls: Value) {
    let entry = extra_fields
        .entry("tls_fingerprint".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(object) = entry {
        object.insert("incoming".to_string(), incoming_tls);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aether_scheduler_core::ClientSessionAffinity;
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
    fn local_execution_report_context_records_request_origin_and_session_affinity() {
        let auth_context = ExecutionRuntimeAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "api-key-1".to_string(),
            username: None,
            api_key_name: None,
            api_key_billing_multiplier: 1.0,
            balance_remaining: None,
            access_allowed: true,
            api_key_is_standalone: false,
        };
        let original_headers = http::HeaderMap::new();
        let provider_request_headers = BTreeMap::new();
        let client_session_affinity = ClientSessionAffinity::new(
            Some("codex".to_string()),
            Some("account=account-1;session=session-1".to_string()),
        );

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
                pool_key_lease: None,
                ranking: None,
                upstream_url: None,
                header_rules: None,
                body_rules: None,
                provider_request_method: None,
                provider_request_headers: Some(&provider_request_headers),
                original_headers: &original_headers,
                request_path: Some("/v1/chat/completions"),
                request_query_string: Some("debug=true&limit=10"),
                request_origin: Some(RequestOrigin {
                    client_ip: Some("203.0.113.8".to_string()),
                    user_agent: Some("Claude-Code/1.0".to_string()),
                }),
                original_request_body_json: Some(&json!({"model": "gpt-5"})),
                original_request_body_base64: None,
                client_session_affinity: Some(&client_session_affinity),
                scheduler_affinity_epoch: None,
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
        assert_eq!(
            report_context["client_session_affinity"],
            json!({
                "client_family": "codex",
                "session_key": "account=account-1;session=session-1"
            })
        );
        assert_eq!(report_context["request_path"], "/v1/chat/completions");
        assert_eq!(report_context["request_query_string"], "limit=10");
        assert_eq!(
            report_context["request_path_and_query"],
            "/v1/chat/completions?limit=10"
        );
    }

    #[test]
    fn local_execution_report_context_treats_stream_generate_content_path_as_client_stream() {
        let auth_context = ExecutionRuntimeAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "api-key-1".to_string(),
            username: None,
            api_key_name: None,
            api_key_billing_multiplier: 1.0,
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
                model: "gemini-3.1-flash-image-preview",
                provider_name: "Gemini",
                provider_id: "provider-1",
                endpoint_id: "endpoint-1",
                key_id: "key-1",
                key_name: None,
                model_id: None,
                global_model_id: None,
                global_model_name: None,
                provider_api_format: "gemini:generate_content",
                client_api_format: "gemini:generate_content",
                mapped_model: None,
                candidate_group_id: None,
                pool_key_lease: None,
                ranking: None,
                upstream_url: None,
                header_rules: None,
                body_rules: None,
                provider_request_method: None,
                provider_request_headers: Some(&provider_request_headers),
                original_headers: &original_headers,
                request_path: Some(
                    "/v1beta/models/gemini-3.1-flash-image-preview:streamGenerateContent",
                ),
                request_query_string: Some("key=secret&alt=sse"),
                request_origin: None,
                original_request_body_json: Some(&json!({
                    "contents": [{"role": "user", "parts": [{"text": "hi"}]}]
                })),
                original_request_body_base64: None,
                client_session_affinity: None,
                scheduler_affinity_epoch: None,
                client_requested_stream: false,
                upstream_is_stream: true,
                has_envelope: false,
                needs_conversion: false,
                extra_fields: Map::new(),
            });

        assert_eq!(report_context["client_requested_stream"], true);
        assert_eq!(report_context["request_query_string"], "alt=sse");
        assert_eq!(
            report_context["request_path_and_query"],
            "/v1beta/models/gemini-3.1-flash-image-preview:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn local_execution_report_context_records_forwarded_tls_fingerprint() {
        let auth_context = ExecutionRuntimeAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "api-key-1".to_string(),
            username: None,
            api_key_name: None,
            api_key_billing_multiplier: 1.0,
            balance_remaining: None,
            access_allowed: true,
            api_key_is_standalone: false,
        };
        let mut original_headers = http::HeaderMap::new();
        original_headers.insert("x-aether-tls-ja3", "ja3-value".parse().unwrap());
        original_headers.insert("x-aether-tls-ja4", "ja4-value".parse().unwrap());
        original_headers.insert("x-aether-tls-protocol", "TLSv1.3".parse().unwrap());
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
                pool_key_lease: None,
                ranking: None,
                upstream_url: None,
                header_rules: None,
                body_rules: None,
                provider_request_method: None,
                provider_request_headers: Some(&provider_request_headers),
                original_headers: &original_headers,
                request_path: None,
                request_query_string: None,
                request_origin: None,
                original_request_body_json: Some(&json!({"model": "gpt-5"})),
                original_request_body_base64: None,
                client_session_affinity: None,
                scheduler_affinity_epoch: None,
                client_requested_stream: false,
                upstream_is_stream: false,
                has_envelope: false,
                needs_conversion: false,
                extra_fields: Map::new(),
            });

        assert_eq!(
            report_context["tls_fingerprint"]["incoming"],
            json!({
                "source": "forwarded_header",
                "ja3": "ja3-value",
                "ja4": "ja4-value",
                "protocol": "TLSv1.3"
            })
        );
    }
}
