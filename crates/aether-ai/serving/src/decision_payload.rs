use std::collections::BTreeMap;

use aether_ai_formats::api::{
    ExecutionRuntimeAuthContext, EXECUTION_RUNTIME_STREAM_DECISION_ACTION,
    EXECUTION_RUNTIME_SYNC_DECISION_ACTION,
};
use aether_contracts::{
    ExecutionTimeouts, ProxySnapshot, ResolvedTransportProfile, TRANSPORT_BACKEND_HYPER_RUSTLS,
    TRANSPORT_BACKEND_REQWEST_RUSTLS, TRANSPORT_HTTP_MODE_HTTP1_ONLY,
};
use serde_json::{json, Map, Value};

use crate::{AiExecutionDecision, AiRequestGzipPolicy, ConversionMode, ExecutionStrategy};

pub struct AiExecutionDecisionResponseParts {
    pub decision_is_stream: bool,
    pub decision_kind: String,
    pub execution_strategy: ExecutionStrategy,
    pub conversion_mode: ConversionMode,
    pub request_id: String,
    pub candidate_id: String,
    pub provider_name: String,
    pub provider_type: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub key_id: String,
    pub upstream_base_url: String,
    pub upstream_url: String,
    pub provider_request_method: Option<String>,
    pub auth_header: Option<String>,
    pub auth_value: Option<String>,
    pub provider_api_format: String,
    pub client_api_format: String,
    pub model_name: String,
    pub mapped_model: String,
    pub prompt_cache_key: Option<String>,
    pub provider_request_headers: BTreeMap<String, String>,
    pub provider_request_body: Option<serde_json::Value>,
    pub provider_request_body_base64: Option<String>,
    pub content_type: Option<String>,
    pub content_encoding: Option<String>,
    pub request_gzip: Option<AiRequestGzipPolicy>,
    pub proxy: Option<ProxySnapshot>,
    pub transport_profile: Option<ResolvedTransportProfile>,
    pub timeouts: Option<ExecutionTimeouts>,
    pub upstream_is_stream: bool,
    pub report_kind: Option<String>,
    pub report_context: Option<serde_json::Value>,
    pub auth_context: ExecutionRuntimeAuthContext,
}

pub fn build_ai_execution_decision_response(
    mut parts: AiExecutionDecisionResponseParts,
) -> AiExecutionDecision {
    parts.report_context = attach_outgoing_tls_fingerprint(
        parts.report_context,
        parts.transport_profile.as_ref(),
        parts.proxy.as_ref(),
    );

    AiExecutionDecision {
        action: ai_execution_decision_action(parts.decision_is_stream).to_string(),
        decision_kind: Some(parts.decision_kind),
        execution_strategy: Some(parts.execution_strategy.as_str().to_string()),
        conversion_mode: Some(parts.conversion_mode.as_str().to_string()),
        request_id: Some(parts.request_id),
        candidate_id: Some(parts.candidate_id),
        provider_name: Some(parts.provider_name),
        provider_type: Some(parts.provider_type),
        provider_id: Some(parts.provider_id),
        endpoint_id: Some(parts.endpoint_id),
        key_id: Some(parts.key_id),
        upstream_base_url: Some(parts.upstream_base_url),
        upstream_url: Some(parts.upstream_url),
        provider_request_method: parts.provider_request_method,
        auth_header: parts.auth_header,
        auth_value: parts.auth_value,
        provider_api_format: Some(parts.provider_api_format.clone()),
        client_api_format: Some(parts.client_api_format.clone()),
        provider_contract: Some(parts.provider_api_format),
        client_contract: Some(parts.client_api_format),
        model_name: Some(parts.model_name),
        mapped_model: Some(parts.mapped_model),
        prompt_cache_key: parts.prompt_cache_key,
        extra_headers: BTreeMap::new(),
        provider_request_headers: parts.provider_request_headers,
        provider_request_body: parts.provider_request_body,
        provider_request_body_base64: parts.provider_request_body_base64,
        content_type: parts.content_type,
        content_encoding: parts.content_encoding,
        request_gzip: parts.request_gzip,
        proxy: parts.proxy,
        transport_profile: parts.transport_profile,
        timeouts: parts.timeouts,
        upstream_is_stream: parts.upstream_is_stream,
        report_kind: parts.report_kind,
        report_context: parts.report_context,
        auth_context: Some(parts.auth_context),
    }
}

pub const fn ai_execution_decision_action(decision_is_stream: bool) -> &'static str {
    if decision_is_stream {
        EXECUTION_RUNTIME_STREAM_DECISION_ACTION
    } else {
        EXECUTION_RUNTIME_SYNC_DECISION_ACTION
    }
}

