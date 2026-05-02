use aether_contracts::ExecutionPlan;
use serde_json::{json, Value};

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
    attempt_identity_from_report_context, build_local_attempt_identities, local_attempt_slot_count,
    local_execution_candidate_metadata_from_report_context, ExecutionAttemptIdentity,
    LocalExecutionCandidateMetadata,
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
pub(crate) use self::health::{project_local_failure_health, project_local_success_health};
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
        LocalFailoverClassification::StopStatusCode | LocalFailoverClassification::StopErrorPattern
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
