use async_trait::async_trait;
use std::collections::VecDeque;
use tracing::warn;

use super::super::{
    build_lazy_local_openai_chat_candidate_attempt_source,
    maybe_build_local_openai_chat_decision_payload_for_candidate, AppState, GatewayControlDecision,
    GatewayError, LocalOpenAiChatCandidateAttempt, LocalOpenAiChatCandidateAttemptSource,
    LocalOpenAiChatDecisionInput, LocalOpenAiChatRequestPreparation,
};
use super::diagnostic::{
    set_local_openai_chat_candidate_evaluation_diagnostic, set_local_openai_chat_miss_diagnostic,
};
use super::openai_chat_upstream_is_stream_for_candidate;
use super::resolve::resolve_local_openai_chat_decision_input;
use crate::ai_serving::planner::candidate_materialization::LocalExecutionAttemptSource;
use crate::ai_serving::planner::common::OPENAI_CHAT_STREAM_PLAN_KIND;
use crate::ai_serving::planner::plan_builders::{
    build_openai_chat_stream_plan_from_decision, AiStreamAttempt,
};
use crate::ai_serving::planner::runtime_miss::apply_local_runtime_candidate_terminal_reason;
use crate::ai_serving::planner::standard::build_local_openai_chat_upstream_url;
use crate::ai_serving::transport::{
    is_windsurf_provider_transport, local_openai_chat_transport_unsupported_reason,
};
use crate::clock::request_distribution_seed;
use crate::stage_metrics::{
    observe_gateway_stage_ms, record_openai_chat_stream_payload_build_prefetch_avoided,
    record_openai_chat_stream_payload_build_selected,
    record_openai_chat_stream_raw_candidates_scanned,
    record_openai_chat_stream_target_select_selected_rank,
};
use crate::upstream_admission::upstream_target_key_from_url;

const OPENAI_CHAT_STREAM_TARGET_SELECT_WINDOW_ENV: &str =
    "AETHER_GATEWAY_OPENAI_CHAT_STREAM_TARGET_SELECT_WINDOW";
const DEFAULT_OPENAI_CHAT_STREAM_TARGET_SELECT_WINDOW: usize = 2;
const MAX_OPENAI_CHAT_STREAM_TARGET_SELECT_WINDOW: usize = 8;

pub(crate) struct LocalOpenAiChatStreamAttemptSource<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    body_json: serde_json::Value,
    input: LocalOpenAiChatDecisionInput,
    candidates: LocalOpenAiChatCandidateAttemptSource<'a>,
    prefetched_attempts: VecDeque<LocalOpenAiChatCandidateAttempt>,
    request_preparation: LocalOpenAiChatRequestPreparation,
}

pub(crate) async fn build_local_openai_chat_stream_attempt_source<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    body_json: &'a serde_json::Value,
    plan_kind: &str,
) -> Result<Option<(LocalOpenAiChatStreamAttemptSource<'a>, usize)>, GatewayError> {
    if plan_kind != OPENAI_CHAT_STREAM_PLAN_KIND {
        return Ok(None);
    }

    let attempt_source_started_at = std::time::Instant::now();
    let Some(input) = resolve_local_openai_chat_decision_input(
        state, parts, trace_id, decision, body_json, plan_kind, true,
    )
    .await?
    else {
        return Ok(None);
    };
    let effective_body_json = input.effective_body_json(body_json).clone();

    let (candidates, candidate_count) = build_lazy_local_openai_chat_candidate_attempt_source(
        state,
        trace_id,
        &input,
        &effective_body_json,
        true,
    )
    .await;
    if candidate_count == 0 {
        set_local_openai_chat_candidate_evaluation_diagnostic(
            state,
            trace_id,
            decision,
            plan_kind,
            Some(input.requested_model.as_str()),
            0,
        );
        return Ok(None);
    }
    set_local_openai_chat_candidate_evaluation_diagnostic(
        state,
        trace_id,
        decision,
        plan_kind,
        Some(input.requested_model.as_str()),
        candidate_count,
    );
    observe_gateway_stage_ms(
        "openai_chat_attempt_source_build",
        attempt_source_started_at.elapsed().as_millis() as u64,
    );

    Ok(Some((
        LocalOpenAiChatStreamAttemptSource {
            state,
            parts,
            trace_id,
            body_json: effective_body_json,
            input,
            candidates,
            prefetched_attempts: VecDeque::new(),
            request_preparation: LocalOpenAiChatRequestPreparation::default(),
        },
        candidate_count,
    )))
}

