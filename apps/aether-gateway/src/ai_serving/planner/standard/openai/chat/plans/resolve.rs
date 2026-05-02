use tracing::warn;

use super::super::{GatewayControlDecision, LocalOpenAiChatDecisionInput};
use super::diagnostic::set_local_openai_chat_miss_diagnostic;
use crate::ai_serving::planner::common::extract_standard_requested_model;
use crate::ai_serving::planner::decision_input::{
    build_local_requested_model_decision_input, resolve_local_authenticated_decision_input,
};
use crate::ai_serving::resolve_local_decision_execution_runtime_auth_context;
use crate::AppState;

pub(crate) async fn resolve_local_openai_chat_decision_input(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
    record_miss_diagnostic: bool,
) -> Option<LocalOpenAiChatDecisionInput> {
    let Some(auth_context) = resolve_local_decision_execution_runtime_auth_context(decision) else {
        warn!(
            trace_id = %trace_id,
            route_class = ?decision.route_class,
            route_family = ?decision.route_family,
            route_kind = ?decision.route_kind,
            "gateway local openai chat decision skipped: missing_auth_context"
        );
        if record_miss_diagnostic {
            set_local_openai_chat_miss_diagnostic(
                state,
                trace_id,
                decision,
                plan_kind,
                extract_standard_requested_model(body_json).as_deref(),
                "missing_auth_context",
            );
        }
        return None;
    };

    let Some(requested_model) = extract_standard_requested_model(body_json) else {
        warn!(
            trace_id = %trace_id,
            "gateway local openai chat decision skipped: missing_requested_model"
        );
        if record_miss_diagnostic {
            set_local_openai_chat_miss_diagnostic(
                state,
                trace_id,
                decision,
                plan_kind,
                None,
                "missing_requested_model",
            );
        }
        return None;
    };

    let resolved_input = match resolve_local_authenticated_decision_input(
        state,
        auth_context.clone(),
        Some(requested_model.as_str()),
        None,
    )
    .await
    {
        Ok(Some(resolved_input)) => resolved_input,
        Ok(None) => {
            warn!(
                trace_id = %trace_id,
                user_id = %auth_context.user_id,
                api_key_id = %auth_context.api_key_id,
                "gateway local openai chat decision skipped: auth_snapshot_missing"
            );
            if record_miss_diagnostic {
                set_local_openai_chat_miss_diagnostic(
                    state,
                    trace_id,
                    decision,
                    plan_kind,
                    Some(requested_model.as_str()),
                    "auth_snapshot_missing",
                );
            }
            return None;
        }
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                error = ?err,
                "gateway local openai chat decision auth snapshot read failed"
            );
            if record_miss_diagnostic {
                set_local_openai_chat_miss_diagnostic(
                    state,
                    trace_id,
                    decision,
                    plan_kind,
                    Some(requested_model.as_str()),
                    "auth_snapshot_read_failed",
                );
            }
            return None;
        }
    };

    let mut input = build_local_requested_model_decision_input(resolved_input, requested_model);
    input.request_auth_channel = decision.request_auth_channel.clone();
    Some(input)
}
