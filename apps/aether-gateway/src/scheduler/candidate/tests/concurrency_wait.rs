use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use aether_data::DataLayerError;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredPoolKeyCandidateRowsQuery,
    StoredRequestedModelCandidateRowsQuery,
};
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::quota::StoredProviderQuotaSnapshot;
use aether_scheduler_core::{SchedulerAffinityTarget, SchedulerMinimalCandidateSelectionCandidate};
use async_trait::async_trait;
use tokio::sync::Notify;

use crate::data::auth::GatewayAuthApiKeySnapshot;
use crate::data::candidate_selection::MinimalCandidateSelectionRowSource;
use crate::scheduler::config::SchedulerOrderingConfig;
use crate::scheduler::state::SchedulerRuntimeState;
use crate::GatewayError;

use super::super::{
    is_exact_all_skipped_by_auth_limit,
    list_selectable_candidates_for_required_capability_without_requested_model_with_auth_limit_signal,
    list_selectable_candidates_with_skip_reasons, select_with_auth_concurrency_wait,
    SchedulerSkippedCandidate,
};
use super::support::{sample_auth_snapshot, sample_provider, sample_row};

const POLL_INTERVAL: Duration = Duration::from_millis(2);

enum RecentReadAction {
    Keep,
    Release,
    ReleaseAndReplaceCandidate,
    ReleaseAndRemoveCandidates,
    Fail,
}

struct CountingState {
    rows: Mutex<Vec<StoredMinimalCandidateSelectionRow>>,
    recent: Mutex<Vec<StoredRequestCandidate>>,
    recent_actions: Mutex<VecDeque<RecentReadAction>>,
    row_reads: AtomicUsize,
    format_reads: AtomicUsize,
    provider_reads: AtomicUsize,
    key_reads: AtomicUsize,
    quota_reads: AtomicUsize,
    recent_reads: AtomicUsize,
    row_error_at: Option<usize>,
    first_row_delay: Duration,
    poll_observed: Notify,
}

impl CountingState {
    fn blocked() -> Self {
        Self {
            rows: Mutex::new(vec![sample_row()]),
            recent: Mutex::new(vec![active_candidate()]),
            recent_actions: Mutex::new(VecDeque::new()),
            row_reads: AtomicUsize::new(0),
            format_reads: AtomicUsize::new(0),
            provider_reads: AtomicUsize::new(0),
            key_reads: AtomicUsize::new(0),
            quota_reads: AtomicUsize::new(0),
            recent_reads: AtomicUsize::new(0),
            row_error_at: None,
            first_row_delay: Duration::ZERO,
            poll_observed: Notify::new(),
        }
    }

    fn on_recent_reads(self, actions: impl IntoIterator<Item = RecentReadAction>) -> Self {
        *self.recent_actions.lock().unwrap() = actions.into_iter().collect();
        self
    }
}

fn active_candidate() -> StoredRequestCandidate {
    let now_ms = i64::try_from(crate::clock::current_unix_secs() * 1000).unwrap();
    StoredRequestCandidate::new(
        "active-candidate".to_string(),
        "active-request".to_string(),
        Some("user-1".to_string()),
        Some("api-key-1".to_string()),
        None,
        None,
        0,
        0,
        Some("provider-1".to_string()),
        Some("endpoint-1".to_string()),
        Some("key-1".to_string()),
        RequestCandidateStatus::Streaming,
        None,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        now_ms,
        Some(now_ms),
        None,
    )
    .unwrap()
}

fn limited_auth() -> GatewayAuthApiKeySnapshot {
    let mut auth = sample_auth_snapshot("api-key-1");
    auth.api_key_concurrent_limit = Some(1);
    auth
}

#[async_trait]
impl MinimalCandidateSelectionRowSource for CountingState {
    async fn read_minimal_candidate_selection_rows_for_api_format_and_global_model(
        &self,
        _api_format: &str,
        _global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        panic!("requested-model selection should use its paged query")
    }

