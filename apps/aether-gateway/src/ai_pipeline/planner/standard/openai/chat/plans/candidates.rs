use std::collections::BTreeSet;

use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

use super::super::{GatewayError, LocalOpenAiChatDecisionInput};
use crate::ai_pipeline::{GatewayAuthApiKeySnapshot, PlannerAppState};
use crate::clock::current_unix_secs;
use crate::AppState;

pub(crate) async fn list_local_openai_chat_candidates(
    state: &AppState,
    input: &LocalOpenAiChatDecisionInput,
    require_streaming: bool,
) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
    let planner_state = PlannerAppState::new(state);
    let now_unix_secs = current_unix_secs();
    let mut combined = Vec::new();
    let mut seen = BTreeSet::new();

    let api_formats = if require_streaming {
        vec!["openai:chat", "claude:chat", "gemini:chat", "openai:cli"]
    } else {
        vec![
            "openai:chat",
            "claude:chat",
            "gemini:chat",
            "openai:cli",
            "openai:compact",
        ]
    };

    for api_format in api_formats {
        let auth_snapshot = if api_format == "openai:chat" {
            Some(&input.auth_snapshot)
        } else {
            None
        };
        let mut candidates = planner_state
            .list_selectable_candidates(
                api_format,
                &input.requested_model,
                require_streaming,
                auth_snapshot,
                now_unix_secs,
            )
            .await?;
        if api_format != "openai:chat" {
            candidates.retain(|candidate| {
                auth_snapshot_allows_cross_format_openai_chat_candidate(
                    &input.auth_snapshot,
                    &input.requested_model,
                    candidate,
                )
            });
        }
        for candidate in candidates {
            let candidate_key = format!(
                "{}:{}:{}:{}:{}",
                candidate.provider_id,
                candidate.endpoint_id,
                candidate.key_id,
                candidate.model_id,
                candidate.selected_provider_model_name,
            );
            if seen.insert(candidate_key) {
                combined.push(candidate);
            }
        }
    }

    Ok(combined)
}

fn auth_snapshot_allows_cross_format_openai_chat_candidate(
    auth_snapshot: &GatewayAuthApiKeySnapshot,
    requested_model: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> bool {
    if let Some(allowed_providers) = auth_snapshot.effective_allowed_providers() {
        let provider_allowed = allowed_providers.iter().any(|value| {
            value
                .trim()
                .eq_ignore_ascii_case(candidate.provider_id.trim())
                || value
                    .trim()
                    .eq_ignore_ascii_case(candidate.provider_name.trim())
        });
        if !provider_allowed {
            return false;
        }
    }

    if let Some(allowed_models) = auth_snapshot.effective_allowed_models() {
        let model_allowed = allowed_models
            .iter()
            .any(|value| value == requested_model || value == &candidate.global_model_name);
        if !model_allowed {
            return false;
        }
    }

    true
}
