use aether_ai_serving::{
    ai_candidate_extra_data_with_ranking, ai_should_persist_available_candidate_for_pool_key,
    ai_should_persist_skipped_candidate_for_pool_membership,
    run_ai_available_candidate_persistence, run_ai_candidate_materialization,
    run_ai_skipped_candidate_persistence, AiAvailableCandidatePersistencePort,
    AiCandidateMaterializationOutcome, AiCandidateMaterializationPort, AiCandidateResolutionMode,
    AiSkippedCandidatePersistencePort,
};
use aether_scheduler_core::{ClientSessionAffinity, SchedulerMinimalCandidateSelectionCandidate};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{BTreeSet, VecDeque};
use std::convert::Infallible;
use std::sync::Arc;
use tracing::warn;
use uuid::Uuid;

use crate::ai_serving::planner::candidate_affinity_cache::remember_scheduler_affinity_for_candidate;
use crate::ai_serving::planner::candidate_resolution::{
    resolve_and_rank_local_execution_candidates,
    resolve_and_rank_local_execution_candidates_without_transport_pair_gate,
    resolve_and_rank_logical_local_execution_candidates, EligibleLocalExecutionCandidate,
    LocalExecutionCandidateKind, SkippedLocalExecutionCandidate,
};
use crate::ai_serving::planner::candidate_source::{
    LocalCandidatePreselectionKeyMode, LocalCandidatePreselectionPageCursor,
};
use crate::ai_serving::planner::materialization_policy::LocalCandidatePersistencePolicy;
use crate::ai_serving::planner::pool_scheduler::PoolKeyCursor;
use crate::ai_serving::planner::runtime_miss::record_local_runtime_candidate_skip_reason;
use crate::ai_serving::planner::CandidateFailureDiagnostic;
use crate::ai_serving::{GatewayAuthApiKeySnapshot, PlannerAppState};
use crate::clock::current_unix_ms;
use crate::handlers::shared::provider_pool::admin_provider_pool_config_from_config_value;
use crate::orchestration::{local_attempt_slot_count, ExecutionAttemptIdentity};
use crate::{AppState, GatewayError};

const POOL_KEY_RETRY_INDEX_STRIDE: u32 = 100;

#[derive(Debug, Clone)]
pub(crate) struct LocalExecutionCandidateAttempt {
    pub(crate) eligible: EligibleLocalExecutionCandidate,
    pub(crate) candidate_index: u32,
    pub(crate) retry_index: u32,
    pub(crate) candidate_id: String,
}

pub(crate) struct LocalExecutionCandidateAttemptSource<'a> {
    items: VecDeque<LocalExecutionCandidateAttemptSourceItem<'a>>,
}

type DecorateSkippedCandidateFn<'a> = Arc<
    dyn Fn(SkippedLocalExecutionCandidate) -> SkippedLocalExecutionCandidate + Send + Sync + 'a,
>;

#[async_trait]
pub(crate) trait LocalExecutionAttemptSource<T>: Send {
    async fn next_execution_attempt(&mut self) -> Result<Option<T>, GatewayError>;

    async fn drain_execution_attempts(&mut self) -> Result<Vec<T>, GatewayError>;

    async fn promote_hedge_preheated_pool_candidates(
        &mut self,
        provider_id: &str,
        pool_group_id: &str,
        attempted_key_ids: &BTreeSet<String>,
    ) -> Result<Option<(Option<String>, String, Vec<String>)>, GatewayError> {
        let _ = (provider_id, pool_group_id, attempted_key_ids);
        Ok(None)
    }
}

enum LocalExecutionCandidateAttemptSourceItem<'a> {
    Static {
        attempts: VecDeque<LocalExecutionCandidateAttempt>,
    },
    Pool {
        cursor: PoolKeyCursor<'a>,
        candidate_index: u32,
        pending_attempts: VecDeque<LocalExecutionCandidateAttempt>,
    },
    RequestedModelPage {
        cursor: Box<RequestedModelAttemptPageCursor<'a>>,
    },
}

impl<'a> LocalExecutionCandidateAttemptSource<'a> {
    pub(crate) fn from_static_attempts_for_image_bridge(
        attempts: Vec<LocalExecutionCandidateAttempt>,
    ) -> Self {
        let mut items = VecDeque::new();
        if !attempts.is_empty() {
            items.push_back(LocalExecutionCandidateAttemptSourceItem::Static {
                attempts: VecDeque::from(attempts),
            });
        }
        Self { items }
    }

    pub(crate) async fn next_attempt(&mut self) -> Option<LocalExecutionCandidateAttempt> {
        loop {
            let front = self.items.front_mut()?;
            match front {
                LocalExecutionCandidateAttemptSourceItem::Static { attempts } => {
                    if let Some(attempt) = attempts.pop_front() {
                        if attempts.is_empty() {
                            self.items.pop_front();
                        }
                        return Some(attempt);
                    }
                    self.items.pop_front();
                }
                LocalExecutionCandidateAttemptSourceItem::Pool {
                    cursor,
                    candidate_index,
                    pending_attempts,
                } => {
                    if let Some(attempt) = pending_attempts.pop_front() {
                        return Some(attempt);
                    }
                    let Some(candidate) = cursor.next_key().await else {
                        cursor.log_exhausted();
                        let _ = cursor.take_skipped_candidates();
                        self.items.pop_front();
                        continue;
                    };
                    *pending_attempts = build_unpersisted_local_execution_candidate_attempts(
                        candidate,
                        *candidate_index,
                    );
                }
                LocalExecutionCandidateAttemptSourceItem::RequestedModelPage { cursor } => {
                    let Some(attempt) = cursor.next_attempt().await else {
                        self.items.pop_front();
                        continue;
                    };
                    return Some(attempt);
                }
            }
        }
    }

    pub(crate) fn drain_static_attempts(&mut self) -> Vec<LocalExecutionCandidateAttempt> {
        self.items.clear();
        Vec::new()
    }