#[async_trait]
impl LocalExecutionAttemptSource<AiStreamAttempt> for LocalOpenAiChatStreamAttemptSource<'_> {
    async fn next_execution_attempt(&mut self) -> Result<Option<AiStreamAttempt>, GatewayError> {
        let select_started_at = std::time::Instant::now();
        let selected = self.next_execution_attempt_with_target_select().await?;
        observe_gateway_stage_ms(
            "openai_chat_stream_target_select",
            select_started_at.elapsed().as_millis() as u64,
        );
        Ok(selected)
    }

    async fn drain_execution_attempts(&mut self) -> Result<Vec<AiStreamAttempt>, GatewayError> {
        let mut drained = Vec::new();
        for attempt in self.candidates.drain_static_attempts() {
            if let Some(attempt) = self.build_stream_attempt(attempt).await? {
                drained.push(attempt);
            }
        }
        Ok(drained)
    }
}

impl LocalOpenAiChatStreamAttemptSource<'_> {
    async fn next_execution_attempt_with_target_select(
        &mut self,
    ) -> Result<Option<AiStreamAttempt>, GatewayError> {
        loop {
            let Some(attempt) = self.next_raw_attempt_with_target_select().await? else {
                apply_local_runtime_candidate_terminal_reason(
                    self.state,
                    self.trace_id,
                    "no_local_stream_plans",
                );
                return Ok(None);
            };

            let plan_started_at = std::time::Instant::now();
            record_openai_chat_stream_payload_build_selected();
            match self.build_stream_attempt(attempt).await? {
                Some(attempt) => {
                    observe_gateway_stage_ms(
                        "stream_candidate_plan_build",
                        plan_started_at.elapsed().as_millis() as u64,
                    );
                    return Ok(Some(attempt));
                }
                None => {
                    observe_gateway_stage_ms(
                        "stream_candidate_plan_build",
                        plan_started_at.elapsed().as_millis() as u64,
                    );
                    continue;
                }
            }
        }
    }

    async fn next_raw_attempt_with_target_select(
        &mut self,
    ) -> Result<Option<LocalOpenAiChatCandidateAttempt>, GatewayError> {
        let select_window = openai_chat_stream_target_select_window();
        if select_window <= 1 {
            return self.next_raw_attempt_linear().await;
        }
        let mut attempts = Vec::with_capacity(select_window);
        for _ in 0..select_window {
            match self.next_raw_attempt_linear().await? {
                Some(attempt) => attempts.push(attempt),
                None => break,
            }
        }
        if attempts.is_empty() {
            return Ok(None);
        }
        record_openai_chat_stream_raw_candidates_scanned(attempts.len());
        let seed = request_distribution_seed();
        let target_keys = attempts
            .iter()
            .map(|attempt| self.lightweight_target_key_for_attempt(attempt))
            .collect::<Vec<_>>();
        for target_key in target_keys.iter().flatten() {
            self.state
                .upstream_target_admission
                .record_raw_seen_for_target_key(target_key);
        }
        let selected_index = if target_keys.iter().all(Option::is_some) {
            let choices = attempts
                .iter()
                .zip(target_keys.iter())
                .map(|(attempt, target_key)| {
                    let target_key = target_key.as_deref().unwrap_or("-");
                    let snapshot = self
                        .state
                        .upstream_target_admission
                        .snapshot_for_target_key(target_key);
                    TargetSelectChoice {
                        target_key,
                        identity: target_select_candidate_identity(attempt),
                        in_flight: snapshot
                            .as_ref()
                            .map(|snapshot| snapshot.in_flight)
                            .unwrap_or(0),
                        selection_pressure_total: snapshot
                            .as_ref()
                            .map(|snapshot| snapshot.selection_pressure_total)
                            .unwrap_or(0),
                    }
                })
                .collect::<Vec<_>>();
            select_target_index(seed, &choices)
        } else {
            0
        };
        record_openai_chat_stream_target_select_selected_rank(selected_index);
        record_openai_chat_stream_payload_build_prefetch_avoided(attempts.len().saturating_sub(1));
        if let Some(Some(target_key)) = target_keys.get(selected_index) {
            self.state
                .upstream_target_admission
                .record_preselect_for_target_key(target_key);
        }
        let selected = attempts.remove(selected_index);
        self.prefetched_attempts.extend(attempts);
        Ok(Some(selected))
    }

    async fn next_raw_attempt_linear(
        &mut self,
    ) -> Result<Option<LocalOpenAiChatCandidateAttempt>, GatewayError> {
        if let Some(attempt) = self.prefetched_attempts.pop_front() {
            return Ok(Some(attempt));
        }
        let source_started_at = std::time::Instant::now();
        let attempt = self.candidates.next_attempt().await?;
        observe_gateway_stage_ms(
            "stream_candidate_source_next",
            source_started_at.elapsed().as_millis() as u64,
        );
        Ok(attempt)
    }

    fn lightweight_target_key_for_attempt(
        &self,
        attempt: &LocalOpenAiChatCandidateAttempt,
    ) -> Option<String> {
        let provider_api_format = attempt.eligible.provider_api_format.trim();
        if !provider_api_format.eq_ignore_ascii_case("openai:chat") {
            return None;
        }
        let transport = &attempt.eligible.transport;
        if transport
            .provider
            .provider_type
            .trim()
            .eq_ignore_ascii_case("grok")
            || is_windsurf_provider_transport(transport)
            || local_openai_chat_transport_unsupported_reason(transport).is_some()
        {
            return None;
        }
        if transport.provider.proxy.is_some()
            || transport.endpoint.proxy.is_some()
            || transport.key.proxy.is_some()
        {
            return None;
        }
        let upstream_url = build_local_openai_chat_upstream_url(self.parts, transport)?;
        upstream_target_key_from_url(upstream_url.as_str(), None)
    }

    async fn build_stream_attempt(
        &mut self,
        attempt: LocalOpenAiChatCandidateAttempt,
    ) -> Result<Option<AiStreamAttempt>, GatewayError> {
        let upstream_is_stream = openai_chat_upstream_is_stream_for_candidate(
            &attempt.eligible.transport,
            attempt.eligible.provider_api_format.as_str(),
            true,
        );
        let Some(payload) = maybe_build_local_openai_chat_decision_payload_for_candidate(
            self.state,
            self.parts,
            self.trace_id,
            &self.body_json,
            &self.input,
            Some(&mut self.request_preparation),
            attempt,
            OPENAI_CHAT_STREAM_PLAN_KIND,
            "openai_chat_stream_success",
            upstream_is_stream,
        )
        .await?
        else {
            return Ok(None);
        };

        match build_openai_chat_stream_plan_from_decision(self.parts, &self.body_json, payload) {
            Ok(value) => Ok(value),
            Err(err) => {
                warn!(
                    trace_id = %self.trace_id,
                    error = ?err,
                    "gateway local openai chat stream decision plan build failed"
                );
                Ok(None)
            }
        }
    }
}

