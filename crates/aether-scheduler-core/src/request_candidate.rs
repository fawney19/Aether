use aether_contracts::{ExecutionError, ExecutionPlan};
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate, UpsertRequestCandidateRecord,
};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct SchedulerRequestCandidateReportContext {
    pub request_id: Option<String>,
    pub candidate_id: Option<String>,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub candidate_index: Option<u32>,
    pub retry_index: u32,
    pub provider_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub key_id: Option<String>,
    pub client_api_format: Option<String>,
    pub provider_api_format: Option<String>,
    pub proxy: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchedulerResolvedReportRequestCandidateSlot {
    pub id: String,
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub candidate_index: u32,
    pub retry_index: u32,
    pub provider_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub key_id: Option<String>,
    pub extra_data: Option<Value>,
    pub created_at_unix_ms: u64,
    pub started_at_unix_ms: Option<u64>,
    pub finished_at_unix_ms: Option<u64>,
}

pub struct SchedulerExecutionRequestCandidateSeed {
    pub upsert_record: UpsertRequestCandidateRecord,
    pub report_context: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerRequestCandidateStatusUpdate {
    pub status: RequestCandidateStatus,
    pub status_code: Option<u16>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub latency_ms: Option<u64>,
    pub started_at_unix_ms: Option<u64>,
    pub finished_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct LocalRequestCandidateStatusRecordInput<'a> {
    pub plan: &'a ExecutionPlan,
    pub report_context: Option<&'a Value>,
    pub status_update: SchedulerRequestCandidateStatusUpdate,
}

#[derive(Debug, Clone)]
pub struct ReportRequestCandidateStatusRecordInput {
    pub slot: SchedulerResolvedReportRequestCandidateSlot,
    pub status_update: SchedulerRequestCandidateStatusUpdate,
    pub now_unix_ms: u64,
}

pub fn execution_error_details(
    error: Option<&ExecutionError>,
    body_json: Option<&Value>,
) -> (Option<String>, Option<String>) {
    match error {
        Some(error) => (
            Some(format!("{:?}", error.kind)),
            Some(error.message.trim().to_string()).filter(|value| !value.is_empty()),
        ),
        None => (
            None,
            body_json
                .and_then(extract_error_message)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
        ),
    }
}

pub fn parse_request_candidate_report_context(
    report_context: Option<&Value>,
) -> Option<SchedulerRequestCandidateReportContext> {
    let report_context = report_context?;
    let retry_index = report_context
        .get("retry_index")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    Some(SchedulerRequestCandidateReportContext {
        request_id: string_field(report_context, "request_id"),
        candidate_id: string_field(report_context, "candidate_id"),
        user_id: string_field(report_context, "user_id"),
        api_key_id: string_field(report_context, "api_key_id"),
        candidate_index: report_context
            .get("candidate_index")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
        retry_index: u32::try_from(retry_index).unwrap_or(u32::MAX),
        provider_id: string_field(report_context, "provider_id"),
        endpoint_id: string_field(report_context, "endpoint_id"),
        key_id: string_field(report_context, "key_id"),
        client_api_format: string_field(report_context, "client_api_format"),
        provider_api_format: string_field(report_context, "provider_api_format"),
        proxy: report_context
            .get("proxy")
            .cloned()
            .filter(|value| !value.is_null()),
    })
}

pub fn resolve_report_request_candidate_slot(
    existing_candidates: &[StoredRequestCandidate],
    metadata: SchedulerRequestCandidateReportContext,
    now_unix_ms: u64,
    generated_candidate_id: String,
) -> Option<SchedulerResolvedReportRequestCandidateSlot> {
    let request_id = metadata.request_id.clone()?;
    let matched_candidate = match_existing_report_candidate(existing_candidates, &metadata);
    let synthesized_extra_data = build_report_candidate_extra_data(&metadata);
    let created_at_unix_ms = matched_candidate
        .as_ref()
        .map(|candidate| candidate.created_at_unix_ms)
        .unwrap_or(now_unix_ms);
    let candidate_index = matched_candidate
        .as_ref()
        .map(|candidate| candidate.candidate_index)
        .or(metadata.candidate_index)
        .unwrap_or_else(|| next_candidate_index(existing_candidates));
    let retry_index = matched_candidate
        .as_ref()
        .map(|candidate| candidate.retry_index)
        .unwrap_or(metadata.retry_index);

    Some(SchedulerResolvedReportRequestCandidateSlot {
        id: matched_candidate
            .as_ref()
            .map(|candidate| candidate.id.clone())
            .or(metadata.candidate_id)
            .unwrap_or(generated_candidate_id),
        request_id,
        user_id: matched_candidate
            .as_ref()
            .and_then(|candidate| candidate.user_id.clone())
            .or(metadata.user_id),
        api_key_id: matched_candidate
            .as_ref()
            .and_then(|candidate| candidate.api_key_id.clone())
            .or(metadata.api_key_id),
        candidate_index,
        retry_index,
        provider_id: matched_candidate
            .as_ref()
            .and_then(|candidate| candidate.provider_id.clone())
            .or(metadata.provider_id),
        endpoint_id: matched_candidate
            .as_ref()
            .and_then(|candidate| candidate.endpoint_id.clone())
            .or(metadata.endpoint_id),
        key_id: matched_candidate
            .as_ref()
            .and_then(|candidate| candidate.key_id.clone())
            .or(metadata.key_id),
        extra_data: merge_request_candidate_extra_data(
            matched_candidate
                .as_ref()
                .and_then(|candidate| candidate.extra_data.clone()),
            synthesized_extra_data,
        ),
        created_at_unix_ms,
        started_at_unix_ms: matched_candidate
            .as_ref()
            .and_then(|candidate| candidate.started_at_unix_ms),
        finished_at_unix_ms: matched_candidate
            .as_ref()
            .and_then(|candidate| candidate.finished_at_unix_ms),
    })
}

pub fn build_execution_request_candidate_seed(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    started_at_unix_ms: u64,
    generated_candidate_id: String,
) -> SchedulerExecutionRequestCandidateSeed {
    let mut context = report_context
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let request_id = string_field(&Value::Object(context.clone()), "request_id")
        .unwrap_or_else(|| plan.request_id.clone());
    let candidate_index = context
        .get("candidate_index")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0);
    let retry_index = context
        .get("retry_index")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0);
    let candidate_id = string_field(&Value::Object(context.clone()), "candidate_id")
        .unwrap_or(generated_candidate_id);
    let user_id = string_field(&Value::Object(context.clone()), "user_id");
    let api_key_id = string_field(&Value::Object(context.clone()), "api_key_id");

    context.insert("request_id".to_string(), Value::String(request_id.clone()));
    context.insert(
        "candidate_id".to_string(),
        Value::String(candidate_id.clone()),
    );
    context.insert(
        "candidate_index".to_string(),
        Value::Number(candidate_index.into()),
    );
    context.insert(
        "provider_id".to_string(),
        Value::String(plan.provider_id.clone()),
    );
    context.insert(
        "endpoint_id".to_string(),
        Value::String(plan.endpoint_id.clone()),
    );
    context.insert("key_id".to_string(), Value::String(plan.key_id.clone()));

    SchedulerExecutionRequestCandidateSeed {
        upsert_record: UpsertRequestCandidateRecord {
            id: candidate_id,
            request_id,
            user_id,
            api_key_id,
            username: None,
            api_key_name: None,
            candidate_index,
            retry_index,
            provider_id: Some(plan.provider_id.clone()),
            endpoint_id: Some(plan.endpoint_id.clone()),
            key_id: Some(plan.key_id.clone()),
            status: RequestCandidateStatus::Pending,
            skip_reason: None,
            is_cached: Some(false),
            status_code: None,
            error_type: None,
            error_message: None,
            latency_ms: None,
            concurrent_requests: None,
            extra_data: None,
            required_capabilities: None,
            created_at_unix_ms: Some(started_at_unix_ms),
            started_at_unix_ms: Some(started_at_unix_ms),
            finished_at_unix_ms: None,
        },
        report_context: Value::Object(context),
    }
}

