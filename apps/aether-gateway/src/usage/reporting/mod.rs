use std::collections::BTreeMap;

use aether_contracts::ExecutionError;
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_scheduler_core::{execution_error_details, SchedulerRequestCandidateStatusUpdate};
use tracing::{debug, warn};

use crate::clock::current_unix_ms;
use crate::log_ids::short_request_id;
use crate::orchestration::{apply_local_report_effect, LocalReportEffect};
use crate::request_candidate_runtime::record_report_request_candidate_status;
use crate::{AppState, GatewayError};

mod context;
use context::{report_context_is_locally_actionable, resolve_locally_actionable_report_context};

use aether_usage_runtime::{
    is_local_ai_stream_report_kind, is_local_ai_sync_report_kind, report_request_id,
    should_handle_local_stream_report, should_handle_local_sync_report,
    sync_report_represents_failure,
};
pub(crate) use aether_usage_runtime::{GatewayStreamReportRequest, GatewaySyncReportRequest};

fn log_local_report_handled(
    trace_id: &str,
    report_kind: &str,
    report_scope: &'static str,
    report_context: Option<&serde_json::Value>,
) {
    debug!(
        event_name = "execution_report_handled_locally",
        log_type = "debug",
        debug_context = "redacted",
        trace_id = %trace_id,
        report_scope,
        report_kind = %report_kind,
        report_request_id = %short_request_id(report_request_id(report_context)),
        has_report_context = report_context.is_some(),
        "gateway handled execution report locally"
    );
}

fn log_local_report_effect_only(
    trace_id: &str,
    report_kind: &str,
    report_scope: &'static str,
    report_context: Option<&serde_json::Value>,
) {
    debug!(
        event_name = "execution_report_effect_handled_locally",
        log_type = "debug",
        debug_context = "redacted",
        trace_id = %trace_id,
        report_scope,
        report_kind = %report_kind,
        report_request_id = %short_request_id(report_request_id(report_context)),
        has_report_context = report_context.is_some(),
        "gateway handled execution report locally without actionable request-candidate context"
    );
}

fn log_dropped_report(
    trace_id: &str,
    report_kind: &str,
    report_scope: &'static str,
    report_context: Option<&serde_json::Value>,
) {
    warn!(
        event_name = "execution_report_dropped",
        log_type = "ops",
        status = "dropped",
        trace_id = %trace_id,
        report_scope,
        report_kind = %report_kind,
        report_request_id = %short_request_id(report_request_id(report_context)),
        has_report_context = report_context.is_some(),
        "gateway dropped execution report because local handling context was not actionable"
    );
}

pub(crate) async fn submit_sync_report(
    state: &AppState,
    mut payload: GatewaySyncReportRequest,
) -> Result<(), GatewayError> {
    let original_report_context = payload.report_context.take();
    if let Some(report_context) =
        resolve_locally_actionable_report_context(state, original_report_context.as_ref()).await
    {
        payload.report_context = Some(report_context);
        if should_handle_local_sync_report(
            payload.report_context.as_ref(),
            payload.report_kind.as_str(),
        ) {
            handle_local_sync_report(state, &payload).await;
            log_local_report_handled(
                payload.trace_id.as_str(),
                &payload.report_kind,
                "sync",
                payload.report_context.as_ref(),
            );
            return Ok(());
        }
    }
    payload.report_context = original_report_context;

    if should_handle_local_sync_report(
        payload.report_context.as_ref(),
        payload.report_kind.as_str(),
    ) {
        handle_local_sync_report(state, &payload).await;
        log_local_report_handled(
            payload.trace_id.as_str(),
            &payload.report_kind,
            "sync",
            payload.report_context.as_ref(),
        );
        return Ok(());
    }

    if payload.report_context.is_some()
        && is_local_ai_sync_report_kind(payload.report_kind.as_str())
    {
        handle_local_sync_report(state, &payload).await;
        log_local_report_effect_only(
            payload.trace_id.as_str(),
            &payload.report_kind,
            "sync",
            payload.report_context.as_ref(),
        );
        return Ok(());
    }

    log_dropped_report(
        payload.trace_id.as_str(),
        &payload.report_kind,
        "sync",
        payload.report_context.as_ref(),
    );
    Ok(())
}

pub(crate) fn spawn_sync_report(state: AppState, payload: GatewaySyncReportRequest) {
    let report_request_id_for_log =
        short_request_id(report_request_id(payload.report_context.as_ref()));
    tokio::spawn(async move {
        let trace_id = payload.trace_id.clone();
        if let Err(err) = submit_sync_report(&state, payload).await {
            warn!(
                event_name = "execution_report_submit_failed",
                log_type = "ops",
                trace_id = %trace_id,
                report_scope = "sync",
                report_request_id = %report_request_id_for_log,
                error = ?err,
                "gateway failed to submit sync execution report"
            );
        }
    });
}

