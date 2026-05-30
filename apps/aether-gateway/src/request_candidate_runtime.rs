use aether_contracts::ExecutionPlan;
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate, UpsertRequestCandidateRecord,
};
use aether_scheduler_core::{
    build_execution_request_candidate_seed, build_local_request_candidate_status_record,
    build_report_request_candidate_status_record,
    finalize_execution_request_candidate_report_context, parse_request_candidate_report_context,
    resolve_report_request_candidate_slot as resolve_report_request_candidate_slot_from_candidates,
    LocalRequestCandidateStatusRecordInput, ReportRequestCandidateStatusRecordInput,
    SchedulerMinimalCandidateSelectionCandidate, SchedulerRequestCandidateStatusUpdate,
    SchedulerResolvedReportRequestCandidateSlot,
};
use aether_usage_runtime::build_locally_actionable_report_context_from_request_candidate;
use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::clock::current_unix_ms;
use crate::log_ids::short_request_id;
use crate::GatewayError;

#[derive(Debug, Clone)]
pub(crate) struct LocalRequestCandidateStatusSnapshot {
    candidate_id: String,
    request_id: String,
    user_id: Option<String>,
    api_key_id: Option<String>,
    candidate_index: u32,
    retry_index: u32,
    provider_id: String,
    endpoint_id: String,
    key_id: String,
}

#[async_trait]
pub(crate) trait RequestCandidateRuntimeReader {
    async fn read_request_candidates_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredRequestCandidate>, GatewayError>;
}

#[async_trait]
pub(crate) trait RequestCandidateRuntimeWriter {
    fn has_request_candidate_data_writer(&self) -> bool;

    async fn upsert_request_candidate(
        &self,
        candidate: UpsertRequestCandidateRecord,
    ) -> Result<Option<StoredRequestCandidate>, GatewayError>;
}

#[async_trait]
pub(crate) trait RequestCandidateRuntimeCapabilityReader {
    async fn read_request_candidate_user_model_capability_settings(
        &self,
        user_id: &str,
    ) -> Result<Option<Value>, GatewayError>;

    async fn read_request_candidate_api_key_force_capabilities(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<Option<Value>, GatewayError>;
}

pub(crate) async fn resolve_request_candidate_required_capabilities(
    state: &(impl RequestCandidateRuntimeCapabilityReader + ?Sized),
    user_id: &str,
    api_key_id: &str,
    requested_model: Option<&str>,
    explicit_required_capabilities: Option<&Value>,
    enable_model_directives: bool,
) -> Option<Value> {
    let mut merged = serde_json::Map::new();

    match state
        .read_request_candidate_user_model_capability_settings(user_id)
        .await
    {
        Ok(settings) => merge_capability_object(
            &mut merged,
            select_requested_model_capabilities(
                settings.as_ref(),
                requested_model,
                enable_model_directives,
            ),
        ),
        Err(error) => {
            warn!(
                user_id = %user_id,
                api_key_id = %api_key_id,
                requested_model = requested_model.unwrap_or_default(),
                error = ?error,
                "gateway request candidate user model capabilities lookup failed"
            );
        }
    }

    match state
        .read_request_candidate_api_key_force_capabilities(user_id, api_key_id)
        .await
    {
        Ok(force_capabilities) => {
            merge_capability_object(&mut merged, force_capabilities.as_ref());
        }
        Err(error) => {
            warn!(
                user_id = %user_id,
                api_key_id = %api_key_id,
                requested_model = requested_model.unwrap_or_default(),
                error = ?error,
                "gateway request candidate api key capabilities lookup failed"
            );
        }
    }

    merge_capability_object(&mut merged, explicit_required_capabilities);

    (!merged.is_empty()).then_some(Value::Object(merged))
}

fn merge_capability_object(target: &mut serde_json::Map<String, Value>, source: Option<&Value>) {
    let Some(source) = source.and_then(Value::as_object) else {
        return;
    };

    for (capability, value) in source {
        if capability.trim().is_empty() {
            continue;
        }
        target.insert(capability.clone(), value.clone());
    }
}

fn select_requested_model_capabilities<'a>(
    settings: Option<&'a Value>,
    requested_model: Option<&str>,
    enable_model_directives: bool,
) -> Option<&'a Value> {
    let requested_model = requested_model
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let settings = settings?.as_object()?;

    find_model_capabilities(settings, requested_model).or_else(|| {
        enable_model_directives
            .then(|| crate::ai_serving::model_directive_base_model(requested_model))
            .flatten()
            .as_deref()
            .and_then(|base_model| find_model_capabilities(settings, base_model))
    })
}

