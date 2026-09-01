//! Terminal settlement for a local stream attempt whose future is dropped
//! mid-flight.
//!
//! A local stream attempt writes its `usage` row and its `request_candidates`
//! slot as `pending` before it dispatches to the provider, then keeps running
//! inside the downstream request future. When the client disconnects, axum drops
//! that future: the remaining `.await`s never resume and nothing settles either
//! row. They stay `pending` until the maintenance sweeper rewrites them as a 504
//! timeout roughly ten minutes later, which loses the real outcome and the real
//! latency.
//!
//! The stream transport therefore keeps a guard alive across the window between
//! the `pending` write and terminal settlement, and settles the attempt from
//! `Drop` when that window is left by cancellation instead of by a terminal
//! state.

use std::sync::Arc;
use std::time::Instant;

use aether_contracts::ExecutionPlan;
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_scheduler_core::SchedulerRequestCandidateStatusUpdate;
use aether_usage_runtime::{
    build_usage_event_data_seed_describing_request_bodies, UsageEvent, UsageEventData,
    UsageEventType,
};
use serde_json::{json, Value};
use tracing::warn;

use crate::clock::current_unix_ms as current_request_candidate_unix_ms;
use crate::execution_runtime::attempt_lifecycle::CLIENT_CANCELLED_STATUS_CODE;
use crate::execution_runtime::transport_failure::StreamCandidateWatchdogProgress;
use crate::log_ids::short_request_id;
use crate::request_candidate_runtime::{
    record_local_request_candidate_status_snapshot, LocalRequestCandidateStatusSnapshot,
};
use crate::request_diagnostics::{
    attach_request_diagnostics_to_report_context, current_request_diagnostics, RequestDiagnostics,
};
use crate::AppState;

