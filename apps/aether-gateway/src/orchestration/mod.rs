use aether_contracts::ExecutionPlan;
use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::AppState;

mod adaptive;
mod attempt;
mod classifier;
mod effects;
mod health;
mod policy;
mod recovery;
mod report_effects;

pub(crate) use self::adaptive::{
    project_local_adaptive_rate_limit, project_local_adaptive_success,
    LocalAdaptiveRateLimitProjection, LocalAdaptiveSuccessProjection,
};
pub(crate) use self::attempt::{
    attempt_identity_from_report_context, build_local_attempt_identities,
    insert_pool_key_lease_report_context_fields, local_attempt_slot_count,
    local_execution_candidate_metadata_from_report_context, ExecutionAttemptIdentity,
    LocalExecutionCandidateMetadata, SCHEDULER_AFFINITY_EPOCH_REPORT_FIELD,
};
pub(crate) use self::classifier::{
    classify_local_failover, local_failover_error_message, LocalFailoverClassification,
    LocalFailoverInput,
};
pub(crate) use self::effects::{
    apply_local_execution_effect, LocalAdaptiveRateLimitEffect, LocalAdaptiveSuccessEffect,
    LocalAttemptFailureEffect, LocalExecutionEffect, LocalExecutionEffectContext,
    LocalHealthFailureEffect, LocalHealthSuccessEffect, LocalOAuthInvalidationEffect,
    LocalPoolErrorEffect,
};
pub(crate) use self::health::{
    project_local_failure_health, project_local_key_circuit_closed,
    project_local_key_circuit_failure, project_local_success_health,
};
pub(crate) use self::policy::{
    append_local_failover_policy_to_value, local_failover_policy_from_report_context,
    local_failover_policy_from_transport, resolve_local_failover_policy, LocalFailoverPolicy,
    LocalFailoverRegexRule,
};
pub(crate) use self::recovery::{
    analyze_local_failover, recover_local_failover_decision, LocalFailoverAnalysis,
    LocalFailoverDecision,
};
#[cfg(test)]
pub(crate) use self::report_effects::clear_local_report_effect_caches_for_tests;
pub(crate) use self::report_effects::{
    apply_local_report_effect, store_local_gemini_file_mapping, LocalReportEffect,
};

pub(crate) async fn resolve_local_failover_analysis_for_attempt(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    status_code: u16,
    response_text: Option<&str>,
) -> LocalFailoverAnalysis {
    if attempt_identity_from_report_context(report_context).is_none() {
        return LocalFailoverAnalysis::use_default();
    }

    let policy = resolve_local_failover_policy(state, plan, report_context).await;
    analyze_local_failover(&policy, LocalFailoverInput::new(status_code, response_text))
}

pub(crate) async fn resolve_local_failover_decision_for_attempt(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    status_code: u16,
    response_text: Option<&str>,
) -> LocalFailoverDecision {
    resolve_local_failover_analysis_for_attempt(
        state,
        plan,
        report_context,
        status_code,
        response_text,
    )
    .await
    .decision
}

pub(crate) fn build_local_error_flow_metadata(
    status_code: u16,
    response_text: Option<&str>,
    analysis: LocalFailoverAnalysis,
) -> Value {
    let safe_to_expose = matches!(
        analysis.classification,
        LocalFailoverClassification::StopStatusCode
            | LocalFailoverClassification::StopErrorPattern
            | LocalFailoverClassification::StopExecutionError
    );
    let propagation = match analysis.decision {
        LocalFailoverDecision::RetryNextCandidate => "suppressed",
        LocalFailoverDecision::StopLocalFailover if safe_to_expose => "converted",
        LocalFailoverDecision::StopLocalFailover => "suppressed",
        LocalFailoverDecision::UseDefault if status_code >= 400 => "passthrough",
        LocalFailoverDecision::UseDefault => "none",
    };
    json!({
        "stage": "candidate",
        "source": "upstream_response",
        "status_code": status_code,
        "classification": analysis.classification.as_str(),
        "decision": analysis.decision.as_str(),
        "retryable": matches!(analysis.decision, LocalFailoverDecision::RetryNextCandidate),
        "safe_to_expose": safe_to_expose,
        "propagation": propagation,
        "message": local_failover_error_message(response_text),
    })
}

