use std::collections::BTreeMap;

use aether_ai_formats::api::{
    ExecutionRuntimeAuthContext, EXECUTION_RUNTIME_STREAM_DECISION_ACTION,
    EXECUTION_RUNTIME_SYNC_DECISION_ACTION,
};
use aether_contracts::{ExecutionTimeouts, ProxySnapshot};

use crate::{AiExecutionDecision, ConversionMode, ExecutionStrategy};

pub struct AiExecutionDecisionResponseParts {
    pub decision_is_stream: bool,
    pub decision_kind: String,
    pub execution_strategy: ExecutionStrategy,
    pub conversion_mode: ConversionMode,
    pub request_id: String,
    pub candidate_id: String,
    pub provider_name: String,
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
    pub proxy: Option<ProxySnapshot>,
    pub tls_profile: Option<String>,
    pub timeouts: Option<ExecutionTimeouts>,
    pub upstream_is_stream: bool,
    pub report_kind: Option<String>,
    pub report_context: Option<serde_json::Value>,
    pub auth_context: ExecutionRuntimeAuthContext,
}

pub fn build_ai_execution_decision_response(
    parts: AiExecutionDecisionResponseParts,
) -> AiExecutionDecision {
    AiExecutionDecision {
        action: ai_execution_decision_action(parts.decision_is_stream).to_string(),
        decision_kind: Some(parts.decision_kind),
        execution_strategy: Some(parts.execution_strategy.as_str().to_string()),
        conversion_mode: Some(parts.conversion_mode.as_str().to_string()),
        request_id: Some(parts.request_id),
        candidate_id: Some(parts.candidate_id),
        provider_name: Some(parts.provider_name),
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
        proxy: parts.proxy,
        tls_profile: parts.tls_profile,
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