fn elapsed_ms_since(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

/// The facts the guard needs to settle the attempt it is watching.
///
/// This is held for the whole attempt, so it is deliberately free of request
/// bodies. A request body can be megabytes, and holding one per in-flight
/// attempt would cost far more than the row it settles: the usage seed is built
/// with [`build_usage_event_data_seed_describing_request_bodies`], which derives
/// every capture state, body reference and derived request fact from the real
/// plan and report context but keeps neither body. The terminal write it
/// produces therefore preserves the capture the `pending` write recorded instead
/// of clearing it.
struct ArmedAttempt {
    request_id: String,
    candidate_id: Option<String>,
    candidate: Option<LocalRequestCandidateStatusSnapshot>,
    // Boxed: the guard lives inside the stream request future, which is already
    // very large, and `UsageEventData` is a wide struct.
    usage_seed: Option<Box<UsageEventData>>,
    request_diagnostics: Option<Arc<RequestDiagnostics>>,
    candidate_started_unix_ms: u64,
    candidate_started_at: Instant,
}

/// Settles an attempt as cancelled when its future is dropped before the
/// transport reaches a terminal state.
///
/// The guard is created disarmed and stays inert until [`Self::arm`] is called,
/// so an attempt that is dropped before it owns any `pending` row does not grow
/// a settlement row it never had. The owner disarms it as soon as the attempt
/// completes, whichever way it completes: from that point terminal settlement
/// belongs to the transport (for streams, to the stream finalizer that lives in
/// the response body), and the guard must not write a second terminal state.
///
/// A stream candidate also runs under a first-byte watchdog that drops the
/// attempt future when it gives up. That drop is not a client disconnect and the
/// watchdog settles the attempt itself, so the guard stands down for it.
pub(crate) struct AttemptCancellationGuard {
    state: AppState,
    error_type: &'static str,
    error_message: &'static str,
    watchdog: Option<Arc<StreamCandidateWatchdogProgress>>,
    armed: Option<ArmedAttempt>,
}

impl AttemptCancellationGuard {
    pub(crate) fn disarmed(
        state: &AppState,
        error_type: &'static str,
        error_message: &'static str,
    ) -> Self {
        Self {
            state: state.clone(),
            error_type,
            error_message,
            watchdog: StreamCandidateWatchdogProgress::current(),
            armed: None,
        }
    }

    /// Takes ownership of the attempt's settlement until it is disarmed.
    pub(crate) fn arm(
        &mut self,
        plan: &ExecutionPlan,
        report_context: Option<&Value>,
        candidate: Option<&LocalRequestCandidateStatusSnapshot>,
        candidate_started_unix_ms: u64,
        candidate_started_at: Instant,
    ) {
        let usage_seed = self.state.usage_runtime.is_enabled().then(|| {
            Box::new(build_usage_event_data_seed_describing_request_bodies(
                plan,
                report_context,
            ))
        });
        self.armed = Some(ArmedAttempt {
            request_id: plan.request_id.clone(),
            candidate_id: plan.candidate_id.clone(),
            candidate: candidate.cloned(),
            usage_seed,
            request_diagnostics: current_request_diagnostics(),
            candidate_started_unix_ms,
            candidate_started_at,
        });
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = None;
    }
}

/// Writes the candidate terminal row and the terminal usage event for an attempt
/// that never reached its own terminal path.
async fn settle_cancelled_attempt(
    state: AppState,
    armed: ArmedAttempt,
    error_type: &'static str,
    error_message: &'static str,
) {
    let ArmedAttempt {
        request_id,
        candidate_id: _,
        candidate,
        usage_seed,
        request_diagnostics,
        candidate_started_unix_ms,
        candidate_started_at,
    } = armed;
    let terminal_unix_ms = current_request_candidate_unix_ms();
    let latency_ms = elapsed_ms_since(candidate_started_at);

    if let Some(candidate) = candidate.as_ref() {
        record_local_request_candidate_status_snapshot(
            &state,
            candidate,
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Cancelled,
                status_code: Some(CLIENT_CANCELLED_STATUS_CODE),
                error_type: Some(error_type.to_string()),
                error_message: Some(error_message.to_string()),
                latency_ms: Some(latency_ms),
                started_at_unix_ms: Some(candidate_started_unix_ms),
                finished_at_unix_ms: Some(terminal_unix_ms),
            },
        )
        .await;
    }

    let Some(usage_data) = usage_seed else {
        return;
    };
    let mut usage_data = *usage_data;
    // The seed was built when the attempt was armed, so it predates the
    // diagnostics it should carry. Attaching them to the seed's metadata is the
    // same write the report context would have carried into a seed built here:
    // both land the same keys in the same object.
    usage_data.request_metadata = attach_request_diagnostics_to_report_context(
        usage_data.request_metadata.take(),
        request_diagnostics.as_ref(),
    );
    usage_data.status_code = Some(CLIENT_CANCELLED_STATUS_CODE);
    usage_data.error_message = Some(error_message.to_string());
    usage_data.error_category = Some("cancelled".to_string());
    usage_data.response_time_ms = Some(latency_ms);
    let error_body = json!({
        "error": {
            "type": error_type,
            "message": error_message,
            "code": CLIENT_CANCELLED_STATUS_CODE
        }
    });
    usage_data.response_headers = Some(json!({"content-type": "application/json"}));
    usage_data.response_body = Some(error_body.clone());
    usage_data.client_response_headers = Some(json!({"content-type": "application/json"}));
    usage_data.client_response_body = Some(error_body);

    state
        .usage_runtime
        .record_terminal_event_direct(
            state.usage_lifecycle_data_state().as_ref(),
            UsageEvent::new(UsageEventType::Cancelled, request_id, usage_data),
        )
        .await;
}

