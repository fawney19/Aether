use tracing::warn;

use crate::ai_pipeline::PlannerAppState;
use crate::{AppState, GatewayControlSyncDecisionResponse};

#[path = "decision/cross_format.rs"]
mod cross_format;
#[path = "decision/same_format.rs"]
mod same_format;
#[path = "decision/support.rs"]
mod support;

use self::cross_format::build_cross_format_local_openai_chat_decision_payload_for_candidate;
use self::same_format::build_same_format_local_openai_chat_decision_payload_for_candidate;
use self::support::mark_skipped_local_openai_chat_candidate;
pub(super) use self::support::{
    materialize_local_openai_chat_candidate_attempts, LocalOpenAiChatCandidateAttempt,
    LocalOpenAiChatDecisionInput,
};

pub(super) async fn maybe_build_local_openai_chat_decision_payload_for_candidate(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalOpenAiChatDecisionInput,
    attempt: LocalOpenAiChatCandidateAttempt,
    decision_kind: &str,
    report_kind: &str,
    upstream_is_stream: bool,
) -> Option<GatewayControlSyncDecisionResponse> {
    let planner_state = PlannerAppState::new(state);
    let LocalOpenAiChatCandidateAttempt {
        candidate,
        candidate_index,
        candidate_id,
    } = attempt;
    let transport = match planner_state
        .read_provider_transport_snapshot(
            &candidate.provider_id,
            &candidate.endpoint_id,
            &candidate.key_id,
        )
        .await
    {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            mark_skipped_local_openai_chat_candidate(
                state,
                input,
                trace_id,
                &candidate,
                candidate_index,
                &candidate_id,
                "transport_snapshot_missing",
            )
            .await;
            return None;
        }
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                error = ?err,
                "gateway local openai chat decision provider transport read failed"
            );
            mark_skipped_local_openai_chat_candidate(
                state,
                input,
                trace_id,
                &candidate,
                candidate_index,
                &candidate_id,
                "transport_snapshot_read_failed",
            )
            .await;
            return None;
        }
    };

    let provider_api_format = transport.endpoint.api_format.trim().to_ascii_lowercase();
    match provider_api_format.as_str() {
        "openai:chat" => {
            build_same_format_local_openai_chat_decision_payload_for_candidate(
                state,
                parts,
                trace_id,
                body_json,
                input,
                &candidate,
                candidate_index,
                &candidate_id,
                decision_kind,
                report_kind,
                upstream_is_stream,
                &transport,
            )
            .await
        }
        "claude:chat" | "gemini:chat" | "openai:cli" | "openai:compact" => {
            build_cross_format_local_openai_chat_decision_payload_for_candidate(
                state,
                parts,
                trace_id,
                body_json,
                input,
                &candidate,
                candidate_index,
                &candidate_id,
                decision_kind,
                upstream_is_stream,
                &transport,
                provider_api_format.as_str(),
            )
            .await
        }
        _ => {
            mark_skipped_local_openai_chat_candidate(
                state,
                input,
                trace_id,
                &candidate,
                candidate_index,
                &candidate_id,
                "transport_unsupported",
            )
            .await;
            None
        }
    }
}