pub(crate) async fn submit_stream_report(
    state: &AppState,
    mut payload: GatewayStreamReportRequest,
) -> Result<(), GatewayError> {
    let original_report_context = payload.report_context.take();
    if let Some(report_context) =
        resolve_locally_actionable_report_context(state, original_report_context.as_ref()).await
    {
        payload.report_context = Some(report_context);
        if should_handle_local_stream_report(
            payload.report_context.as_ref(),
            payload.report_kind.as_str(),
        ) {
            handle_local_stream_report(state, &payload).await;
            log_local_report_handled(
                payload.trace_id.as_str(),
                &payload.report_kind,
                "stream",
                payload.report_context.as_ref(),
            );
            return Ok(());
        }
    }
    payload.report_context = original_report_context;

    if should_handle_local_stream_report(
        payload.report_context.as_ref(),
        payload.report_kind.as_str(),
    ) {
        handle_local_stream_report(state, &payload).await;
        log_local_report_handled(
            payload.trace_id.as_str(),
            &payload.report_kind,
            "stream",
            payload.report_context.as_ref(),
        );
        return Ok(());
    }

    if payload.report_context.is_some()
        && is_local_ai_stream_report_kind(payload.report_kind.as_str())
    {
        handle_local_stream_report(state, &payload).await;
        log_local_report_effect_only(
            payload.trace_id.as_str(),
            &payload.report_kind,
            "stream",
            payload.report_context.as_ref(),
        );
        return Ok(());
    }

    log_dropped_report(
        payload.trace_id.as_str(),
        &payload.report_kind,
        "stream",
        payload.report_context.as_ref(),
    );
    Ok(())
}

async fn handle_local_sync_report(state: &AppState, payload: &GatewaySyncReportRequest) {
    let terminal_unix_ms = current_unix_ms();
    let (error_type, error_message) =
        execution_error_details(None::<&ExecutionError>, payload.body_json.as_ref());
    let status = if sync_report_represents_failure(payload, error_type.as_deref()) {
        RequestCandidateStatus::Failed
    } else {
        RequestCandidateStatus::Success
    };
    let latency_ms = payload
        .telemetry
        .as_ref()
        .and_then(|telemetry| telemetry.elapsed_ms);
    record_report_request_candidate_status(
        state,
        payload.report_context.as_ref(),
        SchedulerRequestCandidateStatusUpdate {
            status,
            status_code: Some(payload.status_code),
            error_type,
            error_message,
            latency_ms,
            started_at_unix_ms: None,
            finished_at_unix_ms: Some(terminal_unix_ms),
        },
    )
    .await;
    apply_local_report_effect(state, LocalReportEffect::Sync { payload }).await;
}

