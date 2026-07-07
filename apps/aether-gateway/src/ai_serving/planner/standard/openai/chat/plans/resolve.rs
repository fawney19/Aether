use tracing::warn;

use super::super::{GatewayControlDecision, LocalOpenAiChatDecisionInput};
use super::diagnostic::set_local_openai_chat_miss_diagnostic;
use crate::ai_serving::planner::common::extract_standard_requested_model;
use crate::ai_serving::planner::decision_input::{
    attach_routing_policy_to_local_requested_model_input,
    build_local_requested_model_decision_input, resolve_local_authenticated_decision_input,
};
use crate::ai_serving::resolve_local_decision_execution_runtime_auth_context;
use crate::client_session_affinity::client_session_affinity_from_parts;
use crate::stage_metrics::observe_gateway_stage_ms;
use crate::{AppState, GatewayError};

pub(crate) async fn resolve_local_openai_chat_decision_input(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
    record_miss_diagnostic: bool,
) -> Result<Option<LocalOpenAiChatDecisionInput>, GatewayError> {
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
        return Ok(None);
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
        return Ok(None);
    };

    let auth_started_at = std::time::Instant::now();
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
            return Ok(None);
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
            return Err(err);
        }
    };
    observe_gateway_stage_ms(
        "openai_chat_decision_input_auth",
        auth_started_at.elapsed().as_millis() as u64,
    );

    let mut input = build_local_requested_model_decision_input(resolved_input, requested_model);
    input.request_auth_channel = decision.request_auth_channel.clone();
    let affinity_started_at = std::time::Instant::now();
    input.client_session_affinity = client_session_affinity_from_parts(parts, Some(body_json));
    observe_gateway_stage_ms(
        "openai_chat_decision_input_affinity",
        affinity_started_at.elapsed().as_millis() as u64,
    );
    let routing_started_at = std::time::Instant::now();
    if let Err(err) = attach_routing_policy_to_local_requested_model_input(
        state,
        parts,
        &mut input,
        body_json,
        "openai:chat",
    )
    .await
    {
        warn!(
            trace_id = %trace_id,
            error = ?err,
            "gateway local openai chat decision routing profile resolution failed"
        );
        return Err(err);
    }
    observe_gateway_stage_ms(
        "openai_chat_decision_input_routing",
        routing_started_at.elapsed().as_millis() as u64,
    );
    Ok(Some(input))
}
