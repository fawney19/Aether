use super::super::GatewayControlDecision;
use crate::ai_serving::planner::runtime_miss::{
    set_local_runtime_candidate_evaluation_diagnostic, set_local_runtime_miss_diagnostic_reason,
};
use crate::AppState;

pub(crate) fn set_local_openai_chat_miss_diagnostic(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    requested_model: Option<&str>,
    reason: &str,
) {
    set_local_runtime_miss_diagnostic_reason(
        state,
        trace_id,
        decision,
        plan_kind,
        requested_model,
        reason,
    );
}

pub(crate) fn set_local_openai_chat_candidate_evaluation_diagnostic(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    requested_model: Option<&str>,
    candidate_count: usize,
) {
    set_local_runtime_candidate_evaluation_diagnostic(
        state,
        trace_id,
        decision,
        plan_kind,
        requested_model,
        candidate_count,
    );
}