async fn handle_local_stream_report(state: &AppState, payload: &GatewayStreamReportRequest) {
    let terminal_unix_ms = current_unix_ms();
    let latency_ms = payload
        .telemetry
        .as_ref()
        .and_then(|telemetry| telemetry.elapsed_ms);
    record_report_request_candidate_status(
        state,
        payload.report_context.as_ref(),
        SchedulerRequestCandidateStatusUpdate {
            status: RequestCandidateStatus::Success,
            status_code: Some(payload.status_code),
            error_type: None,
            error_message: None,
            latency_ms,
            started_at_unix_ms: None,
            finished_at_unix_ms: Some(terminal_unix_ms),
        },
    )
    .await;
    apply_local_report_effect(state, LocalReportEffect::Stream { payload }).await;
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_data::repository::gemini_file_mappings::{
        GeminiFileMappingReadRepository, InMemoryGeminiFileMappingRepository,
    };
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data::repository::usage::InMemoryUsageReadRepository;
    use aether_data::repository::video_tasks::InMemoryVideoTaskRepository;
    use aether_data_contracts::repository::candidates::{
        RequestCandidateReadRepository, RequestCandidateStatus, StoredRequestCandidate,
    };
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogReadRepository, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use aether_data_contracts::repository::video_tasks::{
        UpsertVideoTask, VideoTaskStatus, VideoTaskWriteRepository,
    };
    use serde_json::json;

    use super::{
        resolve_locally_actionable_report_context, submit_stream_report, submit_sync_report,
        GatewayStreamReportRequest, GatewaySyncReportRequest,
    };
    use crate::data::GatewayDataState;
    use crate::AppState;

    fn sample_request_candidate(id: &str, request_id: &str) -> StoredRequestCandidate {
        StoredRequestCandidate::new(
            id.to_string(),
            request_id.to_string(),
            Some("user-reporting-tests-123".to_string()),
            Some("api-key-reporting-tests-123".to_string()),
            Some("alice".to_string()),
            Some("default".to_string()),
            0,
            0,
            Some("provider-reporting-tests-123".to_string()),
            Some("endpoint-reporting-tests-123".to_string()),
            Some("key-reporting-tests-123".to_string()),
            RequestCandidateStatus::Pending,
            None,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            1_700_000_000_000,
            Some(1_700_000_000_000),
            None,
        )
        .expect("request candidate should build")
    }

    fn sample_request_candidate_with_transport(
        id: &str,
        request_id: &str,
        user_id: &str,
        api_key_id: &str,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> StoredRequestCandidate {
        StoredRequestCandidate::new(
            id.to_string(),
            request_id.to_string(),
            Some(user_id.to_string()),
            Some(api_key_id.to_string()),
            Some("alice".to_string()),
            Some("default".to_string()),
            0,
            0,
            Some(provider_id.to_string()),
            Some(endpoint_id.to_string()),
            Some(key_id.to_string()),
            RequestCandidateStatus::Pending,
            None,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            1_700_000_000_000,
            Some(1_700_000_000_000),
            None,
        )
        .expect("request candidate should build")
    }

    fn build_test_state(repository: Arc<InMemoryRequestCandidateRepository>) -> AppState {
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                    repository,
                    Arc::new(InMemoryUsageReadRepository::default()),
                ),
            )
    }

    fn build_video_test_state(
        video_repository: Arc<InMemoryVideoTaskRepository>,
        request_candidate_repository: Arc<InMemoryRequestCandidateRepository>,
    ) -> AppState {
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_video_task_and_request_candidate_repository_for_tests(
                    video_repository,
                    request_candidate_repository,
                ),
            )
    }

    fn build_gemini_file_mapping_test_state(
        request_candidate_repository: Arc<InMemoryRequestCandidateRepository>,
        gemini_file_mapping_repository: Arc<InMemoryGeminiFileMappingRepository>,
    ) -> AppState {
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
            GatewayDataState::with_request_candidate_and_gemini_file_mapping_repository_for_tests(
                request_candidate_repository,
                gemini_file_mapping_repository,
            ),
        )
    }

    fn build_provider_catalog_test_state(
        repository: Arc<InMemoryProviderCatalogReadRepository>,
    ) -> AppState {
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository),
            )
    }

    fn sample_provider_catalog_provider(
        provider_id: &str,
        provider_type: &str,
    ) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            provider_id.to_string(),
            provider_type.to_string(),
            None,
            provider_type.to_string(),
        )
        .expect("provider should build")
    }

    fn sample_provider_catalog_key(key_id: &str, provider_id: &str) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            key_id.to_string(),
            provider_id.to_string(),
            "default".to_string(),
            "bearer".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["openai:cli"])),
            "sk-codex-test".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    fn sample_codex_paid_headers() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("x-codex-plan-type".to_string(), "team".to_string()),
            (
                "x-codex-primary-used-percent".to_string(),
                "100".to_string(),
            ),
            (
                "x-codex-secondary-used-percent".to_string(),
                "31".to_string(),
            ),
            (
                "x-codex-primary-window-minutes".to_string(),
                "300".to_string(),
            ),
            (
                "x-codex-secondary-window-minutes".to_string(),
                "10080".to_string(),
            ),
            (
                "x-codex-primary-reset-after-seconds".to_string(),
                "15160".to_string(),
            ),
            (
                "x-codex-secondary-reset-after-seconds".to_string(),
                "524059".to_string(),
            ),
            (
                "x-codex-primary-reset-at".to_string(),
                "1776148929".to_string(),
            ),
            (
                "x-codex-secondary-reset-at".to_string(),
                "1776657828".to_string(),
            ),
        ])
    }

    async fn seed_video_task(
        repository: &InMemoryVideoTaskRepository,
        id: &str,
        short_id: Option<&str>,
        request_id: &str,
        user_id: &str,
        api_key_id: &str,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
        client_api_format: &str,
        provider_api_format: &str,
    ) {
        repository
            .upsert(UpsertVideoTask {
                id: id.to_string(),
                short_id: short_id.map(ToOwned::to_owned),
                request_id: request_id.to_string(),
                user_id: Some(user_id.to_string()),
                api_key_id: Some(api_key_id.to_string()),
                username: Some("video-user".to_string()),
                api_key_name: Some("video-key".to_string()),
                external_task_id: Some("ext-video-task-reporting-123".to_string()),
                provider_id: Some(provider_id.to_string()),
                endpoint_id: Some(endpoint_id.to_string()),
                key_id: Some(key_id.to_string()),
                client_api_format: Some(client_api_format.to_string()),
                provider_api_format: Some(provider_api_format.to_string()),
                format_converted: false,
                model: Some("video-model".to_string()),
                prompt: Some("video prompt".to_string()),
                original_request_body: Some(json!({"prompt": "video prompt"})),
                duration_seconds: Some(4),
                resolution: Some("720p".to_string()),
                aspect_ratio: Some("16:9".to_string()),
                size: Some("1280x720".to_string()),
                status: VideoTaskStatus::Submitted,
                progress_percent: 0,
                progress_message: None,
                retry_count: 0,
                poll_interval_seconds: 10,
                next_poll_at_unix_secs: Some(1_700_000_010),
                poll_count: 0,
                max_poll_count: 360,
                created_at_unix_ms: 1_700_000_000,
                submitted_at_unix_secs: Some(1_700_000_000),
                completed_at_unix_secs: None,
                updated_at_unix_secs: 1_700_000_000,
                error_code: None,
                error_message: None,
                video_url: None,
                request_metadata: None,
            })
            .await
            .expect("video task should upsert");
    }

    #[tokio::test]
    async fn keeps_request_id_only_context_non_actionable_without_existing_candidate() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = build_test_state(repository);
        let report_context = json!({
            "request_id": "req-reporting-weak-123",
            "client_api_format": "openai:chat"
        });

        let resolved =
            resolve_locally_actionable_report_context(&state, Some(&report_context)).await;

        assert!(resolved.is_none());
    }

    #[tokio::test]
    async fn submit_sync_report_handles_request_id_only_context_locally_when_unique_candidate_exists(
    ) {
        let repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
            sample_request_candidate("cand-reporting-sync-123", "req-reporting-sync-123"),
        ]));
        let state = build_test_state(Arc::clone(&repository));

        submit_sync_report(
            &state,
            GatewaySyncReportRequest {
                trace_id: "trace-reporting-sync-123".to_string(),
                report_kind: "openai_chat_sync_success".to_string(),
                report_context: Some(json!({
                    "request_id": "req-reporting-sync-123",
                    "client_api_format": "openai:chat"
                })),
                status_code: 200,
                headers: BTreeMap::new(),
                body_json: None,
                client_body_json: None,
                body_base64: None,
                telemetry: None,
            },
        )
        .await
        .expect("sync report should stay local");

        let stored = repository
            .list_by_request_id("req-reporting-sync-123")
            .await
            .expect("request candidates should list");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, "cand-reporting-sync-123");
        assert_eq!(stored[0].status, RequestCandidateStatus::Success);
        assert_eq!(
            stored[0].provider_id.as_deref(),
            Some("provider-reporting-tests-123")
        );
    }

    #[tokio::test]
    async fn submit_sync_report_handles_openai_image_success_locally_when_unique_candidate_exists()
    {
        let repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
            sample_request_candidate(
                "cand-reporting-image-sync-123",
                "req-reporting-image-sync-123",
            ),
        ]));
        let state = build_test_state(Arc::clone(&repository));

        submit_sync_report(
            &state,
            GatewaySyncReportRequest {
                trace_id: "trace-reporting-image-sync-123".to_string(),
                report_kind: "openai_image_sync_success".to_string(),
                report_context: Some(json!({
                    "request_id": "req-reporting-image-sync-123",
                    "client_api_format": "openai:image"
                })),
                status_code: 200,
                headers: BTreeMap::new(),
                body_json: Some(json!({
                    "created": 1776855978,
                    "data": [{
                        "b64_json": "aGVsbG8="
                    }]
                })),
                client_body_json: None,
                body_base64: None,
                telemetry: None,
            },
        )
        .await
        .expect("image sync report should stay local");

        let stored = repository
            .list_by_request_id("req-reporting-image-sync-123")
            .await
            .expect("request candidates should list");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, "cand-reporting-image-sync-123");
        assert_eq!(stored[0].status, RequestCandidateStatus::Success);
        assert_eq!(stored[0].status_code, Some(200));
    }

    #[tokio::test]
    async fn submit_sync_report_treats_null_error_field_as_success() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
            sample_request_candidate("cand-reporting-sync-null-1", "req-reporting-sync-null-1"),
        ]));
        let state = build_test_state(Arc::clone(&repository));

        submit_sync_report(
            &state,
            GatewaySyncReportRequest {
                trace_id: "trace-reporting-sync-null-1".to_string(),
                report_kind: "claude_cli_sync_success".to_string(),
                report_context: Some(json!({
                    "request_id": "req-reporting-sync-null-1",
                    "client_api_format": "claude:cli",
                    "provider_api_format": "openai:cli"
                })),
                status_code: 200,
                headers: BTreeMap::new(),
                body_json: Some(json!({
                    "id": "resp_1",
                    "status": "completed",
                    "error": null
                })),
                client_body_json: None,
                body_base64: None,
                telemetry: None,
            },
        )
        .await
        .expect("sync report should stay local");

        let stored = repository
            .list_by_request_id("req-reporting-sync-null-1")
            .await
            .expect("request candidates should list");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].status, RequestCandidateStatus::Success);
        assert_eq!(stored[0].status_code, Some(200));
    }

    #[tokio::test]
    async fn submit_stream_report_handles_request_id_only_context_locally_when_unique_candidate_exists(
    ) {
        let repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
            sample_request_candidate("cand-reporting-stream-123", "req-reporting-stream-123"),
        ]));
        let state = build_test_state(Arc::clone(&repository));

        submit_stream_report(
            &state,
            GatewayStreamReportRequest {
                trace_id: "trace-reporting-stream-123".to_string(),
                report_kind: "openai_chat_stream_success".to_string(),
                report_context: Some(json!({
                    "request_id": "req-reporting-stream-123",
                    "client_api_format": "openai:chat"
                })),
                status_code: 200,
                headers: BTreeMap::new(),
                provider_body_base64: None,
                provider_body_state: None,
                client_body_base64: None,
                client_body_state: None,
                terminal_summary: None,
                telemetry: None,
            },
        )
        .await
        .expect("stream report should stay local");

        let stored = repository
            .list_by_request_id("req-reporting-stream-123")
            .await
            .expect("request candidates should list");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, "cand-reporting-stream-123");
        assert_eq!(stored[0].status, RequestCandidateStatus::Success);
        assert_eq!(
            stored[0].endpoint_id.as_deref(),
            Some("endpoint-reporting-tests-123")
        );
    }

    #[tokio::test]
    async fn submit_sync_report_updates_codex_quota_from_response_headers() {
        crate::orchestration::clear_local_report_effect_caches_for_tests();

        let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider_catalog_provider(
                "provider-codex-sync",
                "codex",
            )],
            Vec::new(),
            vec![sample_provider_catalog_key(
                "key-codex-sync",
                "provider-codex-sync",
            )],
        ));
        let state = build_provider_catalog_test_state(Arc::clone(&provider_catalog_repository));

        submit_sync_report(
            &state,
            GatewaySyncReportRequest {
                trace_id: "trace-codex-reporting-sync".to_string(),
                report_kind: "openai_cli_sync_success".to_string(),
                report_context: Some(json!({
                    "request_id": "req-codex-reporting-sync",
                    "key_id": "key-codex-sync"
                })),
                status_code: 200,
                headers: sample_codex_paid_headers(),
                body_json: None,
                client_body_json: None,
                body_base64: None,
                telemetry: None,
            },
        )
        .await
        .expect("sync report should stay local");

        let reloaded = provider_catalog_repository
            .list_keys_by_ids(&["key-codex-sync".to_string()])
            .await
            .expect("keys should list");
        let codex = reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get("codex"))
            .and_then(serde_json::Value::as_object)
            .expect("codex metadata should exist");
        assert_eq!(codex.get("primary_used_percent"), Some(&json!(31.0)));
        assert_eq!(codex.get("secondary_used_percent"), Some(&json!(100.0)));
        let quota = reloaded[0]
            .status_snapshot
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|snapshot| snapshot.get("quota"))
            .and_then(serde_json::Value::as_object)
            .expect("quota snapshot should exist");
        assert_eq!(quota.get("provider_type"), Some(&json!("codex")));
        assert_eq!(quota.get("source"), Some(&json!("response_headers")));
        assert_eq!(quota.get("code"), Some(&json!("exhausted")));
        assert_eq!(quota.get("updated_at"), quota.get("observed_at"));
    }

    #[tokio::test]
    async fn submit_stream_report_updates_codex_quota_from_response_headers() {
        crate::orchestration::clear_local_report_effect_caches_for_tests();

        let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider_catalog_provider(
                "provider-codex-stream",
                "codex",
            )],
            Vec::new(),
            vec![sample_provider_catalog_key(
                "key-codex-stream",
                "provider-codex-stream",
            )],
        ));
        let state = build_provider_catalog_test_state(Arc::clone(&provider_catalog_repository));

        submit_stream_report(
            &state,
            GatewayStreamReportRequest {
                trace_id: "trace-codex-reporting-stream".to_string(),
                report_kind: "openai_cli_stream_success".to_string(),
                report_context: Some(json!({
                    "request_id": "req-codex-reporting-stream",
                    "key_id": "key-codex-stream"
                })),
                status_code: 200,
                headers: sample_codex_paid_headers(),
                provider_body_base64: None,
                provider_body_state: None,
                client_body_base64: None,
                client_body_state: None,
                terminal_summary: None,
                telemetry: None,
            },
        )
        .await
        .expect("stream report should stay local");

        let reloaded = provider_catalog_repository
            .list_keys_by_ids(&["key-codex-stream".to_string()])
            .await
            .expect("keys should list");
        let codex = reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get("codex"))
            .and_then(serde_json::Value::as_object)
            .expect("codex metadata should exist");
        assert_eq!(codex.get("primary_used_percent"), Some(&json!(31.0)));
        assert_eq!(codex.get("secondary_used_percent"), Some(&json!(100.0)));
        let quota = reloaded[0]
            .status_snapshot
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|snapshot| snapshot.get("quota"))
            .and_then(serde_json::Value::as_object)
            .expect("quota snapshot should exist");
        assert_eq!(quota.get("provider_type"), Some(&json!("codex")));
        assert_eq!(quota.get("source"), Some(&json!("response_headers")));
        assert_eq!(quota.get("code"), Some(&json!("exhausted")));
        assert_eq!(quota.get("updated_at"), quota.get("observed_at"));
    }

    #[tokio::test]
    async fn submit_sync_report_stores_gemini_file_mapping_locally_when_payload_contains_file_json()
    {
        let request_candidate_repository =
            Arc::new(InMemoryRequestCandidateRepository::seed(vec![
                sample_request_candidate(
                    "cand-gemini-files-store-123",
                    "req-gemini-files-store-123",
                ),
            ]));
        let gemini_file_mapping_repository =
            Arc::new(InMemoryGeminiFileMappingRepository::default());
        let state = build_gemini_file_mapping_test_state(
            Arc::clone(&request_candidate_repository),
            Arc::clone(&gemini_file_mapping_repository),
        );

        submit_sync_report(
            &state,
            GatewaySyncReportRequest {
                trace_id: "trace-gemini-files-store-123".to_string(),
                report_kind: "gemini_files_store_mapping".to_string(),
                report_context: Some(json!({
                    "request_id": "req-gemini-files-store-123",
                    "candidate_id": "cand-gemini-files-store-123",
                    "candidate_index": 0,
                    "provider_id": "provider-reporting-tests-123",
                    "endpoint_id": "endpoint-reporting-tests-123",
                    "key_id": "key-reporting-tests-123",
                    "file_key_id": "key-reporting-tests-123",
                    "user_id": "user-reporting-tests-123",
                })),
                status_code: 200,
                headers: BTreeMap::from([(
                    "content-type".to_string(),
                    "application/json".to_string(),
                )]),
                body_json: Some(json!({
                    "file": {
                        "name": "abc123",
                        "displayName": "test-image",
                        "mimeType": "image/png"
                    }
                })),
                client_body_json: None,
                body_base64: None,
                telemetry: None,
            },
        )
        .await
        .expect("gemini files mapping report should stay local");

        let stored = gemini_file_mapping_repository
            .find_by_file_name("files/abc123")
            .await
            .expect("gemini file mapping should read")
            .expect("gemini file mapping should exist");
        assert_eq!(stored.key_id, "key-reporting-tests-123");
        assert_eq!(stored.user_id.as_deref(), Some("user-reporting-tests-123"));
        assert_eq!(stored.display_name.as_deref(), Some("test-image"));
        assert_eq!(stored.mime_type.as_deref(), Some("image/png"));
    }

    #[tokio::test]
    async fn submit_sync_report_stores_gemini_file_mapping_without_actionable_candidate_context() {
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let gemini_file_mapping_repository =
            Arc::new(InMemoryGeminiFileMappingRepository::default());
        let state = build_gemini_file_mapping_test_state(
            Arc::clone(&request_candidate_repository),
            Arc::clone(&gemini_file_mapping_repository),
        );

        submit_sync_report(
            &state,
            GatewaySyncReportRequest {
                trace_id: "trace-gemini-files-store-no-candidate-123".to_string(),
                report_kind: "gemini_files_store_mapping".to_string(),
                report_context: Some(json!({
                    "request_id": "req-gemini-files-store-no-candidate-123",
                    "file_key_id": "key-reporting-tests-123",
                    "user_id": "user-reporting-tests-123",
                })),
                status_code: 200,
                headers: BTreeMap::from([(
                    "content-type".to_string(),
                    "application/json".to_string(),
                )]),
                body_json: Some(json!({
                    "file": {
                        "name": "fallback123",
                        "displayName": "fallback-image",
                        "mimeType": "image/png"
                    }
                })),
                client_body_json: None,
                body_base64: None,
                telemetry: None,
            },
        )
        .await
        .expect("gemini files mapping fallback report should stay local");

        let stored = gemini_file_mapping_repository
            .find_by_file_name("files/fallback123")
            .await
            .expect("gemini file mapping should read")
            .expect("gemini file mapping should exist");
        assert_eq!(stored.key_id, "key-reporting-tests-123");
        assert_eq!(stored.user_id.as_deref(), Some("user-reporting-tests-123"));
        assert_eq!(stored.display_name.as_deref(), Some("fallback-image"));
        assert_eq!(stored.mime_type.as_deref(), Some("image/png"));
    }

    #[tokio::test]
    async fn submit_sync_report_deletes_gemini_file_mapping_locally_on_success() {
        let request_candidate_repository =
            Arc::new(InMemoryRequestCandidateRepository::seed(vec![
                sample_request_candidate(
                    "cand-gemini-files-delete-123",
                    "req-gemini-files-delete-123",
                ),
            ]));
        let gemini_file_mapping_repository = Arc::new(InMemoryGeminiFileMappingRepository::seed([
            aether_data::repository::gemini_file_mappings::StoredGeminiFileMapping::new(
                "mapping-gemini-files-delete-123".to_string(),
                "files/delete-me".to_string(),
                "key-reporting-tests-123".to_string(),
                1_700_000_000,
                1_700_172_800,
            )
            .expect("gemini file mapping should build"),
        ]));
        let state = build_gemini_file_mapping_test_state(
            Arc::clone(&request_candidate_repository),
            Arc::clone(&gemini_file_mapping_repository),
        );

        submit_sync_report(
            &state,
            GatewaySyncReportRequest {
                trace_id: "trace-gemini-files-delete-123".to_string(),
                report_kind: "gemini_files_delete_mapping".to_string(),
                report_context: Some(json!({
                    "request_id": "req-gemini-files-delete-123",
                    "candidate_id": "cand-gemini-files-delete-123",
                    "candidate_index": 0,
                    "provider_id": "provider-reporting-tests-123",
                    "endpoint_id": "endpoint-reporting-tests-123",
                    "key_id": "key-reporting-tests-123",
                    "file_name": "delete-me",
                })),
                status_code: 204,
                headers: BTreeMap::new(),
                body_json: None,
                client_body_json: None,
                body_base64: None,
                telemetry: None,
            },
        )
        .await
        .expect("gemini files delete mapping report should stay local");

        assert!(gemini_file_mapping_repository
            .find_by_file_name("files/delete-me")
            .await
            .expect("gemini file mapping should read")
            .is_none());
    }

    #[tokio::test]
    async fn submit_sync_report_treats_openai_video_delete_404_success_as_local_success() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
            sample_request_candidate(
                "cand-reporting-video-delete-123",
                "req-reporting-video-delete-123",
            ),
        ]));
        let state = build_test_state(Arc::clone(&repository));

        submit_sync_report(
            &state,
            GatewaySyncReportRequest {
                trace_id: "trace-reporting-video-delete-123".to_string(),
                report_kind: "openai_video_delete_sync_success".to_string(),
                report_context: Some(json!({
                    "request_id": "req-reporting-video-delete-123",
                    "provider_id": "provider-reporting-tests-123",
                    "endpoint_id": "endpoint-reporting-tests-123",
                    "key_id": "key-reporting-tests-123",
                })),
                status_code: 404,
                headers: BTreeMap::new(),
                body_json: None,
                client_body_json: None,
                body_base64: None,
                telemetry: None,
            },
        )
        .await
        .expect("video delete sync report should stay local");

        let stored = repository
            .list_by_request_id("req-reporting-video-delete-123")
            .await
            .expect("request candidates should list");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].status, RequestCandidateStatus::Success);
        assert_eq!(stored[0].status_code, Some(404));
    }

    #[tokio::test]
    async fn submit_sync_report_handles_local_task_id_only_context_locally_when_video_task_exists()
    {
        let video_repository = Arc::new(InMemoryVideoTaskRepository::default());
        seed_video_task(
            &video_repository,
            "task-openai-video-reporting-123",
            None,
            "req-openai-video-reporting-123",
            "user-openai-video-reporting-123",
            "api-key-openai-video-reporting-123",
            "provider-openai-video-reporting-123",
            "endpoint-openai-video-reporting-123",
            "key-openai-video-reporting-123",
            "openai:video",
            "openai:video",
        )
        .await;
        let request_candidate_repository =
            Arc::new(InMemoryRequestCandidateRepository::seed(vec![
                sample_request_candidate_with_transport(
                    "cand-openai-video-reporting-123",
                    "req-openai-video-reporting-123",
                    "user-openai-video-reporting-123",
                    "api-key-openai-video-reporting-123",
                    "provider-openai-video-reporting-123",
                    "endpoint-openai-video-reporting-123",
                    "key-openai-video-reporting-123",
                ),
            ]));
        let state =
            build_video_test_state(video_repository, Arc::clone(&request_candidate_repository));

        submit_sync_report(
            &state,
            GatewaySyncReportRequest {
                trace_id: "trace-openai-video-reporting-123".to_string(),
                report_kind: "openai_video_create_sync_success".to_string(),
                report_context: Some(json!({
                    "local_task_id": "task-openai-video-reporting-123"
                })),
                status_code: 200,
                headers: BTreeMap::new(),
                body_json: None,
                client_body_json: None,
                body_base64: None,
                telemetry: None,
            },
        )
        .await
        .expect("video report should stay local");

        let stored = request_candidate_repository
            .list_by_request_id("req-openai-video-reporting-123")
            .await
            .expect("request candidates should list");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, "cand-openai-video-reporting-123");
        assert_eq!(stored[0].status, RequestCandidateStatus::Success);
    }

    #[tokio::test]
    async fn submit_sync_report_handles_local_short_id_only_context_locally_when_video_task_exists()
    {
        let video_repository = Arc::new(InMemoryVideoTaskRepository::default());
        seed_video_task(
            &video_repository,
            "task-gemini-video-reporting-123",
            Some("short-gemini-video-reporting-123"),
            "req-gemini-video-reporting-123",
            "user-gemini-video-reporting-123",
            "api-key-gemini-video-reporting-123",
            "provider-gemini-video-reporting-123",
            "endpoint-gemini-video-reporting-123",
            "key-gemini-video-reporting-123",
            "gemini:video",
            "gemini:video",
        )
        .await;
        let request_candidate_repository =
            Arc::new(InMemoryRequestCandidateRepository::seed(vec![
                sample_request_candidate_with_transport(
                    "cand-gemini-video-reporting-123",
                    "req-gemini-video-reporting-123",
                    "user-gemini-video-reporting-123",
                    "api-key-gemini-video-reporting-123",
                    "provider-gemini-video-reporting-123",
                    "endpoint-gemini-video-reporting-123",
                    "key-gemini-video-reporting-123",
                ),
            ]));
        let state =
            build_video_test_state(video_repository, Arc::clone(&request_candidate_repository));

        submit_sync_report(
            &state,
            GatewaySyncReportRequest {
                trace_id: "trace-gemini-video-reporting-123".to_string(),
                report_kind: "gemini_video_create_sync_success".to_string(),
                report_context: Some(json!({
                    "local_short_id": "short-gemini-video-reporting-123"
                })),
                status_code: 200,
                headers: BTreeMap::new(),
                body_json: None,
                client_body_json: None,
                body_base64: None,
                telemetry: None,
            },
        )
        .await
        .expect("video report should stay local");

        let stored = request_candidate_repository
            .list_by_request_id("req-gemini-video-reporting-123")
            .await
            .expect("request candidates should list");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, "cand-gemini-video-reporting-123");
        assert_eq!(stored[0].status, RequestCandidateStatus::Success);
    }

    #[tokio::test]
    async fn submit_sync_report_handles_task_id_only_context_locally_when_video_task_id_exists() {
        let video_repository = Arc::new(InMemoryVideoTaskRepository::default());
        seed_video_task(
            &video_repository,
            "task-openai-video-task-id-123",
            None,
            "req-openai-video-task-id-123",
            "user-openai-video-task-id-123",
            "api-key-openai-video-task-id-123",
            "provider-openai-video-task-id-123",
            "endpoint-openai-video-task-id-123",
            "key-openai-video-task-id-123",
            "openai:video",
            "openai:video",
        )
        .await;
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state =
            build_video_test_state(video_repository, Arc::clone(&request_candidate_repository));

        submit_sync_report(
            &state,
            GatewaySyncReportRequest {
                trace_id: "trace-openai-video-task-id-123".to_string(),
                report_kind: "openai_video_cancel_sync_success".to_string(),
                report_context: Some(json!({
                    "task_id": "task-openai-video-task-id-123"
                })),
                status_code: 200,
                headers: BTreeMap::new(),
                body_json: None,
                client_body_json: None,
                body_base64: None,
                telemetry: None,
            },
        )
        .await
        .expect("video cancel report should stay local");

        let stored = request_candidate_repository
            .list_by_request_id("req-openai-video-task-id-123")
            .await
            .expect("request candidates should list");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].status, RequestCandidateStatus::Success);
        assert_eq!(
            stored[0].provider_id.as_deref(),
            Some("provider-openai-video-task-id-123")
        );
        assert_eq!(
            stored[0].endpoint_id.as_deref(),
            Some("endpoint-openai-video-task-id-123")
        );
        assert_eq!(
            stored[0].key_id.as_deref(),
            Some("key-openai-video-task-id-123")
        );
    }

    #[tokio::test]
    async fn submit_sync_report_handles_external_task_id_context_locally_when_video_task_exists() {
        let video_repository = Arc::new(InMemoryVideoTaskRepository::default());
        seed_video_task(
            &video_repository,
            "task-gemini-video-external-id-123",
            Some("short-gemini-video-external-id-123"),
            "req-gemini-video-external-id-123",
            "user-gemini-video-external-id-123",
            "api-key-gemini-video-external-id-123",
            "provider-gemini-video-external-id-123",
            "endpoint-gemini-video-external-id-123",
            "key-gemini-video-external-id-123",
            "gemini:video",
            "gemini:video",
        )
        .await;
        video_repository
            .upsert(UpsertVideoTask {
                id: "task-gemini-video-external-id-123".to_string(),
                short_id: Some("short-gemini-video-external-id-123".to_string()),
                request_id: "req-gemini-video-external-id-123".to_string(),
                user_id: Some("user-gemini-video-external-id-123".to_string()),
                api_key_id: Some("api-key-gemini-video-external-id-123".to_string()),
                username: Some("video-user".to_string()),
                api_key_name: Some("video-key".to_string()),
                external_task_id: Some("models/veo-3/operations/ext-gemini-video-123".to_string()),
                provider_id: Some("provider-gemini-video-external-id-123".to_string()),
                endpoint_id: Some("endpoint-gemini-video-external-id-123".to_string()),
                key_id: Some("key-gemini-video-external-id-123".to_string()),
                client_api_format: Some("gemini:video".to_string()),
                provider_api_format: Some("gemini:video".to_string()),
                format_converted: false,
                model: Some("video-model".to_string()),
                prompt: Some("video prompt".to_string()),
                original_request_body: Some(json!({"prompt": "video prompt"})),
                duration_seconds: Some(4),
                resolution: Some("720p".to_string()),
                aspect_ratio: Some("16:9".to_string()),
                size: Some("1280x720".to_string()),
                status: VideoTaskStatus::Submitted,
                progress_percent: 0,
                progress_message: None,
                retry_count: 0,
                poll_interval_seconds: 10,
                next_poll_at_unix_secs: Some(1_700_000_010),
                poll_count: 0,
                max_poll_count: 360,
                created_at_unix_ms: 1_700_000_000,
                submitted_at_unix_secs: Some(1_700_000_000),
                completed_at_unix_secs: None,
                updated_at_unix_secs: 1_700_000_000,
                error_code: None,
                error_message: None,
                video_url: None,
                request_metadata: None,
            })
            .await
            .expect("video task should update external id");
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state =
            build_video_test_state(video_repository, Arc::clone(&request_candidate_repository));

        submit_sync_report(
            &state,
            GatewaySyncReportRequest {
                trace_id: "trace-gemini-video-external-id-123".to_string(),
                report_kind: "gemini_video_cancel_sync_success".to_string(),
                report_context: Some(json!({
                    "user_id": "user-gemini-video-external-id-123",
                    "task_id": "models/veo-3/operations/ext-gemini-video-123"
                })),
                status_code: 200,
                headers: BTreeMap::new(),
                body_json: None,
                client_body_json: None,
                body_base64: None,
                telemetry: None,
            },
        )
        .await
        .expect("gemini video cancel report should stay local");

        let stored = request_candidate_repository
            .list_by_request_id("req-gemini-video-external-id-123")
            .await
            .expect("request candidates should list");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].status, RequestCandidateStatus::Success);
        assert_eq!(
            stored[0].provider_id.as_deref(),
            Some("provider-gemini-video-external-id-123")
        );
        assert_eq!(
            stored[0].endpoint_id.as_deref(),
            Some("endpoint-gemini-video-external-id-123")
        );
        assert_eq!(
            stored[0].key_id.as_deref(),
            Some("key-gemini-video-external-id-123")
        );
    }
}