fn find_model_capabilities<'a>(
    settings: &'a serde_json::Map<String, Value>,
    requested_model: &str,
) -> Option<&'a Value> {
    settings.get(requested_model).or_else(|| {
        settings.iter().find_map(|(model_name, capabilities)| {
            model_name
                .trim()
                .eq_ignore_ascii_case(requested_model)
                .then_some(capabilities)
        })
    })
}

fn request_candidate_status_label(status: RequestCandidateStatus) -> &'static str {
    match status {
        RequestCandidateStatus::Available => "available",
        RequestCandidateStatus::Unused => "unused",
        RequestCandidateStatus::Pending => "pending",
        RequestCandidateStatus::Streaming => "streaming",
        RequestCandidateStatus::Success => "success",
        RequestCandidateStatus::Failed => "failed",
        RequestCandidateStatus::Cancelled => "cancelled",
        RequestCandidateStatus::Skipped => "skipped",
    }
}

pub(crate) fn snapshot_local_request_candidate_status(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Option<LocalRequestCandidateStatusSnapshot> {
    let candidate_id = plan
        .candidate_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let metadata = parse_request_candidate_report_context(report_context);
    let candidate_index = metadata
        .as_ref()
        .and_then(|metadata| metadata.candidate_index)
        .unwrap_or(0);

    Some(LocalRequestCandidateStatusSnapshot {
        candidate_id: candidate_id.to_string(),
        request_id: plan.request_id.clone(),
        user_id: metadata
            .as_ref()
            .and_then(|metadata| metadata.user_id.clone()),
        api_key_id: metadata
            .as_ref()
            .and_then(|metadata| metadata.api_key_id.clone()),
        candidate_index,
        retry_index: metadata
            .as_ref()
            .map(|metadata| metadata.retry_index)
            .unwrap_or(0),
        provider_id: plan.provider_id.clone(),
        endpoint_id: plan.endpoint_id.clone(),
        key_id: plan.key_id.clone(),
    })
}

async fn persist_local_request_candidate_status_record(
    state: &(impl RequestCandidateRuntimeWriter + ?Sized),
    record: UpsertRequestCandidateRecord,
) {
    let candidate_id = record.id.clone();
    let request_id = short_request_id(record.request_id.as_str());
    let candidate_index = record.candidate_index;
    let retry_index = record.retry_index;
    let status = record.status;

    match state.upsert_request_candidate(record).await {
        Ok(Some(stored)) => {
            debug!(
                event_name = "request_candidate_status_persisted",
                log_type = "event",
                request_id = %request_id,
                candidate_id = %stored.id,
                candidate_index,
                retry_index,
                status = request_candidate_status_label(status),
                source = "local_status",
                "gateway persisted request candidate status update"
            );
        }
        Ok(None) => {
            warn!(
                event_name = "request_candidate_writer_unavailable",
                log_type = "event",
                request_id = %request_id,
                candidate_id = %candidate_id,
                candidate_index,
                retry_index,
                status = request_candidate_status_label(status),
                source = "local_status",
                "gateway skipped request candidate persistence because writer is unavailable"
            );
        }
        Err(err) => {
            warn!(
                event_name = "request_candidate_status_persist_failed",
                log_type = "event",
                request_id = %request_id,
                candidate_id = %candidate_id,
                error = ?err,
                "gateway failed to persist request candidate status update"
            );
        }
    }
}

pub(crate) async fn record_local_request_candidate_status(
    state: &(impl RequestCandidateRuntimeWriter + ?Sized),
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    status_update: SchedulerRequestCandidateStatusUpdate,
) {
    let Some(record) =
        build_local_request_candidate_status_record(LocalRequestCandidateStatusRecordInput {
            plan,
            report_context,
            status_update,
        })
    else {
        return;
    };
    persist_local_request_candidate_status_record(state, record).await;
}

pub(crate) async fn record_local_request_candidate_extra_data(
    state: &(impl RequestCandidateRuntimeWriter + ?Sized),
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    status: RequestCandidateStatus,
    status_code: Option<u16>,
    latency_ms: Option<u64>,
    extra_data: Value,
) {
    let Some(snapshot) = snapshot_local_request_candidate_status(plan, report_context) else {
        return;
    };
    let record = UpsertRequestCandidateRecord {
        id: snapshot.candidate_id.clone(),
        request_id: snapshot.request_id.clone(),
        user_id: snapshot.user_id.clone(),
        api_key_id: snapshot.api_key_id.clone(),
        username: None,
        api_key_name: None,
        candidate_index: snapshot.candidate_index,
        retry_index: snapshot.retry_index,
        provider_id: Some(snapshot.provider_id.clone()),
        endpoint_id: Some(snapshot.endpoint_id.clone()),
        key_id: Some(snapshot.key_id.clone()),
        status,
        skip_reason: None,
        is_cached: None,
        status_code,
        error_type: None,
        error_message: None,
        latency_ms,
        concurrent_requests: None,
        extra_data: Some(extra_data),
        required_capabilities: None,
        created_at_unix_ms: None,
        started_at_unix_ms: None,
        finished_at_unix_ms: None,
    };
    persist_local_request_candidate_status_record(state, record).await;
}

pub(crate) async fn record_local_request_candidate_status_snapshot(
    state: &(impl RequestCandidateRuntimeWriter + ?Sized),
    snapshot: &LocalRequestCandidateStatusSnapshot,
    status_update: SchedulerRequestCandidateStatusUpdate,
) {
    let SchedulerRequestCandidateStatusUpdate {
        status,
        status_code,
        error_type,
        error_message,
        latency_ms,
        started_at_unix_ms,
        finished_at_unix_ms,
    } = status_update;
    let record = UpsertRequestCandidateRecord {
        id: snapshot.candidate_id.clone(),
        request_id: snapshot.request_id.clone(),
        user_id: snapshot.user_id.clone(),
        api_key_id: snapshot.api_key_id.clone(),
        username: None,
        api_key_name: None,
        candidate_index: snapshot.candidate_index,
        retry_index: snapshot.retry_index,
        provider_id: Some(snapshot.provider_id.clone()),
        endpoint_id: Some(snapshot.endpoint_id.clone()),
        key_id: Some(snapshot.key_id.clone()),
        status,
        skip_reason: None,
        is_cached: None,
        status_code,
        error_type,
        error_message,
        latency_ms,
        concurrent_requests: None,
        extra_data: None,
        required_capabilities: None,
        created_at_unix_ms: None,
        started_at_unix_ms,
        finished_at_unix_ms,
    };
    persist_local_request_candidate_status_record(state, record).await;
}

pub(crate) async fn record_report_request_candidate_status(
    state: &(impl RequestCandidateRuntimeReader + RequestCandidateRuntimeWriter + ?Sized),
    report_context: Option<&Value>,
    status_update: SchedulerRequestCandidateStatusUpdate,
) {
    let Some(slot) = resolve_report_request_candidate_slot(state, report_context).await else {
        return;
    };
    let request_id = slot.request_id.clone();
    let request_id_for_log = short_request_id(request_id.as_str());
    let candidate_index = slot.candidate_index;
    let retry_index = slot.retry_index;
    let record =
        build_report_request_candidate_status_record(ReportRequestCandidateStatusRecordInput {
            slot,
            status_update,
            now_unix_ms: current_unix_ms(),
        });
    let candidate_id = record.id.clone();
    let status = record.status;

    match state.upsert_request_candidate(record).await {
        Ok(Some(stored)) => {
            debug!(
                event_name = "request_candidate_report_status_persisted",
                log_type = "event",
                request_id = %request_id_for_log,
                candidate_id = %stored.id,
                candidate_index,
                retry_index,
                status = request_candidate_status_label(status),
                source = "report_status",
                "gateway persisted report-driven request candidate status update"
            );
        }
        Ok(None) => {
            warn!(
                event_name = "request_candidate_writer_unavailable",
                log_type = "event",
                request_id = %request_id_for_log,
                candidate_id = %candidate_id,
                candidate_index,
                retry_index,
                status = request_candidate_status_label(status),
                source = "report_status",
                "gateway skipped request candidate persistence because writer is unavailable"
            );
        }
        Err(err) => {
            warn!(
                event_name = "request_candidate_report_status_persist_failed",
                log_type = "event",
                request_id = %request_id_for_log,
                candidate_index,
                retry_index,
                error = ?err,
                "gateway failed to persist report-driven request candidate status update"
            );
        }
    }
}

pub(crate) async fn ensure_execution_request_candidate_slot(
    state: &(impl RequestCandidateRuntimeWriter + ?Sized),
    plan: &mut ExecutionPlan,
    report_context: &mut Option<Value>,
) {
    if !state.has_request_candidate_data_writer() {
        warn!(
            event_name = "request_candidate_writer_unavailable",
            log_type = "event",
            request_id = %short_request_id(plan.request_id.as_str()),
            provider_id = %plan.provider_id,
            endpoint_id = %plan.endpoint_id,
            key_id = %plan.key_id,
            source = "seed",
            "gateway skipped request candidate seed because writer is unavailable"
        );
        return;
    }
    let existing_candidate_id = plan
        .candidate_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let report_candidate_id = parse_request_candidate_report_context(report_context.as_ref())
        .and_then(|metadata| metadata.candidate_id);
    if existing_candidate_id.as_deref().is_some()
        && report_candidate_id.as_deref() == existing_candidate_id.as_deref()
    {
        return;
    }

    let seed = build_execution_request_candidate_seed(
        plan,
        report_context.as_ref(),
        current_unix_ms(),
        existing_candidate_id.unwrap_or_else(|| Uuid::new_v4().to_string()),
    );
    let generated_candidate_id = seed.upsert_record.id.clone();
    let request_id = short_request_id(plan.request_id.as_str());

    let candidate_id = match state.upsert_request_candidate(seed.upsert_record).await {
        Ok(Some(stored)) => {
            info!(
                event_name = "request_candidate_slot_seeded",
                log_type = "event",
                request_id = %request_id,
                candidate_id = %stored.id,
                provider_id = %plan.provider_id,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                source = "seed",
                "gateway seeded execution request candidate slot"
            );
            stored.id
        }
        Ok(None) => {
            warn!(
                event_name = "request_candidate_writer_unavailable",
                log_type = "event",
                request_id = %request_id,
                candidate_id = %generated_candidate_id,
                provider_id = %plan.provider_id,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                source = "seed",
                "gateway skipped request candidate seed because writer is unavailable"
            );
            generated_candidate_id
        }
        Err(err) => {
            warn!(
                event_name = "request_candidate_slot_seed_failed",
                log_type = "event",
                request_id = %request_id,
                error = ?err,
                "gateway failed to seed execution request candidate slot"
            );
            return;
        }
    };

    plan.candidate_id = Some(candidate_id.clone());
    *report_context = Some(finalize_execution_request_candidate_report_context(
        seed.report_context,
        &candidate_id,
    ));
}

pub(crate) async fn persist_available_local_candidate(
    state: &(impl RequestCandidateRuntimeWriter + ?Sized),
    trace_id: &str,
    user_id: &str,
    api_key_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    retry_index: u32,
    candidate_id: &str,
    required_capabilities: Option<&Value>,
    extra_data: Option<serde_json::Value>,
    created_at_unix_ms: u64,
    error_context: &'static str,
) -> String {
    match state
        .upsert_request_candidate(UpsertRequestCandidateRecord {
            id: candidate_id.to_string(),
            request_id: trace_id.to_string(),
            user_id: Some(user_id.to_string()),
            api_key_id: Some(api_key_id.to_string()),
            username: None,
            api_key_name: None,
            candidate_index,
            retry_index,
            provider_id: Some(candidate.provider_id.clone()),
            endpoint_id: Some(candidate.endpoint_id.clone()),
            key_id: Some(candidate.key_id.clone()),
            status: RequestCandidateStatus::Available,
            skip_reason: None,
            is_cached: Some(false),
            status_code: None,
            error_type: None,
            error_message: None,
            latency_ms: None,
            concurrent_requests: None,
            extra_data,
            required_capabilities: required_capabilities.cloned(),
            created_at_unix_ms: Some(created_at_unix_ms),
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
        })
        .await
    {
        Ok(Some(stored)) => {
            debug!(
                event_name = "request_candidate_status_persisted",
                log_type = "event",
                request_id = %short_request_id(trace_id),
                candidate_id = %stored.id,
                candidate_index,
                retry_index,
                status = "available",
                source = "planner_available",
                provider_id = %candidate.provider_id,
                endpoint_id = %candidate.endpoint_id,
                key_id = %candidate.key_id,
                has_required_capabilities = required_capabilities.is_some(),
                "gateway persisted available local request candidate"
            );
            stored.id
        }
        Ok(None) => {
            warn!(
                event_name = "request_candidate_writer_unavailable",
                log_type = "event",
                request_id = %short_request_id(trace_id),
                candidate_id = %candidate_id,
                candidate_index,
                retry_index,
                status = "available",
                source = "planner_available",
                provider_id = %candidate.provider_id,
                endpoint_id = %candidate.endpoint_id,
                key_id = %candidate.key_id,
                "gateway skipped request candidate persistence because writer is unavailable"
            );
            candidate_id.to_string()
        }
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                candidate_id = %candidate_id,
                error = ?err,
                "{error_context}"
            );
            candidate_id.to_string()
        }
    }
}