impl Drop for AttemptCancellationGuard {
    fn drop(&mut self) {
        let Some(armed) = self.armed.take() else {
            return;
        };
        if self
            .watchdog
            .as_ref()
            .is_some_and(|watchdog| watchdog.abandoned())
        {
            return;
        }
        let state = self.state.clone();
        let error_type = self.error_type;
        let error_message = self.error_message;
        // `Drop` cannot await, and the settlement writes touch the database.
        // Hand them to the runtime so they survive the dropped request future.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            warn!(
                event_name = "local_attempt_cancellation_guard_no_runtime",
                log_type = "ops",
                request_id = %short_request_id(armed.request_id.as_str()),
                candidate_id = ?armed.candidate_id,
                error_type,
                "gateway could not settle dropped local attempt because no Tokio runtime is available"
            );
            return;
        };
        handle.spawn(async move {
            settle_cancelled_attempt(state, armed, error_type, error_message).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_contracts::RequestBody;
    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_data::repository::usage::InMemoryUsageReadRepository;
    use aether_data_contracts::repository::candidates::RequestCandidateReadRepository;
    use aether_data_contracts::repository::usage::{
        StoredRequestUsageAudit, UsageBodyCaptureState, UsageReadRepository, UsageWriteRepository,
    };
    use aether_usage_runtime::{
        build_lifecycle_usage_seed, build_pending_usage_record, UsageRuntimeConfig,
    };
    use std::collections::BTreeMap;
    use std::time::Duration;

    use crate::request_candidate_runtime::{
        ensure_execution_request_candidate_slot, snapshot_local_request_candidate_status,
    };

    const TEST_ERROR_TYPE: &str = "local_stream_attempt_cancelled";
    const TEST_ERROR_MESSAGE: &str =
        "Local stream attempt was dropped before terminal finalization.";

    fn test_stream_plan(request_id: &str) -> ExecutionPlan {
        ExecutionPlan {
            request_id: request_id.to_string(),
            candidate_id: None,
            provider_name: Some("Anthropic".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.test/v1/messages".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true, "service_tier": "priority"})),
            stream: true,
            client_api_format: "claude:messages".to_string(),
            provider_api_format: "claude:messages".to_string(),
            model_name: Some("claude-sonnet-4-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn test_report_context() -> Option<Value> {
        Some(json!({
            "candidate_index": 0,
            "retry_index": 0,
            "user_id": "user-cancel",
            "api_key_id": "api-key-cancel",
            "client_api_format": "claude:messages",
            "provider_api_format": "claude:messages",
            "request_path": "/v1/messages",
            "request_path_and_query": "/v1/messages?beta=true",
            "upstream_url": "https://example.test/v1/messages",
            "mapped_model": "claude-sonnet-4-5",
            "original_request_body": {"stream": true, "messages": []},
        }))
    }

    fn test_state(
        usage_repository: &Arc<InMemoryUsageReadRepository>,
        request_candidate_repository: &Arc<InMemoryRequestCandidateRepository>,
    ) -> AppState {
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                    Arc::clone(request_candidate_repository),
                    Arc::clone(usage_repository),
                ),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            })
    }

    /// Writes the `pending` rows the same way a stream attempt does before it
    /// dispatches to the provider, and returns the candidate slot snapshot the
    /// attempt owns from that point on.
    async fn record_pending_attempt(
        state: &AppState,
        plan: &mut ExecutionPlan,
        report_context: &mut Option<Value>,
        candidate_started_unix_ms: u64,
    ) -> LocalRequestCandidateStatusSnapshot {
        ensure_execution_request_candidate_slot(state, plan, report_context).await;
        state.usage_runtime.record_pending(
            state.usage_lifecycle_data_state().as_ref(),
            build_lifecycle_usage_seed(plan, report_context.as_ref()),
        );
        let snapshot = snapshot_local_request_candidate_status(plan, report_context.as_ref())
            .expect("attempt should own a candidate slot");
        record_local_request_candidate_status_snapshot(
            state,
            &snapshot,
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Pending,
                status_code: None,
                error_type: None,
                error_message: None,
                latency_ms: None,
                started_at_unix_ms: Some(candidate_started_unix_ms),
                finished_at_unix_ms: None,
            },
        )
        .await;
        snapshot
    }

    async fn wait_for_usage_status(
        usage_repository: &InMemoryUsageReadRepository,
        request_id: &str,
        status: &str,
    ) -> Option<StoredRequestUsageAudit> {
        for _ in 0..50 {
            if let Some(usage) = usage_repository
                .find_by_request_id(request_id)
                .await
                .expect("usage should read")
            {
                if usage.status == status {
                    return Some(usage);
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        None
    }

    #[tokio::test]
    async fn armed_guard_settles_a_dropped_attempt_as_cancelled() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = test_state(&usage_repository, &request_candidate_repository);
        let mut plan = test_stream_plan("stream-cancel-guard-request");
        let mut report_context = test_report_context();
        let candidate_started_unix_ms = current_request_candidate_unix_ms();
        let snapshot = record_pending_attempt(
            &state,
            &mut plan,
            &mut report_context,
            candidate_started_unix_ms,
        )
        .await;

        {
            let mut guard =
                AttemptCancellationGuard::disarmed(&state, TEST_ERROR_TYPE, TEST_ERROR_MESSAGE);
            guard.arm(
                &plan,
                report_context.as_ref(),
                Some(&snapshot),
                candidate_started_unix_ms,
                Instant::now(),
            );
        }

        let usage = wait_for_usage_status(
            usage_repository.as_ref(),
            "stream-cancel-guard-request",
            "cancelled",
        )
        .await
        .expect("cancelled usage should be recorded");
        assert_eq!(usage.billing_status, "void");
        assert_eq!(usage.status_code, Some(CLIENT_CANCELLED_STATUS_CODE));
        assert_eq!(usage.error_category.as_deref(), Some("cancelled"));
        assert!(usage.response_time_ms.is_some());

        let candidates = request_candidate_repository
            .list_by_request_id("stream-cancel-guard-request")
            .await
            .expect("candidates should read");
        let candidate = candidates.first().expect("candidate row should exist");
        assert_eq!(candidate.status, RequestCandidateStatus::Cancelled);
        assert_eq!(candidate.status_code, Some(CLIENT_CANCELLED_STATUS_CODE));
        assert_eq!(candidate.error_type.as_deref(), Some(TEST_ERROR_TYPE));
        assert!(candidate.finished_at_unix_ms.is_some());
    }

    /// The guard holds no request body, so its settlement write must describe the
    /// capture rather than deny it: a typed `none` capture state would clear the
    /// stored request body instead of leaving it alone.
    #[tokio::test]
    async fn settling_a_dropped_attempt_leaves_the_captured_request_body_alone() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = test_state(&usage_repository, &request_candidate_repository);
        let mut plan = test_stream_plan("stream-cancel-guard-capture");
        let mut report_context = test_report_context();
        let candidate_started_unix_ms = current_request_candidate_unix_ms();
        let snapshot = record_pending_attempt(
            &state,
            &mut plan,
            &mut report_context,
            candidate_started_unix_ms,
        )
        .await;
        // Stand in for a write that already captured this request's body.
        let captured_body = json!({"stream": true, "service_tier": "priority"});
        let mut capture = build_pending_usage_record(
            &plan,
            report_context.as_ref(),
            current_request_candidate_unix_ms() / 1_000,
        )
        .expect("pending usage record should build");
        capture.provider_request_body = Some(captured_body.clone());
        capture.provider_request_body_state = Some(UsageBodyCaptureState::Inline);
        usage_repository
            .upsert(capture)
            .await
            .expect("captured request body should upsert");

        {
            let mut guard =
                AttemptCancellationGuard::disarmed(&state, TEST_ERROR_TYPE, TEST_ERROR_MESSAGE);
            guard.arm(
                &plan,
                report_context.as_ref(),
                Some(&snapshot),
                candidate_started_unix_ms,
                Instant::now(),
            );
        }

        let usage = wait_for_usage_status(
            usage_repository.as_ref(),
            "stream-cancel-guard-capture",
            "cancelled",
        )
        .await
        .expect("cancelled usage should be recorded");
        assert_eq!(usage.provider_request_body, Some(captured_body));
        assert_ne!(
            usage.provider_request_body_state,
            Some(UsageBodyCaptureState::None)
        );
    }

    #[tokio::test]
    async fn guard_stands_down_when_the_watchdog_abandons_the_attempt() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = test_state(&usage_repository, &request_candidate_repository);
        let mut plan = test_stream_plan("stream-watchdog-guard-request");
        let mut report_context = test_report_context();
        let candidate_started_unix_ms = current_request_candidate_unix_ms();
        let snapshot = record_pending_attempt(
            &state,
            &mut plan,
            &mut report_context,
            candidate_started_unix_ms,
        )
        .await;

        let watchdog = StreamCandidateWatchdogProgress::shared();
        Arc::clone(&watchdog)
            .scope(async {
                let mut guard =
                    AttemptCancellationGuard::disarmed(&state, TEST_ERROR_TYPE, TEST_ERROR_MESSAGE);
                guard.arm(
                    &plan,
                    report_context.as_ref(),
                    Some(&snapshot),
                    candidate_started_unix_ms,
                    Instant::now(),
                );
                // The watchdog gives up and takes over settlement before the
                // abandoned attempt is dropped.
                watchdog.mark_abandoned();
            })
            .await;

        assert!(wait_for_usage_status(
            usage_repository.as_ref(),
            "stream-watchdog-guard-request",
            "cancelled",
        )
        .await
        .is_none());
    }

    #[tokio::test]
    async fn disarmed_guard_leaves_the_attempt_pending() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = test_state(&usage_repository, &request_candidate_repository);
        let mut plan = test_stream_plan("stream-disarmed-guard-request");
        let mut report_context = test_report_context();
        let candidate_started_unix_ms = current_request_candidate_unix_ms();
        let snapshot = record_pending_attempt(
            &state,
            &mut plan,
            &mut report_context,
            candidate_started_unix_ms,
        )
        .await;

        {
            let mut guard =
                AttemptCancellationGuard::disarmed(&state, TEST_ERROR_TYPE, TEST_ERROR_MESSAGE);
            guard.arm(
                &plan,
                report_context.as_ref(),
                Some(&snapshot),
                candidate_started_unix_ms,
                Instant::now(),
            );
            guard.disarm();
        }

        assert!(wait_for_usage_status(
            usage_repository.as_ref(),
            "stream-disarmed-guard-request",
            "cancelled",
        )
        .await
        .is_none());
    }
}