pub fn build_local_request_candidate_status_record(
    input: LocalRequestCandidateStatusRecordInput<'_>,
) -> Option<UpsertRequestCandidateRecord> {
    let LocalRequestCandidateStatusRecordInput {
        plan,
        report_context,
        status_update,
    } = input;
    let SchedulerRequestCandidateStatusUpdate {
        status,
        status_code,
        error_type,
        error_message,
        latency_ms,
        started_at_unix_ms,
        finished_at_unix_ms,
    } = status_update;

    let candidate_id = plan
        .candidate_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let metadata = parse_request_candidate_report_context(report_context)?;
    let candidate_index = metadata.candidate_index?;

    Some(UpsertRequestCandidateRecord {
        id: candidate_id.to_string(),
        request_id: plan.request_id.clone(),
        user_id: metadata.user_id,
        api_key_id: metadata.api_key_id,
        username: None,
        api_key_name: None,
        candidate_index,
        retry_index: metadata.retry_index,
        provider_id: Some(plan.provider_id.clone()),
        endpoint_id: Some(plan.endpoint_id.clone()),
        key_id: Some(plan.key_id.clone()),
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
    })
}

pub fn build_report_request_candidate_status_record(
    input: ReportRequestCandidateStatusRecordInput,
) -> UpsertRequestCandidateRecord {
    let ReportRequestCandidateStatusRecordInput {
        slot,
        status_update,
        now_unix_ms,
    } = input;
    let SchedulerRequestCandidateStatusUpdate {
        status,
        status_code,
        error_type,
        error_message,
        latency_ms,
        started_at_unix_ms,
        finished_at_unix_ms,
    } = status_update;

    let terminal_unix_secs = finished_at_unix_ms.unwrap_or(now_unix_ms);
    let started_at_unix_ms = started_at_unix_ms
        .or(slot.started_at_unix_ms)
        .or_else(|| status.is_attempted(None).then_some(terminal_unix_secs));
    let finished_at_unix_ms = finished_at_unix_ms
        .or(slot.finished_at_unix_ms)
        .or_else(|| is_terminal_candidate_status(status).then_some(terminal_unix_secs));

    UpsertRequestCandidateRecord {
        id: slot.id,
        request_id: slot.request_id,
        user_id: slot.user_id,
        api_key_id: slot.api_key_id,
        username: None,
        api_key_name: None,
        candidate_index: slot.candidate_index,
        retry_index: slot.retry_index,
        provider_id: slot.provider_id,
        endpoint_id: slot.endpoint_id,
        key_id: slot.key_id,
        status,
        skip_reason: None,
        is_cached: None,
        status_code,
        error_type,
        error_message,
        latency_ms,
        concurrent_requests: None,
        extra_data: slot.extra_data,
        required_capabilities: None,
        created_at_unix_ms: Some(slot.created_at_unix_ms),
        started_at_unix_ms,
        finished_at_unix_ms,
    }
}