pub(crate) async fn persist_skipped_local_candidate(
    state: &(impl RequestCandidateRuntimeWriter + ?Sized),
    trace_id: &str,
    user_id: &str,
    api_key_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    retry_index: u32,
    candidate_id: &str,
    required_capabilities: Option<&Value>,
    skip_reason: &str,
    extra_data: Option<serde_json::Value>,
    finished_at_unix_ms: u64,
    error_context: &'static str,
) {
    match state
        .upsert_request_candidate(UpsertRequestCandidateRecord {
            id: candidate_id.to_string(),
            request_id: trace_id.to_string(),
            user_id: Some(user_id.to_string()),
            api_key_id: Some(api_key_id.to_string()),
            username: None,
            api_key_name: None,
            candidate_index,
            retry_index,
            provider_id: Some(candidate.provider_id.clone()),
            endpoint_id: Some(candidate.endpoint_id.clone()),
            key_id: Some(candidate.key_id.clone()),
            status: RequestCandidateStatus::Skipped,
            skip_reason: Some(skip_reason.to_string()),
            is_cached: Some(false),
            status_code: None,
            error_type: None,
            error_message: None,
            latency_ms: None,
            concurrent_requests: None,
            extra_data,
            required_capabilities: required_capabilities.cloned(),
            created_at_unix_ms: None,
            started_at_unix_ms: None,
            finished_at_unix_ms: Some(finished_at_unix_ms),
        })
        .await
    {
        Ok(Some(stored)) => {
            debug!(
                event_name = "request_candidate_status_persisted",
                log_type = "event",
                request_id = %short_request_id(trace_id),
                candidate_id = %stored.id,
                candidate_index,
                retry_index,
                status = "skipped",
                skip_reason,
                source = "planner_skipped",
                provider_id = %candidate.provider_id,
                endpoint_id = %candidate.endpoint_id,
                key_id = %candidate.key_id,
                has_required_capabilities = required_capabilities.is_some(),
                "gateway persisted skipped local request candidate"
            );
        }
        Ok(None) => {
            warn!(
                event_name = "request_candidate_writer_unavailable",
                log_type = "event",
                request_id = %short_request_id(trace_id),
                candidate_id = %candidate_id,
                candidate_index,
                retry_index,
                status = "skipped",
                skip_reason,
                source = "planner_skipped",
                provider_id = %candidate.provider_id,
                endpoint_id = %candidate.endpoint_id,
                key_id = %candidate.key_id,
                "gateway skipped request candidate persistence because writer is unavailable"
            );
        }
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                candidate_id = %candidate_id,
                skip_reason,
                error = ?err,
                "{error_context}"
            );
        }
    }
}