    pub(crate) async fn promote_hedge_preheated_pool_candidates(
        &mut self,
        provider_id: &str,
        pool_group_id: &str,
        attempted_key_ids: &BTreeSet<String>,
    ) -> Result<Option<(Option<String>, String, Vec<String>)>, GatewayError> {
        let previous_next_key_id = peek_next_key_id_from_items(&self.items);
        let promoted_key_ids = promote_hedge_preheated_pool_candidates_in_items(
            &mut self.items,
            provider_id,
            pool_group_id,
            attempted_key_ids,
        )
        .await?;
        if promoted_key_ids.is_empty() {
            return Ok(None);
        }

        let next_key_id = peek_next_key_id_from_items(&self.items);
        if previous_next_key_id == next_key_id {
            return Ok(None);
        }
        let Some(next_key_id) = next_key_id else {
            return Ok(None);
        };

        Ok(Some((previous_next_key_id, next_key_id, promoted_key_ids)))
    }
}

async fn promote_hedge_preheated_pool_candidates_in_items(
    items: &mut VecDeque<LocalExecutionCandidateAttemptSourceItem<'_>>,
    provider_id: &str,
    pool_group_id: &str,
    attempted_key_ids: &BTreeSet<String>,
) -> Result<Vec<String>, GatewayError> {
    for item in items {
        match item {
            LocalExecutionCandidateAttemptSourceItem::Static { .. } => {}
            LocalExecutionCandidateAttemptSourceItem::Pool {
                cursor,
                candidate_index,
                pending_attempts,
            } => {
                let promoted_candidates = cursor
                    .promote_preheated_candidates_after_fast_fail(
                        provider_id,
                        pool_group_id,
                        attempted_key_ids,
                    )
                    .await;
                if !promoted_candidates.is_empty() {
                    return Ok(prepend_promoted_pool_attempts(
                        pending_attempts,
                        *candidate_index,
                        promoted_candidates,
                    ));
                }
            }
            LocalExecutionCandidateAttemptSourceItem::RequestedModelPage { cursor } => {
                let promoted_key_ids = promote_hedge_preheated_pool_candidates_in_loaded_items(
                    &mut cursor.pending_items,
                    provider_id,
                    pool_group_id,
                    attempted_key_ids,
                )
                .await?;
                if !promoted_key_ids.is_empty() {
                    return Ok(promoted_key_ids);
                }
            }
        }
    }
    Ok(Vec::new())
}

async fn promote_hedge_preheated_pool_candidates_in_loaded_items(
    items: &mut VecDeque<LocalExecutionCandidateAttemptSourceItem<'_>>,
    provider_id: &str,
    pool_group_id: &str,
    attempted_key_ids: &BTreeSet<String>,
) -> Result<Vec<String>, GatewayError> {
    for item in items {
        if let LocalExecutionCandidateAttemptSourceItem::Pool {
            cursor,
            candidate_index,
            pending_attempts,
        } = item
        {
            let promoted_candidates = cursor
                .promote_preheated_candidates_after_fast_fail(
                    provider_id,
                    pool_group_id,
                    attempted_key_ids,
                )
                .await;
            if !promoted_candidates.is_empty() {
                return Ok(prepend_promoted_pool_attempts(
                    pending_attempts,
                    *candidate_index,
                    promoted_candidates,
                ));
            }
        }
    }
    Ok(Vec::new())
}

fn prepend_promoted_pool_attempts(
    pending_attempts: &mut VecDeque<LocalExecutionCandidateAttempt>,
    candidate_index: u32,
    promoted_candidates: Vec<EligibleLocalExecutionCandidate>,
) -> Vec<String> {
    let mut promoted_key_ids = Vec::with_capacity(promoted_candidates.len());
    let mut reordered_attempts = VecDeque::new();
    for candidate in promoted_candidates {
        promoted_key_ids.push(candidate.candidate.key_id.clone());
        reordered_attempts.extend(build_unpersisted_local_execution_candidate_attempts(
            candidate,
            candidate_index,
        ));
    }
    let mut previous_pending = std::mem::take(pending_attempts);
    reordered_attempts.append(&mut previous_pending);
    *pending_attempts = reordered_attempts;
    promoted_key_ids
}

fn peek_next_key_id_from_items(
    items: &VecDeque<LocalExecutionCandidateAttemptSourceItem<'_>>,
) -> Option<String> {
    for item in items {
        match item {
            LocalExecutionCandidateAttemptSourceItem::Static { attempts } => {
                if let Some(attempt) = attempts.front() {
                    return Some(attempt.eligible.candidate.key_id.clone());
                }
            }
            LocalExecutionCandidateAttemptSourceItem::Pool {
                cursor,
                pending_attempts,
                ..
            } => {
                if let Some(attempt) = pending_attempts.front() {
                    return Some(attempt.eligible.candidate.key_id.clone());
                }
                if let Some(key_id) = cursor.peek_queued_key_id() {
                    return Some(key_id.to_string());
                }
            }
            LocalExecutionCandidateAttemptSourceItem::RequestedModelPage { cursor } => {
                if let Some(key_id) = peek_next_key_id_from_items(&cursor.pending_items) {
                    return Some(key_id);
                }
            }
        }
    }
    None
}

impl LocalExecutionCandidateAttempt {
    pub(crate) fn attempt_identity(&self) -> ExecutionAttemptIdentity {
        ExecutionAttemptIdentity::new(self.candidate_index, self.retry_index)
            .with_pool_key_index(self.eligible.orchestration.pool_key_index)
    }
}

fn effective_retry_index(retry_index: u32, pool_key_index: Option<u32>) -> u32 {
    pool_key_index
        .and_then(|index| {
            index
                .checked_mul(POOL_KEY_RETRY_INDEX_STRIDE)
                .and_then(|base| base.checked_add(retry_index))
        })
        .unwrap_or(retry_index)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalAvailableCandidatePersistenceContext<'a> {
    pub(crate) user_id: &'a str,
    pub(crate) api_key_id: &'a str,
    pub(crate) required_capabilities: Option<&'a Value>,
    pub(crate) error_context: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalSkippedCandidatePersistenceContext<'a> {
    pub(crate) user_id: &'a str,
    pub(crate) api_key_id: &'a str,
    pub(crate) required_capabilities: Option<&'a Value>,
    pub(crate) error_context: &'static str,
    pub(crate) record_runtime_miss_diagnostic: bool,
}

pub(crate) use aether_ai_serving::AiCandidateResolutionMode as LocalCandidateResolutionMode;