    async fn read_minimal_candidate_selection_rows_for_api_format_and_requested_model(
        &self,
        _api_format: &str,
        _requested_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        panic!("requested-model selection should use its paged query")
    }

    async fn read_minimal_candidate_selection_rows_for_api_format_and_requested_model_page(
        &self,
        query: &StoredRequestedModelCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let read = self.row_reads.fetch_add(1, Ordering::SeqCst) + 1;
        if read == 1 && !self.first_row_delay.is_zero() {
            tokio::time::sleep(self.first_row_delay).await;
        }
        if self.row_error_at == Some(read) {
            return Err(DataLayerError::Postgres(
                "candidate query failed".to_string(),
            ));
        }
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|row| {
                row.endpoint_api_format == query.api_format
                    && row.global_model_name == query.requested_model_name
            })
            .skip(query.offset as usize)
            .take(query.limit as usize)
            .cloned()
            .collect())
    }

    async fn read_minimal_candidate_selection_rows_for_api_format(
        &self,
        _api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.format_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.rows.lock().unwrap().clone())
    }

    async fn read_pool_key_candidate_rows_for_group(
        &self,
        _query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        panic!("the test has no provider pool")
    }
}

#[async_trait]
impl SchedulerRuntimeState for CountingState {
    async fn read_provider_quota_snapshot(
        &self,
        _provider_id: &str,
    ) -> Result<Option<StoredProviderQuotaSnapshot>, GatewayError> {
        self.quota_reads.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }

    async fn read_provider_catalog_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, GatewayError> {
        self.provider_reads.fetch_add(1, Ordering::SeqCst);
        Ok(provider_ids
            .iter()
            .map(|id| sample_provider(id, None))
            .collect())
    }