fn openai_chat_stream_target_select_window() -> usize {
    std::env::var(OPENAI_CHAT_STREAM_TARGET_SELECT_WINDOW_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_OPENAI_CHAT_STREAM_TARGET_SELECT_WINDOW)
        .clamp(1, MAX_OPENAI_CHAT_STREAM_TARGET_SELECT_WINDOW)
}

#[derive(Clone, Copy)]
struct TargetSelectCandidateIdentity<'a> {
    provider_id: &'a str,
    endpoint_id: &'a str,
    key_id: &'a str,
    candidate_id: &'a str,
}

#[derive(Clone, Copy)]
struct TargetSelectChoice<'a> {
    target_key: &'a str,
    identity: TargetSelectCandidateIdentity<'a>,
    in_flight: usize,
    selection_pressure_total: u64,
}

fn select_target_index(seed: u64, choices: &[TargetSelectChoice<'_>]) -> usize {
    choices
        .iter()
        .enumerate()
        .min_by_key(|(index, choice)| {
            target_select_score(
                seed,
                choice.target_key,
                &choice.identity,
                *index,
                choice.in_flight,
                choice.selection_pressure_total,
            )
        })
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn target_select_candidate_identity(
    attempt: &LocalOpenAiChatCandidateAttempt,
) -> TargetSelectCandidateIdentity<'_> {
    TargetSelectCandidateIdentity {
        provider_id: &attempt.eligible.candidate.provider_id,
        endpoint_id: &attempt.eligible.candidate.endpoint_id,
        key_id: &attempt.eligible.candidate.key_id,
        candidate_id: &attempt.candidate_id,
    }
}

fn target_select_tie_break(
    seed: u64,
    target_key: &str,
    identity: &TargetSelectCandidateIdentity<'_>,
    index: usize,
) -> u64 {
    let mut hash = seed ^ ((index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    hash = hash_string(hash, target_key);
    hash = hash_string(hash, identity.provider_id);
    hash = hash_string(hash, identity.endpoint_id);
    hash = hash_string(hash, identity.key_id);
    hash_string(hash, identity.candidate_id)
}

fn hash_string(mut hash: u64, value: &str) -> u64 {
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01B3);
    }
    hash
}

fn target_select_score(
    seed: u64,
    target_key: &str,
    identity: &TargetSelectCandidateIdentity<'_>,
    index: usize,
    in_flight: usize,
    selected_total: u64,
) -> (usize, u64, u64) {
    (
        in_flight,
        selected_total,
        target_select_tie_break(seed, target_key, identity, index),
    )
}

pub(crate) async fn build_local_openai_chat_stream_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Vec<AiStreamAttempt>, GatewayError> {
    if plan_kind != OPENAI_CHAT_STREAM_PLAN_KIND {
        return Ok(Vec::new());
    }

    let Some(input) = resolve_local_openai_chat_decision_input(
        state, parts, trace_id, decision, body_json, plan_kind, true,
    )
    .await?
    else {
        return Ok(Vec::new());
    };

    let Some((mut attempt_source, candidate_count)) =
        build_local_openai_chat_stream_attempt_source(
            state, parts, trace_id, decision, body_json, plan_kind,
        )
        .await?
    else {
        set_local_openai_chat_candidate_evaluation_diagnostic(
            state,
            trace_id,
            decision,
            plan_kind,
            Some(input.requested_model.as_str()),
            0,
        );
        return Ok(Vec::new());
    };

    let mut plans = Vec::new();
    while let Some(attempt) = attempt_source.next_execution_attempt().await? {
        plans.push(attempt);
        if plans.len() >= candidate_count {
            break;
        }
    }

    apply_local_runtime_candidate_terminal_reason(state, trace_id, "no_local_stream_plans");

    Ok(plans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity<'a>(
        endpoint_id: &'a str,
        candidate_id: &'a str,
    ) -> TargetSelectCandidateIdentity<'a> {
        TargetSelectCandidateIdentity {
            provider_id: "provider",
            endpoint_id,
            key_id: "key",
            candidate_id,
        }
    }

    #[test]
    fn target_select_score_prefers_lower_in_flight() {
        let busy = identity("endpoint-a", "candidate-a");
        let idle = identity("endpoint-b", "candidate-b");

        assert!(
            target_select_score(7, "http://127.0.0.1:18182|proxy=-", &idle, 1, 0, 10)
                < target_select_score(7, "http://127.0.0.1:18181|proxy=-", &busy, 0, 5, 0)
        );
    }

    #[test]
    fn target_select_tie_break_distinguishes_equivalent_targets() {
        let left = identity("endpoint-a", "candidate-a");
        let right = identity("endpoint-b", "candidate-b");

        assert_ne!(
            target_select_tie_break(11, "http://127.0.0.1:18181|proxy=-", &left, 0),
            target_select_tie_break(11, "http://127.0.0.1:18182|proxy=-", &right, 1)
        );
    }

    #[test]
    fn select_target_index_prefers_lower_in_flight_target() {
        let choices = [
            TargetSelectChoice {
                target_key: "http://127.0.0.1:18181|proxy=-",
                identity: identity("endpoint-a", "candidate-a"),
                in_flight: 8,
                selection_pressure_total: 0,
            },
            TargetSelectChoice {
                target_key: "http://127.0.0.1:18182|proxy=-",
                identity: identity("endpoint-b", "candidate-b"),
                in_flight: 1,
                selection_pressure_total: 100,
            },
        ];

        assert_eq!(select_target_index(17, &choices), 1);
    }

    #[test]
    fn select_target_index_uses_selection_pressure_before_tie_break() {
        let choices = [
            TargetSelectChoice {
                target_key: "http://127.0.0.1:18181|proxy=-",
                identity: identity("endpoint-a", "candidate-a"),
                in_flight: 0,
                selection_pressure_total: 20,
            },
            TargetSelectChoice {
                target_key: "http://127.0.0.1:18182|proxy=-",
                identity: identity("endpoint-b", "candidate-b"),
                in_flight: 0,
                selection_pressure_total: 1,
            },
        ];

        assert_eq!(select_target_index(19, &choices), 1);
    }
}
