use aether_ai_serving::{
    ai_candidate_extra_data_with_ranking, ai_should_persist_available_candidate_for_pool_key,
    ai_should_persist_skipped_candidate_for_pool_membership,
    run_ai_available_candidate_persistence, run_ai_candidate_materialization,
    run_ai_skipped_candidate_persistence, AiAvailableCandidatePersistencePort,
    AiCandidateMaterializationOutcome, AiCandidateMaterializationPort,
    AiCandidatePreselectionOutcome, AiSkippedCandidatePersistencePort,
};
use aether_dispatch_core::{DispatchSequence, DispatchSequenceItem};
use aether_routing_core::{
    rank_vector_for_candidate, CandidateKind, ResolvedRoutingPolicy, RoutingCandidateFacts,
    RoutingCandidateTrace, RoutingDecisionTrace,
};
use aether_scheduler_core::{
    ClientSessionAffinity, SchedulerMinimalCandidateSelectionCandidate, SchedulerRankingOutcome,
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use tracing::warn;
use uuid::Uuid;

use crate::ai_serving::planner::candidate_affinity_cache::remember_scheduler_affinity_for_candidate_at_epoch;
use crate::ai_serving::planner::candidate_ranking::scheduler_ordering_config_for_routing_policy;
use crate::ai_serving::planner::candidate_resolution::{
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
use crate::cache::{
    candidate_page_cache_stale_ttl, candidate_page_cache_ttl_from_env,
    record_candidate_page_resolve_cache_follower_wait, record_candidate_page_resolve_cache_hit,
    record_candidate_page_resolve_cache_load, record_candidate_page_resolve_cache_miss,
    CacheLoadObserver, CandidateResolvedPageCacheKey, CandidateResolvedPageSnapshot,
};
use crate::clock::current_unix_ms;
use crate::dispatch::refs::dispatch_ref_for_local_candidate;
use crate::handlers::shared::provider_pool::admin_provider_pool_config_from_config_value;
use crate::orchestration::{local_attempt_slot_count, ExecutionAttemptIdentity};
use crate::scheduler::candidate::is_auth_api_key_concurrency_limit_skip_reason;
use crate::scheduler::config::SchedulerSchedulingMode;
use crate::stage_metrics::observe_gateway_stage_ms;
use crate::{AppState, GatewayError};

const POOL_KEY_RETRY_INDEX_STRIDE: u32 = 100;
const AUTH_API_KEY_CONCURRENCY_WAIT_BUDGET: Duration = Duration::from_millis(100);
const AUTH_API_KEY_CONCURRENCY_RETRY_DELAY: Duration = Duration::from_millis(10);

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
}

enum LocalExecutionCandidateAttemptSourceItem<'a> {
    Static {
        attempts: DispatchSequence<LocalExecutionCandidateAttempt>,
    },
    Pool {
        cursor: PoolKeyCursor<'a>,
        candidate_index: u32,
        pending_attempts: DispatchSequence<LocalExecutionCandidateAttempt>,
        pool_exhaustion_persistence: Option<PoolGroupExhaustionPersistenceContext>,
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
                attempts: dispatch_sequence_from_attempts(attempts),
            });
        }
        Self { items }
    }

    pub(crate) async fn next_attempt(
        &mut self,
    ) -> Result<Option<LocalExecutionCandidateAttempt>, GatewayError> {
        loop {
            let Some(front) = self.items.front_mut() else {
                return Ok(None);
            };
            match front {
                LocalExecutionCandidateAttemptSourceItem::Static { attempts } => {
                    if let Some(attempt) = next_attempt_from_dispatch_sequence(attempts) {
                        if dispatch_sequence_exhausted(attempts) {
                            self.items.pop_front();
                        }
                        return Ok(Some(attempt));
                    }
                    self.items.pop_front();
                }
                LocalExecutionCandidateAttemptSourceItem::Pool {
                    cursor,
                    candidate_index,
                    pending_attempts,
                    pool_exhaustion_persistence,
                } => {
                    if let Some(attempt) = next_attempt_from_dispatch_sequence(pending_attempts) {
                        return Ok(Some(attempt));
                    }
                    let Some(candidate) = cursor.next_key().await else {
                        if let Some(skipped) = cursor.exhausted_group_skipped_candidate() {
                            persist_pool_group_exhaustion_skipped_candidate(
                                pool_exhaustion_persistence.as_ref(),
                                *candidate_index,
                                skipped,
                            )
                            .await;
                        }
                        cursor.log_exhausted();
                        let _ = cursor.take_skipped_candidates();
                        self.items.pop_front();
                        continue;
                    };
                    *pending_attempts = dispatch_sequence_from_attempts(
                        build_unpersisted_local_execution_candidate_attempts(
                            candidate,
                            *candidate_index,
                        )
                        .into(),
                    );
                }
                LocalExecutionCandidateAttemptSourceItem::RequestedModelPage { cursor } => {
                    let Some(attempt) = cursor.next_attempt().await? else {
                        self.items.pop_front();
                        continue;
                    };
                    return Ok(Some(attempt));
                }
            }
        }
    }

    pub(crate) fn drain_static_attempts(&mut self) -> Vec<LocalExecutionCandidateAttempt> {
        self.items.clear();
        Vec::new()
    }
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

#[derive(Clone)]
struct PoolGroupExhaustionPersistenceContext {
    app: AppState,
    trace_id: String,
    user_id: String,
    api_key_id: String,
    required_capabilities: Option<Value>,
    error_context: &'static str,
    client_api_format: String,
    routing_policy: Option<ResolvedRoutingPolicy>,
}