struct GatewayLocalCandidateMaterializationPort<'a, F, G> {
    state: PlannerAppState<'a>,
    trace_id: &'a str,
    client_api_format: &'a str,
    requested_model: Option<&'a str>,
    auth_snapshot: Option<&'a GatewayAuthApiKeySnapshot>,
    client_session_affinity: Option<&'a ClientSessionAffinity>,
    required_capabilities: Option<&'a Value>,
    sticky_session_token: Option<&'a str>,
    request_auth_channel: Option<&'a str>,
    persistence_policy: LocalCandidatePersistencePolicy<'a>,
    resolution_mode: LocalCandidateResolutionMode,
    build_available_extra_data: F,
    decorate_skipped_candidate: G,
}

struct GatewayAvailableCandidatePersistencePort<'a, F> {
    state: PlannerAppState<'a>,
    trace_id: &'a str,
    user_id: &'a str,
    api_key_id: &'a str,
    required_capabilities: Option<&'a Value>,
    error_context: &'static str,
    created_at_unix_ms: u64,
    build_extra_data: F,
}

struct GatewaySkippedCandidatePersistencePort<'a> {
    state: &'a AppState,
    trace_id: &'a str,
    user_id: &'a str,
    api_key_id: &'a str,
    required_capabilities: Option<&'a Value>,
    error_context: &'static str,
    record_runtime_miss_diagnostic: bool,
}

#[async_trait]
impl<F, G> AiCandidateMaterializationPort for GatewayLocalCandidateMaterializationPort<'_, F, G>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
    G: Fn(SkippedLocalExecutionCandidate) -> SkippedLocalExecutionCandidate + Send + Sync,
{
    type Candidate = SchedulerMinimalCandidateSelectionCandidate;
    type Eligible = EligibleLocalExecutionCandidate;
    type Skipped = SkippedLocalExecutionCandidate;
    type Attempt = LocalExecutionCandidateAttempt;
    type Error = Infallible;

    async fn resolve_and_rank_candidates(
        &self,
        candidates: Vec<Self::Candidate>,
    ) -> Result<(Vec<Self::Eligible>, Vec<Self::Skipped>), Self::Error> {
        let requested_model = self.requested_model.map(str::to_string);
        let resolved = match self.resolution_mode {
            AiCandidateResolutionMode::Standard => {
                resolve_and_rank_local_execution_candidates(
                    self.state,
                    candidates,
                    self.client_api_format,
                    requested_model.as_deref().unwrap_or_default(),
                    self.auth_snapshot,
                    self.client_session_affinity,
                    self.required_capabilities,
                    self.sticky_session_token,
                    self.request_auth_channel,
                )
                .await
            }
            AiCandidateResolutionMode::WithoutTransportPairGate => {
                resolve_and_rank_local_execution_candidates_without_transport_pair_gate(
                    self.state,
                    candidates,
                    self.client_api_format,
                    requested_model.as_deref(),
                    self.auth_snapshot,
                    self.client_session_affinity,
                    self.required_capabilities,
                    self.sticky_session_token,
                    self.request_auth_channel,
                )
                .await
            }
        };
        Ok(resolved)
    }

    fn decorate_skipped_candidate(&self, skipped: Self::Skipped) -> Self::Skipped {
        (self.decorate_skipped_candidate)(skipped)
    }

    fn remember_first_candidate_affinity(&self, candidates: &[Self::Eligible]) {
        remember_first_local_candidate_affinity(
            self.state,
            self.auth_snapshot,
            self.client_session_affinity,
            self.client_api_format,
            self.requested_model,
            candidates,
        );
    }

    async fn persist_available_candidates(
        &self,
        candidates: Vec<Self::Eligible>,
    ) -> Result<Vec<Self::Attempt>, Self::Error> {
        Ok(persist_available_local_execution_candidates_with_context(
            self.state,
            self.trace_id,
            self.persistence_policy.available,
            candidates,
            &self.build_available_extra_data,
        )
        .await)
    }

    async fn persist_skipped_candidates(
        &self,
        starting_candidate_index: u32,
        skipped_candidates: Vec<Self::Skipped>,
    ) -> Result<(), Self::Error> {
        persist_skipped_local_execution_candidates_with_context(
            self.state.app(),
            self.trace_id,
            self.persistence_policy.skipped,
            starting_candidate_index,
            skipped_candidates,
        )
        .await;
        Ok(())
    }
}

#[async_trait]
impl<F> AiAvailableCandidatePersistencePort for GatewayAvailableCandidatePersistencePort<'_, F>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
{
    type Candidate = EligibleLocalExecutionCandidate;
    type Attempt = LocalExecutionCandidateAttempt;
    type ExtraData = Value;
    type Error = Infallible;

    fn attempt_slot_count(&self, candidate: &Self::Candidate) -> u32 {
        local_attempt_slot_count(&candidate.transport)
    }

    fn build_extra_data(&self, candidate: &Self::Candidate) -> Option<Self::ExtraData> {
        ai_candidate_extra_data_with_ranking(
            (self.build_extra_data)(candidate),
            candidate.ranking.as_ref(),
        )
    }

    fn generate_candidate_id(&self) -> String {
        Uuid::new_v4().to_string()
    }

    fn should_persist_available_candidate(&self, candidate: &Self::Candidate) -> bool {
        should_persist_available_local_candidate(candidate)
    }

    async fn persist_available_candidate(
        &self,
        candidate: &Self::Candidate,
        candidate_index: u32,
        retry_index: u32,
        generated_candidate_id: &str,
        extra_data: Option<Self::ExtraData>,
    ) -> Result<String, Self::Error> {
        Ok(self
            .state
            .persist_available_local_candidate(
                self.trace_id,
                self.user_id,
                self.api_key_id,
                &candidate.candidate,
                candidate_index,
                effective_retry_index(retry_index, candidate.orchestration.pool_key_index),
                generated_candidate_id,
                self.required_capabilities,
                extra_data,
                self.created_at_unix_ms,
                self.error_context,
            )
            .await)
    }

    fn build_attempt(
        &self,
        candidate: Self::Candidate,
        candidate_index: u32,
        retry_index: u32,
        candidate_id: String,
    ) -> Self::Attempt {
        let retry_index =
            effective_retry_index(retry_index, candidate.orchestration.pool_key_index);
        LocalExecutionCandidateAttempt {
            eligible: candidate,
            candidate_index,
            retry_index,
            candidate_id,
        }
    }
}

