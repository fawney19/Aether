use aether_ai_serving::{
    run_ai_candidate_preselection, AiCandidatePreselectionOutcome, AiCandidatePreselectionPort,
};
use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;
use aether_routing_core::ResolvedRoutingPolicy;
use aether_runtime::ConcurrencyPermit;
use aether_scheduler_core::{
    enumerate_minimal_candidate_selection_with_model_directives, normalize_api_format,
    resolve_requested_global_model_name_with_model_directives_and_request_operation,
    row_supports_requested_model_with_model_directives_and_request_operation,
    ClientSessionAffinity, EnumerateMinimalCandidateSelectionInput,
    SchedulerMinimalCandidateSelectionCandidate,
};
use async_trait::async_trait;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use crate::ai_serving::planner::candidate_affinity_cache::has_explicit_session_affinity;
use crate::ai_serving::planner::candidate_resolution::SkippedLocalExecutionCandidate;
use crate::ai_serving::{GatewayAuthApiKeySnapshot, PlannerAppState};
use crate::cache::{
    candidate_page_cache_stale_ttl, candidate_page_cache_ttl_from_env,
    record_candidate_page_cache_follower_wait, record_candidate_page_cache_hit,
    record_candidate_page_cache_load, record_candidate_page_cache_miss,
    record_candidate_page_cache_none, record_candidate_row_page_cache_follower_wait,
    record_candidate_row_page_cache_hit, record_candidate_row_page_cache_load,
    record_candidate_row_page_cache_miss, record_candidate_row_page_cache_none, CacheLoadObserver,
    CandidatePageCacheKey, CandidatePageSnapshot, CandidateRowPageCacheKey,
};
use crate::clock::request_distribution_seed;
use crate::data::candidate_selection::{
    read_requested_model_rows_fast_path_page, requested_model_candidate_names,
    MinimalCandidateSelectionRowSource, RequestedModelCandidateRowsPage,
    REQUESTED_MODEL_CANDIDATE_PAGE_SIZE, REQUESTED_MODEL_MAX_SCANNED_ROWS,
};
use crate::scheduler::candidate::SchedulerSkippedCandidate;
use crate::scheduler::config::{SchedulerOrderingConfig, SchedulerSchedulingMode};
use crate::stage_metrics::observe_gateway_stage_ms;
use crate::GatewayError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalCandidatePreselectionKeyMode {
    ProviderEndpointKeyModel,
    ProviderEndpointKeyModelAndApiFormat,
}

impl LocalCandidatePreselectionKeyMode {
    pub(crate) fn cache_key_name(self) -> &'static str {
        match self {
            Self::ProviderEndpointKeyModel => "provider_endpoint_key_model",
            Self::ProviderEndpointKeyModelAndApiFormat => "provider_endpoint_key_model_api_format",
        }
    }
}

struct GatewayLocalCandidatePreselectionPort<'a> {
    state: PlannerAppState<'a>,
    client_api_format: &'a str,
    requested_model: &'a str,
    request_operation: Option<&'a str>,
    require_streaming: bool,
    required_capabilities: Option<&'a serde_json::Value>,
    auth_snapshot: &'a GatewayAuthApiKeySnapshot,
    routing_policy: Option<&'a ResolvedRoutingPolicy>,
    client_session_affinity: Option<&'a ClientSessionAffinity>,
    use_api_format_alias_match: bool,
    key_mode: LocalCandidatePreselectionKeyMode,
    candidate_api_formats: Vec<String>,
    model_directive_routing_models: BTreeMap<String, String>,
    ranking_seed: u64,
}

impl GatewayLocalCandidatePreselectionPort<'_> {
    fn model_directive_base_model(&self, candidate_api_format: &str) -> Option<&str> {
        self.model_directive_routing_models
            .get(&crate::ai_serving::normalize_api_format_alias(
                candidate_api_format,
            ))
            .map(String::as_str)
    }

    fn routing_model(&self, candidate_api_format: &str) -> &str {
        self.model_directive_base_model(candidate_api_format)
            .unwrap_or(self.requested_model)
    }
}

#[async_trait]
impl AiCandidatePreselectionPort for GatewayLocalCandidatePreselectionPort<'_> {
    type Candidate = SchedulerMinimalCandidateSelectionCandidate;
    type Skipped = SkippedLocalExecutionCandidate;
    type Error = GatewayError;

    fn candidate_api_formats(&self) -> Vec<String> {
        self.candidate_api_formats.clone()
    }

    fn candidate_api_format_matches_client(&self, candidate_api_format: &str) -> bool {
        if self.use_api_format_alias_match {
            crate::ai_serving::api_format_alias_matches(
                candidate_api_format,
                self.client_api_format,
            )
        } else {
            candidate_api_format == self.client_api_format
        }
    }

    async fn list_candidates_for_api_format(
        &self,
        candidate_api_format: &str,
        matches_client_format: bool,
    ) -> Result<(Vec<Self::Candidate>, Vec<Self::Skipped>), Self::Error> {
        let auth_snapshot = matches_client_format.then_some(self.auth_snapshot);
        let (candidates, skipped_candidates) = self
            .state
            .list_selectable_candidates_with_skip_reasons_for_request_operation(
                candidate_api_format,
                self.routing_model(candidate_api_format),
                self.require_streaming,
                self.required_capabilities,
                auth_snapshot,
                self.client_session_affinity,
                self.ranking_seed,
                false,
                self.request_operation,
            )
            .await?;

        Ok((
            candidates,
            skipped_candidates
                .into_iter()
                .map(skipped_local_execution_candidate_from_scheduler_skip)
                .collect(),
        ))
    }

    fn candidate_allowed(
        &self,
        candidate: &Self::Candidate,
        candidate_api_format: &str,
        matches_client_format: bool,
    ) -> bool {
        routing_policy_allows_provider(self.routing_policy, candidate)
            && (matches_client_format
                || auth_snapshot_allows_cross_format_candidate(
                    self.auth_snapshot,
                    self.requested_model,
                    self.model_directive_base_model(candidate_api_format),
                    candidate,
                ))
    }

    fn skipped_candidate_allowed(
        &self,
        skipped_candidate: &Self::Skipped,
        candidate_api_format: &str,
        matches_client_format: bool,
    ) -> bool {
        routing_policy_allows_provider(self.routing_policy, &skipped_candidate.candidate)
            && (matches_client_format
                || auth_snapshot_allows_cross_format_candidate(
                    self.auth_snapshot,
                    self.requested_model,
                    self.model_directive_base_model(candidate_api_format),
                    &skipped_candidate.candidate,
                ))
    }

    fn candidate_key(&self, candidate: &Self::Candidate) -> String {
        local_candidate_preselection_key(candidate, self.key_mode)
    }

    fn skipped_candidate_key(&self, skipped_candidate: &Self::Skipped) -> String {
        local_candidate_preselection_key(&skipped_candidate.candidate, self.key_mode)
    }
}