    async fn read_provider_catalog_keys_by_ids(
        &self,
        _key_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, GatewayError> {
        self.key_reads.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }

    async fn read_recent_request_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, GatewayError> {
        assert_eq!(
            limit, 128,
            "polls must use the same sample as full selection"
        );
        let read = self.recent_reads.fetch_add(1, Ordering::SeqCst) + 1;
        let action = self
            .recent_actions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(RecentReadAction::Keep);
        if matches!(action, RecentReadAction::Fail) {
            return Err(GatewayError::Internal(
                "recent candidates failed".to_string(),
            ));
        }
        if matches!(
            action,
            RecentReadAction::Release
                | RecentReadAction::ReleaseAndReplaceCandidate
                | RecentReadAction::ReleaseAndRemoveCandidates
        ) {
            for candidate in self.recent.lock().unwrap().iter_mut() {
                candidate.status = RequestCandidateStatus::Success;
                candidate.finished_at_unix_ms = Some(crate::clock::current_unix_secs() * 1000);
            }
        }
        match action {
            RecentReadAction::ReleaseAndReplaceCandidate => {
                self.rows.lock().unwrap()[0].key_id = "replacement-key".to_string();
            }
            RecentReadAction::ReleaseAndRemoveCandidates => self.rows.lock().unwrap().clear(),
            _ => {}
        }
        if read > 1 {
            self.poll_observed.notify_one();
        }
        Ok(self.recent.lock().unwrap().clone())
    }

    fn provider_key_rpm_reset_at(&self, _key_id: &str, _now_unix_secs: u64) -> Option<u64> {
        None
    }

    fn read_cached_scheduler_affinity_target(
        &self,
        _cache_key: &str,
        _ttl: Duration,
    ) -> Option<SchedulerAffinityTarget> {
        None
    }

    fn scheduler_affinity_epoch(&self) -> u64 {
        0
    }

    fn remember_scheduler_affinity_target(
        &self,
        _cache_key: &str,
        _target: SchedulerAffinityTarget,
        _ttl: Duration,
        _max_entries: usize,
    ) {
    }

    fn remember_scheduler_affinity_target_for_epoch(
        &self,
        _cache_key: &str,
        _target: SchedulerAffinityTarget,
        _ttl: Duration,
        _max_entries: usize,
        _expected_epoch: Option<u64>,
    ) -> bool {
        true
    }
}

type Selection = (
    Vec<SchedulerMinimalCandidateSelectionCandidate>,
    Vec<SchedulerSkippedCandidate>,
);

async fn select_requested_model(
    state: &CountingState,
    auth: Option<&GatewayAuthApiKeySnapshot>,
    timeout: Duration,
) -> Result<Selection, GatewayError> {
    select_with_auth_concurrency_wait(
        state,
        auth,
        crate::clock::current_unix_secs(),
        timeout,
        POLL_INTERVAL,
        |now| async move {
            let result = list_selectable_candidates_with_skip_reasons(
                state,
                state,
                "openai:chat",
                "gpt-4.1",
                false,
                None,
                auth,
                None,
                now,
                false,
                SchedulerOrderingConfig::default(),
            )
            .await?;
            let blocked = is_exact_all_skipped_by_auth_limit(&result.0, &result.1);
            Ok((result, blocked))
        },
    )
    .await
}

#[tokio::test]
async fn concurrent_blocked_selectors_only_prepare_at_start_and_deadline() {
    let state = CountingState::blocked();
    let auth = limited_auth();
    let outcomes = futures_util::future::join_all(
        (0..8).map(|_| select_requested_model(&state, Some(&auth), Duration::from_millis(80))),
    )
    .await;

    for outcome in outcomes {
        let (selected, skipped) = outcome.unwrap();
        assert!(is_exact_all_skipped_by_auth_limit(&selected, &skipped));
    }
    assert_eq!(state.row_reads.load(Ordering::SeqCst), 16);
    assert_eq!(state.provider_reads.load(Ordering::SeqCst), 32);
    assert_eq!(state.key_reads.load(Ordering::SeqCst), 16);
    assert_eq!(state.quota_reads.load(Ordering::SeqCst), 16);
    assert!(state.recent_reads.load(Ordering::SeqCst) > 16);
    assert_eq!(state.format_reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn released_auth_slot_rebuilds_changed_candidates() {
    let state = CountingState::blocked().on_recent_reads([
        RecentReadAction::Keep,
        RecentReadAction::Keep,
        RecentReadAction::ReleaseAndReplaceCandidate,
    ]);
    let auth = limited_auth();
    let (selected, skipped) = select_requested_model(&state, Some(&auth), Duration::from_secs(1))
        .await
        .unwrap();

    assert!(skipped.is_empty());
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].key_id, "replacement-key");
    assert_eq!(state.row_reads.load(Ordering::SeqCst), 2);
    assert_eq!(state.recent_reads.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn released_auth_slot_does_not_reuse_removed_candidates() {
    let state = CountingState::blocked().on_recent_reads([
        RecentReadAction::Keep,
        RecentReadAction::ReleaseAndRemoveCandidates,
    ]);
    let (selected, skipped) =
        select_requested_model(&state, Some(&limited_auth()), Duration::from_secs(1))
            .await
            .unwrap();

    assert!(selected.is_empty());
    assert!(skipped.is_empty());
    assert_eq!(state.row_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn lightweight_poll_errors_are_propagated_without_another_full_query() {
    let state =
        CountingState::blocked().on_recent_reads([RecentReadAction::Keep, RecentReadAction::Fail]);
    let error = select_requested_model(&state, Some(&limited_auth()), Duration::from_secs(1))
        .await
        .unwrap_err();

    assert!(
        matches!(error, GatewayError::Internal(message) if message == "recent candidates failed")
    );
    assert_eq!(state.row_reads.load(Ordering::SeqCst), 1);
    assert_eq!(state.recent_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn recovered_selection_errors_are_propagated() {
    let mut state = CountingState::blocked()
        .on_recent_reads([RecentReadAction::Keep, RecentReadAction::Release]);
    state.row_error_at = Some(2);
    let error = select_requested_model(&state, Some(&limited_auth()), Duration::from_secs(1))
        .await
        .unwrap_err();

    assert!(
        matches!(error, GatewayError::Internal(message) if message.contains("candidate query failed"))
    );
    assert_eq!(state.row_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn first_selection_time_consumes_the_wait_budget() {
    let mut state = CountingState::blocked();
    state.first_row_delay = Duration::from_millis(30);
    let (selected, skipped) =
        select_requested_model(&state, Some(&limited_auth()), Duration::from_millis(5))
            .await
            .unwrap();

    assert!(is_exact_all_skipped_by_auth_limit(&selected, &skipped));
    assert_eq!(state.row_reads.load(Ordering::SeqCst), 1);
    assert_eq!(state.recent_reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_stops_polling_and_candidate_queries() {
    let state = CountingState::blocked();
    let auth = limited_auth();
    let mut selection = Box::pin(select_requested_model(
        &state,
        Some(&auth),
        Duration::from_secs(1),
    ));
    tokio::select! {
        result = &mut selection => panic!("selection completed before cancellation: {result:?}"),
        _ = state.poll_observed.notified() => {}
    }
    drop(selection);
    let recent_reads = state.recent_reads.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert!(recent_reads >= 2);
    assert_eq!(state.recent_reads.load(Ordering::SeqCst), recent_reads);
    assert_eq!(state.row_reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn no_model_capability_selection_does_not_reenumerate_during_polls() {
    let state = CountingState::blocked();
    let auth = limited_auth();
    let selected = select_with_auth_concurrency_wait(
        &state,
        Some(&auth),
        crate::clock::current_unix_secs(),
        Duration::from_millis(80),
        POLL_INTERVAL,
        |now| {
            list_selectable_candidates_for_required_capability_without_requested_model_with_auth_limit_signal(
                &state,
                &state,
                "openai:chat",
                "cache_1h",
                false,
                Some(&auth),
                None,
                now,
                SchedulerOrderingConfig::default(),
            )
        },
    )
    .await
    .unwrap();

    assert!(selected.is_empty());
    assert_eq!(state.format_reads.load(Ordering::SeqCst), 2);
    assert_eq!(state.row_reads.load(Ordering::SeqCst), 2);
    assert!(state.recent_reads.load(Ordering::SeqCst) > 2);
}

#[tokio::test]
async fn absent_or_disabled_auth_limits_do_not_poll() {
    for limit in [None, Some(0), Some(-1)] {
        let state = CountingState::blocked();
        let mut auth = limited_auth();
        auth.api_key_concurrent_limit = limit;
        let (selected, _) = select_requested_model(&state, Some(&auth), Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(state.row_reads.load(Ordering::SeqCst), 1);
        assert_eq!(state.recent_reads.load(Ordering::SeqCst), 0);
    }
    let state = CountingState::blocked();
    assert_eq!(
        select_requested_model(&state, None, Duration::from_secs(1))
            .await
            .unwrap()
            .0
            .len(),
        1
    );
    assert_eq!(state.recent_reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn auth_wait_keeps_candidate_row_and_lifecycle_counting_semantics() {
    let state = CountingState::blocked();
    let mut duplicate = active_candidate();
    duplicate.id = "second-attempt-same-request".to_string();
    state.recent.lock().unwrap().push(duplicate);
    let mut auth = limited_auth();
    auth.api_key_concurrent_limit = Some(2);
    let (selected, skipped) = select_requested_model(&state, Some(&auth), Duration::ZERO)
        .await
        .unwrap();
    assert!(is_exact_all_skipped_by_auth_limit(&selected, &skipped));

    let state = CountingState::blocked();
    state.recent.lock().unwrap()[0].finished_at_unix_ms =
        Some(crate::clock::current_unix_secs() * 1000);
    assert_eq!(
        select_requested_model(&state, Some(&limited_auth()), Duration::ZERO)
            .await
            .unwrap()
            .0
            .len(),
        1
    );

    let state = CountingState::blocked();
    state.recent.lock().unwrap()[0].started_at_unix_ms =
        Some(crate::clock::current_unix_secs().saturating_sub(301) * 1000);
    assert_eq!(
        select_requested_model(&state, Some(&limited_auth()), Duration::ZERO)
            .await
            .unwrap()
            .0
            .len(),
        1
    );
}