#[async_trait]
impl AiSkippedCandidatePersistencePort for GatewaySkippedCandidatePersistencePort<'_> {
    type Skipped = SkippedLocalExecutionCandidate;
    type ExtraData = Value;
    type Error = Infallible;

    fn should_persist_skipped_candidate(&self, candidate: &Self::Skipped) -> bool {
        should_persist_skipped_local_candidate(candidate)
    }

    fn build_extra_data(&self, candidate: &Self::Skipped) -> Option<Self::ExtraData> {
        ai_candidate_extra_data_with_ranking(
            candidate.extra_data.clone(),
            candidate.ranking.as_ref(),
        )
    }

    fn generate_candidate_id(&self) -> String {
        Uuid::new_v4().to_string()
    }

    async fn persist_skipped_candidate(
        &self,
        candidate: &Self::Skipped,
        candidate_index: u32,
        generated_candidate_id: &str,
        extra_data: Option<Self::ExtraData>,
    ) -> Result<(), Self::Error> {
        persist_skipped_local_execution_candidate(
            self.state,
            self.trace_id,
            self.user_id,
            self.api_key_id,
            &candidate.candidate,
            candidate_index,
            generated_candidate_id,
            self.required_capabilities,
            candidate.skip_reason,
            extra_data,
            self.error_context,
            self.record_runtime_miss_diagnostic,
        )
        .await;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_local_execution_candidates_with_serving<F, G>(
    state: PlannerAppState<'_>,
    trace_id: &str,
    client_api_format: &str,
    requested_model: Option<&str>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    client_session_affinity: Option<&ClientSessionAffinity>,
    required_capabilities: Option<&Value>,
    sticky_session_token: Option<&str>,
    request_auth_channel: Option<&str>,
    persistence_policy: LocalCandidatePersistencePolicy<'_>,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    preselection_skipped: Vec<SkippedLocalExecutionCandidate>,
    resolution_mode: LocalCandidateResolutionMode,
    build_available_extra_data: F,
    decorate_skipped_candidate: G,
) -> AiCandidateMaterializationOutcome<LocalExecutionCandidateAttempt>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
    G: Fn(SkippedLocalExecutionCandidate) -> SkippedLocalExecutionCandidate + Send + Sync,
{
    let port = GatewayLocalCandidateMaterializationPort {
        state,
        trace_id,
        client_api_format,
        requested_model,
        auth_snapshot,
        client_session_affinity,
        required_capabilities,
        sticky_session_token,
        request_auth_channel,
        persistence_policy,
        resolution_mode,
        build_available_extra_data,
        decorate_skipped_candidate,
    };

    match run_ai_candidate_materialization(&port, candidates, preselection_skipped).await {
        Ok(outcome) => outcome,
        Err(error) => match error {},
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_local_execution_candidate_attempt_source_with_serving<'a, F, G>(
    state: PlannerAppState<'a>,
    trace_id: &str,
    client_api_format: &str,
    requested_model: Option<&str>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    client_session_affinity: Option<&ClientSessionAffinity>,
    required_capabilities: Option<&Value>,
    sticky_session_token: Option<&str>,
    request_auth_channel: Option<&str>,
    persistence_policy: LocalCandidatePersistencePolicy<'_>,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    preselection_skipped: Vec<SkippedLocalExecutionCandidate>,
    resolution_mode: LocalCandidateResolutionMode,
    build_available_extra_data: F,
    decorate_skipped_candidate: G,
) -> (LocalExecutionCandidateAttemptSource<'a>, usize)
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
    G: Fn(SkippedLocalExecutionCandidate) -> SkippedLocalExecutionCandidate + Send + Sync,
{
    let (candidates, resolved_skipped) = resolve_and_rank_logical_local_execution_candidates(
        state,
        candidates,
        client_api_format,
        requested_model,
        auth_snapshot,
        client_session_affinity,
        required_capabilities,
        sticky_session_token,
        request_auth_channel,
        resolution_mode,
    )
    .await;
    let skipped_candidate_count = preselection_skipped.len() + resolved_skipped.len();
    let skipped_candidates = preselection_skipped
        .into_iter()
        .chain(resolved_skipped)
        .map(decorate_skipped_candidate)
        .collect::<Vec<_>>();
    let candidate_count = candidates.len() + skipped_candidate_count;
    if persistence_policy.skipped.record_runtime_miss_diagnostic {
        for skipped_candidate in &skipped_candidates {
            record_local_runtime_candidate_skip_reason(
                state.app(),
                trace_id,
                skipped_candidate.skip_reason,
            );
        }
    }

    remember_first_local_candidate_affinity(
        state,
        auth_snapshot,
        client_session_affinity,
        client_api_format,
        requested_model,
        &candidates,
    );

    let (items, _) = build_logical_candidate_items(
        state,
        candidates,
        0,
        sticky_session_token,
        requested_model,
        request_auth_channel,
    );

    (
        LocalExecutionCandidateAttemptSource { items },
        candidate_count,
    )
}

fn build_logical_candidate_items<'a>(
    state: PlannerAppState<'a>,
    candidates: Vec<EligibleLocalExecutionCandidate>,
    starting_candidate_index: u32,
    sticky_session_token: Option<&str>,
    requested_model: Option<&str>,
    request_auth_channel: Option<&str>,
) -> (VecDeque<LocalExecutionCandidateAttemptSourceItem<'a>>, u32) {
    let mut items = VecDeque::new();
    let mut next_candidate_index = starting_candidate_index;
    for candidate in candidates {
        let candidate_index = next_candidate_index;
        next_candidate_index = next_candidate_index.saturating_add(1);
        match candidate.kind {
            LocalExecutionCandidateKind::SingleKey => {
                let attempts = build_unpersisted_local_execution_candidate_attempts(
                    candidate,
                    candidate_index,
                );
                if !attempts.is_empty() {
                    items.push_back(LocalExecutionCandidateAttemptSourceItem::Static { attempts });
                }
            }
            LocalExecutionCandidateKind::PoolGroup => {
                items.push_back(LocalExecutionCandidateAttemptSourceItem::Pool {
                    cursor: PoolKeyCursor::new(
                        state,
                        candidate,
                        sticky_session_token,
                        requested_model,
                        request_auth_channel,
                    ),
                    candidate_index,
                    pending_attempts: VecDeque::new(),
                });
            }
        }
    }
    (items, next_candidate_index)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_lazy_requested_model_execution_candidate_attempt_source_with_serving<
    'a,
    F,
    G,
>(
    state: PlannerAppState<'a>,
    trace_id: &str,
    client_api_format: &str,
    requested_model: &str,
    require_streaming: bool,
    auth_snapshot: &GatewayAuthApiKeySnapshot,
    client_session_affinity: Option<&ClientSessionAffinity>,
    required_capabilities: Option<&Value>,
    sticky_session_token: Option<&str>,
    request_auth_channel: Option<&str>,
    persistence_policy: LocalCandidatePersistencePolicy<'_>,
    use_api_format_alias_match: bool,
    key_mode: LocalCandidatePreselectionKeyMode,
    resolution_mode: LocalCandidateResolutionMode,
    build_available_extra_data: F,
    decorate_skipped_candidate: G,
) -> (LocalExecutionCandidateAttemptSource<'a>, usize)
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync + 'a,
    G: Fn(SkippedLocalExecutionCandidate) -> SkippedLocalExecutionCandidate + Send + Sync + 'a,
{
    let _ = build_available_extra_data;
    let decorate_skipped_candidate = Arc::new(decorate_skipped_candidate);
    let record_runtime_miss_diagnostic = persistence_policy.skipped.record_runtime_miss_diagnostic;
    let page_cursor = LocalCandidatePreselectionPageCursor::new(
        state,
        client_api_format,
        requested_model,
        require_streaming,
        required_capabilities,
        auth_snapshot,
        client_session_affinity,
        use_api_format_alias_match,
        key_mode,
    )
    .await;
    let mut cursor = RequestedModelAttemptPageCursor {
        state,
        trace_id: trace_id.to_string(),
        client_api_format: client_api_format.to_string(),
        requested_model: requested_model.to_string(),
        auth_snapshot: auth_snapshot.clone(),
        client_session_affinity: client_session_affinity.cloned(),
        required_capabilities: required_capabilities.cloned(),
        sticky_session_token: sticky_session_token.map(str::to_string),
        request_auth_channel: request_auth_channel.map(str::to_string),
        record_runtime_miss_diagnostic,
        resolution_mode,
        decorate_skipped_candidate,
        page_cursor,
        pending_items: VecDeque::new(),
        candidate_count: 0,
        next_candidate_index: 0,
        remembered_affinity: false,
    };
    cursor.load_next_page().await;
    let candidate_count = cursor.candidate_count;
    let mut items = VecDeque::new();
    if !cursor.pending_items.is_empty() {
        items.push_back(
            LocalExecutionCandidateAttemptSourceItem::RequestedModelPage {
                cursor: Box::new(cursor),
            },
        );
    }
    (
        LocalExecutionCandidateAttemptSource { items },
        candidate_count,
    )
}

struct RequestedModelAttemptPageCursor<'a> {
    state: PlannerAppState<'a>,
    trace_id: String,
    client_api_format: String,
    requested_model: String,
    auth_snapshot: GatewayAuthApiKeySnapshot,
    client_session_affinity: Option<ClientSessionAffinity>,
    required_capabilities: Option<Value>,
    sticky_session_token: Option<String>,
    request_auth_channel: Option<String>,
    record_runtime_miss_diagnostic: bool,
    resolution_mode: LocalCandidateResolutionMode,
    decorate_skipped_candidate: DecorateSkippedCandidateFn<'a>,
    page_cursor: LocalCandidatePreselectionPageCursor<'a>,
    pending_items: VecDeque<LocalExecutionCandidateAttemptSourceItem<'a>>,
    candidate_count: usize,
    next_candidate_index: u32,
    remembered_affinity: bool,
}

impl<'a> RequestedModelAttemptPageCursor<'a> {
    async fn next_attempt(&mut self) -> Option<LocalExecutionCandidateAttempt> {
        loop {
            if let Some(attempt) = pop_attempt_from_items(&mut self.pending_items).await {
                return Some(attempt);
            }
            if !self.load_next_page().await {
                return None;
            }
        }
    }

    async fn load_next_page(&mut self) -> bool {
        loop {
            let page = match self.page_cursor.next_page().await {
                Ok(Some(page)) => page,
                Ok(None) => return false,
                Err(error) => {
                    warn!(
                        trace_id = %self.trace_id,
                        error = ?error,
                        "gateway lazy requested-model candidate page read failed"
                    );
                    return false;
                }
            };

            let (candidates, resolved_skipped) =
                resolve_and_rank_logical_local_execution_candidates(
                    self.state,
                    page.candidates,
                    &self.client_api_format,
                    Some(&self.requested_model),
                    Some(&self.auth_snapshot),
                    self.client_session_affinity.as_ref(),
                    self.required_capabilities.as_ref(),
                    self.sticky_session_token.as_deref(),
                    self.request_auth_channel.as_deref(),
                    self.resolution_mode,
                )
                .await;
            let skipped_candidates = page
                .skipped_candidates
                .into_iter()
                .chain(resolved_skipped)
                .map(|skipped| (self.decorate_skipped_candidate)(skipped))
                .collect::<Vec<_>>();
            self.candidate_count = self
                .candidate_count
                .saturating_add(candidates.len() + skipped_candidates.len());
            if self.record_runtime_miss_diagnostic {
                for skipped_candidate in &skipped_candidates {
                    record_local_runtime_candidate_skip_reason(
                        self.state.app(),
                        &self.trace_id,
                        skipped_candidate.skip_reason,
                    );
                }
            }
            if !self.remembered_affinity && !candidates.is_empty() {
                remember_first_local_candidate_affinity(
                    self.state,
                    Some(&self.auth_snapshot),
                    self.client_session_affinity.as_ref(),
                    &self.client_api_format,
                    Some(&self.requested_model),
                    &candidates,
                );
                self.remembered_affinity = true;
            }
            let (items, next_candidate_index) = build_logical_candidate_items(
                self.state,
                candidates,
                self.next_candidate_index,
                self.sticky_session_token.as_deref(),
                Some(&self.requested_model),
                self.request_auth_channel.as_deref(),
            );
            self.next_candidate_index = next_candidate_index;
            if !items.is_empty() {
                self.pending_items = items;
                return true;
            }
        }
    }
}

async fn pop_attempt_from_items(
    items: &mut VecDeque<LocalExecutionCandidateAttemptSourceItem<'_>>,
) -> Option<LocalExecutionCandidateAttempt> {
    loop {
        let front = items.front_mut()?;
        match front {
            LocalExecutionCandidateAttemptSourceItem::Static { attempts } => {
                if let Some(attempt) = attempts.pop_front() {
                    if attempts.is_empty() {
                        items.pop_front();
                    }
                    return Some(attempt);
                }
                items.pop_front();
            }
            LocalExecutionCandidateAttemptSourceItem::Pool {
                cursor,
                candidate_index,
                pending_attempts,
            } => {
                if let Some(attempt) = pending_attempts.pop_front() {
                    return Some(attempt);
                }
                let Some(candidate) = cursor.next_key().await else {
                    cursor.log_exhausted();
                    let _ = cursor.take_skipped_candidates();
                    items.pop_front();
                    continue;
                };
                *pending_attempts = build_unpersisted_local_execution_candidate_attempts(
                    candidate,
                    *candidate_index,
                );
            }
            LocalExecutionCandidateAttemptSourceItem::RequestedModelPage { .. } => {
                items.pop_front();
            }
        }
    }
}

pub(crate) fn remember_first_local_candidate_affinity(
    state: PlannerAppState<'_>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    client_session_affinity: Option<&ClientSessionAffinity>,
    client_api_format: &str,
    requested_model: Option<&str>,
    candidates: &[EligibleLocalExecutionCandidate],
) {
    let Some(first_candidate) = candidates.first() else {
        return;
    };
    let affinity_requested_model = requested_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(first_candidate.candidate.global_model_name.as_str());
    remember_scheduler_affinity_for_candidate(
        state,
        auth_snapshot,
        client_session_affinity,
        client_api_format,
        affinity_requested_model,
        &first_candidate.candidate,
    );
}

fn should_persist_available_local_candidate(eligible: &EligibleLocalExecutionCandidate) -> bool {
    ai_should_persist_available_candidate_for_pool_key(eligible.orchestration.pool_key_index)
}

fn should_persist_skipped_local_candidate(candidate: &SkippedLocalExecutionCandidate) -> bool {
    let is_pool_candidate = candidate.transport.as_ref().is_some_and(|transport| {
        admin_provider_pool_config_from_config_value(transport.provider.config.as_ref()).is_some()
    });
    ai_should_persist_skipped_candidate_for_pool_membership(is_pool_candidate)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_available_local_execution_candidates<F>(
    state: PlannerAppState<'_>,
    trace_id: &str,
    user_id: &str,
    api_key_id: &str,
    required_capabilities: Option<&Value>,
    candidates: Vec<EligibleLocalExecutionCandidate>,
    error_context: &'static str,
    build_extra_data: F,
) -> Vec<LocalExecutionCandidateAttempt>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
{
    let port = GatewayAvailableCandidatePersistencePort {
        state,
        trace_id,
        user_id,
        api_key_id,
        required_capabilities,
        error_context,
        created_at_unix_ms: current_unix_ms(),
        build_extra_data,
    };

    match run_ai_available_candidate_persistence(&port, candidates).await {
        Ok(attempts) => attempts,
        Err(error) => match error {},
    }
}

pub(crate) async fn persist_available_local_execution_candidates_with_context<F>(
    state: PlannerAppState<'_>,
    trace_id: &str,
    context: LocalAvailableCandidatePersistenceContext<'_>,
    candidates: Vec<EligibleLocalExecutionCandidate>,
    build_extra_data: F,
) -> Vec<LocalExecutionCandidateAttempt>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
{
    persist_available_local_execution_candidates(
        state,
        trace_id,
        context.user_id,
        context.api_key_id,
        context.required_capabilities,
        candidates,
        context.error_context,
        build_extra_data,
    )
    .await
}

async fn persist_available_local_execution_candidate_at_index<F>(
    state: PlannerAppState<'_>,
    trace_id: &str,
    context: LocalAvailableCandidatePersistenceContext<'_>,
    candidate: EligibleLocalExecutionCandidate,
    candidate_index: u32,
    build_extra_data: &F,
) -> Vec<LocalExecutionCandidateAttempt>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
{
    let attempt_slots = local_attempt_slot_count(&candidate.transport).max(1);
    let extra_data = ai_candidate_extra_data_with_ranking(
        build_extra_data(&candidate),
        candidate.ranking.as_ref(),
    );
    let should_persist = should_persist_available_local_candidate(&candidate);
    let mut attempts = Vec::with_capacity(attempt_slots as usize);
    let mut owned_candidate = Some(candidate);

    for retry_index in 0..attempt_slots {
        let candidate_ref = owned_candidate
            .as_ref()
            .expect("candidate should remain available until final retry");
        let generated_candidate_id = Uuid::new_v4().to_string();
        let candidate_id = if should_persist {
            state
                .persist_available_local_candidate(
                    trace_id,
                    context.user_id,
                    context.api_key_id,
                    &candidate_ref.candidate,
                    candidate_index,
                    effective_retry_index(retry_index, candidate_ref.orchestration.pool_key_index),
                    generated_candidate_id.as_str(),
                    context.required_capabilities,
                    extra_data.clone(),
                    current_unix_ms(),
                    context.error_context,
                )
                .await
        } else {
            generated_candidate_id
        };

        let candidate = if retry_index + 1 == attempt_slots {
            owned_candidate
                .take()
                .expect("final retry should consume owned candidate")
        } else {
            candidate_ref.clone()
        };
        let retry_index =
            effective_retry_index(retry_index, candidate.orchestration.pool_key_index);
        attempts.push(LocalExecutionCandidateAttempt {
            eligible: candidate,
            candidate_index,
            retry_index,
            candidate_id,
        });
    }

    attempts
}

fn build_unpersisted_local_execution_candidate_attempts(
    candidate: EligibleLocalExecutionCandidate,
    candidate_index: u32,
) -> VecDeque<LocalExecutionCandidateAttempt> {
    let attempt_slots = local_attempt_slot_count(&candidate.transport).max(1);
    let mut attempts = VecDeque::with_capacity(attempt_slots as usize);
    let mut owned_candidate = Some(candidate);

    for retry_index in 0..attempt_slots {
        let candidate = if retry_index + 1 == attempt_slots {
            owned_candidate
                .take()
                .expect("final retry should consume owned candidate")
        } else {
            owned_candidate
                .as_ref()
                .expect("candidate should remain available until final retry")
                .clone()
        };
        let retry_index =
            effective_retry_index(retry_index, candidate.orchestration.pool_key_index);
        attempts.push_back(LocalExecutionCandidateAttempt {
            eligible: candidate,
            candidate_index,
            retry_index,
            candidate_id: Uuid::new_v4().to_string(),
        });
    }

    attempts
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_skipped_local_execution_candidate(
    state: &AppState,
    trace_id: &str,
    user_id: &str,
    api_key_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    required_capabilities: Option<&Value>,
    skip_reason: &'static str,
    extra_data: Option<Value>,
    error_context: &'static str,
    record_runtime_miss_diagnostic: bool,
) {
    if record_runtime_miss_diagnostic {
        record_local_runtime_candidate_skip_reason(state, trace_id, skip_reason);
    }

    PlannerAppState::new(state)
        .persist_skipped_local_candidate(
            trace_id,
            user_id,
            api_key_id,
            candidate,
            candidate_index,
            0,
            candidate_id,
            required_capabilities,
            skip_reason,
            extra_data,
            current_unix_ms(),
            error_context,
        )
        .await;
}

pub(crate) async fn mark_skipped_local_execution_candidate(
    state: &AppState,
    trace_id: &str,
    context: LocalSkippedCandidatePersistenceContext<'_>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
) {
    persist_skipped_local_execution_candidate(
        state,
        trace_id,
        context.user_id,
        context.api_key_id,
        candidate,
        candidate_index,
        candidate_id,
        context.required_capabilities,
        skip_reason,
        None,
        context.error_context,
        context.record_runtime_miss_diagnostic,
    )
    .await;
}

pub(crate) async fn mark_skipped_local_execution_candidate_with_extra_data(
    state: &AppState,
    trace_id: &str,
    context: LocalSkippedCandidatePersistenceContext<'_>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
    extra_data: Option<Value>,
) {
    persist_skipped_local_execution_candidate(
        state,
        trace_id,
        context.user_id,
        context.api_key_id,
        candidate,
        candidate_index,
        candidate_id,
        context.required_capabilities,
        skip_reason,
        extra_data,
        context.error_context,
        context.record_runtime_miss_diagnostic,
    )
    .await;
}

pub(crate) async fn mark_skipped_local_execution_candidate_with_failure_diagnostic(
    state: &AppState,
    trace_id: &str,
    context: LocalSkippedCandidatePersistenceContext<'_>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
    diagnostic: CandidateFailureDiagnostic,
) {
    mark_skipped_local_execution_candidate_with_extra_data(
        state,
        trace_id,
        context,
        candidate,
        candidate_index,
        candidate_id,
        skip_reason,
        Some(diagnostic.to_extra_data()),
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_skipped_local_execution_candidates(
    state: &AppState,
    trace_id: &str,
    user_id: &str,
    api_key_id: &str,
    required_capabilities: Option<&Value>,
    starting_candidate_index: u32,
    skipped_candidates: Vec<SkippedLocalExecutionCandidate>,
    error_context: &'static str,
    record_runtime_miss_diagnostic: bool,
) {
    let port = GatewaySkippedCandidatePersistencePort {
        state,
        trace_id,
        user_id,
        api_key_id,
        required_capabilities,
        error_context,
        record_runtime_miss_diagnostic,
    };

    match run_ai_skipped_candidate_persistence(&port, starting_candidate_index, skipped_candidates)
        .await
    {
        Ok(()) => {}
        Err(error) => match error {},
    }
}

pub(crate) async fn persist_skipped_local_execution_candidates_with_context(
    state: &AppState,
    trace_id: &str,
    context: LocalSkippedCandidatePersistenceContext<'_>,
    starting_candidate_index: u32,
    skipped_candidates: Vec<SkippedLocalExecutionCandidate>,
) {
    persist_skipped_local_execution_candidates(
        state,
        trace_id,
        context.user_id,
        context.api_key_id,
        context.required_capabilities,
        starting_candidate_index,
        skipped_candidates,
        context.error_context,
        context.record_runtime_miss_diagnostic,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;

    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };
    use aether_scheduler_core::{
        SchedulerMinimalCandidateSelectionCandidate, SchedulerPriorityMode, SchedulerRankingMode,
        SchedulerRankingOutcome,
    };
    use serde_json::json;

    use super::*;
    use crate::data::GatewayDataState;
    use crate::orchestration::LocalExecutionCandidateMetadata;

    fn sample_candidate(key_id: &str) -> SchedulerMinimalCandidateSelectionCandidate {
        SchedulerMinimalCandidateSelectionCandidate {
            provider_id: "provider-1".to_string(),
            provider_name: "provider-1".to_string(),
            provider_type: "codex".to_string(),
            provider_priority: 10,
            endpoint_id: "endpoint-1".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            key_id: key_id.to_string(),
            key_name: key_id.to_string(),
            key_auth_type: "api_key".to_string(),
            key_internal_priority: 10,
            key_global_priority_for_format: Some(10),
            key_capabilities: None,
            model_id: "model-1".to_string(),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            selected_provider_model_name: "gpt-5".to_string(),
            mapping_matched_model: None,
        }
    }

    fn sample_transport(
        key_id: &str,
        provider_config: Option<serde_json::Value>,
    ) -> Arc<crate::ai_serving::GatewayProviderTransportSnapshot> {
        Arc::new(crate::ai_serving::GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "provider-1".to_string(),
                provider_type: "codex".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: false,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: provider_config,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai:chat".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                is_active: true,
                base_url: "https://example.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: key_id.to_string(),
                provider_id: "provider-1".to_string(),
                name: key_id.to_string(),
                auth_type: "api_key".to_string(),
                is_active: true,
                api_formats: Some(vec!["openai:chat".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,

                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: None,
            },
        })
    }

    fn sample_eligible(
        key_id: &str,
        pool_key_index: Option<u32>,
    ) -> EligibleLocalExecutionCandidate {
        EligibleLocalExecutionCandidate {
            kind: LocalExecutionCandidateKind::SingleKey,
            candidate: sample_candidate(key_id),
            transport: sample_transport(
                key_id,
                pool_key_index.map(|_| json!({ "pool_advanced": {} })),
            ),
            provider_api_format: "openai:chat".to_string(),
            orchestration: LocalExecutionCandidateMetadata {
                candidate_group_id: pool_key_index.map(|_| "pool-group".to_string()),
                pool_key_index,
            },
            ranking: None,
        }
    }

    #[tokio::test]
    async fn pool_group_keys_are_not_persisted_as_available_before_attempt() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_request_candidate_repository_for_tests(Arc::clone(
                    &repository,
                )),
            );

        let attempts = persist_available_local_execution_candidates(
            PlannerAppState::new(&app),
            "trace-pool-lazy",
            "user-1",
            "api-key-1",
            None,
            vec![
                sample_eligible("pool-key", Some(0)),
                sample_eligible("pool-key-internal", Some(1)),
                sample_eligible("normal-key", None),
            ],
            "persist should not fail",
            |_| None,
        )
        .await;

        assert_eq!(attempts.len(), 3);
        let stored = app
            .read_request_candidates_by_request_id("trace-pool-lazy")
            .await
            .expect("request candidates should read");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].key_id.as_deref(), Some("normal-key"));
        assert_eq!(stored[0].candidate_index, 2);
    }

    #[test]
    fn pool_key_attempts_use_distinct_effective_retry_indices() {
        let first = build_unpersisted_local_execution_candidate_attempts(
            sample_eligible("pool-key-1", Some(0)),
            0,
        )
        .pop_front()
        .expect("first pool key attempt");
        let second = build_unpersisted_local_execution_candidate_attempts(
            sample_eligible("pool-key-2", Some(1)),
            0,
        )
        .pop_front()
        .expect("second pool key attempt");

        assert_eq!(first.retry_index, 0);
        assert_eq!(second.retry_index, 100);
        assert_eq!(first.attempt_identity().retry_index, 0);
        assert_eq!(second.attempt_identity().retry_index, 100);
        assert_eq!(first.attempt_identity().pool_key_index, Some(0));
        assert_eq!(second.attempt_identity().pool_key_index, Some(1));
    }

    #[tokio::test]
    async fn available_candidates_persist_ranking_metadata_in_extra_data() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_request_candidate_repository_for_tests(Arc::clone(
                    &repository,
                )),
            );
        let mut eligible = sample_eligible("ranked-key", None);
        eligible.ranking = Some(SchedulerRankingOutcome {
            original_index: 1,
            ranking_index: 0,
            priority_mode: SchedulerPriorityMode::Provider,
            ranking_mode: SchedulerRankingMode::CacheAffinity,
            priority_slot: 7,
            promoted_by: Some("cached_affinity"),
            demoted_by: Some("cross_format"),
        });

        persist_available_local_execution_candidates(
            PlannerAppState::new(&app),
            "trace-ranking-extra-data",
            "user-1",
            "api-key-1",
            None,
            vec![eligible],
            "persist should not fail",
            |_| Some(json!({ "existing": "value" })),
        )
        .await;

        let stored = app
            .read_request_candidates_by_request_id("trace-ranking-extra-data")
            .await
            .expect("request candidates should read");
        assert_eq!(stored.len(), 1);
        let extra_data = stored[0]
            .extra_data
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .expect("ranking metadata should persist as object extra data");
        assert_eq!(extra_data.get("existing"), Some(&json!("value")));
        assert_eq!(
            extra_data.get("ranking_mode"),
            Some(&json!("CacheAffinity"))
        );
        assert_eq!(extra_data.get("priority_mode"), Some(&json!("Provider")));
        assert_eq!(extra_data.get("ranking_index"), Some(&json!(0)));
        assert_eq!(extra_data.get("priority_slot"), Some(&json!(7)));
        assert_eq!(
            extra_data.get("promoted_by"),
            Some(&json!("cached_affinity"))
        );
        assert_eq!(extra_data.get("demoted_by"), Some(&json!("cross_format")));
    }

    #[tokio::test]
    async fn dynamic_attempt_source_does_not_drain_unexecuted_single_keys() {
        let mut source = LocalExecutionCandidateAttemptSource {
            items: VecDeque::from([LocalExecutionCandidateAttemptSourceItem::Static {
                attempts: build_unpersisted_local_execution_candidate_attempts(
                    sample_eligible("normal-key", None),
                    0,
                ),
            }]),
        };

        let first = source
            .next_attempt()
            .await
            .expect("first attempt should be available");
        assert_eq!(first.eligible.candidate.key_id, "normal-key");

        let remaining = source.drain_static_attempts();
        assert!(remaining.is_empty());
        assert!(source.next_attempt().await.is_none());
    }

    #[tokio::test]
    async fn pool_internal_skipped_candidates_are_not_persisted() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_request_candidate_repository_for_tests(Arc::clone(
                    &repository,
                )),
            );

        persist_skipped_local_execution_candidates(
            &app,
            "trace-pool-skipped",
            "user-1",
            "api-key-1",
            None,
            0,
            vec![
                SkippedLocalExecutionCandidate {
                    candidate: sample_candidate("pool-skipped"),
                    skip_reason: "pool_cooldown",
                    transport: Some(sample_transport(
                        "pool-skipped",
                        Some(json!({ "pool_advanced": {} })),
                    )),
                    ranking: None,
                    extra_data: None,
                },
                SkippedLocalExecutionCandidate {
                    candidate: sample_candidate("normal-skipped"),
                    skip_reason: "key_inactive",
                    transport: None,
                    ranking: Some(SchedulerRankingOutcome {
                        original_index: 2,
                        ranking_index: 1,
                        priority_mode: SchedulerPriorityMode::Provider,
                        ranking_mode: SchedulerRankingMode::CacheAffinity,
                        priority_slot: 9,
                        promoted_by: None,
                        demoted_by: Some("cross_format"),
                    }),
                    extra_data: Some(json!({ "existing": "value" })),
                },
            ],
            "persist skipped should not fail",
            false,
        )
        .await;

        let stored = app
            .read_request_candidates_by_request_id("trace-pool-skipped")
            .await
            .expect("request candidates should read");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].key_id.as_deref(), Some("normal-skipped"));
        assert_eq!(stored[0].candidate_index, 0);
        let extra_data = stored[0]
            .extra_data
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .expect("skipped ranking metadata should persist");
        assert_eq!(extra_data.get("existing"), Some(&json!("value")));
        assert_eq!(
            extra_data.get("ranking_mode"),
            Some(&json!("CacheAffinity"))
        );
        assert_eq!(extra_data.get("priority_mode"), Some(&json!("Provider")));
        assert_eq!(extra_data.get("ranking_index"), Some(&json!(1)));
        assert_eq!(extra_data.get("priority_slot"), Some(&json!(9)));
        assert_eq!(extra_data.get("demoted_by"), Some(&json!("cross_format")));
    }
}