pub fn finalize_execution_request_candidate_report_context(
    report_context: Value,
    candidate_id: &str,
) -> Value {
    let mut context = report_context.as_object().cloned().unwrap_or_default();
    let candidate_id = candidate_id.trim();
    if !candidate_id.is_empty() {
        context.insert(
            "candidate_id".to_string(),
            Value::String(candidate_id.to_string()),
        );
    }
    Value::Object(context)
}

pub fn is_terminal_candidate_status(status: RequestCandidateStatus) -> bool {
    matches!(
        status,
        RequestCandidateStatus::Unused
            | RequestCandidateStatus::Success
            | RequestCandidateStatus::Failed
            | RequestCandidateStatus::Cancelled
            | RequestCandidateStatus::Skipped
    )
}

fn extract_error_message(body_json: &Value) -> Option<&str> {
    body_json
        .get("error")
        .and_then(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
        })
        .or_else(|| body_json.get("message").and_then(Value::as_str))
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn match_existing_report_candidate<'a>(
    candidates: &'a [StoredRequestCandidate],
    metadata: &SchedulerRequestCandidateReportContext,
) -> Option<&'a StoredRequestCandidate> {
    if let Some(candidate_id) = metadata.candidate_id.as_deref() {
        if let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.id == candidate_id)
        {
            return Some(candidate);
        }
    }

    if let Some(candidate_index) = metadata.candidate_index {
        if let Some(candidate) = candidates.iter().find(|candidate| {
            candidate.candidate_index == candidate_index
                && candidate.retry_index == metadata.retry_index
        }) {
            return Some(candidate);
        }
    }

    candidates
        .iter()
        .filter(|candidate| {
            candidate.provider_id.as_deref() == metadata.provider_id.as_deref()
                && candidate.endpoint_id.as_deref() == metadata.endpoint_id.as_deref()
                && candidate.key_id.as_deref() == metadata.key_id.as_deref()
        })
        .max_by_key(|candidate| {
            (
                candidate.retry_index,
                candidate.candidate_index,
                candidate.created_at_unix_ms,
            )
        })
}

fn next_candidate_index(candidates: &[StoredRequestCandidate]) -> u32 {
    candidates
        .iter()
        .map(|candidate| candidate.candidate_index)
        .max()
        .map(|value| value.saturating_add(1))
        .unwrap_or_default()
}

