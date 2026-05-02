use std::collections::BTreeMap;

use aether_ai_formats::api::ExecutionRuntimeAuthContext;
use aether_contracts::{ExecutionPlan, ExecutionTimeouts, ProxySnapshot};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStrategy {
    GatewayAffinityForward,
    RawPublicProxy,
    LocalSameFormat,
    LocalCrossFormat,
}

impl ExecutionStrategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GatewayAffinityForward => "gateway_affinity_forward",
            Self::RawPublicProxy => "raw_public_proxy",
            Self::LocalSameFormat => "local_same_format",
            Self::LocalCrossFormat => "local_cross_format",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionMode {
    None,
    RequestOnly,
    ResponseOnly,
    Bidirectional,
}

impl ConversionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::RequestOnly => "request_only",
            Self::ResponseOnly => "response_only",
            Self::Bidirectional => "bidirectional",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AiExecutionPlanPayload {
    pub action: String,
    #[serde(default)]
    pub plan_kind: Option<String>,
    #[serde(default)]
    pub plan: Option<ExecutionPlan>,
    #[serde(default)]
    pub report_kind: Option<String>,
    #[serde(default)]
    pub report_context: Option<serde_json::Value>,
    #[serde(default)]
    pub auth_context: Option<ExecutionRuntimeAuthContext>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AiExecutionDecision {
    pub action: String,
    #[serde(default)]
    pub decision_kind: Option<String>,
    #[serde(default)]
    pub execution_strategy: Option<String>,
    #[serde(default)]
    pub conversion_mode: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub candidate_id: Option<String>,
    #[serde(default)]
    pub provider_name: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub upstream_base_url: Option<String>,
    #[serde(default)]
    pub upstream_url: Option<String>,
    #[serde(default)]
    pub provider_request_method: Option<String>,
    #[serde(default)]
    pub auth_header: Option<String>,
    #[serde(default)]
    pub auth_value: Option<String>,
    #[serde(default)]
    pub provider_api_format: Option<String>,
    #[serde(default)]
    pub client_api_format: Option<String>,
    #[serde(default)]
    pub provider_contract: Option<String>,
    #[serde(default)]
    pub client_contract: Option<String>,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub mapped_model: Option<String>,
    #[serde(default)]
    pub prompt_cache_key: Option<String>,
    #[serde(default)]
    pub extra_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub provider_request_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub provider_request_body: Option<serde_json::Value>,
    #[serde(default)]
    pub provider_request_body_base64: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub proxy: Option<ProxySnapshot>,
    #[serde(default)]
    pub tls_profile: Option<String>,
    #[serde(default)]
    pub timeouts: Option<ExecutionTimeouts>,
    #[serde(default)]
    pub upstream_is_stream: bool,
    #[serde(default)]
    pub report_kind: Option<String>,
    #[serde(default)]
    pub report_context: Option<serde_json::Value>,
    #[serde(default)]
    pub auth_context: Option<ExecutionRuntimeAuthContext>,
}

#[derive(Debug)]
pub struct AiSyncAttempt {
    pub plan: ExecutionPlan,
    pub report_kind: Option<String>,
    pub report_context: Option<serde_json::Value>,
}

#[derive(Debug)]
pub struct AiStreamAttempt {
    pub plan: ExecutionPlan,
    pub report_kind: Option<String>,
    pub report_context: Option<serde_json::Value>,
}

pub fn augment_sync_report_context(
    report_context: Option<serde_json::Value>,
    provider_request_headers: &BTreeMap<String, String>,
    _provider_request_body: &serde_json::Value,
) -> serde_json::Result<Option<serde_json::Value>> {
    let mut report_context = match report_context {
        Some(serde_json::Value::Object(map)) => map,
        Some(_) => serde_json::Map::new(),
        None => serde_json::Map::new(),
    };

    report_context.insert(
        "provider_request_headers".to_string(),
        serde_json::to_value(provider_request_headers)?,
    );

    Ok(Some(serde_json::Value::Object(report_context)))
}

fn decision_has_exact_provider_request(payload: &AiExecutionDecision) -> bool {
    !payload.provider_request_headers.is_empty()
        && (payload.provider_request_body.is_some()
            || payload
                .provider_request_body_base64
                .as_ref()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false))
}

pub fn generic_decision_missing_exact_provider_request(payload: &AiExecutionDecision) -> bool {
    !decision_has_exact_provider_request(payload)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        augment_sync_report_context, generic_decision_missing_exact_provider_request,
        AiExecutionDecision,
    };

    #[test]
    fn generic_decision_detects_missing_exact_provider_request() {
        let payload = AiExecutionDecision {
            action: "local".to_string(),
            decision_kind: Some("sync".to_string()),
            execution_strategy: None,
            conversion_mode: None,
            request_id: None,
            candidate_id: None,
            provider_name: None,
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            upstream_base_url: None,
            upstream_url: None,
            provider_request_method: None,
            auth_header: None,
            auth_value: None,
            provider_api_format: None,
            client_api_format: None,
            provider_contract: None,
            client_contract: None,
            model_name: None,
            mapped_model: None,
            prompt_cache_key: None,
            extra_headers: Default::default(),
            provider_request_headers: Default::default(),
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
        };

        assert!(generic_decision_missing_exact_provider_request(&payload));
    }

    #[test]
    fn augment_sync_report_context_attaches_provider_request_headers_only() {
        let report_context = augment_sync_report_context(
            Some(serde_json::json!({"trace_id": "abc"})),
            &BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
            &serde_json::json!({"model": "gpt-5"}),
        )
        .expect("context should serialize")
        .expect("context should exist");

        assert_eq!(
            report_context["provider_request_headers"]["content-type"],
            "application/json"
        );
        assert!(
            report_context.get("provider_request_body").is_none(),
            "provider request body should not be copied into report context"
        );
    }
}