fn attach_outgoing_tls_fingerprint(
    report_context: Option<Value>,
    transport_profile: Option<&ResolvedTransportProfile>,
    proxy: Option<&ProxySnapshot>,
) -> Option<Value> {
    let mut report_context = match report_context {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    let tls_fingerprint = report_context
        .entry("tls_fingerprint".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(object) = tls_fingerprint {
        object.insert(
            "outgoing".to_string(),
            outgoing_tls_fingerprint_value(transport_profile, proxy),
        );
    }
    Some(Value::Object(report_context))
}

fn outgoing_tls_fingerprint_value(
    transport_profile: Option<&ResolvedTransportProfile>,
    proxy: Option<&ProxySnapshot>,
) -> Value {
    let via_local_proxy = proxy
        .and_then(|value| value.node_id.as_deref())
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let backend = transport_profile
        .map(|profile| profile.backend.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(if via_local_proxy {
            TRANSPORT_BACKEND_HYPER_RUSTLS
        } else {
            TRANSPORT_BACKEND_REQWEST_RUSTLS
        });
    let http_mode = transport_profile
        .map(|profile| profile.http_mode.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("auto");
    let alpn_offered = if http_mode.eq_ignore_ascii_case(TRANSPORT_HTTP_MODE_HTTP1_ONLY) {
        json!(["http/1.1"])
    } else {
        json!(["h2", "http/1.1"])
    };
    let transport_path = if via_local_proxy {
        "aether_proxy_tunnel"
    } else if proxy.is_some() {
        "direct_with_proxy"
    } else {
        "direct"
    };

    let mut object = Map::new();
    object.insert(
        "source".to_string(),
        Value::String("aether_transport_config".to_string()),
    );
    object.insert("observed".to_string(), Value::Bool(false));
    object.insert(
        "transport_path".to_string(),
        Value::String(transport_path.to_string()),
    );
    object.insert("backend".to_string(), Value::String(backend.to_string()));
    object.insert(
        "http_mode".to_string(),
        Value::String(http_mode.to_string()),
    );
    object.insert("tls_stack".to_string(), Value::String("rustls".to_string()));
    object.insert(
        "tls_versions_offered".to_string(),
        json!(["TLS1.3", "TLS1.2"]),
    );
    object.insert("alpn_offered".to_string(), alpn_offered);

    if let Some(profile) = transport_profile {
        object.insert(
            "profile_id".to_string(),
            Value::String(profile.profile_id.clone()),
        );
        object.insert(
            "pool_scope".to_string(),
            Value::String(profile.pool_scope.clone()),
        );
    }

    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_contracts::{TRANSPORT_HTTP_MODE_AUTO, TRANSPORT_POOL_SCOPE_KEY};

    fn sample_parts() -> AiExecutionDecisionResponseParts {
        AiExecutionDecisionResponseParts {
            decision_is_stream: false,
            decision_kind: "local_sync".to_string(),
            execution_strategy: ExecutionStrategy::LocalSameFormat,
            conversion_mode: ConversionMode::None,
            request_id: "trace-1".to_string(),
            candidate_id: "candidate-1".to_string(),
            provider_name: "OpenAI".to_string(),
            provider_type: "openai".to_string(),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            upstream_base_url: "https://api.example.com".to_string(),
            upstream_url: "https://api.example.com/v1/chat/completions".to_string(),
            provider_request_method: None,
            auth_header: None,
            auth_value: None,
            provider_api_format: "openai:chat".to_string(),
            client_api_format: "openai:chat".to_string(),
            model_name: "gpt-5".to_string(),
            mapped_model: "gpt-5".to_string(),
            prompt_cache_key: None,
            provider_request_headers: BTreeMap::new(),
            provider_request_body: Some(json!({"model": "gpt-5"})),
            provider_request_body_base64: None,
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            request_gzip: None,
            proxy: None,
            transport_profile: None,
            timeouts: None,
            upstream_is_stream: false,
            report_kind: None,
            report_context: Some(json!({
                "tls_fingerprint": {
                    "incoming": {
                        "source": "forwarded_header",
                        "ja3": "incoming-ja3"
                    }
                }
            })),
            auth_context: ExecutionRuntimeAuthContext {
                user_id: "user-1".to_string(),
                api_key_id: "api-key-1".to_string(),
                username: None,
                api_key_name: None,
                balance_remaining: None,
                access_allowed: true,
                api_key_is_standalone: false,
            },
        }
    }

    #[test]
    fn decision_response_records_outgoing_tls_fingerprint_without_dropping_incoming() {
        let mut parts = sample_parts();
        parts.transport_profile = Some(ResolvedTransportProfile {
            profile_id: "claude_code_nodejs".to_string(),
            backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.to_string(),
            http_mode: TRANSPORT_HTTP_MODE_AUTO.to_string(),
            pool_scope: TRANSPORT_POOL_SCOPE_KEY.to_string(),
            header_fingerprint: None,
            extra: None,
        });

        let decision = build_ai_execution_decision_response(parts);
        let tls_fingerprint = &decision.report_context.unwrap()["tls_fingerprint"];

        assert_eq!(tls_fingerprint["incoming"]["ja3"], "incoming-ja3");
        assert_eq!(
            tls_fingerprint["outgoing"]["profile_id"],
            "claude_code_nodejs"
        );
        assert_eq!(
            tls_fingerprint["outgoing"]["backend"],
            TRANSPORT_BACKEND_REQWEST_RUSTLS
        );
        assert_eq!(tls_fingerprint["outgoing"]["observed"], false);
        assert_eq!(
            tls_fingerprint["outgoing"]["alpn_offered"],
            json!(["h2", "http/1.1"])
        );
    }

    #[test]
    fn decision_response_defaults_outgoing_tls_to_proxy_hyper_backend_for_node_proxy() {
        let mut parts = sample_parts();
        parts.proxy = Some(ProxySnapshot {
            node_id: Some("proxy-node-1".to_string()),
            ..ProxySnapshot::default()
        });

        let decision = build_ai_execution_decision_response(parts);
        let outgoing = &decision.report_context.unwrap()["tls_fingerprint"]["outgoing"];

        assert_eq!(outgoing["transport_path"], "aether_proxy_tunnel");
        assert_eq!(outgoing["backend"], TRANSPORT_BACKEND_HYPER_RUSTLS);
    }
}