impl PoolGroupExhaustionPersistenceContext {
    fn new(
        app: AppState,
        trace_id: &str,
        context: LocalSkippedCandidatePersistenceContext<'_>,
        client_api_format: &str,
        routing_policy: Option<&ResolvedRoutingPolicy>,
    ) -> Self {
        Self {
            app,
            trace_id: trace_id.to_string(),
            user_id: context.user_id.to_string(),
            api_key_id: context.api_key_id.to_string(),
            required_capabilities: context.required_capabilities.cloned(),
            error_context: context.error_context,
            client_api_format: client_api_format.to_string(),
            routing_policy: routing_policy.cloned(),
        }
    }
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
    routing_policy: Option<&'a ResolvedRoutingPolicy>,
    sticky_session_token: Option<&'a str>,
    request_auth_channel: Option<&'a str>,
    persistence_policy: LocalCandidatePersistencePolicy<'a>,
    resolution_mode: LocalCandidateResolutionMode,
    scheduler_cache_affinity_enabled: bool,
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
        let resolved = resolve_and_rank_logical_local_execution_candidates(
            self.state,
            candidates,
            self.client_api_format,
            self.requested_model,
            self.auth_snapshot,
            self.client_session_affinity,
            self.required_capabilities,
            self.routing_policy,
            self.sticky_session_token,
            self.request_auth_channel,
            self.resolution_mode,
        )
        .await;
        Ok(resolved)
    }

    fn decorate_skipped_candidate(&self, skipped: Self::Skipped) -> Self::Skipped {
        (self.decorate_skipped_candidate)(skipped)
    }

    fn remember_first_candidate_affinity(&self, candidates: &[Self::Eligible]) {
        if !self.scheduler_cache_affinity_enabled {
            return;
        }
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
        Ok(materialize_logical_local_execution_candidate_attempts(
            self.state,
            self.trace_id,
            self.persistence_policy.available,
            self.persistence_policy
                .skipped
                .record_runtime_miss_diagnostic,
            candidates,
            self.routing_policy,
            self.client_api_format,
            self.sticky_session_token,
            self.requested_model,
            self.request_auth_channel,
            &self.build_available_extra_data,
        )
        .await)
    }

    async fn persist_skipped_candidates(
        &self,
        starting_candidate_index: u32,
        skipped_candidates: Vec<Self::Skipped>,
    ) -> Result<(), Self::Error> {
        let skipped_candidates = attach_routing_trace_to_skipped_candidates(
            self.routing_policy,
            self.client_api_format,
            starting_candidate_index,
            skipped_candidates,
        );
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
        available_candidate_extra_data_with_dispatch_ref(candidate, &self.build_extra_data)
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
    routing_policy: Option<&ResolvedRoutingPolicy>,
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
    let scheduler_cache_affinity_enabled =
        scheduler_cache_affinity_enabled(state, routing_policy).await;
    let port = GatewayLocalCandidateMaterializationPort {
        state,
        trace_id,
        client_api_format,
        requested_model,
        auth_snapshot,
        client_session_affinity,
        required_capabilities,
        routing_policy,
        sticky_session_token,
        request_auth_channel,
        persistence_policy,
        resolution_mode,
        scheduler_cache_affinity_enabled,
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
    routing_policy: Option<&ResolvedRoutingPolicy>,
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
    let scheduler_cache_affinity_enabled =
        scheduler_cache_affinity_enabled(state, routing_policy).await;
    let _ = build_available_extra_data;
    let (candidates, resolved_skipped) = resolve_and_rank_logical_local_execution_candidates(
        state,
        candidates,
        client_api_format,
        requested_model,
        auth_snapshot,
        client_session_affinity,
        required_capabilities,
        routing_policy,
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

    if scheduler_cache_affinity_enabled {
        remember_first_local_candidate_affinity(
            state,
            auth_snapshot,
            client_session_affinity,
            client_api_format,
            requested_model,
            &candidates,
        );
    }
    persist_skipped_local_execution_candidates_with_context(
        state.app(),
        trace_id,
        persistence_policy.skipped,
        u32::try_from(candidates.len()).unwrap_or(u32::MAX),
        attach_routing_trace_to_skipped_candidates(
            routing_policy,
            client_api_format,
            u32::try_from(candidates.len()).unwrap_or(u32::MAX),
            skipped_candidates,
        ),
    )
    .await;

    let (items, _) = build_logical_candidate_items(
        state,
        candidates,
        0,
        Some(trace_id),
        persistence_policy.skipped.record_runtime_miss_diagnostic,
        sticky_session_token,
        requested_model,
        request_auth_channel,
        routing_policy,
        Some(PoolGroupExhaustionPersistenceContext::new(
            state.app().clone(),
            trace_id,
            persistence_policy.skipped,
            client_api_format,
            routing_policy,
        )),
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
    trace_id: Option<&str>,
    record_runtime_miss_diagnostic: bool,
    sticky_session_token: Option<&str>,
    requested_model: Option<&str>,
    request_auth_channel: Option<&str>,
    routing_policy: Option<&ResolvedRoutingPolicy>,
    pool_exhaustion_persistence: Option<PoolGroupExhaustionPersistenceContext>,
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
                    items.push_back(LocalExecutionCandidateAttemptSourceItem::Static {
                        attempts: dispatch_sequence_from_attempts(attempts.into()),
                    });
                }
            }
            LocalExecutionCandidateKind::PoolGroup => {
                let cursor = PoolKeyCursor::new_with_routing_policy(
                    state,
                    candidate,
                    sticky_session_token,
                    requested_model,
                    request_auth_channel,
                    routing_policy,
                );
                let cursor = if let Some(trace_id) = trace_id {
                    cursor.with_runtime_miss_diagnostic(trace_id, record_runtime_miss_diagnostic)
                } else {
                    cursor
                };
                items.push_back(LocalExecutionCandidateAttemptSourceItem::Pool {
                    cursor,
                    candidate_index,
                    pending_attempts: DispatchSequence::new(Vec::new()),
                    pool_exhaustion_persistence: pool_exhaustion_persistence.clone(),
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
    routing_policy: Option<&ResolvedRoutingPolicy>,
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
    let scheduler_cache_affinity_enabled =
        scheduler_cache_affinity_enabled(state, routing_policy).await;
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
        routing_policy,
        client_session_affinity,
        request_auth_channel,
        use_api_format_alias_match,
        key_mode,
        sticky_session_token.is_none(),
        Some(trace_id),
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
        routing_policy: routing_policy.cloned(),
        sticky_session_token: sticky_session_token.map(str::to_string),
        request_auth_channel: request_auth_channel.map(str::to_string),
        skipped_user_id: persistence_policy.skipped.user_id.to_string(),
        skipped_api_key_id: persistence_policy.skipped.api_key_id.to_string(),
        skipped_required_capabilities: persistence_policy.skipped.required_capabilities.cloned(),
        skipped_error_context: persistence_policy.skipped.error_context,
        record_runtime_miss_diagnostic,
        resolution_mode,
        decorate_skipped_candidate,
        page_cursor,
        pending_items: VecDeque::new(),
        candidate_count: 0,
        next_candidate_index: 0,
        remembered_affinity: false,
        scheduler_cache_affinity_enabled,
        auth_api_key_concurrency_wait_deadline: None,
        deferred_error: None,
    };
    if let Err(error) = cursor.load_next_page().await {
        cursor.deferred_error = Some(error);
    }
    let candidate_count = cursor.candidate_count;
    let mut items = VecDeque::new();
    if !cursor.pending_items.is_empty() || cursor.deferred_error.is_some() {
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
    routing_policy: Option<ResolvedRoutingPolicy>,
    sticky_session_token: Option<String>,
    request_auth_channel: Option<String>,
    skipped_user_id: String,
    skipped_api_key_id: String,
    skipped_required_capabilities: Option<Value>,
    skipped_error_context: &'static str,
    record_runtime_miss_diagnostic: bool,
    resolution_mode: LocalCandidateResolutionMode,
    decorate_skipped_candidate: DecorateSkippedCandidateFn<'a>,
    page_cursor: LocalCandidatePreselectionPageCursor<'a>,
    pending_items: VecDeque<LocalExecutionCandidateAttemptSourceItem<'a>>,
    candidate_count: usize,
    next_candidate_index: u32,
    remembered_affinity: bool,
    scheduler_cache_affinity_enabled: bool,
    auth_api_key_concurrency_wait_deadline: Option<Instant>,
    deferred_error: Option<GatewayError>,
}

impl<'a> RequestedModelAttemptPageCursor<'a> {
    async fn next_attempt(
        &mut self,
    ) -> Result<Option<LocalExecutionCandidateAttempt>, GatewayError> {
        if let Some(error) = self.deferred_error.take() {
            return Err(error);
        }
        loop {
            if let Some(attempt) = pop_attempt_from_items(&mut self.pending_items).await {
                return Ok(Some(attempt));
            }
            if !self.load_next_page().await? {
                return Ok(None);
            }
        }
    }

    async fn load_next_page(&mut self) -> Result<bool, GatewayError> {
        loop {
            let page_started_at = std::time::Instant::now();
            let page = match self.page_cursor.next_page().await {
                Ok(Some(page)) => page,
                Ok(None) => {
                    observe_gateway_stage_ms(
                        "candidate_page_load",
                        page_started_at.elapsed().as_millis() as u64,
                    );
                    return Ok(false);
                }
                Err(error) => {
                    observe_gateway_stage_ms(
                        "candidate_page_load",
                        page_started_at.elapsed().as_millis() as u64,
                    );
                    if matches!(error, GatewayError::AdmissionTimeout { .. }) {
                        return Err(error);
                    }
                    warn!(
                        trace_id = %self.trace_id,
                        error = ?error,
                        "gateway lazy requested-model candidate page read failed"
                    );
                    return Ok(false);
                }
            };
            observe_gateway_stage_ms(
                "candidate_page_load",
                page_started_at.elapsed().as_millis() as u64,
            );

            if page_is_exact_auth_api_key_concurrency_limited(&page) {
                if self.wait_for_auth_api_key_concurrency_retry().await {
                    continue;
                }
                self.persist_final_auth_api_key_concurrency_skips(page.skipped_candidates)
                    .await;
                return Ok(false);
            }

            let resolve_started_at = std::time::Instant::now();
            let (candidates, resolved_skipped) =
                resolve_priority_candidate_page_with_cache(self, page.candidates).await;
            observe_gateway_stage_ms(
                "candidate_page_resolve",
                resolve_started_at.elapsed().as_millis() as u64,
            );
            let skipped_candidates = page
                .skipped_candidates
                .into_iter()
                .chain(resolved_skipped)
                .map(|skipped| (self.decorate_skipped_candidate)(skipped))
                .collect::<Vec<_>>();
            let skipped_candidate_count = skipped_candidates.len();
            self.candidate_count = self
                .candidate_count
                .saturating_add(candidates.len() + skipped_candidate_count);
            if self.scheduler_cache_affinity_enabled
                && !self.remembered_affinity
                && !candidates.is_empty()
            {
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
                Some(&self.trace_id),
                self.record_runtime_miss_diagnostic,
                self.sticky_session_token.as_deref(),
                Some(&self.requested_model),
                self.request_auth_channel.as_deref(),
                self.routing_policy.as_ref(),
                Some(PoolGroupExhaustionPersistenceContext {
                    app: self.state.app().clone(),
                    trace_id: self.trace_id.clone(),
                    user_id: self.skipped_user_id.clone(),
                    api_key_id: self.skipped_api_key_id.clone(),
                    required_capabilities: self.skipped_required_capabilities.clone(),
                    error_context: self.skipped_error_context,
                    client_api_format: self.client_api_format.clone(),
                    routing_policy: self.routing_policy.clone(),
                }),
            );
            self.next_candidate_index = next_candidate_index
                .saturating_add(u32::try_from(skipped_candidate_count).unwrap_or(u32::MAX));
            if !items.is_empty() {
                self.pending_items = items;
                return Ok(true);
            }
            let skipped_starting_candidate_index = next_candidate_index;
            let skipped_persistence = LocalSkippedCandidatePersistenceContext {
                user_id: self.skipped_user_id.as_str(),
                api_key_id: self.skipped_api_key_id.as_str(),
                required_capabilities: self.skipped_required_capabilities.as_ref(),
                error_context: self.skipped_error_context,
                record_runtime_miss_diagnostic: self.record_runtime_miss_diagnostic,
            };
            persist_skipped_local_execution_candidates_with_context(
                self.state.app(),
                &self.trace_id,
                skipped_persistence,
                skipped_starting_candidate_index,
                attach_routing_trace_to_skipped_candidates(
                    self.routing_policy.as_ref(),
                    &self.client_api_format,
                    skipped_starting_candidate_index,
                    skipped_candidates,
                ),
            )
            .await;
        }
    }

    async fn wait_for_auth_api_key_concurrency_retry(&mut self) -> bool {
        let now = Instant::now();
        let deadline = *self
            .auth_api_key_concurrency_wait_deadline
            .get_or_insert(now + AUTH_API_KEY_CONCURRENCY_WAIT_BUDGET);
        if now >= deadline {
            return false;
        }

        let sleep_duration =
            AUTH_API_KEY_CONCURRENCY_RETRY_DELAY.min(deadline.saturating_duration_since(now));
        tokio::time::sleep(sleep_duration).await;
        self.page_cursor.restart_scan();
        true
    }

    async fn persist_final_auth_api_key_concurrency_skips(
        &mut self,
        skipped_candidates: Vec<SkippedLocalExecutionCandidate>,
    ) {
        let skipped_candidates = skipped_candidates
            .into_iter()
            .map(|skipped| (self.decorate_skipped_candidate)(skipped))
            .collect::<Vec<_>>();
        let skipped_candidate_count = skipped_candidates.len();
        self.candidate_count = self.candidate_count.saturating_add(skipped_candidate_count);
        let skipped_persistence = LocalSkippedCandidatePersistenceContext {
            user_id: self.skipped_user_id.as_str(),
            api_key_id: self.skipped_api_key_id.as_str(),
            required_capabilities: self.skipped_required_capabilities.as_ref(),
            error_context: self.skipped_error_context,
            record_runtime_miss_diagnostic: self.record_runtime_miss_diagnostic,
        };
        persist_skipped_local_execution_candidates_with_context(
            self.state.app(),
            &self.trace_id,
            skipped_persistence,
            self.next_candidate_index,
            attach_routing_trace_to_skipped_candidates(
                self.routing_policy.as_ref(),
                &self.client_api_format,
                self.next_candidate_index,
                skipped_candidates,
            ),
        )
        .await;
        self.next_candidate_index = self
            .next_candidate_index
            .saturating_add(u32::try_from(skipped_candidate_count).unwrap_or(u32::MAX));
    }
}

fn page_is_exact_auth_api_key_concurrency_limited(
    page: &AiCandidatePreselectionOutcome<
        SchedulerMinimalCandidateSelectionCandidate,
        SkippedLocalExecutionCandidate,
    >,
) -> bool {
    page.candidates.is_empty()
        && !page.skipped_candidates.is_empty()
        && page
            .skipped_candidates
            .iter()
            .all(|skipped| is_auth_api_key_concurrency_limit_skip_reason(skipped.skip_reason))
}

async fn pop_attempt_from_items(
    items: &mut VecDeque<LocalExecutionCandidateAttemptSourceItem<'_>>,
) -> Option<LocalExecutionCandidateAttempt> {
    loop {
        let front = items.front_mut()?;
        match front {
            LocalExecutionCandidateAttemptSourceItem::Static { attempts } => {
                if let Some(attempt) = next_attempt_from_dispatch_sequence(attempts) {
                    if dispatch_sequence_exhausted(attempts) {
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
                pool_exhaustion_persistence,
            } => {
                if let Some(attempt) = next_attempt_from_dispatch_sequence(pending_attempts) {
                    return Some(attempt);
                }
                let Some(candidate) = cursor.next_key().await else {
                    if let Some(skipped) = cursor.exhausted_group_skipped_candidate() {
                        persist_pool_group_exhaustion_skipped_candidate(
                            pool_exhaustion_persistence.as_ref(),
                            *candidate_index,
                            skipped,
                        )
                        .await;
                    }
                    cursor.log_exhausted();
                    let _ = cursor.take_skipped_candidates();
                    items.pop_front();
                    continue;
                };
                *pending_attempts = dispatch_sequence_from_attempts(
                    build_unpersisted_local_execution_candidate_attempts(
                        candidate,
                        *candidate_index,
                    )
                    .into(),
                );
            }
            LocalExecutionCandidateAttemptSourceItem::RequestedModelPage { .. } => {
                items.pop_front();
            }
        }
    }
}

async fn scheduler_cache_affinity_enabled(
    state: PlannerAppState<'_>,
    routing_policy: Option<&ResolvedRoutingPolicy>,
) -> bool {
    scheduler_ordering_config_for_routing_policy(state, routing_policy)
        .await
        .scheduling_mode
        == SchedulerSchedulingMode::CacheAffinity
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
    remember_scheduler_affinity_for_candidate_at_epoch(
        state,
        auth_snapshot,
        client_session_affinity,
        client_api_format,
        affinity_requested_model,
        &first_candidate.candidate,
        first_candidate.orchestration.scheduler_affinity_epoch,
    );
}

async fn resolve_priority_candidate_page_with_cache(
    cursor: &RequestedModelAttemptPageCursor<'_>,
    page_candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    if !should_cache_resolved_candidate_page(cursor) {
        return resolve_and_rank_logical_local_execution_candidates(
            cursor.state,
            page_candidates,
            &cursor.client_api_format,
            Some(&cursor.requested_model),
            Some(&cursor.auth_snapshot),
            cursor.client_session_affinity.as_ref(),
            cursor.required_capabilities.as_ref(),
            cursor.routing_policy.as_ref(),
            cursor.sticky_session_token.as_deref(),
            cursor.request_auth_channel.as_deref(),
            cursor.resolution_mode,
        )
        .await;
    }

    let key = CandidateResolvedPageCacheKey::new(
        &cursor.requested_model,
        &cursor.client_api_format,
        true,
        &cursor.auth_snapshot,
        cursor.required_capabilities.as_ref(),
        cursor.routing_policy.as_ref(),
        cursor.request_auth_channel.as_deref(),
        cursor.state.app().scheduler_affinity_epoch(),
        cursor.page_cursor.resolved_page_cache_preselection_mode(),
        cursor
            .page_cursor
            .resolved_page_cache_use_api_format_alias_match(),
        cursor.client_session_affinity.as_ref(),
        cursor.resolution_mode,
    );
    let page_candidates_for_fallback = page_candidates.clone();
    let page_candidates_for_load = page_candidates;
    let cache = cursor.state.app().candidate_resolved_page_cache.clone();
    let ttl = candidate_page_cache_ttl_from_env();
    let stale_ttl = candidate_page_cache_stale_ttl(ttl);
    let cached = cache
        .get_or_load_once_stale_while_refreshing(
            key,
            ttl,
            stale_ttl,
            || async move {
                let (candidates, resolved_skipped) =
                    resolve_and_rank_logical_local_execution_candidates(
                        cursor.state,
                        page_candidates_for_load,
                        &cursor.client_api_format,
                        Some(&cursor.requested_model),
                        Some(&cursor.auth_snapshot),
                        cursor.client_session_affinity.as_ref(),
                        cursor.required_capabilities.as_ref(),
                        cursor.routing_policy.as_ref(),
                        cursor.sticky_session_token.as_deref(),
                        cursor.request_auth_channel.as_deref(),
                        cursor.resolution_mode,
                    )
                    .await;
                Ok::<_, GatewayError>(Some(Arc::new(CandidateResolvedPageSnapshot {
                    candidates,
                    resolved_skipped,
                })))
            },
            CacheLoadObserver::new()
                .on_hit(record_candidate_page_resolve_cache_hit)
                .on_miss(record_candidate_page_resolve_cache_miss)
                .on_load(record_candidate_page_resolve_cache_load)
                .on_follower_wait(record_candidate_page_resolve_cache_follower_wait),
        )
        .await
        .unwrap_or(None);

    match cached {
        Some(snapshot) => (
            snapshot.candidates.clone(),
            snapshot.resolved_skipped.clone(),
        ),
        None => {
            if page_candidates_for_fallback.is_empty() {
                return (Vec::new(), Vec::new());
            }
            resolve_and_rank_logical_local_execution_candidates(
                cursor.state,
                page_candidates_for_fallback,
                &cursor.client_api_format,
                Some(&cursor.requested_model),
                Some(&cursor.auth_snapshot),
                cursor.client_session_affinity.as_ref(),
                cursor.required_capabilities.as_ref(),
                cursor.routing_policy.as_ref(),
                cursor.sticky_session_token.as_deref(),
                cursor.request_auth_channel.as_deref(),
                cursor.resolution_mode,
            )
            .await
        }
    }
}

fn should_cache_resolved_candidate_page(cursor: &RequestedModelAttemptPageCursor<'_>) -> bool {
    cursor.sticky_session_token.is_none()
        && cursor
            .page_cursor
            .should_cache_current_priority_resolved_page()
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

#[allow(clippy::too_many_arguments)]
async fn materialize_logical_local_execution_candidate_attempts<F>(
    state: PlannerAppState<'_>,
    trace_id: &str,
    context: LocalAvailableCandidatePersistenceContext<'_>,
    record_runtime_miss_diagnostic: bool,
    candidates: Vec<EligibleLocalExecutionCandidate>,
    routing_policy: Option<&ResolvedRoutingPolicy>,
    client_api_format: &str,
    sticky_session_token: Option<&str>,
    requested_model: Option<&str>,
    request_auth_channel: Option<&str>,
    build_extra_data: &F,
) -> Vec<LocalExecutionCandidateAttempt>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
{
    let mut attempts = Vec::new();

    for (candidate_index, candidate) in candidates.into_iter().enumerate() {
        let candidate_index = u32::try_from(candidate_index).unwrap_or(u32::MAX);
        match candidate.kind {
            LocalExecutionCandidateKind::SingleKey => {
                attempts.extend(
                    persist_available_local_execution_candidate_at_index(
                        state,
                        trace_id,
                        context,
                        candidate,
                        candidate_index,
                        routing_policy,
                        client_api_format,
                        build_extra_data,
                    )
                    .await,
                );
            }
            LocalExecutionCandidateKind::PoolGroup => {
                let mut cursor = PoolKeyCursor::new_with_routing_policy(
                    state,
                    candidate,
                    sticky_session_token,
                    requested_model,
                    request_auth_channel,
                    routing_policy,
                )
                .with_runtime_miss_diagnostic(trace_id, record_runtime_miss_diagnostic);
                let attempt_count_before_pool = attempts.len();
                while let Some(candidate) = cursor.next_key().await {
                    attempts.extend(build_unpersisted_local_execution_candidate_attempts(
                        candidate,
                        candidate_index,
                    ));
                }
                let _ = cursor.take_skipped_candidates();
                if attempts.len() == attempt_count_before_pool {
                    let skipped = cursor.exhausted_group_skipped_candidate();
                    cursor.log_exhausted();
                    if let Some(skipped) = skipped {
                        let pool_exhaustion_context = PoolGroupExhaustionPersistenceContext::new(
                            state.app().clone(),
                            trace_id,
                            LocalSkippedCandidatePersistenceContext {
                                user_id: context.user_id,
                                api_key_id: context.api_key_id,
                                required_capabilities: context.required_capabilities,
                                error_context: context.error_context,
                                record_runtime_miss_diagnostic: false,
                            },
                            client_api_format,
                            routing_policy,
                        );
                        persist_pool_group_exhaustion_skipped_candidate(
                            Some(&pool_exhaustion_context),
                            candidate_index,
                            skipped,
                        )
                        .await;
                    }
                }
            }
        }
    }

    attempts
}

async fn persist_available_local_execution_candidate_at_index<F>(
    state: PlannerAppState<'_>,
    trace_id: &str,
    context: LocalAvailableCandidatePersistenceContext<'_>,
    candidate: EligibleLocalExecutionCandidate,
    candidate_index: u32,
    routing_policy: Option<&ResolvedRoutingPolicy>,
    client_api_format: &str,
    build_extra_data: &F,
) -> Vec<LocalExecutionCandidateAttempt>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
{
    let attempt_slots = local_attempt_slot_count(&candidate.transport).max(1);
    let extra_data = ai_candidate_extra_data_with_ranking(
        available_candidate_base_extra_data_with_dispatch_ref(&candidate, build_extra_data),
        candidate.ranking.as_ref(),
    );
    let extra_data = attach_routing_trace_to_extra_data(
        routing_policy,
        client_api_format,
        &candidate.candidate,
        candidate.kind,
        candidate.ranking.as_ref(),
        None,
        Some(candidate_index),
        extra_data,
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

fn available_candidate_extra_data_with_dispatch_ref<F>(
    candidate: &EligibleLocalExecutionCandidate,
    build_extra_data: &F,
) -> Option<Value>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
{
    ai_candidate_extra_data_with_ranking(
        available_candidate_base_extra_data_with_dispatch_ref(candidate, build_extra_data),
        candidate.ranking.as_ref(),
    )
}

fn available_candidate_base_extra_data_with_dispatch_ref<F>(
    candidate: &EligibleLocalExecutionCandidate,
    build_extra_data: &F,
) -> Option<Value>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
{
    let dispatch_ref = serde_json::to_value(dispatch_ref_for_local_candidate(candidate)).ok()?;
    let mut object = match build_extra_data(candidate) {
        Some(Value::Object(object)) => object,
        Some(value) => {
            let mut object = serde_json::Map::new();
            object.insert("extra".to_string(), value);
            object
        }
        None => serde_json::Map::new(),
    };
    object.insert("dispatch_ref".to_string(), dispatch_ref);
    Some(Value::Object(object))
}

fn attach_routing_trace_to_skipped_candidates(
    routing_policy: Option<&ResolvedRoutingPolicy>,
    client_api_format: &str,
    starting_candidate_index: u32,
    skipped_candidates: Vec<SkippedLocalExecutionCandidate>,
) -> Vec<SkippedLocalExecutionCandidate> {
    skipped_candidates
        .into_iter()
        .enumerate()
        .map(|(offset, skipped)| {
            let selected_order =
                starting_candidate_index.saturating_add(u32::try_from(offset).unwrap_or(u32::MAX));
            attach_routing_trace_to_skipped_candidate(
                routing_policy,
                client_api_format,
                selected_order,
                skipped,
            )
        })
        .collect()
}

fn attach_routing_trace_to_skipped_candidate(
    routing_policy: Option<&ResolvedRoutingPolicy>,
    client_api_format: &str,
    selected_order: u32,
    mut skipped_candidate: SkippedLocalExecutionCandidate,
) -> SkippedLocalExecutionCandidate {
    let kind = if skipped_candidate
        .transport
        .as_ref()
        .is_some_and(|transport| {
            admin_provider_pool_config_from_config_value(transport.provider.config.as_ref())
                .is_some()
        }) {
        LocalExecutionCandidateKind::PoolGroup
    } else {
        LocalExecutionCandidateKind::SingleKey
    };
    skipped_candidate.extra_data = attach_routing_trace_to_extra_data(
        routing_policy,
        client_api_format,
        &skipped_candidate.candidate,
        kind,
        skipped_candidate.ranking.as_ref(),
        Some(skipped_candidate.skip_reason),
        Some(selected_order),
        skipped_candidate.extra_data,
    );
    skipped_candidate
}

#[allow(clippy::too_many_arguments)]
fn attach_routing_trace_to_extra_data(
    routing_policy: Option<&ResolvedRoutingPolicy>,
    client_api_format: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    kind: LocalExecutionCandidateKind,
    ranking: Option<&SchedulerRankingOutcome>,
    skip_reason: Option<&'static str>,
    selected_order: Option<u32>,
    extra_data: Option<Value>,
) -> Option<Value> {
    let Some(policy) = routing_policy else {
        return extra_data;
    };
    let routing_trace = routing_trace_for_candidate(
        policy,
        client_api_format,
        candidate,
        kind,
        ranking,
        skip_reason,
        selected_order,
    );
    Some(merge_routing_trace_into_extra_data(
        extra_data,
        routing_trace,
    ))
}

fn merge_routing_trace_into_extra_data(
    extra_data: Option<Value>,
    routing_trace: RoutingDecisionTrace,
) -> Value {
    let mut object = match extra_data {
        Some(Value::Object(object)) => object,
        Some(value) => {
            let mut object = serde_json::Map::new();
            object.insert("extra".to_string(), value);
            object
        }
        None => serde_json::Map::new(),
    };
    object.insert(
        "routing_trace".to_string(),
        serde_json::json!(routing_trace),
    );
    Value::Object(object)
}

fn routing_trace_for_candidate(
    policy: &ResolvedRoutingPolicy,
    client_api_format: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    kind: LocalExecutionCandidateKind,
    ranking: Option<&SchedulerRankingOutcome>,
    skip_reason: Option<&'static str>,
    selected_order: Option<u32>,
) -> RoutingDecisionTrace {
    let candidate_kind = routing_candidate_kind(kind);
    let mut trace = crate::routing::build_routing_trace_seed(policy, client_api_format);
    trace.global_candidates.push(RoutingCandidateTrace {
        candidate_kind,
        provider_id: candidate.provider_id.clone(),
        endpoint_id: candidate.endpoint_id.clone(),
        model_id: candidate.model_id.clone(),
        key_id: match candidate_kind {
            CandidateKind::Provider => Some(candidate.key_id.clone()),
            CandidateKind::PoolGroup => None,
        },
        ranking_vector: rank_vector_for_candidate(
            &policy.ranking_overlay,
            &RoutingCandidateFacts {
                candidate_kind,
                provider_id: candidate.provider_id.clone(),
                endpoint_id: candidate.endpoint_id.clone(),
                model_id: candidate.model_id.clone(),
                key_id: match candidate_kind {
                    CandidateKind::Provider => Some(candidate.key_id.clone()),
                    CandidateKind::PoolGroup => None,
                },
                provider_priority: candidate.provider_priority,
                key_priority: candidate
                    .key_global_priority_for_format
                    .unwrap_or(candidate.key_internal_priority),
            },
        ),
        skip_reason: skip_reason.map(str::to_string),
        selected_order,
    });
    if let Some(ranking) = ranking {
        trace.runtime_facts.cache_affinity_hit = ranking.promoted_by == Some("cached_affinity");
    }
    trace
}

fn routing_candidate_kind(kind: LocalExecutionCandidateKind) -> CandidateKind {
    match kind {
        LocalExecutionCandidateKind::SingleKey => CandidateKind::Provider,
        LocalExecutionCandidateKind::PoolGroup => CandidateKind::PoolGroup,
    }
}

fn dispatch_sequence_from_attempts(
    attempts: Vec<LocalExecutionCandidateAttempt>,
) -> DispatchSequence<LocalExecutionCandidateAttempt> {
    DispatchSequence::new(
        attempts
            .into_iter()
            .map(|attempt| DispatchSequenceItem {
                candidate_index: attempt.candidate_index,
                retry_index: attempt.retry_index,
                candidate: attempt,
                mark: aether_dispatch_core::DispatchSequenceMark::Pending,
            })
            .collect(),
    )
}

fn next_attempt_from_dispatch_sequence(
    sequence: &mut DispatchSequence<LocalExecutionCandidateAttempt>,
) -> Option<LocalExecutionCandidateAttempt> {
    let attempt = sequence.next()?.candidate.clone();
    let _ = sequence.mark_succeeded();
    Some(attempt)
}

fn dispatch_sequence_exhausted(
    sequence: &mut DispatchSequence<LocalExecutionCandidateAttempt>,
) -> bool {
    sequence.next().is_none()
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

async fn persist_pool_group_exhaustion_skipped_candidate(
    context: Option<&PoolGroupExhaustionPersistenceContext>,
    candidate_index: u32,
    skipped: SkippedLocalExecutionCandidate,
) {
    let Some(context) = context else {
        return;
    };
    let skipped = attach_routing_trace_to_skipped_candidate(
        context.routing_policy.as_ref(),
        &context.client_api_format,
        candidate_index,
        skipped,
    );
    let extra_data =
        ai_candidate_extra_data_with_ranking(skipped.extra_data.clone(), skipped.ranking.as_ref());
    let candidate_id = Uuid::new_v4().to_string();
    persist_skipped_local_execution_candidate(
        &context.app,
        &context.trace_id,
        &context.user_id,
        &context.api_key_id,
        &skipped.candidate,
        candidate_index,
        candidate_id.as_str(),
        context.required_capabilities.as_ref(),
        skipped.skip_reason,
        extra_data,
        context.error_context,
        false,
    )
    .await;
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

    use aether_data::repository::auth::InMemoryAuthApiKeySnapshotRepository;
    use aether_data::repository::auth::StoredAuthApiKeySnapshot;
    use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::candidates::RequestCandidateStatus;
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };
    use aether_scheduler_core::{
        build_scheduler_affinity_cache_key_for_api_key_id,
        SchedulerMinimalCandidateSelectionCandidate, SchedulerPriorityMode, SchedulerRankingMode,
        SchedulerRankingOutcome,
    };
    use serde_json::json;

    use super::*;
    use crate::data::GatewayDataState;
    use crate::orchestration::LocalExecutionCandidateMetadata;
    use crate::scheduler::affinity::SCHEDULER_AFFINITY_TTL;

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
                upstream_metadata: None,
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
                pool_key_lease: None,
                scheduler_affinity_epoch: None,
            },
            ranking: None,
        }
    }

    fn sample_auth_snapshot() -> GatewayAuthApiKeySnapshot {
        GatewayAuthApiKeySnapshot::from_stored(
            StoredAuthApiKeySnapshot::new(
                "user-1".to_string(),
                "alice".to_string(),
                Some("alice@example.com".to_string()),
                "user".to_string(),
                "local".to_string(),
                true,
                false,
                None,
                None,
                None,
                "api-key-1".to_string(),
                Some("default".to_string()),
                true,
                false,
                false,
                Some(60),
                Some(5),
                Some(4_102_444_800),
                None,
                None,
                None,
            )
            .expect("stored auth snapshot should build"),
            0,
        )
    }

    fn no_extra_data(_: &EligibleLocalExecutionCandidate) -> Option<Value> {
        None
    }

    fn identity_skipped_candidate(
        candidate: SkippedLocalExecutionCandidate,
    ) -> SkippedLocalExecutionCandidate {
        candidate
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
            )
            .without_request_candidate_queue_for_tests();

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
        assert_eq!(
            stored[0]
                .extra_data
                .as_ref()
                .and_then(|value| value.get("dispatch_ref"))
                .and_then(|value| value.get("SingleKey"))
                .and_then(|value| value.get("key"))
                .and_then(|value| value.get("key_id")),
            Some(&json!("normal-key"))
        );
    }

    #[test]
    fn materialization_port_ignores_scheduler_affinity_when_cache_affinity_disabled() {
        let app = AppState::new().expect("state should build");
        let auth_snapshot = sample_auth_snapshot();
        let port = GatewayLocalCandidateMaterializationPort {
            state: PlannerAppState::new(&app),
            trace_id: "trace-affinity-disabled",
            client_api_format: "openai:chat",
            requested_model: Some("gpt-5"),
            auth_snapshot: Some(&auth_snapshot),
            client_session_affinity: None,
            required_capabilities: None,
            routing_policy: None,
            sticky_session_token: None,
            request_auth_channel: None,
            persistence_policy: LocalCandidatePersistencePolicy {
                available: LocalAvailableCandidatePersistenceContext {
                    user_id: "user-1",
                    api_key_id: "api-key-1",
                    required_capabilities: None,
                    error_context: "test available",
                },
                skipped: LocalSkippedCandidatePersistenceContext {
                    user_id: "user-1",
                    api_key_id: "api-key-1",
                    required_capabilities: None,
                    error_context: "test skipped",
                    record_runtime_miss_diagnostic: false,
                },
            },
            resolution_mode: LocalCandidateResolutionMode::Standard,
            scheduler_cache_affinity_enabled: false,
            build_available_extra_data: no_extra_data,
            decorate_skipped_candidate: identity_skipped_candidate,
        };
        let candidate = sample_eligible("key-a", None);
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");

        aether_ai_serving::AiCandidateMaterializationPort::remember_first_candidate_affinity(
            &port,
            &[candidate],
        );

        assert!(app
            .read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL)
            .is_none());
    }

    #[tokio::test]
    async fn resolved_candidate_page_cache_requires_fixed_order_or_explicit_affinity() {
        let app = AppState::new().expect("state should build");
        let auth_snapshot = sample_auth_snapshot();
        let mut page_cursor = LocalCandidatePreselectionPageCursor::new(
            PlannerAppState::new(&app),
            "openai:chat",
            "gpt-5",
            true,
            None,
            &auth_snapshot,
            None,
            None,
            None,
            false,
            LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModel,
            true,
            Some("trace-no-session-affinity"),
        )
        .await;
        page_cursor.mark_priority_page_emitted_for_tests();
        let cursor = RequestedModelAttemptPageCursor {
            state: PlannerAppState::new(&app),
            trace_id: "trace-no-session-affinity".to_string(),
            client_api_format: "openai:chat".to_string(),
            requested_model: "gpt-5".to_string(),
            auth_snapshot: auth_snapshot.clone(),
            client_session_affinity: None,
            required_capabilities: None,
            routing_policy: None,
            sticky_session_token: None,
            request_auth_channel: None,
            skipped_user_id: "user-1".to_string(),
            skipped_api_key_id: "api-key-1".to_string(),
            skipped_required_capabilities: None,
            skipped_error_context: "test skipped",
            record_runtime_miss_diagnostic: false,
            resolution_mode: LocalCandidateResolutionMode::Standard,
            decorate_skipped_candidate: Arc::new(identity_skipped_candidate),
            page_cursor,
            pending_items: VecDeque::new(),
            candidate_count: 0,
            next_candidate_index: 0,
            remembered_affinity: false,
            scheduler_cache_affinity_enabled: false,
            auth_api_key_concurrency_wait_deadline: None,
            deferred_error: None,
        };

        assert!(!should_cache_resolved_candidate_page(&cursor));

        let sticky_cursor = RequestedModelAttemptPageCursor {
            sticky_session_token: Some("sticky-token".to_string()),
            ..cursor
        };

        assert!(!should_cache_resolved_candidate_page(&sticky_cursor));

        let mut page_cursor = LocalCandidatePreselectionPageCursor::new(
            PlannerAppState::new(&app),
            "openai:chat",
            "gpt-5",
            true,
            None,
            &auth_snapshot,
            None,
            Some(&ClientSessionAffinity::from_session_key("chat-session-1")),
            None,
            false,
            LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModel,
            true,
            Some("trace-session-affinity"),
        )
        .await;
        page_cursor.mark_priority_page_emitted_for_tests();
        let cursor = RequestedModelAttemptPageCursor {
            client_session_affinity: Some(ClientSessionAffinity::from_session_key(
                "chat-session-1",
            )),
            page_cursor,
            sticky_session_token: None,
            ..sticky_cursor
        };

        assert!(should_cache_resolved_candidate_page(&cursor));

        let fixed_order_app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled().with_system_config_values_for_tests([(
                    "scheduling_mode".to_string(),
                    json!("fixed_order"),
                )]),
            );
        let mut page_cursor = LocalCandidatePreselectionPageCursor::new(
            PlannerAppState::new(&fixed_order_app),
            "openai:chat",
            "gpt-5",
            true,
            None,
            &auth_snapshot,
            None,
            None,
            None,
            false,
            LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModel,
            true,
            Some("trace-fixed-order"),
        )
        .await;
        page_cursor.mark_priority_page_emitted_for_tests();
        let cursor = RequestedModelAttemptPageCursor {
            state: PlannerAppState::new(&fixed_order_app),
            trace_id: "trace-fixed-order".to_string(),
            client_api_format: "openai:chat".to_string(),
            requested_model: "gpt-5".to_string(),
            auth_snapshot,
            client_session_affinity: None,
            required_capabilities: None,
            routing_policy: None,
            sticky_session_token: None,
            request_auth_channel: None,
            skipped_user_id: "user-1".to_string(),
            skipped_api_key_id: "api-key-1".to_string(),
            skipped_required_capabilities: None,
            skipped_error_context: "test skipped",
            record_runtime_miss_diagnostic: false,
            resolution_mode: LocalCandidateResolutionMode::Standard,
            decorate_skipped_candidate: Arc::new(identity_skipped_candidate),
            page_cursor,
            pending_items: VecDeque::new(),
            candidate_count: 0,
            next_candidate_index: 0,
            remembered_affinity: false,
            scheduler_cache_affinity_enabled: false,
            auth_api_key_concurrency_wait_deadline: None,
            deferred_error: None,
        };

        assert!(should_cache_resolved_candidate_page(&cursor));
    }

    #[tokio::test]
    async fn logical_materialization_does_not_persist_pool_group_representative() {
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    Arc::new(InMemoryAuthApiKeySnapshotRepository::default()),
                    Arc::new(InMemoryMinimalCandidateSelectionReadRepository::default()),
                    Arc::new(InMemoryProviderCatalogReadRepository::seed(
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                    )),
                    Arc::clone(&request_candidate_repository),
                    "test-encryption-key",
                ),
            )
            .without_request_candidate_queue_for_tests();
        let mut pool_group = sample_eligible("pool-group", None);
        pool_group.kind = LocalExecutionCandidateKind::PoolGroup;
        pool_group.transport = sample_transport(
            "pool-group",
            Some(json!({ "pool_advanced": { "scheduling_presets": [] } })),
        );

        let attempts = materialize_logical_local_execution_candidate_attempts(
            PlannerAppState::new(&app),
            "trace-logical-pool",
            LocalAvailableCandidatePersistenceContext {
                user_id: "user-1",
                api_key_id: "api-key-1",
                required_capabilities: None,
                error_context: "persist should not fail",
            },
            false,
            vec![pool_group, sample_eligible("normal-key", None)],
            None,
            "openai:chat",
            None,
            Some("gpt-5"),
            None,
            &|_| None,
        )
        .await;

        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].candidate_index, 1);
        assert_eq!(attempts[0].eligible.candidate.key_id, "normal-key");

        let stored = app
            .read_request_candidates_by_request_id("trace-logical-pool")
            .await
            .expect("request candidates should read");
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].key_id.as_deref(), Some("pool-group"));
        assert_eq!(stored[0].status, RequestCandidateStatus::Skipped);
        assert_eq!(
            stored[0].skip_reason.as_deref(),
            Some("pool_group_exhausted")
        );
        assert_eq!(stored[0].candidate_index, 0);
        assert_eq!(
            stored[0]
                .extra_data
                .as_ref()
                .and_then(|value| value.get("pool_group_exhaustion"))
                .and_then(|value| value.get("scanned_keys")),
            Some(&json!(0))
        );
        assert_eq!(stored[1].key_id.as_deref(), Some("normal-key"));
        assert_eq!(stored[1].candidate_index, 1);
        assert_eq!(
            stored[1]
                .extra_data
                .as_ref()
                .and_then(|value| value.get("dispatch_ref"))
                .and_then(|value| value.get("SingleKey"))
                .and_then(|value| value.get("key"))
                .and_then(|value| value.get("key_id")),
            Some(&json!("normal-key"))
        );
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
            )
            .without_request_candidate_queue_for_tests();
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
        assert_eq!(
            extra_data
                .get("dispatch_ref")
                .and_then(|value| value.get("SingleKey"))
                .and_then(|value| value.get("key"))
                .and_then(|value| value.get("key_id")),
            Some(&json!("ranked-key"))
        );
    }

    #[tokio::test]
    async fn dynamic_attempt_source_does_not_drain_unexecuted_single_keys() {
        let mut source = LocalExecutionCandidateAttemptSource {
            items: VecDeque::from([LocalExecutionCandidateAttemptSourceItem::Static {
                attempts: dispatch_sequence_from_attempts(
                    build_unpersisted_local_execution_candidate_attempts(
                        sample_eligible("normal-key", None),
                        0,
                    )
                    .into(),
                ),
            }]),
        };

        let first = source
            .next_attempt()
            .await
            .expect("first attempt read should succeed")
            .expect("first attempt should be available");
        assert_eq!(first.eligible.candidate.key_id, "normal-key");

        let remaining = source.drain_static_attempts();
        assert!(remaining.is_empty());
        assert!(source
            .next_attempt()
            .await
            .expect("remaining attempt read should succeed")
            .is_none());
    }

    #[tokio::test]
    async fn dynamic_pool_exhaustion_persists_group_skip_summary() {
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    Arc::new(InMemoryAuthApiKeySnapshotRepository::default()),
                    Arc::new(InMemoryMinimalCandidateSelectionReadRepository::default()),
                    Arc::new(InMemoryProviderCatalogReadRepository::seed(
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                    )),
                    Arc::clone(&request_candidate_repository),
                    "test-encryption-key",
                ),
            )
            .without_request_candidate_queue_for_tests();
        let mut pool_group = sample_eligible("pool-group", None);
        pool_group.kind = LocalExecutionCandidateKind::PoolGroup;
        pool_group.transport = sample_transport(
            "pool-group",
            Some(json!({ "pool_advanced": { "scheduling_presets": [] } })),
        );
        let cursor = PoolKeyCursor::new(
            PlannerAppState::new(&app),
            pool_group,
            None,
            Some("gpt-5"),
            None,
        );
        let pool_exhaustion_persistence = PoolGroupExhaustionPersistenceContext::new(
            app.clone(),
            "trace-dynamic-pool",
            LocalSkippedCandidatePersistenceContext {
                user_id: "user-1",
                api_key_id: "api-key-1",
                required_capabilities: None,
                error_context: "persist should not fail",
                record_runtime_miss_diagnostic: false,
            },
            "openai:chat",
            None,
        );
        let mut source = LocalExecutionCandidateAttemptSource {
            items: VecDeque::from([LocalExecutionCandidateAttemptSourceItem::Pool {
                cursor,
                candidate_index: 0,
                pending_attempts: DispatchSequence::new(Vec::new()),
                pool_exhaustion_persistence: Some(pool_exhaustion_persistence),
            }]),
        };

        assert!(source
            .next_attempt()
            .await
            .expect("pool attempt read should succeed")
            .is_none());

        let stored = app
            .read_request_candidates_by_request_id("trace-dynamic-pool")
            .await
            .expect("request candidates should read");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].status, RequestCandidateStatus::Skipped);
        assert_eq!(
            stored[0].skip_reason.as_deref(),
            Some("pool_group_exhausted")
        );
        assert_eq!(stored[0].candidate_index, 0);
        assert_eq!(stored[0].key_id.as_deref(), Some("pool-group"));
        assert_eq!(
            stored[0]
                .extra_data
                .as_ref()
                .and_then(|value| value.get("pool_group_exhaustion"))
                .and_then(|value| value.get("scanned_keys")),
            Some(&json!(0))
        );
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
            )
            .without_request_candidate_queue_for_tests();

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