pub(crate) fn with_error_flow_report_context(
    report_context: Option<&Value>,
    error_flow: Value,
) -> Option<Value> {
    let mut object = report_context?.as_object()?.clone();
    object.insert("error_flow".to_string(), error_flow);
    Some(Value::Object(object))
}

pub(crate) fn with_upstream_response_report_context(
    report_context: Option<&Value>,
    status_code: u16,
    headers: Option<&std::collections::BTreeMap<String, String>>,
    body: Option<&Value>,
    body_ref: Option<&str>,
    body_state: Option<&str>,
) -> Option<Value> {
    let mut object = report_context?.as_object()?.clone();
    let mut upstream_response = serde_json::Map::new();
    upstream_response.insert("status_code".to_string(), json!(status_code));
    if let Some(headers) = headers {
        upstream_response.insert("headers".to_string(), trace_headers_to_json(headers));
    }
    if let Some(body) = body {
        upstream_response.insert("body".to_string(), body.clone());
    }
    if let Some(body_ref) = body_ref {
        upstream_response.insert("body_ref".to_string(), json!(body_ref));
    }
    if let Some(body_state) = body_state {
        upstream_response.insert("body_state".to_string(), json!(body_state));
    }
    object.insert(
        "upstream_response".to_string(),
        Value::Object(upstream_response),
    );
    Some(Value::Object(object))
}

pub(crate) fn trace_upstream_response_body(
    body_json: Option<&Value>,
    body_bytes: &[u8],
) -> Option<Value> {
    if let Some(body_json) = body_json {
        return Some(limit_trace_upstream_response_body_json(body_json));
    }

    if body_bytes.is_empty() {
        return None;
    }

    if let Ok(text) = std::str::from_utf8(body_bytes) {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        if let Ok(json_body) = serde_json::from_str::<Value>(text) {
            return Some(limit_trace_upstream_response_body_json(&json_body));
        }
        return Some(Value::String(limit_trace_upstream_response_text(text)));
    }

    Some(json!({
        "encoding": "base64",
        "data": base64::engine::general_purpose::STANDARD.encode(
            &body_bytes[..body_bytes.len().min(crate::MAX_ERROR_BODY_BYTES)]
        ),
        "truncated": body_bytes.len() > crate::MAX_ERROR_BODY_BYTES,
    }))
}

fn limit_trace_upstream_response_body_json(body_json: &Value) -> Value {
    let Ok(serialized) = serde_json::to_vec(body_json) else {
        return body_json.clone();
    };
    if serialized.len() <= crate::MAX_ERROR_BODY_BYTES {
        return body_json.clone();
    }
    Value::String(limit_trace_upstream_response_text(
        String::from_utf8_lossy(&serialized).as_ref(),
    ))
}

fn limit_trace_upstream_response_text(text: &str) -> String {
    let mut bytes = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        let len = ch.len_utf8();
        if bytes + len > crate::MAX_ERROR_BODY_BYTES {
            out.push_str("...[truncated]");
            return out;
        }
        bytes += len;
        out.push(ch);
    }
    out
}

fn trace_headers_to_json(headers: &std::collections::BTreeMap<String, String>) -> Value {
    Value::Object(Map::from_iter(headers.iter().map(|(key, value)| {
        (
            key.clone(),
            Value::String(mask_trace_header_value(key, value)),
        )
    })))
}

fn mask_trace_header_value(name: &str, value: &str) -> String {
    if !trace_header_is_sensitive(name) {
        return value.to_string();
    }
    if value.len() <= 8 {
        return "****".to_string();
    }
    format!("{}****{}", &value[..4], &value[value.len() - 4..])
}

fn trace_header_is_sensitive(name: &str) -> bool {
    [
        "authorization",
        "x-api-key",
        "api-key",
        "x-goog-api-key",
        "cookie",
        "set-cookie",
        "proxy-authorization",
    ]
    .iter()
    .any(|candidate| name.trim().eq_ignore_ascii_case(candidate))
}
