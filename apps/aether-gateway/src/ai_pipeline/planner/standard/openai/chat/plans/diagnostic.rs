use std::collections::BTreeMap;

use super::super::{GatewayControlDecision, LocalExecutionRuntimeMissDiagnostic};
use crate::AppState;

pub(crate) fn build_local_openai_chat_miss_diagnostic(
    decision: &GatewayControlDecision,
    plan_kind: &str,
    requested_model: Option<&str>,
    reason: &str,
) -> LocalExecutionRuntimeMissDiagnostic {
    LocalExecutionRuntimeMissDiagnostic {
        reason: reason.to_string(),
        route_family: decision.route_family.clone(),
        route_kind: decision.route_kind.clone(),
        public_path: Some(decision.public_path.clone()),
        plan_kind: Some(plan_kind.to_string()),
        requested_model: requested_model.map(ToOwned::to_owned),
        candidate_count: None,
        skipped_candidate_count: None,
        skip_reasons: BTreeMap::new(),
    }
}

pub(crate) fn set_local_openai_chat_miss_diagnostic(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    requested_model: Option<&str>,
    reason: &str,
) {
    state.set_local_execution_runtime_miss_diagnostic(
        trace_id,
        build_local_openai_chat_miss_diagnostic(decision, plan_kind, requested_model, reason),
    );
}
