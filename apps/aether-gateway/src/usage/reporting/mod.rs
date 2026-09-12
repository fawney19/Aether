use std::collections::BTreeMap;

use aether_contracts::ExecutionError;
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_scheduler_core::{execution_error_details, SchedulerRequestCandidateStatusUpdate};
use tracing::{debug, warn};

use crate::clock::current_unix_ms;
use crate::log_ids::short_request_id;
use crate::orchestration::{apply_local_report_effect, LocalReportEffect};
use crate::request_candidate_runtime::record_report_request_candidate_status;
use crate::task_runtime::{spawn_fire_and_forget, TASK_KEY_USAGE_SYNC_REPORT};
use crate::{AppState, GatewayError};

mod context;
pub(crate) use context::{
    attach_internal_gateway_report_capability, resolve_bound_internal_gateway_report_context,
};
use context::{report_context_is_locally_actionable, resolve_locally_actionable_report_context};

use aether_usage_runtime::{
    is_local_ai_stream_report_kind, is_local_ai_sync_report_kind, report_request_id,
    should_handle_local_stream_report, should_handle_local_sync_report,
    stream_report_missing_terminal_event, stream_report_represents_failure,
    sync_report_represents_failure, STREAM_MISSING_TERMINAL_EVENT_CATEGORY,
    STREAM_MISSING_TERMINAL_EVENT_MESSAGE, STREAM_TERMINAL_ERROR_CATEGORY,
    STREAM_TERMINAL_ERROR_MESSAGE,
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
    spawn_fire_and_forget(TASK_KEY_USAGE_SYNC_REPORT, async move {
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
    let failed = stream_report_represents_failure(payload);
    let missing_terminal_event = stream_report_missing_terminal_event(payload);
    record_report_request_candidate_status(
        state,
        payload.report_context.as_ref(),
        SchedulerRequestCandidateStatusUpdate {
            status: if failed {
                RequestCandidateStatus::Failed
            } else {
                RequestCandidateStatus::Success
            },
            status_code: Some(payload.status_code),
            error_type: failed.then(|| {
                if payload.status_code >= 400 {
                    "stream_http_error".to_string()
                } else if missing_terminal_event {
                    STREAM_MISSING_TERMINAL_EVENT_CATEGORY.to_string()
                } else {
                    STREAM_TERMINAL_ERROR_CATEGORY.to_string()
                }
            }),
            error_message: failed.then(|| {
                payload
                    .terminal_summary
                    .as_ref()
                    .and_then(|summary| summary.parser_error.clone())
                    .unwrap_or_else(|| {
                        if missing_terminal_event {
                            STREAM_MISSING_TERMINAL_EVENT_MESSAGE.to_string()
                        } else {
                            STREAM_TERMINAL_ERROR_MESSAGE.to_string()
                        }
                    })
            }),
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

    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
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
    use aether_data_contracts::repository::usage::UsageBodyCaptureState;
    use aether_data_contracts::repository::video_tasks::{
        UpsertVideoTask, VideoTaskStatus, VideoTaskWriteRepository,
    };
    use base64::Engine as _;
    use serde_json::json;

    use super::{
        attach_internal_gateway_report_capability, resolve_bound_internal_gateway_report_context,
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

    async fn mint_internal_report_capability(
        state: &AppState,
        trace_id: &str,
        report_kind: &str,
        context: serde_json::Value,
    ) -> serde_json::Value {
        mint_internal_report_capability_with_headers(
            state,
            trace_id,
            report_kind,
            &BTreeMap::new(),
            context,
        )
        .await
    }

    async fn mint_internal_report_capability_with_headers(
        state: &AppState,
        trace_id: &str,
        report_kind: &str,
        provider_request_headers: &BTreeMap<String, String>,
        context: serde_json::Value,
    ) -> serde_json::Value {
        let mut report_context = Some(context);
        attach_internal_gateway_report_capability(
            state,
            trace_id,
            Some(report_kind),
            provider_request_headers,
            &mut report_context,
        )
        .await
        .expect("report capability should mint");
        report_context.expect("report context should remain present")
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
                GatewayDataState::with_provider_catalog_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
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
        let credential_state = AppState::new()
            .expect("credential state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let encrypted_api_key = credential_state
            .seal_provider_catalog_key_api_key(provider_id, key_id, "sk-codex-test")
            .expect("api key should encrypt");
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
            Some(json!(["openai:responses"])),
            encrypted_api_key,
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
        external_task_id: &str,
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
                external_task_id: Some(external_task_id.to_string()),
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
    async fn internal_report_capability_validates_once_without_candidate_persistence() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = build_test_state(repository);
        let mut context = mint_internal_report_capability(
            &state,
            "trace-capability-valid-123",
            "openai_chat_sync_success",
            json!({
                "request_id": "req-capability-valid-123",
                "candidate_id": "cand-capability-valid-123",
                "user_id": "user-capability-valid-123",
                "provider_id": "provider-reporting-tests-123",
                "key_id": "key-reporting-tests-123",
            }),
        )
        .await;
        context
            .as_object_mut()
            .expect("context should be an object")
            .extend([
                (
                    "provider_response_headers".to_string(),
                    json!({"x-ratelimit-limit": "100"}),
                ),
                ("upstream_response".to_string(), json!({"id": "resp-123"})),
                ("error_flow".to_string(), json!({"stage": "upstream"})),
                (
                    "client_response_headers".to_string(),
                    json!({"content-type": "application/json"}),
                ),
            ]);

        let resolved = resolve_bound_internal_gateway_report_context(
            &state,
            "trace-capability-valid-123",
            "openai_chat_sync_error",
            Some(&context),
        )
        .await
        .expect("capability lookup should succeed")
        .expect("capability should validate");
        assert!(resolved.get("_aether_internal_report_capability").is_none());
        assert_eq!(resolved["upstream_response"]["id"], "resp-123");

        let replay = resolve_bound_internal_gateway_report_context(
            &state,
            "trace-capability-valid-123",
            "openai_chat_sync_success",
            Some(&context),
        )
        .await
        .expect("capability lookup should succeed");
        assert!(replay.is_none(), "a consumed capability must not replay");
    }

    #[tokio::test]
    async fn internal_report_capability_rejects_missing_unknown_trace_and_scope_mismatches() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = build_test_state(repository);
        let protected = json!({
            "request_id": "req-capability-reject-123",
            "candidate_id": "cand-capability-reject-123",
            "user_id": "user-capability-reject-123",
            "provider_id": "provider-capability-reject-123",
            "key_id": "key-capability-reject-123",
        });
        let minted = mint_internal_report_capability(
            &state,
            "trace-capability-reject-123",
            "openai_chat_stream_success",
            protected.clone(),
        )
        .await;

        let unknown = {
            let mut value = minted.clone();
            value["_aether_internal_report_capability"] =
                json!(uuid::Uuid::new_v4().simple().to_string());
            value
        };
        for (trace_id, report_kind, context) in [
            (
                "trace-capability-reject-123",
                "openai_chat_stream_success",
                protected.clone(),
            ),
            (
                "trace-capability-reject-123",
                "openai_chat_stream_success",
                unknown,
            ),
            (
                "trace-capability-attacker-123",
                "openai_chat_stream_success",
                minted.clone(),
            ),
            (
                "trace-capability-reject-123",
                "gemini_files_store_mapping",
                minted.clone(),
            ),
            (
                "trace-capability-reject-123",
                "openai_video_create_sync_finalize",
                minted.clone(),
            ),
        ] {
            let rejected = resolve_bound_internal_gateway_report_context(
                &state,
                trace_id,
                report_kind,
                Some(&context),
            )
            .await
            .expect("capability lookup should succeed");
            assert!(
                rejected.is_none(),
                "invalid capability use must be rejected"
            );
        }

        let valid = resolve_bound_internal_gateway_report_context(
            &state,
            "trace-capability-reject-123",
            "openai_chat_sync_finalize",
            Some(&minted),
        )
        .await
        .expect("capability lookup should succeed");
        assert!(
            valid.is_some(),
            "invalid attempts must not consume the capability"
        );
    }

    #[tokio::test]
    async fn internal_report_capability_rejects_every_protected_identity_mutation() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = build_test_state(repository);
        let minted = mint_internal_report_capability(
            &state,
            "trace-capability-fields-123",
            "openai_video_create_sync_finalize",
            json!({
                "request_id": "req-capability-fields-123",
                "candidate_id": "cand-capability-fields-123",
                "user_id": "user-capability-fields-123",
                "api_key_id": "api-key-capability-fields-123",
                "provider_id": "provider-capability-fields-123",
                "endpoint_id": "endpoint-capability-fields-123",
                "key_id": "key-capability-fields-123",
                "file_key_id": "file-key-capability-fields-123",
                "task_id": "task-capability-fields-123",
                "local_task_id": "local-task-capability-fields-123",
                "local_short_id": "short-capability-fields-123",
                "file_name": "files/capability-fields-123",
                "client_api_format": "openai:video",
                "upstream_url": "https://provider.example/v1/videos",
                "has_envelope": false,
                "needs_conversion": false,
            }),
        )
        .await;

        for field in [
            "user_id",
            "api_key_id",
            "provider_id",
            "endpoint_id",
            "key_id",
            "file_key_id",
            "task_id",
            "local_task_id",
            "local_short_id",
            "file_name",
            "client_api_format",
            "upstream_url",
            "has_envelope",
            "needs_conversion",
        ] {
            let mut forged = minted.clone();
            forged[field] = json!(format!("attacker-{field}"));
            let resolved = resolve_bound_internal_gateway_report_context(
                &state,
                "trace-capability-fields-123",
                "openai_video_create_sync_finalize",
                Some(&forged),
            )
            .await
            .expect("capability lookup should succeed");
            assert!(resolved.is_none(), "mutating {field} must be rejected");
        }

        for report_kind in [
            "openai_video_delete_sync_finalize",
            "openai_video_cancel_sync_finalize",
            "gemini_video_create_sync_finalize",
        ] {
            let resolved = resolve_bound_internal_gateway_report_context(
                &state,
                "trace-capability-fields-123",
                report_kind,
                Some(&minted),
            )
            .await
            .expect("capability lookup should succeed");
            assert!(resolved.is_none(), "cross-operation use must be rejected");
        }

        let valid = resolve_bound_internal_gateway_report_context(
            &state,
            "trace-capability-fields-123",
            "openai_video_create_sync_error",
            Some(&minted),
        )
        .await
        .expect("capability lookup should succeed");
        assert!(valid.is_some());
    }

    #[tokio::test]
    async fn internal_report_capability_allows_only_the_bound_kiro_web_search_transform() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = build_test_state(repository);
        let minted = mint_internal_report_capability(
            &state,
            "trace-capability-kiro-search-123",
            "openai_chat_stream_success",
            json!({
                "request_id": "req-capability-kiro-search-123",
                "candidate_id": "cand-capability-kiro-search-123",
                "user_id": "user-capability-kiro-search-123",
                "provider_id": "provider-capability-kiro-search-123",
                "key_id": "key-capability-kiro-search-123",
                "upstream_url": "https://kiro.example/generateAssistantResponse",
                "has_envelope": true,
                "needs_conversion": true,
                "envelope_name": aether_provider_transport::kiro::KIRO_ENVELOPE_NAME,
            }),
        )
        .await;

        let mut forged_target = minted.clone();
        forged_target["upstream_url"] = json!("https://attacker.example/forged");
        forged_target["has_envelope"] = json!(false);
        forged_target["needs_conversion"] = json!(false);
        forged_target["kiro_web_search_mcp"] = json!(true);
        forged_target
            .as_object_mut()
            .expect("context should be an object")
            .remove("envelope_name");
        let rejected = resolve_bound_internal_gateway_report_context(
            &state,
            "trace-capability-kiro-search-123",
            "openai_chat_stream_success",
            Some(&forged_target),
        )
        .await
        .expect("capability lookup should succeed");
        assert!(rejected.is_none(), "the upstream target must remain bound");

        let mut synthetic = minted;
        synthetic["has_envelope"] = json!(false);
        synthetic["needs_conversion"] = json!(false);
        synthetic["kiro_web_search_mcp"] = json!(true);
        synthetic
            .as_object_mut()
            .expect("context should be an object")
            .remove("envelope_name");
        let resolved = resolve_bound_internal_gateway_report_context(
            &state,
            "trace-capability-kiro-search-123",
            "openai_chat_stream_success",
            Some(&synthetic),
        )
        .await
        .expect("capability lookup should succeed")
        .expect("the pre-bound Kiro web-search transform should validate");
        assert_eq!(resolved["kiro_web_search_mcp"], json!(true));
        assert_eq!(
            resolved["upstream_url"],
            json!("https://kiro.example/generateAssistantResponse")
        );
    }

    #[tokio::test]
    async fn internal_report_capability_binds_final_provider_request_headers() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = build_test_state(repository);
        let headers = BTreeMap::from([
            (
                "authorization".to_string(),
                "Bearer final-token".to_string(),
            ),
            ("content-type".to_string(), "application/json".to_string()),
        ]);
        let minted = mint_internal_report_capability_with_headers(
            &state,
            "trace-capability-headers-123",
            "openai_video_create_sync_success",
            &headers,
            json!({
                "request_id": "req-capability-headers-123",
                "provider_request_headers": {"authorization": "Bearer stale-token"},
            }),
        )
        .await;
        assert_eq!(minted["provider_request_headers"], json!(headers));

        let mut forged = minted.clone();
        forged["provider_request_headers"]["authorization"] = json!("Bearer attacker-token");
        let rejected = resolve_bound_internal_gateway_report_context(
            &state,
            "trace-capability-headers-123",
            "openai_video_create_sync_success",
            Some(&forged),
        )
        .await
        .expect("capability lookup should succeed");
        assert!(
            rejected.is_none(),
            "final request headers must remain bound"
        );

        let resolved = resolve_bound_internal_gateway_report_context(
            &state,
            "trace-capability-headers-123",
            "openai_video_create_sync_success",
            Some(&minted),
        )
        .await
        .expect("capability lookup should succeed")
        .expect("the authoritative final headers should validate");
        assert_eq!(resolved["provider_request_headers"], json!(headers));
    }

    #[tokio::test]
    async fn internal_report_capability_validates_windsurf_native_observation_shape() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = build_test_state(repository);
        let minted = mint_internal_report_capability(
            &state,
            "trace-capability-windsurf-123",
            "openai_responses_stream_success",
            json!({"request_id": "req-capability-windsurf-123"}),
        )
        .await;

        for (native_runtime, port) in [
            (json!(false), json!(42_137)),
            (json!(true), json!(0)),
            (json!(true), json!(65_536)),
            (json!(true), json!("42137")),
        ] {
            let mut invalid = minted.clone();
            invalid["windsurf_native_runtime"] = native_runtime;
            invalid["windsurf_language_server_port"] = port;
            let rejected = resolve_bound_internal_gateway_report_context(
                &state,
                "trace-capability-windsurf-123",
                "openai_responses_stream_success",
                Some(&invalid),
            )
            .await
            .expect("capability lookup should succeed");
            assert!(rejected.is_none(), "invalid Windsurf metadata must fail");
        }

        let mut valid = minted;
        valid["windsurf_native_runtime"] = json!(true);
        valid["windsurf_language_server_port"] = json!(42_137);
        let resolved = resolve_bound_internal_gateway_report_context(
            &state,
            "trace-capability-windsurf-123",
            "openai_responses_stream_success",
            Some(&valid),
        )
        .await
        .expect("capability lookup should succeed")
        .expect("valid Windsurf native metadata should pass");
        assert_eq!(resolved["windsurf_native_runtime"], json!(true));
        assert_eq!(resolved["windsurf_language_server_port"], json!(42_137));
    }

    #[tokio::test]
    async fn internal_report_capability_accepts_only_non_deferred_late_bound_reservations() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = build_test_state(repository);
        let minted = mint_internal_report_capability(
            &state,
            "trace-capability-reservation-123",
            "openai_chat_sync_success",
            json!({
                "request_id": "req-capability-reservation-123",
                "candidate_id": "cand-capability-reservation-123",
                "user_id": "user-capability-reservation-123",
                "provider_id": "provider-capability-reservation-123",
                "key_id": "key-capability-reservation-123",
            }),
        )
        .await;

        let mut deferred = minted.clone();
        deferred["plan_usage_reservation_token"] = json!(uuid::Uuid::new_v4().to_string());
        deferred["plan_usage_reservation_deferred"] = json!(true);
        let rejected = resolve_bound_internal_gateway_report_context(
            &state,
            "trace-capability-reservation-123",
            "openai_chat_sync_success",
            Some(&deferred),
        )
        .await
        .expect("capability lookup should succeed");
        assert!(
            rejected.is_none(),
            "a peer must not defer terminal reservation reconciliation"
        );

        let reservation_token = uuid::Uuid::new_v4().to_string();
        let mut terminal = minted;
        terminal["plan_usage_reservation_token"] = json!(reservation_token);
        terminal["plan_usage_reservation_deferred"] = json!(false);
        let resolved = resolve_bound_internal_gateway_report_context(
            &state,
            "trace-capability-reservation-123",
            "openai_chat_sync_success",
            Some(&terminal),
        )
        .await
        .expect("capability lookup should succeed")
        .expect("a server-issued terminal reservation token should validate");
        assert_eq!(resolved["plan_usage_reservation_deferred"], json!(false));
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
                    "client_api_format": "claude:messages",
                    "provider_api_format": "openai:responses"
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
    async fn submit_openai_responses_stream_report_marks_missing_terminal_event_as_failed() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
            sample_request_candidate(
                "cand-reporting-stream-missing-terminal-1",
                "req-reporting-stream-missing-terminal-1",
            ),
        ]));
        let state = build_test_state(Arc::clone(&repository));
        let provider_sse = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n"
        );

        submit_stream_report(
            &state,
            GatewayStreamReportRequest {
                trace_id: "trace-reporting-stream-missing-terminal-1".to_string(),
                report_kind: "openai_responses_stream_success".to_string(),
                report_context: Some(json!({
                    "request_id": "req-reporting-stream-missing-terminal-1",
                    "client_api_format": "openai:responses",
                    "provider_api_format": "openai:responses"
                })),
                status_code: 200,
                headers: BTreeMap::new(),
                provider_body_base64: Some(
                    base64::engine::general_purpose::STANDARD.encode(provider_sse.as_bytes()),
                ),
                provider_body_state: Some(UsageBodyCaptureState::Inline),
                client_body_base64: None,
                client_body_state: None,
                terminal_summary: None,
                telemetry: None,
            },
        )
        .await
        .expect("stream report should stay local");

        let stored = repository
            .list_by_request_id("req-reporting-stream-missing-terminal-1")
            .await
            .expect("request candidates should list");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].status, RequestCandidateStatus::Failed);
        assert_eq!(stored[0].status_code, Some(200));
        assert_eq!(
            stored[0].error_type.as_deref(),
            Some("stream_missing_terminal_event")
        );
        assert_eq!(
            stored[0].error_message.as_deref(),
            Some(super::STREAM_MISSING_TERMINAL_EVENT_MESSAGE)
        );
        let mut public_candidate = stored[0].clone();
        public_candidate.sanitize_sensitive_diagnostics();
        assert!(public_candidate.error_message.is_none());
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
                report_kind: "openai_responses_sync_success".to_string(),
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
    async fn submit_sync_report_updates_codex_quota_from_provider_response_headers() {
        crate::orchestration::clear_local_report_effect_caches_for_tests();

        let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider_catalog_provider(
                "provider-codex-sync-provider-headers",
                "codex",
            )],
            Vec::new(),
            vec![sample_provider_catalog_key(
                "key-codex-sync-provider-headers",
                "provider-codex-sync-provider-headers",
            )],
        ));
        let state = build_provider_catalog_test_state(Arc::clone(&provider_catalog_repository));

        submit_sync_report(
            &state,
            GatewaySyncReportRequest {
                trace_id: "trace-codex-reporting-sync-provider-headers".to_string(),
                report_kind: "openai_responses_sync_success".to_string(),
                report_context: Some(json!({
                    "request_id": "req-codex-reporting-sync-provider-headers",
                    "key_id": "key-codex-sync-provider-headers",
                    "provider_response_headers": sample_codex_paid_headers()
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

        let reloaded = provider_catalog_repository
            .list_keys_by_ids(&["key-codex-sync-provider-headers".to_string()])
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
                report_kind: "openai_responses_stream_success".to_string(),
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
    async fn submit_stream_report_updates_codex_quota_from_websocket_response_body() {
        crate::orchestration::clear_local_report_effect_caches_for_tests();

        let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider_catalog_provider(
                "provider-codex-websocket",
                "codex",
            )],
            Vec::new(),
            vec![sample_provider_catalog_key(
                "key-codex-websocket",
                "provider-codex-websocket",
            )],
        ));
        let state = build_provider_catalog_test_state(Arc::clone(&provider_catalog_repository));
        let websocket_event = json!({
            "chunks": [{
                "type": "codex.rate_limits",
                "plan_type": "free",
                "rate_limits": {
                    "allowed": true,
                    "limit_reached": false,
                    "primary": {
                        "used_percent": 91,
                        "window_minutes": 43200,
                        "reset_after_seconds": 2590791,
                        "reset_at": 1787154563u64
                    }
                }
            }]
        });
        let body = format!("data: {websocket_event}\n\n");

        submit_stream_report(
            &state,
            GatewayStreamReportRequest {
                trace_id: "trace-codex-reporting-websocket".to_string(),
                report_kind: "openai_responses_stream_success".to_string(),
                report_context: Some(json!({
                    "request_id": "req-codex-reporting-websocket",
                    "key_id": "key-codex-websocket",
                    "websocket_mode": true
                })),
                status_code: 200,
                headers: sample_codex_paid_headers(),
                provider_body_base64: Some(
                    base64::engine::general_purpose::STANDARD.encode(body.as_bytes()),
                ),
                provider_body_state: Some(UsageBodyCaptureState::Inline),
                client_body_base64: None,
                client_body_state: None,
                terminal_summary: None,
                telemetry: None,
            },
        )
        .await
        .expect("stream report should stay local");

        let reloaded = provider_catalog_repository
            .list_keys_by_ids(&["key-codex-websocket".to_string()])
            .await
            .expect("keys should list");
        let codex = reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get("codex"))
            .and_then(serde_json::Value::as_object)
            .expect("codex metadata should exist");
        assert_eq!(codex.get("plan_type"), Some(&json!("free")));
        assert_eq!(codex.get("allowed"), Some(&json!(true)));
        assert_eq!(codex.get("limit_reached"), Some(&json!(false)));
        assert_eq!(codex.get("primary_used_percent"), Some(&json!(91.0)));
        assert_eq!(codex.get("primary_window_minutes"), Some(&json!(43_200u64)));

        let quota = reloaded[0]
            .status_snapshot
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|snapshot| snapshot.get("quota"))
            .and_then(serde_json::Value::as_object)
            .expect("quota snapshot should exist");
        assert_eq!(quota.get("source"), Some(&json!("websocket_response_body")));
        assert_eq!(quota.get("code"), Some(&json!("ok")));
        assert_eq!(quota.get("usage_ratio"), Some(&json!(0.91)));
    }

    #[tokio::test]
    async fn submit_stream_report_marks_codex_websocket_usage_limit_error_exhausted() {
        crate::orchestration::clear_local_report_effect_caches_for_tests();

        let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider_catalog_provider(
                "provider-codex-websocket-limit",
                "codex",
            )],
            Vec::new(),
            vec![sample_provider_catalog_key(
                "key-codex-websocket-limit",
                "provider-codex-websocket-limit",
            )],
        ));
        let state = build_provider_catalog_test_state(Arc::clone(&provider_catalog_repository));
        let websocket_event = json!({
            "type": "error",
            "error": {
                "type": "usage_limit_reached",
                "plan_type": "free",
                "resets_at": 1_787_274_385u64,
                "resets_in_seconds": 2_590_077u64,
            },
            "status_code": 429,
            "headers": {
                "X-Codex-Plan-Type": "free",
                "X-Codex-Primary-Used-Percent": "100",
                "X-Codex-Primary-Window-Minutes": "43200",
                "X-Codex-Primary-Reset-After-Seconds": "2590078",
                "X-Codex-Primary-Reset-At": "1787274385",
                "X-Codex-Credits-Has-Credits": "False",
            },
        });
        let body = format!("data: {websocket_event}\n\n");

        submit_stream_report(
            &state,
            GatewayStreamReportRequest {
                trace_id: "trace-codex-reporting-websocket-limit".to_string(),
                report_kind: "openai_responses_stream_success".to_string(),
                report_context: Some(json!({
                    "request_id": "req-codex-reporting-websocket-limit",
                    "key_id": "key-codex-websocket-limit",
                    "websocket_mode": true,
                })),
                status_code: 429,
                headers: BTreeMap::new(),
                provider_body_base64: Some(
                    base64::engine::general_purpose::STANDARD.encode(body.as_bytes()),
                ),
                provider_body_state: Some(UsageBodyCaptureState::Inline),
                client_body_base64: None,
                client_body_state: None,
                terminal_summary: None,
                telemetry: None,
            },
        )
        .await
        .expect("stream report should stay local");

        let reloaded = provider_catalog_repository
            .list_keys_by_ids(&["key-codex-websocket-limit".to_string()])
            .await
            .expect("keys should list");
        let codex = reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get("codex"))
            .and_then(serde_json::Value::as_object)
            .expect("codex metadata should exist");
        assert_eq!(codex.get("allowed"), Some(&json!(false)));
        assert_eq!(codex.get("limit_reached"), Some(&json!(true)));
        assert_eq!(codex.get("primary_used_percent"), Some(&json!(100.0)));
        assert_eq!(
            codex.get("primary_reset_at"),
            Some(&json!(1_787_274_385u64))
        );

        let quota = reloaded[0]
            .status_snapshot
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|snapshot| snapshot.get("quota"))
            .and_then(serde_json::Value::as_object)
            .expect("quota snapshot should exist");
        assert_eq!(quota.get("source"), Some(&json!("websocket_response_body")));
        assert_eq!(quota.get("code"), Some(&json!("exhausted")));
    }

    #[tokio::test]
    async fn submit_stream_report_updates_codex_quota_from_provider_response_headers() {
        crate::orchestration::clear_local_report_effect_caches_for_tests();

        let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider_catalog_provider(
                "provider-codex-stream-provider-headers",
                "codex",
            )],
            Vec::new(),
            vec![sample_provider_catalog_key(
                "key-codex-stream-provider-headers",
                "provider-codex-stream-provider-headers",
            )],
        ));
        let state = build_provider_catalog_test_state(Arc::clone(&provider_catalog_repository));

        submit_stream_report(
            &state,
            GatewayStreamReportRequest {
                trace_id: "trace-codex-reporting-stream-provider-headers".to_string(),
                report_kind: "openai_responses_stream_success".to_string(),
                report_context: Some(json!({
                    "request_id": "req-codex-reporting-stream-provider-headers",
                    "key_id": "key-codex-stream-provider-headers",
                    "provider_response_headers": sample_codex_paid_headers()
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

        let reloaded = provider_catalog_repository
            .list_keys_by_ids(&["key-codex-stream-provider-headers".to_string()])
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
        let mut mapping =
            aether_data::repository::gemini_file_mappings::StoredGeminiFileMapping::new(
                "mapping-gemini-files-delete-123".to_string(),
                "files/delete-me".to_string(),
                "key-reporting-tests-123".to_string(),
                1_700_000_000,
                1_700_172_800,
            )
            .expect("gemini file mapping should build");
        mapping.user_id = Some("user-reporting-tests-123".to_string());
        let gemini_file_mapping_repository =
            Arc::new(InMemoryGeminiFileMappingRepository::seed([mapping]));
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
                    "user_id": "user-reporting-tests-123",
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
            "ext-video-task-reporting-123",
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
            "ext-video-task-reporting-123",
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
            "ext-video-task-reporting-123",
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
            "models/veo-3/operations/ext-gemini-video-123",
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

    #[tokio::test]
    async fn report_context_task_id_collision_stays_bound_to_requested_user() {
        let video_repository = Arc::new(InMemoryVideoTaskRepository::default());
        seed_video_task(
            &video_repository,
            "shared-task-id",
            Some("victim-short-id"),
            "victim-external-id",
            "req-video-victim",
            "user-video-victim",
            "api-key-video-victim",
            "provider-video-victim",
            "endpoint-video-victim",
            "key-video-victim",
            "gemini:video",
            "gemini:video",
        )
        .await;
        seed_video_task(
            &video_repository,
            "owner-local-id",
            Some("owner-short-id"),
            "shared-task-id",
            "req-video-owner",
            "user-video-owner",
            "api-key-video-owner",
            "provider-video-owner",
            "endpoint-video-owner",
            "key-video-owner",
            "gemini:video",
            "gemini:video",
        )
        .await;
        let state = build_video_test_state(
            video_repository,
            Arc::new(InMemoryRequestCandidateRepository::default()),
        );

        let resolved = resolve_locally_actionable_report_context(
            &state,
            Some(&json!({
                "task_id": "shared-task-id",
                "user_id": "user-video-owner",
            })),
        )
        .await
        .expect("the owner's external task id should resolve");

        assert_eq!(resolved["request_id"], "req-video-owner");
        assert_eq!(resolved["provider_id"], "provider-video-owner");
        assert_eq!(resolved["user_id"], "user-video-owner");

        let foreign_local_id = resolve_locally_actionable_report_context(
            &state,
            Some(&json!({
                "local_task_id": "shared-task-id",
                "user_id": "user-video-owner",
            })),
        )
        .await;
        assert!(foreign_local_id.is_none());
    }
}