fn build_report_candidate_extra_data(
    metadata: &SchedulerRequestCandidateReportContext,
) -> Option<Value> {
    let mut extra_data = serde_json::Map::new();
    extra_data.insert("gateway_execution_runtime".to_string(), Value::Bool(true));
    extra_data.insert("phase".to_string(), Value::String("3c_trial".to_string()));
    if let Some(client_api_format) = metadata.client_api_format.clone() {
        extra_data.insert(
            "client_api_format".to_string(),
            Value::String(client_api_format),
        );
    }
    if let Some(provider_api_format) = metadata.provider_api_format.clone() {
        extra_data.insert(
            "provider_api_format".to_string(),
            Value::String(provider_api_format),
        );
    }
    if let Some(proxy) = metadata.proxy.clone() {
        extra_data.insert("proxy".to_string(), proxy);
    }
    (!extra_data.is_empty()).then_some(Value::Object(extra_data))
}

fn merge_request_candidate_extra_data(
    existing: Option<Value>,
    overlay: Option<Value>,
) -> Option<Value> {
    match (existing, overlay) {
        (Some(Value::Object(mut existing_object)), Some(Value::Object(overlay_object))) => {
            existing_object.extend(overlay_object);
            Some(Value::Object(existing_object))
        }
        (Some(existing), None) => Some(existing),
        (None, Some(overlay)) => Some(overlay),
        (Some(existing), Some(_overlay)) => Some(existing),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use aether_contracts::{
        ExecutionError, ExecutionErrorKind, ExecutionPhase, ExecutionPlan, RequestBody,
    };
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, StoredRequestCandidate,
    };
    use serde_json::{json, Value};

    use super::{
        build_execution_request_candidate_seed, build_local_request_candidate_status_record,
        build_report_request_candidate_status_record, execution_error_details,
        finalize_execution_request_candidate_report_context,
        parse_request_candidate_report_context, resolve_report_request_candidate_slot,
        LocalRequestCandidateStatusRecordInput, ReportRequestCandidateStatusRecordInput,
        SchedulerRequestCandidateStatusUpdate, SchedulerResolvedReportRequestCandidateSlot,
    };

    fn sample_candidate(
        id: &str,
        candidate_index: u32,
        retry_index: u32,
    ) -> StoredRequestCandidate {
        StoredRequestCandidate::new(
            id.to_string(),
            "req-1".to_string(),
            Some("user-1".to_string()),
            Some("key-1".to_string()),
            None,
            None,
            candidate_index as i32,
            retry_index as i32,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("catalog-key-1".to_string()),
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
            Some(110_000),
            None,
        )
        .expect("candidate should build")
    }

    fn sample_plan() -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-1".to_string(),
            candidate_id: None,
            provider_name: Some("openai".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/chat/completions".to_string(),
            headers: Default::default(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5"})),
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        }
    }

    #[test]
    fn parses_report_context_and_resolves_existing_candidate_slot() {
        let metadata = parse_request_candidate_report_context(Some(&json!({
            "request_id": "req-1",
            "candidate_index": 1,
            "retry_index": 2,
            "provider_id": "provider-1",
            "endpoint_id": "endpoint-1",
            "key_id": "catalog-key-1",
            "client_api_format": "openai:chat"
        })))
        .expect("metadata");
        let slot = resolve_report_request_candidate_slot(
            &[sample_candidate("cand-1", 1, 2)],
            metadata,
            123,
            "generated-1".to_string(),
        )
        .expect("slot");

        assert_eq!(slot.id, "cand-1");
        assert_eq!(slot.candidate_index, 1);
        assert_eq!(slot.retry_index, 2);
        assert_eq!(slot.request_id, "req-1");
    }

    #[test]
    fn merges_proxy_trace_info_into_existing_candidate_extra_data() {
        let mut existing = sample_candidate("cand-1", 1, 0);
        existing.extra_data = Some(json!({
            "provider_name": "Codex"
        }));

        let metadata = parse_request_candidate_report_context(Some(&json!({
            "request_id": "req-1",
            "candidate_index": 1,
            "retry_index": 0,
            "provider_id": "provider-1",
            "endpoint_id": "endpoint-1",
            "key_id": "catalog-key-1",
            "client_api_format": "openai:chat",
            "provider_api_format": "openai:cli",
            "proxy": {
                "node_id": "proxy-node-1",
                "node_name": "edge-1",
                "source": "provider"
            }
        })))
        .expect("metadata");

        let slot = resolve_report_request_candidate_slot(
            &[existing],
            metadata,
            123,
            "generated-1".to_string(),
        )
        .expect("slot");

        assert_eq!(
            slot.extra_data
                .as_ref()
                .and_then(|value| value.get("provider_name")),
            Some(&json!("Codex"))
        );
        assert_eq!(
            slot.extra_data
                .as_ref()
                .and_then(|value| value.get("proxy"))
                .and_then(|value| value.get("node_id")),
            Some(&json!("proxy-node-1"))
        );
        assert_eq!(
            slot.extra_data
                .as_ref()
                .and_then(|value| value.get("proxy"))
                .and_then(|value| value.get("source")),
            Some(&json!("provider"))
        );
    }

    #[test]
    fn resolves_error_details_from_execution_error_or_body_json() {
        let error = ExecutionError {
            kind: ExecutionErrorKind::Upstream5xx,
            phase: ExecutionPhase::FirstByte,
            message: " upstream failed ".to_string(),
            upstream_status: Some(502),
            retryable: true,
            failover_recommended: true,
        };
        assert_eq!(
            execution_error_details(Some(&error), None),
            (
                Some("Upstream5xx".to_string()),
                Some("upstream failed".to_string())
            )
        );
        assert_eq!(
            execution_error_details(None, Some(&json!({"error": {"message": "bad request"}}))),
            (None, Some("bad request".to_string()))
        );
    }

    #[test]
    fn builds_execution_request_candidate_seed_and_finalizes_report_context() {
        let seed = build_execution_request_candidate_seed(
            &sample_plan(),
            Some(&json!({
                "request_id": "req-override",
                "candidate_index": 3,
                "retry_index": 2,
                "user_id": "user-1",
                "api_key_id": "api-key-1",
                "client_api_format": "openai:chat"
            })),
            123,
            "generated-1".to_string(),
        );

        assert_eq!(seed.upsert_record.id, "generated-1");
        assert_eq!(seed.upsert_record.request_id, "req-override");
        assert_eq!(seed.upsert_record.candidate_index, 3);
        assert_eq!(seed.upsert_record.retry_index, 2);
        assert_eq!(seed.upsert_record.user_id.as_deref(), Some("user-1"));
        assert_eq!(
            seed.report_context
                .get("provider_id")
                .and_then(Value::as_str),
            Some("provider-1")
        );

        let finalized =
            finalize_execution_request_candidate_report_context(seed.report_context, "cand-final");
        assert_eq!(
            finalized.get("candidate_id").and_then(Value::as_str),
            Some("cand-final")
        );
    }

    #[test]
    fn builds_local_request_candidate_status_record() {
        let mut plan = sample_plan();
        plan.candidate_id = Some("cand-1".to_string());

        let record =
            build_local_request_candidate_status_record(LocalRequestCandidateStatusRecordInput {
                plan: &plan,
                report_context: Some(&json!({
                    "candidate_index": 1,
                    "retry_index": 2,
                    "user_id": "user-1",
                    "api_key_id": "api-key-1"
                })),
                status_update: SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: Some(500),
                    error_type: Some("Upstream5xx".to_string()),
                    error_message: Some("boom".to_string()),
                    latency_ms: Some(42),
                    started_at_unix_ms: Some(100),
                    finished_at_unix_ms: Some(101),
                },
            })
            .expect("record should build");

        assert_eq!(record.id, "cand-1");
        assert_eq!(record.candidate_index, 1);
        assert_eq!(record.retry_index, 2);
        assert_eq!(record.user_id.as_deref(), Some("user-1"));
        assert_eq!(record.status, RequestCandidateStatus::Failed);
    }

    #[test]
    fn builds_report_request_candidate_status_record_with_terminal_timestamps() {
        let record =
            build_report_request_candidate_status_record(ReportRequestCandidateStatusRecordInput {
                slot: SchedulerResolvedReportRequestCandidateSlot {
                    id: "cand-1".to_string(),
                    request_id: "req-1".to_string(),
                    user_id: Some("user-1".to_string()),
                    api_key_id: Some("api-key-1".to_string()),
                    candidate_index: 1,
                    retry_index: 0,
                    provider_id: Some("provider-1".to_string()),
                    endpoint_id: Some("endpoint-1".to_string()),
                    key_id: Some("key-1".to_string()),
                    extra_data: None,
                    created_at_unix_ms: 10,
                    started_at_unix_ms: None,
                    finished_at_unix_ms: None,
                },
                status_update: SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Success,
                    status_code: Some(200),
                    error_type: None,
                    error_message: None,
                    latency_ms: Some(12),
                    started_at_unix_ms: None,
                    finished_at_unix_ms: None,
                },
                now_unix_ms: 123,
            });

        assert_eq!(record.started_at_unix_ms, Some(123));
        assert_eq!(record.finished_at_unix_ms, Some(123));
        assert_eq!(record.created_at_unix_ms, Some(10));
        assert_eq!(record.status, RequestCandidateStatus::Success);
    }
}
