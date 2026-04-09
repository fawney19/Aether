use tracing::warn;

use super::super::{GatewayControlDecision, LocalOpenAiChatDecisionInput};
use super::diagnostic::set_local_openai_chat_miss_diagnostic;
use crate::ai_pipeline::{resolve_local_decision_execution_runtime_auth_context, PlannerAppState};
use crate::clock::current_unix_secs;
use crate::AppState;

pub(crate) async fn resolve_local_openai_chat_decision_input(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
    record_miss_diagnostic: bool,
) -> Option<LocalOpenAiChatDecisionInput> {
    let planner_state = PlannerAppState::new(state);
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
                body_json.get("model").and_then(|value| value.as_str()),
                "missing_auth_context",
            );
        }
        return None;
    };

    let Some(requested_model) = body_json
        .get("model")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
    else {
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

    let auth_snapshot = match planner_state
        .read_auth_api_key_snapshot(
            &auth_context.user_id,
            &auth_context.api_key_id,
            current_unix_secs(),
        )
        .await
    {
        Ok(Some(snapshot)) => snapshot,
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

    Some(LocalOpenAiChatDecisionInput {
        auth_context,
        requested_model,
        auth_snapshot,
    })
}