pub(crate) async fn resolve_locally_actionable_request_candidate_report_context(
    state: &(impl RequestCandidateRuntimeReader + ?Sized),
    context: &Value,
) -> Option<Value> {
    let request_id = context
        .get("request_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let existing_candidates = state
        .read_request_candidates_by_request_id(request_id)
        .await
        .ok()?;
    if existing_candidates.len() != 1 {
        return None;
    }

    build_locally_actionable_report_context_from_request_candidate(context, &existing_candidates[0])
}

async fn resolve_report_request_candidate_slot(
    state: &(impl RequestCandidateRuntimeReader + ?Sized),
    report_context: Option<&Value>,
) -> Option<SchedulerResolvedReportRequestCandidateSlot> {
    let metadata = parse_request_candidate_report_context(report_context)?;
    let request_id = metadata.request_id.clone()?;
    let existing_candidates = state
        .read_request_candidates_by_request_id(request_id.as_str())
        .await
        .ok()
        .unwrap_or_default();
    resolve_report_request_candidate_slot_from_candidates(
        &existing_candidates,
        metadata,
        current_unix_ms(),
        Uuid::new_v4().to_string(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use aether_contracts::{ExecutionPlan, RequestBody};
    use aether_data::repository::auth::{
        InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord,
    };
    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_data::repository::usage::InMemoryUsageReadRepository;
    use aether_data_contracts::repository::candidates::{
        RequestCandidateReadRepository, RequestCandidateStatus, StoredRequestCandidate,
    };
    use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
    use serde_json::json;

    use super::{
        ensure_execution_request_candidate_slot, persist_available_local_candidate,
        record_report_request_candidate_status, resolve_request_candidate_required_capabilities,
        SchedulerRequestCandidateStatusUpdate,
    };
    use crate::data::GatewayDataState;
    use crate::AppState;

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

    fn build_test_state_with_auth(
        repository: Arc<InMemoryRequestCandidateRepository>,
        auth_repository: Arc<InMemoryAuthApiKeySnapshotRepository>,
    ) -> AppState {
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                    repository,
                    Arc::new(InMemoryUsageReadRepository::default()),
                )
                .with_auth_api_key_reader(auth_repository),
            )
    }

    fn sample_plan() -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-request-candidate-seed-123".to_string(),
            candidate_id: None,
            provider_name: Some("openai".to_string()),
            provider_id: "provider-request-candidate-seed-123".to_string(),
            endpoint_id: "endpoint-request-candidate-seed-123".to_string(),
            key_id: "key-request-candidate-seed-123".to_string(),
            method: "POST".to_string(),
            url: "https://api.openai.example/v1/chat/completions".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5", "messages": []})),
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn sample_minimal_candidate() -> SchedulerMinimalCandidateSelectionCandidate {
        SchedulerMinimalCandidateSelectionCandidate {
            provider_id: "provider-1".to_string(),
            provider_name: "Provider".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 0,
            endpoint_id: "endpoint-1".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            key_id: "provider-key-1".to_string(),
            key_name: "provider-key-1".to_string(),
            key_auth_type: "api_key".to_string(),
            key_internal_priority: 0,
            key_global_priority_for_format: Some(0),
            key_capabilities: Some(json!({"provider_only_capability": true})),
            model_id: "model-1".to_string(),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            selected_provider_model_name: "gpt-5".to_string(),
            mapping_matched_model: None,
        }
    }

    #[tokio::test]
    async fn seeds_execution_request_candidate_slot_for_plan_without_candidate_id() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = build_test_state(Arc::clone(&repository));
        let mut plan = sample_plan();
        let mut report_context = Some(json!({
            "request_id": "req-request-candidate-seed-123",
            "client_api_format": "openai:chat"
        }));

        ensure_execution_request_candidate_slot(&state, &mut plan, &mut report_context).await;

        let candidate_id = plan
            .candidate_id
            .clone()
            .expect("candidate id should be seeded");
        let report_context = report_context.expect("report context should be populated");
        assert_eq!(
            report_context
                .get("candidate_id")
                .and_then(|value| value.as_str()),
            Some(candidate_id.as_str())
        );
        assert_eq!(
            report_context
                .get("candidate_index")
                .and_then(|value| value.as_u64()),
            Some(0)
        );
        assert_eq!(
            report_context
                .get("provider_id")
                .and_then(|value| value.as_str()),
            Some("provider-request-candidate-seed-123")
        );

        let stored = repository
            .list_by_request_id("req-request-candidate-seed-123")
            .await
            .expect("request candidates should read");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, candidate_id);
        assert_eq!(stored[0].status, RequestCandidateStatus::Pending);
        assert_eq!(
            stored[0].provider_id.as_deref(),
            Some("provider-request-candidate-seed-123")
        );
        assert_eq!(
            stored[0].endpoint_id.as_deref(),
            Some("endpoint-request-candidate-seed-123")
        );
        assert_eq!(
            stored[0].key_id.as_deref(),
            Some("key-request-candidate-seed-123")
        );
    }

    #[tokio::test]
    async fn does_not_reseed_execution_request_candidate_slot_when_report_context_matches_plan_candidate_id(
    ) {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = build_test_state(Arc::clone(&repository));
        let mut plan = sample_plan();
        plan.candidate_id = Some("cand-existing-123".to_string());
        let mut report_context = Some(json!({
            "request_id": "req-request-candidate-seed-123",
            "candidate_id": "cand-existing-123"
        }));

        ensure_execution_request_candidate_slot(&state, &mut plan, &mut report_context).await;

        assert_eq!(plan.candidate_id.as_deref(), Some("cand-existing-123"));
        let stored = repository
            .list_by_request_id("req-request-candidate-seed-123")
            .await
            .expect("request candidates should read");
        assert!(stored.is_empty());
        assert_eq!(
            report_context
                .as_ref()
                .and_then(|value| value.get("candidate_id"))
                .and_then(|value| value.as_str()),
            Some("cand-existing-123")
        );
    }

    #[tokio::test]
    async fn seeds_execution_request_candidate_slot_when_plan_candidate_id_lacks_report_context() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = build_test_state(Arc::clone(&repository));
        let mut plan = sample_plan();
        plan.candidate_id = Some("cand-existing-123".to_string());
        let mut report_context = None;

        ensure_execution_request_candidate_slot(&state, &mut plan, &mut report_context).await;

        assert_eq!(plan.candidate_id.as_deref(), Some("cand-existing-123"));
        let report_context = report_context.expect("report context should be populated");
        assert_eq!(
            report_context
                .get("candidate_id")
                .and_then(|value| value.as_str()),
            Some("cand-existing-123")
        );
        let stored = repository
            .list_by_request_id("req-request-candidate-seed-123")
            .await
            .expect("request candidates should read");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, "cand-existing-123");
        assert_eq!(stored[0].status, RequestCandidateStatus::Pending);
    }

    #[tokio::test]
    async fn records_report_request_candidate_status_for_existing_slot() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
            StoredRequestCandidate::new(
                "cand-report-123".to_string(),
                "req-report-123".to_string(),
                Some("user-1".to_string()),
                Some("api-key-1".to_string()),
                None,
                None,
                0,
                0,
                Some("provider-report-123".to_string()),
                Some("endpoint-report-123".to_string()),
                Some("key-report-123".to_string()),
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
                100_000,
                Some(100_000),
                None,
            )
            .expect("request candidate should build"),
        ]));
        let state = build_test_state(Arc::clone(&repository));
        let report_context = json!({
            "request_id": "req-report-123",
            "candidate_id": "cand-report-123",
            "candidate_index": 0,
            "retry_index": 0,
            "provider_id": "provider-report-123",
            "endpoint_id": "endpoint-report-123",
            "key_id": "key-report-123"
        });

        record_report_request_candidate_status(
            &state,
            Some(&report_context),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Success,
                status_code: Some(200),
                error_type: None,
                error_message: None,
                latency_ms: Some(25),
                started_at_unix_ms: Some(101),
                finished_at_unix_ms: Some(102),
            },
        )
        .await;

        let stored = repository
            .list_by_request_id("req-report-123")
            .await
            .expect("request candidates should read");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, "cand-report-123");
        assert_eq!(stored[0].status, RequestCandidateStatus::Success);
        assert_eq!(stored[0].status_code, Some(200));
        assert_eq!(stored[0].latency_ms, Some(25));
        assert_eq!(stored[0].started_at_unix_ms, Some(101));
        assert_eq!(stored[0].finished_at_unix_ms, Some(102));
    }

    #[tokio::test]
    async fn resolves_request_candidate_required_capabilities_from_user_model_and_api_key() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let auth_repository = Arc::new(
            InMemoryAuthApiKeySnapshotRepository::default().with_export_records(vec![
                StoredAuthApiKeyExportRecord::new(
                    "user-1".to_string(),
                    "api-key-1".to_string(),
                    "hash-1".to_string(),
                    None,
                    Some("default".to_string()),
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(json!({"cache_1h": false, "context_1m": true})),
                    true,
                    None,
                    false,
                    0,
                    0,
                    0.0,
                    false,
                )
                .expect("export record should build"),
            ]),
        );
        let state = build_test_state_with_auth(repository, auth_repository)
            .with_auth_user_model_capability_settings_for_tests(
                "user-1",
                json!({
                    "gpt-5": {
                        "cache_1h": true,
                        "context_1m": false
                    }
                }),
            );
        let explicit_required_capabilities = json!({"gemini_files": true});

        let required_capabilities = resolve_request_candidate_required_capabilities(
            &state,
            "user-1",
            "api-key-1",
            Some("gpt-5"),
            Some(&explicit_required_capabilities),
            false,
        )
        .await
        .expect("required capabilities should resolve");

        assert_eq!(required_capabilities["cache_1h"], json!(false));
        assert_eq!(required_capabilities["context_1m"], json!(true));
        assert_eq!(required_capabilities["gemini_files"], json!(true));
    }

    #[tokio::test]
    async fn persists_request_required_capabilities_instead_of_provider_key_capabilities() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = build_test_state(Arc::clone(&repository));
        let required_capabilities = json!({"cache_1h": true});

        persist_available_local_candidate(
            &state,
            "req-runtime-cap-123",
            "user-1",
            "api-key-1",
            &sample_minimal_candidate(),
            0,
            0,
            "cand-runtime-cap-123",
            Some(&required_capabilities),
            None,
            100_000,
            "request candidate persist should succeed",
        )
        .await;

        let stored = repository
            .list_by_request_id("req-runtime-cap-123")
            .await
            .expect("request candidates should read");
        assert_eq!(stored.len(), 1);
        assert_eq!(
            stored[0].required_capabilities,
            Some(required_capabilities.clone())
        );
        assert_ne!(
            stored[0].required_capabilities,
            sample_minimal_candidate().key_capabilities
        );
    }
}