fn resolve_model_directive_routing_models(
    policy: &crate::system_features::ModelDirectivePolicySnapshot,
    candidate_api_formats: &[String],
    requested_model: &str,
) -> BTreeMap<String, String> {
    candidate_api_formats
        .iter()
        .filter_map(|api_format| {
            let api_format = crate::ai_serving::normalize_api_format_alias(api_format);
            let resolution = policy.resolve_reasoning(&api_format, Some(requested_model));
            resolution
                .base_model()
                .map(|base_model| (api_format, base_model.to_string()))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn preselect_local_execution_candidates_with_serving(
    state: PlannerAppState<'_>,
    model_directive_policy: &crate::system_features::ModelDirectivePolicySnapshot,
    client_api_format: &str,
    requested_model: &str,
    request_operation: Option<&str>,
    require_streaming: bool,
    required_capabilities: Option<&serde_json::Value>,
    auth_snapshot: &GatewayAuthApiKeySnapshot,
    routing_policy: Option<&ResolvedRoutingPolicy>,
    client_session_affinity: Option<&ClientSessionAffinity>,
    use_api_format_alias_match: bool,
    key_mode: LocalCandidatePreselectionKeyMode,
) -> Result<
    AiCandidatePreselectionOutcome<
        SchedulerMinimalCandidateSelectionCandidate,
        SkippedLocalExecutionCandidate,
    >,
    GatewayError,
> {
    let candidate_api_formats =
        crate::ai_serving::request_candidate_api_formats(client_api_format, require_streaming)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
    preselect_local_execution_candidates_for_api_formats_with_serving(
        state,
        model_directive_policy,
        client_api_format,
        requested_model,
        request_operation,
        require_streaming,
        required_capabilities,
        auth_snapshot,
        routing_policy,
        client_session_affinity,
        use_api_format_alias_match,
        key_mode,
        candidate_api_formats,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn preselect_local_execution_candidates_for_api_formats_with_serving(
    state: PlannerAppState<'_>,
    model_directive_policy: &crate::system_features::ModelDirectivePolicySnapshot,
    client_api_format: &str,
    requested_model: &str,
    request_operation: Option<&str>,
    require_streaming: bool,
    required_capabilities: Option<&serde_json::Value>,
    auth_snapshot: &GatewayAuthApiKeySnapshot,
    routing_policy: Option<&ResolvedRoutingPolicy>,
    client_session_affinity: Option<&ClientSessionAffinity>,
    use_api_format_alias_match: bool,
    key_mode: LocalCandidatePreselectionKeyMode,
    candidate_api_formats: Vec<String>,
) -> Result<
    AiCandidatePreselectionOutcome<
        SchedulerMinimalCandidateSelectionCandidate,
        SkippedLocalExecutionCandidate,
    >,
    GatewayError,
> {
    let model_directive_routing_models = resolve_model_directive_routing_models(
        model_directive_policy,
        &candidate_api_formats,
        requested_model,
    );
    let port = GatewayLocalCandidatePreselectionPort {
        state,
        client_api_format,
        requested_model,
        request_operation,
        require_streaming,
        required_capabilities,
        auth_snapshot,
        routing_policy,
        client_session_affinity,
        use_api_format_alias_match,
        key_mode,
        candidate_api_formats,
        model_directive_routing_models,
        ranking_seed: request_distribution_seed(),
    };

    run_ai_candidate_preselection(&port).await
}

pub(crate) struct LocalCandidatePreselectionPageCursor<'a> {
    state: PlannerAppState<'a>,
    trace_id: String,
    client_api_format: String,
    requested_model: String,
    request_operation: Option<String>,
    require_streaming: bool,
    required_capabilities: Option<serde_json::Value>,
    auth_snapshot: GatewayAuthApiKeySnapshot,
    routing_policy: Option<ResolvedRoutingPolicy>,
    client_session_affinity: Option<ClientSessionAffinity>,
    request_auth_channel: Option<String>,
    use_api_format_alias_match: bool,
    key_mode: LocalCandidatePreselectionKeyMode,
    allow_priority_page_cache: bool,
    candidate_api_formats: Vec<String>,
    model_directive_routing_models: BTreeMap<String, String>,
    model_directive_policy_cache_key: String,
    ordering_config: SchedulerOrderingConfig,
    ranking_seed: u64,
    priority_page_emitted: bool,
    deferred_pages_by_format: BTreeMap<
        String,
        VecDeque<
            AiCandidatePreselectionOutcome<
                SchedulerMinimalCandidateSelectionCandidate,
                SkippedLocalExecutionCandidate,
            >,
        >,
    >,
    format_index: usize,
    requested_name_indexes: BTreeMap<String, usize>,
    requested_name_offsets: BTreeMap<String, u32>,
    scanned_rows_by_format: BTreeMap<String, u32>,
    resolved_global_model_names: BTreeMap<String, String>,
    fallback_scanned_api_formats: BTreeSet<String>,
    seen_candidate_keys: BTreeSet<String>,
}

impl<'a> LocalCandidatePreselectionPageCursor<'a> {
    fn model_directive_base_model(&self, candidate_api_format: &str) -> Option<&str> {
        self.model_directive_routing_models
            .get(&crate::ai_serving::normalize_api_format_alias(
                candidate_api_format,
            ))
            .map(String::as_str)
    }

    fn routing_model(&self, candidate_api_format: &str) -> &str {
        self.model_directive_base_model(candidate_api_format)
            .unwrap_or(&self.requested_model)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn new(
        state: PlannerAppState<'a>,
        model_directive_policy: &crate::system_features::ModelDirectivePolicySnapshot,
        client_api_format: &str,
        requested_model: &str,
        request_operation: Option<&str>,
        require_streaming: bool,
        required_capabilities: Option<&serde_json::Value>,
        auth_snapshot: &GatewayAuthApiKeySnapshot,
        routing_policy: Option<&ResolvedRoutingPolicy>,
        client_session_affinity: Option<&ClientSessionAffinity>,
        request_auth_channel: Option<&str>,
        use_api_format_alias_match: bool,
        key_mode: LocalCandidatePreselectionKeyMode,
        allow_priority_page_cache: bool,
        trace_id: Option<&str>,
    ) -> Self {
        let candidate_api_formats =
            crate::ai_serving::request_candidate_api_formats(client_api_format, require_streaming)
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>();
        let model_directive_routing_models = resolve_model_directive_routing_models(
            model_directive_policy,
            &candidate_api_formats,
            requested_model,
        );

        let ordering_config =
            super::candidate_ranking::scheduler_ordering_config_for_routing_policy(
                state,
                routing_policy,
            )
            .await;

        Self {
            state,
            trace_id: trace_id.unwrap_or_default().to_string(),
            client_api_format: client_api_format.to_string(),
            requested_model: requested_model.to_string(),
            request_operation: request_operation.map(str::to_string),
            require_streaming,
            required_capabilities: required_capabilities.cloned(),
            auth_snapshot: auth_snapshot.clone(),
            routing_policy: routing_policy.cloned(),
            client_session_affinity: client_session_affinity.cloned(),
            request_auth_channel: request_auth_channel.map(str::to_string),
            use_api_format_alias_match,
            key_mode,
            allow_priority_page_cache,
            candidate_api_formats,
            model_directive_routing_models,
            model_directive_policy_cache_key: model_directive_policy.cache_key().to_string(),
            ordering_config,
            ranking_seed: request_distribution_seed(),
            priority_page_emitted: false,
            deferred_pages_by_format: BTreeMap::new(),
            format_index: 0,
            requested_name_indexes: BTreeMap::new(),
            requested_name_offsets: BTreeMap::new(),
            scanned_rows_by_format: BTreeMap::new(),
            resolved_global_model_names: BTreeMap::new(),
            fallback_scanned_api_formats: BTreeSet::new(),
            seen_candidate_keys: BTreeSet::new(),
        }
    }

    pub(crate) async fn next_page(
        &mut self,
    ) -> Result<
        Option<
            AiCandidatePreselectionOutcome<
                SchedulerMinimalCandidateSelectionCandidate,
                SkippedLocalExecutionCandidate,
            >,
        >,
        GatewayError,
    > {
        if !self.priority_page_emitted {
            self.priority_page_emitted = true;
            let priority_page = self.cached_next_priority_page().await?;
            if !priority_page.candidates.is_empty() || !priority_page.skipped_candidates.is_empty()
            {
                return Ok(Some(priority_page));
            }
        }

        while self.format_index < self.candidate_api_formats.len() {
            let candidate_api_format = self.candidate_api_formats[self.format_index].clone();
            if let Some(outcome) = self.pop_deferred_page(&candidate_api_format) {
                return Ok(Some(outcome));
            }
            let Some(outcome) = self
                .next_page_for_api_format_with_planning_gate(&candidate_api_format)
                .await?
            else {
                self.format_index += 1;
                continue;
            };
            if outcome.candidates.is_empty() && outcome.skipped_candidates.is_empty() {
                continue;
            }
            return Ok(Some(outcome));
        }
        Ok(None)
    }

    pub(crate) fn restart_scan(&mut self) {
        self.format_index = 0;
        self.requested_name_indexes.clear();
        self.requested_name_offsets.clear();
        self.scanned_rows_by_format.clear();
        self.resolved_global_model_names.clear();
        self.fallback_scanned_api_formats.clear();
        self.seen_candidate_keys.clear();
        self.priority_page_emitted = false;
        self.deferred_pages_by_format.clear();
    }

    pub(crate) fn resolved_page_cache_preselection_mode(&self) -> &'static str {
        self.key_mode.cache_key_name()
    }

    pub(crate) fn resolved_page_cache_request_operation(&self) -> Option<&str> {
        self.request_operation.as_deref()
    }

    pub(crate) fn resolved_page_cache_use_api_format_alias_match(&self) -> bool {
        self.use_api_format_alias_match
    }

    pub(crate) fn resolved_page_cache_model_directive_policy_hash(&self) -> &str {
        &self.model_directive_policy_cache_key
    }

    pub(crate) fn should_cache_current_priority_resolved_page(&self) -> bool {
        if !(self.priority_page_emitted
            && self.format_index == 0
            && self.deferred_pages_by_format.is_empty())
        {
            return false;
        }

        match self.ordering_config.scheduling_mode {
            SchedulerSchedulingMode::FixedOrder => true,
            SchedulerSchedulingMode::CacheAffinity => {
                has_explicit_session_affinity(self.client_session_affinity.as_ref())
            }
            SchedulerSchedulingMode::LoadBalance => false,
        }
    }

    fn should_cache_current_priority_page(&self) -> bool {
        self.allow_priority_page_cache && self.should_cache_current_priority_resolved_page()
    }

    #[cfg(test)]
    pub(crate) fn mark_priority_page_emitted_for_tests(&mut self) {
        self.priority_page_emitted = true;
    }

    async fn cached_next_priority_page(
        &mut self,
    ) -> Result<
        AiCandidatePreselectionOutcome<
            SchedulerMinimalCandidateSelectionCandidate,
            SkippedLocalExecutionCandidate,
        >,
        GatewayError,
    > {
        let page = if self.should_cache_current_priority_page() {
            self.cached_next_priority_page_snapshot().await?
        } else {
            self.next_priority_page_with_planning_gate().await?
        };
        self.remember_seen_candidates_from_page(&page);
        Ok(page)
    }

    async fn cached_next_priority_page_snapshot(
        &mut self,
    ) -> Result<
        AiCandidatePreselectionOutcome<
            SchedulerMinimalCandidateSelectionCandidate,
            SkippedLocalExecutionCandidate,
        >,
        GatewayError,
    > {
        let key = CandidatePageCacheKey::new(
            &self.requested_model,
            self.request_operation.as_deref(),
            &self.client_api_format,
            self.require_streaming,
            &self.auth_snapshot,
            self.required_capabilities.as_ref(),
            self.routing_policy.as_ref(),
            self.request_auth_channel.as_deref(),
            self.state.app().scheduler_affinity_epoch(),
            self.key_mode.cache_key_name(),
            self.use_api_format_alias_match,
            self.client_session_affinity.as_ref(),
            &self.model_directive_policy_cache_key,
        );
        let cache = self.state.app().candidate_page_cache.clone();
        let ttl = candidate_page_cache_ttl_from_env();
        let stale_ttl = candidate_page_cache_stale_ttl(ttl);
        let cached = cache
            .get_or_load_once_stale_while_refreshing(
                key,
                ttl,
                stale_ttl,
                || async {
                    let page = self.next_priority_page_with_planning_gate().await?;
                    Ok::<_, GatewayError>(Some(Arc::new(page) as Arc<CandidatePageSnapshot>))
                },
                CacheLoadObserver::new()
                    .on_hit(record_candidate_page_cache_hit)
                    .on_miss(record_candidate_page_cache_miss)
                    .on_load(record_candidate_page_cache_load)
                    .on_follower_wait(record_candidate_page_cache_follower_wait),
            )
            .await?;

        match cached {
            Some(snapshot) => {
                let page = snapshot.as_ref().clone();
                if page.candidates.is_empty() && page.skipped_candidates.is_empty() {
                    record_candidate_page_cache_none();
                }
                Ok(page)
            }
            None => {
                record_candidate_page_cache_none();
                Ok(AiCandidatePreselectionOutcome {
                    candidates: Vec::new(),
                    skipped_candidates: Vec::new(),
                })
            }
        }
    }

    fn remember_seen_candidates_from_page(
        &mut self,
        page: &AiCandidatePreselectionOutcome<
            SchedulerMinimalCandidateSelectionCandidate,
            SkippedLocalExecutionCandidate,
        >,
    ) {
        for candidate in &page.candidates {
            self.seen_candidate_keys
                .insert(local_candidate_preselection_key(candidate, self.key_mode));
        }
        for skipped_candidate in &page.skipped_candidates {
            self.seen_candidate_keys
                .insert(local_candidate_preselection_key(
                    &skipped_candidate.candidate,
                    self.key_mode,
                ));
        }
    }

    async fn next_priority_page(
        &mut self,
    ) -> Result<
        AiCandidatePreselectionOutcome<
            SchedulerMinimalCandidateSelectionCandidate,
            SkippedLocalExecutionCandidate,
        >,
        GatewayError,
    > {
        let mut priority_page = AiCandidatePreselectionOutcome {
            candidates: Vec::new(),
            skipped_candidates: Vec::new(),
        };

        for candidate_api_format in self.candidate_api_formats.clone() {
            let Some(outcome) = self.next_page_for_api_format(&candidate_api_format).await? else {
                continue;
            };
            if matches_client_api_format(
                self.use_api_format_alias_match,
                &candidate_api_format,
                &self.client_api_format,
            ) {
                priority_page.candidates.extend(outcome.candidates);
                priority_page
                    .skipped_candidates
                    .extend(outcome.skipped_candidates);
                continue;
            }

            let (promoted, deferred) = self
                .split_priority_conversion_page(&candidate_api_format, outcome)
                .await;
            priority_page.candidates.extend(promoted.candidates);
            priority_page
                .skipped_candidates
                .extend(promoted.skipped_candidates);
            self.defer_page(candidate_api_format, deferred);
        }

        Ok(priority_page)
    }

    async fn next_priority_page_with_planning_gate(
        &mut self,
    ) -> Result<
        AiCandidatePreselectionOutcome<
            SchedulerMinimalCandidateSelectionCandidate,
            SkippedLocalExecutionCandidate,
        >,
        GatewayError,
    > {
        let _permit = acquire_candidate_planning_gate(self.state, &self.trace_id).await?;
        self.next_priority_page().await
    }

    async fn next_page_for_api_format_with_planning_gate(
        &mut self,
        candidate_api_format: &str,
    ) -> Result<
        Option<
            AiCandidatePreselectionOutcome<
                SchedulerMinimalCandidateSelectionCandidate,
                SkippedLocalExecutionCandidate,
            >,
        >,
        GatewayError,
    > {
        let _permit = acquire_candidate_planning_gate(self.state, &self.trace_id).await?;
        self.next_page_for_api_format(candidate_api_format).await
    }

    async fn split_priority_conversion_page(
        &self,
        candidate_api_format: &str,
        outcome: AiCandidatePreselectionOutcome<
            SchedulerMinimalCandidateSelectionCandidate,
            SkippedLocalExecutionCandidate,
        >,
    ) -> (
        AiCandidatePreselectionOutcome<
            SchedulerMinimalCandidateSelectionCandidate,
            SkippedLocalExecutionCandidate,
        >,
        AiCandidatePreselectionOutcome<
            SchedulerMinimalCandidateSelectionCandidate,
            SkippedLocalExecutionCandidate,
        >,
    ) {
        let mut promoted = AiCandidatePreselectionOutcome {
            candidates: Vec::new(),
            skipped_candidates: Vec::new(),
        };
        let mut deferred = AiCandidatePreselectionOutcome {
            candidates: Vec::new(),
            skipped_candidates: Vec::new(),
        };

        for candidate in outcome.candidates {
            if self
                .cross_format_candidate_keeps_priority(&candidate, candidate_api_format)
                .await
            {
                promoted.candidates.push(candidate);
            } else {
                deferred.candidates.push(candidate);
            }
        }

        for skipped_candidate in outcome.skipped_candidates {
            if self
                .cross_format_candidate_keeps_priority(
                    &skipped_candidate.candidate,
                    candidate_api_format,
                )
                .await
            {
                promoted.skipped_candidates.push(skipped_candidate);
            } else {
                deferred.skipped_candidates.push(skipped_candidate);
            }
        }

        (promoted, deferred)
    }

    async fn cross_format_candidate_keeps_priority(
        &self,
        candidate: &SchedulerMinimalCandidateSelectionCandidate,
        candidate_api_format: &str,
    ) -> bool {
        if matches_client_api_format(
            self.use_api_format_alias_match,
            candidate_api_format,
            &self.client_api_format,
        ) {
            return false;
        }
        super::candidate_transport_ranking_facts::candidate_keeps_priority_on_conversion(
            self.state,
            candidate,
            self.ordering_config,
        )
        .await
    }

    fn defer_page(
        &mut self,
        candidate_api_format: String,
        outcome: AiCandidatePreselectionOutcome<
            SchedulerMinimalCandidateSelectionCandidate,
            SkippedLocalExecutionCandidate,
        >,
    ) {
        if outcome.candidates.is_empty() && outcome.skipped_candidates.is_empty() {
            return;
        }
        self.deferred_pages_by_format
            .entry(candidate_api_format)
            .or_default()
            .push_back(outcome);
    }

    fn pop_deferred_page(
        &mut self,
        candidate_api_format: &str,
    ) -> Option<
        AiCandidatePreselectionOutcome<
            SchedulerMinimalCandidateSelectionCandidate,
            SkippedLocalExecutionCandidate,
        >,
    > {
        loop {
            let pages = self
                .deferred_pages_by_format
                .get_mut(candidate_api_format)?;
            let outcome = pages.pop_front()?;
            if pages.is_empty() {
                self.deferred_pages_by_format.remove(candidate_api_format);
            }
            if !outcome.candidates.is_empty() || !outcome.skipped_candidates.is_empty() {
                return Some(outcome);
            }
        }
    }

    async fn next_page_for_api_format(
        &mut self,
        candidate_api_format: &str,
    ) -> Result<
        Option<
            AiCandidatePreselectionOutcome<
                SchedulerMinimalCandidateSelectionCandidate,
                SkippedLocalExecutionCandidate,
            >,
        >,
        GatewayError,
    > {
        let normalized_api_format = normalize_api_format(candidate_api_format);
        if normalized_api_format.is_empty() {
            return Ok(None);
        }
        let routing_model = self.routing_model(candidate_api_format).to_string();
        let requested_names = requested_model_candidate_names(&routing_model, false);
        let scanned = *self
            .scanned_rows_by_format
            .get(&normalized_api_format)
            .unwrap_or(&0);
        if scanned >= REQUESTED_MODEL_MAX_SCANNED_ROWS {
            return Ok(None);
        }

        loop {
            let requested_name_index = *self
                .requested_name_indexes
                .entry(normalized_api_format.clone())
                .or_insert(0);
            let Some(requested_name) = requested_names.get(requested_name_index) else {
                return self
                    .next_fallback_page_for_api_format(candidate_api_format, &normalized_api_format)
                    .await;
            };
            if requested_name.trim().is_empty() {
                self.requested_name_indexes
                    .insert(normalized_api_format.clone(), requested_name_index + 1);
                continue;
            }

            let offset_key = format!("{normalized_api_format}:{requested_name_index}");
            let offset = *self
                .requested_name_offsets
                .entry(offset_key.clone())
                .or_insert(0);
            let scanned = *self
                .scanned_rows_by_format
                .get(&normalized_api_format)
                .unwrap_or(&0);
            let remaining = REQUESTED_MODEL_MAX_SCANNED_ROWS.saturating_sub(scanned);
            if remaining == 0 {
                return Ok(None);
            }
            let limit = REQUESTED_MODEL_CANDIDATE_PAGE_SIZE.min(remaining);
            let page = self
                .read_requested_model_rows_fast_path_page_cached(
                    &normalized_api_format,
                    requested_name,
                    &routing_model,
                    offset,
                    limit,
                )
                .await?;
            self.scanned_rows_by_format.insert(
                normalized_api_format.clone(),
                scanned.saturating_add(page.scanned_rows),
            );
            self.requested_name_offsets
                .insert(offset_key, offset.saturating_add(limit));
            if page.end_of_requested_name {
                self.requested_name_indexes
                    .insert(normalized_api_format.clone(), requested_name_index + 1);
            }
            if page.scanned_rows == 0 {
                if requested_name_index + 1 >= requested_names.len() {
                    return self
                        .next_fallback_page_for_api_format(
                            candidate_api_format,
                            &normalized_api_format,
                        )
                        .await;
                }
                continue;
            }

            if let Some(outcome) = self
                .build_page_outcome_from_rows(
                    candidate_api_format,
                    &normalized_api_format,
                    page.rows,
                )
                .await?
            {
                return Ok(Some(outcome));
            }
        }
    }

    async fn read_requested_model_rows_fast_path_page_cached(
        &self,
        normalized_api_format: &str,
        requested_name: &str,
        routing_model: &str,
        offset: u32,
        limit: u32,
    ) -> Result<RequestedModelCandidateRowsPage, GatewayError> {
        let key = CandidateRowPageCacheKey::new(
            normalized_api_format,
            routing_model,
            requested_name,
            offset,
            limit,
            false,
        );
        let cache = self.state.app().candidate_row_page_cache.clone();
        let ttl = candidate_page_cache_ttl_from_env();
        let stale_ttl = candidate_page_cache_stale_ttl(ttl);
        let cached = cache
            .get_or_load_once_stale_while_refreshing(
                key,
                ttl,
                stale_ttl,
                || async {
                    let page = read_requested_model_rows_fast_path_page(
                        self.state.app().data.as_ref(),
                        normalized_api_format,
                        routing_model,
                        requested_name,
                        offset,
                        limit,
                        false,
                    )
                    .await
                    .map_err(|err| GatewayError::Internal(err.to_string()))?;
                    Ok::<_, GatewayError>(Some(Arc::new(page)))
                },
                CacheLoadObserver::new()
                    .on_hit(record_candidate_row_page_cache_hit)
                    .on_miss(record_candidate_row_page_cache_miss)
                    .on_load(record_candidate_row_page_cache_load)
                    .on_follower_wait(record_candidate_row_page_cache_follower_wait),
            )
            .await?;

        match cached {
            Some(page) => {
                if page.rows.is_empty() {
                    record_candidate_row_page_cache_none();
                }
                Ok(page.as_ref().clone())
            }
            None => {
                record_candidate_row_page_cache_none();
                Ok(RequestedModelCandidateRowsPage {
                    rows: Vec::new(),
                    scanned_rows: 0,
                    end_of_requested_name: true,
                })
            }
        }
    }

    async fn next_fallback_page_for_api_format(
        &mut self,
        candidate_api_format: &str,
        normalized_api_format: &str,
    ) -> Result<
        Option<
            AiCandidatePreselectionOutcome<
                SchedulerMinimalCandidateSelectionCandidate,
                SkippedLocalExecutionCandidate,
            >,
        >,
        GatewayError,
    > {
        if !self
            .fallback_scanned_api_formats
            .insert(normalized_api_format.to_string())
        {
            return Ok(None);
        }

        let routing_model = self.routing_model(candidate_api_format).to_string();
        let rows = self
            .state
            .app()
            .data
            .read_minimal_candidate_selection_rows_for_api_format(normalized_api_format)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
            .into_iter()
            .filter(|row| {
                row_supports_requested_model_with_model_directives_and_request_operation(
                    row,
                    &routing_model,
                    normalized_api_format,
                    false,
                    self.request_operation.as_deref(),
                )
            })
            .collect::<Vec<_>>();

        self.build_page_outcome_from_rows(candidate_api_format, normalized_api_format, rows)
            .await
    }

    async fn build_page_outcome_from_rows(
        &mut self,
        candidate_api_format: &str,
        normalized_api_format: &str,
        rows: Vec<StoredMinimalCandidateSelectionRow>,
    ) -> Result<
        Option<
            AiCandidatePreselectionOutcome<
                SchedulerMinimalCandidateSelectionCandidate,
                SkippedLocalExecutionCandidate,
            >,
        >,
        GatewayError,
    > {
        let mut rows = rows
            .into_iter()
            .filter(|row| {
                self.seen_candidate_keys.insert(format!(
                    "{}:{}:{}:{}",
                    row.endpoint_id, row.key_id, row.model_id, row.endpoint_api_format
                ))
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return Ok(None);
        }
        let routing_model = self.routing_model(candidate_api_format).to_string();
        let resolved_global_model_name =
            if let Some(value) = self.resolved_global_model_names.get(normalized_api_format) {
                value.clone()
            } else {
                let Some(value) =
                    resolve_requested_global_model_name_with_model_directives_and_request_operation(
                        &rows,
                        &routing_model,
                        normalized_api_format,
                        false,
                        self.request_operation.as_deref(),
                    )
                else {
                    return Ok(None);
                };
                self.resolved_global_model_names
                    .insert(normalized_api_format.to_string(), value.clone());
                value
            };
        rows.retain(|row| row.global_model_name == resolved_global_model_name);
        if rows.is_empty() {
            return Ok(None);
        }

        let auth_constraints = matches_client_api_format(
            self.use_api_format_alias_match,
            candidate_api_format,
            &self.client_api_format,
        )
        .then_some(&self.auth_snapshot)
        .map(crate::data::candidate_selection::auth_snapshot_constraints);
        let enumerated_candidates = enumerate_minimal_candidate_selection_with_model_directives(
            EnumerateMinimalCandidateSelectionInput {
                rows,
                normalized_api_format,
                request_operation: self.request_operation.as_deref(),
                requested_model_name: &routing_model,
                resolved_global_model_name: resolved_global_model_name.as_str(),
                require_streaming: self.require_streaming,
                required_capabilities: self.required_capabilities.as_ref(),
                auth_constraints: auth_constraints.as_ref(),
            },
            false,
        )
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let mut candidates = Vec::new();
        for candidate in enumerated_candidates {
            if !self.candidate_allowed_for_page(&candidate, candidate_api_format) {
                continue;
            }
            if !self
                .seen_candidate_keys
                .insert(local_candidate_preselection_key(&candidate, self.key_mode))
            {
                continue;
            }
            candidates.push(candidate);
        }

        let matches_client_format = matches_client_api_format(
            self.use_api_format_alias_match,
            candidate_api_format,
            &self.client_api_format,
        );
        let auth_snapshot = matches_client_format.then_some(&self.auth_snapshot);
        let (candidates, skipped_candidates) = self
            .state
            .list_selectable_enumerated_candidates_with_skip_reasons(
                candidate_api_format,
                &resolved_global_model_name,
                candidates,
                self.required_capabilities.as_ref(),
                auth_snapshot,
                self.client_session_affinity.as_ref(),
                self.ranking_seed,
            )
            .await?;
        let skipped_candidates = skipped_candidates
            .into_iter()
            .map(skipped_local_execution_candidate_from_scheduler_skip)
            .filter(|skipped_candidate| {
                self.skipped_candidate_allowed_for_page(skipped_candidate, candidate_api_format)
            })
            .collect::<Vec<_>>();

        Ok(Some(AiCandidatePreselectionOutcome {
            candidates,
            skipped_candidates,
        }))
    }

    fn candidate_allowed_for_page(
        &self,
        candidate: &SchedulerMinimalCandidateSelectionCandidate,
        candidate_api_format: &str,
    ) -> bool {
        routing_policy_allows_provider(self.routing_policy.as_ref(), candidate)
            && (matches_client_api_format(
                self.use_api_format_alias_match,
                candidate_api_format,
                &self.client_api_format,
            ) || auth_snapshot_allows_cross_format_candidate(
                &self.auth_snapshot,
                &self.requested_model,
                self.model_directive_base_model(candidate_api_format),
                candidate,
            ))
    }

    fn skipped_candidate_allowed_for_page(
        &self,
        skipped_candidate: &SkippedLocalExecutionCandidate,
        candidate_api_format: &str,
    ) -> bool {
        routing_policy_allows_provider(self.routing_policy.as_ref(), &skipped_candidate.candidate)
            && (matches_client_api_format(
                self.use_api_format_alias_match,
                candidate_api_format,
                &self.client_api_format,
            ) || auth_snapshot_allows_cross_format_candidate(
                &self.auth_snapshot,
                &self.requested_model,
                self.model_directive_base_model(candidate_api_format),
                &skipped_candidate.candidate,
            ))
    }
}

fn skipped_local_execution_candidate_from_scheduler_skip(
    skipped_candidate: SchedulerSkippedCandidate,
) -> SkippedLocalExecutionCandidate {
    SkippedLocalExecutionCandidate {
        candidate: skipped_candidate.candidate,
        skip_reason: skipped_candidate.skip_reason,
        transport: None,
        ranking: None,
        extra_data: None,
    }
}

fn local_candidate_preselection_key(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    mode: LocalCandidatePreselectionKeyMode,
) -> String {
    match mode {
        LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModel => format!(
            "{}:{}:{}:{}:{}",
            candidate.provider_id,
            candidate.endpoint_id,
            candidate.key_id,
            candidate.model_id,
            candidate.selected_provider_model_name,
        ),
        LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat => format!(
            "{}:{}:{}:{}:{}:{}",
            candidate.provider_id,
            candidate.endpoint_id,
            candidate.key_id,
            candidate.model_id,
            candidate.selected_provider_model_name,
            candidate.endpoint_api_format,
        ),
    }
}

async fn acquire_candidate_planning_gate(
    state: PlannerAppState<'_>,
    trace_id: &str,
) -> Result<Option<ConcurrencyPermit>, GatewayError> {
    let Some(gate) = state.app().candidate_planning_gate.as_ref() else {
        return Ok(None);
    };
    let budget = state
        .app()
        .frontdoor_runtime_guards
        .internal_gate_queue_budget;
    let gate_wait_started_at = std::time::Instant::now();
    match tokio::time::timeout(budget, gate.acquire()).await {
        Ok(Ok(permit)) => {
            observe_gateway_stage_ms(
                "candidate_planning_gate_wait",
                gate_wait_started_at.elapsed().as_millis() as u64,
            );
            Ok(Some(permit))
        }
        Ok(Err(err)) => Err(GatewayError::Internal(err.to_string())),
        Err(_) => Err(GatewayError::AdmissionTimeout {
            trace_id: trace_id.to_string(),
            gate: "gateway_candidate_planning",
            queue_budget_ms: budget.as_millis() as u64,
        }),
    }
}

fn matches_client_api_format(
    use_api_format_alias_match: bool,
    candidate_api_format: &str,
    client_api_format: &str,
) -> bool {
    if use_api_format_alias_match {
        crate::ai_serving::api_format_alias_matches(candidate_api_format, client_api_format)
    } else {
        candidate_api_format == client_api_format
    }
}

pub(crate) fn auth_snapshot_allows_cross_format_candidate(
    auth_snapshot: &GatewayAuthApiKeySnapshot,
    requested_model: &str,
    requested_base_model: Option<&str>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> bool {
    if let Some(allowed_providers) = auth_snapshot.effective_allowed_providers() {
        let provider_allowed = allowed_providers.iter().any(|value| {
            aether_scheduler_core::provider_matches_allowed_value(
                value,
                &candidate.provider_id,
                &candidate.provider_name,
                &candidate.provider_type,
            )
        });
        if !provider_allowed {
            return false;
        }
    }

    if let Some(allowed_models) = auth_snapshot.effective_allowed_models() {
        let model_allowed = allowed_models.iter().any(|value| {
            value == requested_model
                || value == &candidate.global_model_name
                || requested_base_model.is_some_and(|base_model| value == base_model)
        });
        if !model_allowed {
            return false;
        }
    }

    true
}

fn routing_policy_allows_provider(
    routing_policy: Option<&ResolvedRoutingPolicy>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> bool {
    match routing_policy {
        Some(policy) => policy
            .ranking_overlay
            .provider_allowed(candidate.provider_id.as_str()),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::GatewayDataState;
    use crate::AppState;
    use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::candidate_selection::{
        MinimalCandidateSelectionReadRepository, StoredProviderModelMapping,
    };
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use std::sync::Arc;

    fn unrestricted_auth_snapshot() -> GatewayAuthApiKeySnapshot {
        GatewayAuthApiKeySnapshot {
            user_id: "user-1".to_string(),
            username: "alice".to_string(),
            email: None,
            user_role: "user".to_string(),
            user_auth_source: "local".to_string(),
            user_is_active: true,
            user_is_deleted: false,
            user_rate_limit: None,
            user_allowed_providers: None,
            user_allowed_api_formats: None,
            user_allowed_models: None,
            api_key_id: "api-key-1".to_string(),
            api_key_name: Some("default".to_string()),
            api_key_is_active: true,
            api_key_is_locked: false,
            api_key_is_standalone: false,
            api_key_rate_limit: None,
            api_key_concurrent_limit: None,
            api_key_expires_at_unix_secs: None,
            api_key_allowed_providers: None,
            api_key_allowed_api_formats: None,
            api_key_allowed_models: None,
            api_key_ip_rules: None,
            currently_usable: true,
        }
    }

    #[tokio::test]
    async fn priority_page_cache_requires_fixed_order_or_explicit_affinity() {
        let repository: Arc<dyn MinimalCandidateSelectionReadRepository> =
            Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(
                Vec::<StoredMinimalCandidateSelectionRow>::new(),
            ));
        let data_state =
            GatewayDataState::with_minimal_candidate_selection_reader_for_tests(repository);
        let app = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(data_state);
        let auth_snapshot = unrestricted_auth_snapshot();
        let model_directive_policy =
            crate::system_features::ModelDirectivePolicySnapshot::load(&app).await;
        let mut cursor = LocalCandidatePreselectionPageCursor::new(
            PlannerAppState::new(&app),
            &model_directive_policy,
            "openai:chat",
            "gpt-5",
            None,
            true,
            None,
            &auth_snapshot,
            None,
            None,
            None,
            true,
            LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat,
            true,
            None,
        )
        .await;
        cursor.mark_priority_page_emitted_for_tests();

        cursor.ordering_config.scheduling_mode = SchedulerSchedulingMode::CacheAffinity;
        assert!(!cursor.should_cache_current_priority_resolved_page());

        cursor.client_session_affinity =
            Some(aether_scheduler_core::ClientSessionAffinity::from_session_key("session-1"));
        assert!(cursor.should_cache_current_priority_resolved_page());

        cursor.ordering_config.scheduling_mode = SchedulerSchedulingMode::FixedOrder;
        assert!(cursor.should_cache_current_priority_resolved_page());

        cursor.ordering_config.scheduling_mode = SchedulerSchedulingMode::LoadBalance;
        assert!(!cursor.should_cache_current_priority_resolved_page());
    }

    fn openai_responses_mapping_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-openai-responses-mapped-1".to_string(),
            provider_name: "openai".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-openai-responses-mapped-1".to_string(),
            endpoint_api_format: "openai:responses".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: "key-openai-responses-mapped-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "bearer".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:responses".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"openai:responses": 1})),
            model_id: "model-openai-responses-mapped-1".to_string(),
            global_model_id: "global-model-openai-responses-mapped-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: Some(vec!["gpt-5(?:\\.\\d+)?".to_string()]),
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-5-upstream".to_string(),
            model_provider_model_mappings: None,
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn standard_candidate_row(
        provider_id: &str,
        api_format: &str,
        provider_priority: i32,
    ) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: provider_id.to_string(),
            provider_name: provider_id.to_string(),
            provider_type: "custom".to_string(),
            provider_priority,
            provider_is_active: true,
            endpoint_id: format!("endpoint-{provider_id}"),
            endpoint_api_format: api_format.to_string(),
            endpoint_api_family: api_format.split(':').next().map(ToOwned::to_owned),
            endpoint_kind: api_format.split(':').nth(1).map(ToOwned::to_owned),
            endpoint_is_active: true,
            key_id: format!("key-{provider_id}"),
            key_name: format!("{provider_id}-key"),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec![api_format.to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 0,
            key_global_priority_by_format: None,
            model_id: format!("model-{provider_id}"),
            global_model_id: "global-model-gpt-5".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-5".to_string(),
            model_provider_model_mappings: None,
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn provider_catalog_for_standard_row(
        row: &StoredMinimalCandidateSelectionRow,
        keep_priority_on_conversion: bool,
    ) -> (
        StoredProviderCatalogProvider,
        StoredProviderCatalogEndpoint,
        StoredProviderCatalogKey,
    ) {
        let provider = StoredProviderCatalogProvider::new(
            row.provider_id.clone(),
            row.provider_name.clone(),
            Some("https://provider.example".to_string()),
            row.provider_type.clone(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            keep_priority_on_conversion,
            true,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .with_routing_fields(row.provider_priority);
        let endpoint = StoredProviderCatalogEndpoint::new(
            row.endpoint_id.clone(),
            row.provider_id.clone(),
            row.endpoint_api_format.clone(),
            row.endpoint_api_family.clone(),
            row.endpoint_kind.clone(),
            row.endpoint_is_active,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://provider.example/v1".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build");
        let key = StoredProviderCatalogKey::new(
            row.key_id.clone(),
            row.provider_id.clone(),
            row.key_name.clone(),
            row.key_auth_type.clone(),
            None,
            row.key_is_active,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!([row.endpoint_api_format.clone()])),
            "plain-upstream-key".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build");
        (provider, endpoint, key)
    }

    fn opg_deepseek_row(
        endpoint_id: &str,
        api_format: &str,
        key_id: &str,
        key_name: &str,
        key_allowed_models: Vec<&str>,
        key_internal_priority: i32,
    ) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-opg".to_string(),
            provider_name: "OpenCode Go".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 1,
            provider_is_active: true,
            endpoint_id: endpoint_id.to_string(),
            endpoint_api_format: api_format.to_string(),
            endpoint_api_family: None,
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: key_id.to_string(),
            key_name: key_name.to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec![api_format.to_string()]),
            key_allowed_models: Some(
                key_allowed_models
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            key_capabilities: None,
            key_internal_priority,
            key_global_priority_by_format: None,
            model_id: "model-opg-deepseek-v4-pro".to_string(),
            global_model_id: "global-model-deepseek-v4-pro".to_string(),
            global_model_name: "deepseek-v4-pro".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "deepseek-v4-pro".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "deepseek-v4-pro".to_string(),
                priority: 1,
                api_formats: None,
                endpoint_ids: Some(vec!["endpoint-opg-openai".to_string()]),
                operations: None,
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    #[tokio::test]
    async fn paged_preselection_falls_back_to_format_scan_for_directive_mapping_match() {
        let repository: Arc<dyn MinimalCandidateSelectionReadRepository> =
            Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed([
                openai_responses_mapping_row(),
            ]));
        let data_state =
            GatewayDataState::with_minimal_candidate_selection_reader_for_tests(repository)
                .with_system_config_values_for_tests([(
                    crate::system_features::ENABLE_MODEL_DIRECTIVES_CONFIG_KEY.to_string(),
                    serde_json::json!(true),
                )]);
        let app = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(data_state);
        let auth_snapshot = unrestricted_auth_snapshot();
        let model_directive_policy =
            crate::system_features::ModelDirectivePolicySnapshot::load(&app).await;
        let mut cursor = LocalCandidatePreselectionPageCursor::new(
            PlannerAppState::new(&app),
            &model_directive_policy,
            "claude:messages",
            "gpt-5.5-xhigh",
            None,
            false,
            None,
            &auth_snapshot,
            None,
            None,
            None,
            true,
            LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat,
            true,
            None,
        )
        .await;

        let page = cursor
            .next_page()
            .await
            .expect("preselection should succeed")
            .expect("mapping fallback should find a provider");

        assert_eq!(page.skipped_candidates.len(), 0);
        assert_eq!(page.candidates.len(), 1);
        assert_eq!(page.candidates[0].endpoint_api_format, "openai:responses");
        assert_eq!(page.candidates[0].global_model_name, "gpt-5");
        assert_eq!(
            page.candidates[0].selected_provider_model_name,
            "gpt-5-upstream"
        );
    }

    #[tokio::test]
    async fn paged_preselection_prefers_operation_scoped_mapping_for_compaction() {
        let mut row = openai_responses_mapping_row();
        row.global_model_mappings = None;
        row.global_model_name = "gpt-5.6-sol".to_string();
        row.model_provider_model_name = "gpt-5.6-sol".to_string();
        row.model_provider_model_mappings = Some(vec![
            StoredProviderModelMapping {
                name: "gpt-5.6-sol".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:responses".to_string()]),
                endpoint_ids: None,
                operations: None,
            },
            StoredProviderModelMapping {
                name: "gpt-5.6-terra".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:responses".to_string()]),
                endpoint_ids: None,
                operations: Some(vec!["compact".to_string()]),
            },
        ]);
        let repository: Arc<dyn MinimalCandidateSelectionReadRepository> =
            Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed([row]));
        let data_state =
            GatewayDataState::with_minimal_candidate_selection_reader_for_tests(repository);
        let app = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(data_state);
        let auth_snapshot = unrestricted_auth_snapshot();
        let model_directive_policy =
            crate::system_features::ModelDirectivePolicySnapshot::load(&app).await;
        let mut cursor = LocalCandidatePreselectionPageCursor::new(
            PlannerAppState::new(&app),
            &model_directive_policy,
            "openai:responses",
            "gpt-5.6-sol",
            Some("compact"),
            false,
            None,
            &auth_snapshot,
            None,
            None,
            None,
            true,
            LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat,
            true,
            None,
        )
        .await;

        let page = cursor
            .next_page()
            .await
            .expect("preselection should succeed")
            .expect("compact mapping should find a provider");

        assert_eq!(page.candidates.len(), 1);
        assert_eq!(
            page.candidates[0].selected_provider_model_name,
            "gpt-5.6-terra"
        );
    }

    #[tokio::test]
    async fn custom_policy_suffix_uses_the_same_base_model_for_candidate_selection() {
        let mut row = openai_responses_mapping_row();
        row.global_model_name = "deployment-alias".to_string();
        row.global_model_mappings = None;
        row.model_provider_model_name = "gpt-5.6-sol".to_string();
        let repository: Arc<dyn MinimalCandidateSelectionReadRepository> =
            Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed([row]));
        let data_state =
            GatewayDataState::with_minimal_candidate_selection_reader_for_tests(repository)
                .with_system_config_values_for_tests([
                    (
                        crate::system_features::ENABLE_MODEL_DIRECTIVES_CONFIG_KEY.to_string(),
                        serde_json::json!(true),
                    ),
                    (
                        crate::system_features::MODEL_DIRECTIVES_CONFIG_KEY.to_string(),
                        serde_json::json!({
                            "reasoning_effort": {
                                "api_formats": {
                                    "openai:responses": {
                                        "suffixes": ["VendorFuture"],
                                        "mappings": {
                                            "VendorFuture": {
                                                "reasoning": { "context": "all_turns" }
                                            }
                                        }
                                    }
                                }
                            }
                        }),
                    ),
                ]);
        let app = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(data_state);
        let auth_snapshot = unrestricted_auth_snapshot();
        let model_directive_policy =
            crate::system_features::ModelDirectivePolicySnapshot::load(&app).await;
        let mut cursor = LocalCandidatePreselectionPageCursor::new(
            PlannerAppState::new(&app),
            &model_directive_policy,
            "openai:responses",
            "deployment-alias-VendorFuture",
            None,
            false,
            None,
            &auth_snapshot,
            None,
            None,
            None,
            true,
            LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat,
            true,
            None,
        )
        .await;

        let page = cursor
            .next_page()
            .await
            .expect("preselection should succeed")
            .expect("custom directive base model should resolve a candidate");

        assert_eq!(page.candidates.len(), 1);
        assert_eq!(page.candidates[0].global_model_name, "deployment-alias");
        assert_eq!(
            page.candidates[0].selected_provider_model_name,
            "gpt-5.6-sol"
        );
    }

    #[tokio::test]
    async fn claude_request_uses_cross_format_key_when_same_provider_messages_key_lacks_model() {
        let repository: Arc<dyn MinimalCandidateSelectionReadRepository> =
            Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed([
                opg_deepseek_row(
                    "endpoint-opg-claude",
                    "claude:messages",
                    "key-opg-messages",
                    "OPG Key Messages",
                    vec!["glm-5", "glm-5.1", "minimax-m2.5", "minimax-m2.7"],
                    1,
                ),
                opg_deepseek_row(
                    "endpoint-opg-openai",
                    "openai:chat",
                    "key-opg-completions",
                    "OPG Key Completions",
                    vec!["deepseek-v4-pro", "glm-5", "glm-5.1", "minimax-m2.7"],
                    10,
                ),
            ]));
        let data_state =
            GatewayDataState::with_minimal_candidate_selection_reader_for_tests(repository);
        let app = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(data_state);
        let auth_snapshot = unrestricted_auth_snapshot();
        let model_directive_policy =
            crate::system_features::ModelDirectivePolicySnapshot::load(&app).await;
        let mut cursor = LocalCandidatePreselectionPageCursor::new(
            PlannerAppState::new(&app),
            &model_directive_policy,
            "claude:messages",
            "deepseek-v4-pro",
            None,
            false,
            None,
            &auth_snapshot,
            None,
            None,
            None,
            true,
            LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat,
            true,
            None,
        )
        .await;

        let page = cursor
            .next_page()
            .await
            .expect("preselection should succeed")
            .expect("openai chat candidate should be found via conversion");

        assert_eq!(page.skipped_candidates.len(), 0);
        assert_eq!(page.candidates.len(), 1);
        assert_eq!(page.candidates[0].endpoint_api_format, "openai:chat");
        assert_eq!(page.candidates[0].key_name, "OPG Key Completions");
        assert_eq!(
            page.candidates[0].selected_provider_model_name,
            "deepseek-v4-pro"
        );
    }

    #[tokio::test]
    async fn first_page_includes_cross_format_candidates_that_keep_conversion_priority() {
        let same_format = standard_candidate_row("provider-claude", "claude:messages", 10);
        let keep_priority_cross =
            standard_candidate_row("provider-openai-responses-keep", "openai:responses", 0);
        let regular_cross =
            standard_candidate_row("provider-openai-responses-regular", "openai:responses", 1);
        let candidate_repository: Arc<dyn MinimalCandidateSelectionReadRepository> =
            Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed([
                same_format.clone(),
                keep_priority_cross.clone(),
                regular_cross.clone(),
            ]));
        let catalog_items = [
            provider_catalog_for_standard_row(&same_format, false),
            provider_catalog_for_standard_row(&keep_priority_cross, true),
            provider_catalog_for_standard_row(&regular_cross, false),
        ];
        let provider_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            catalog_items
                .iter()
                .map(|(provider, _, _)| provider.clone())
                .collect(),
            catalog_items
                .iter()
                .map(|(_, endpoint, _)| endpoint.clone())
                .collect(),
            catalog_items
                .iter()
                .map(|(_, _, key)| key.clone())
                .collect(),
        ));
        let data_state =
            GatewayDataState::with_provider_catalog_and_minimal_candidate_selection_for_tests(
                provider_repository,
                candidate_repository,
            )
            .with_encryption_key_for_tests("development-key");
        let app = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(data_state);
        let auth_snapshot = unrestricted_auth_snapshot();
        let model_directive_policy =
            crate::system_features::ModelDirectivePolicySnapshot::load(&app).await;
        let mut cursor = LocalCandidatePreselectionPageCursor::new(
            PlannerAppState::new(&app),
            &model_directive_policy,
            "claude:messages",
            "gpt-5",
            None,
            false,
            None,
            &auth_snapshot,
            None,
            None,
            None,
            true,
            LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat,
            true,
            None,
        )
        .await;

        let first_page = cursor
            .next_page()
            .await
            .expect("preselection should succeed")
            .expect("same-format and keep-priority conversion candidates should share first page");

        assert_eq!(
            first_page
                .candidates
                .iter()
                .map(|candidate| candidate.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["provider-claude", "provider-openai-responses-keep"]
        );

        let (ranked, skipped) =
            super::super::candidate_resolution::resolve_and_rank_logical_local_execution_candidates(
                PlannerAppState::new(&app),
                first_page.candidates,
                "claude:messages",
                Some("gpt-5"),
                Some(&auth_snapshot),
                None,
                None,
                None,
                None,
                None,
                aether_ai_serving::AiCandidateResolutionMode::Standard,
            )
            .await;

        assert!(skipped.is_empty());
        assert_eq!(
            ranked
                .iter()
                .map(|candidate| candidate.candidate.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["provider-openai-responses-keep", "provider-claude"]
        );

        let second_page = cursor
            .next_page()
            .await
            .expect("preselection should continue")
            .expect("regular conversion candidate should remain in a later page");
        assert_eq!(
            second_page
                .candidates
                .iter()
                .map(|candidate| candidate.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["provider-openai-responses-regular"]
        );
    }
}
