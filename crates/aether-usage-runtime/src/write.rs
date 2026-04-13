use std::collections::BTreeMap;

use aether_contracts::{ExecutionPlan, ExecutionTelemetry};
use aether_data_contracts::repository::usage::UpsertUsageRecord;
use aether_data_contracts::DataLayerError;
use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::request_metadata::{
    build_usage_request_metadata_seed, merge_usage_request_metadata,
    sanitize_usage_request_metadata,
};
use crate::{
    map_usage_from_response, GatewayStreamReportRequest, GatewaySyncReportRequest,
    StandardizedUsage, UsageEvent, UsageEventData, UsageEventType,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageLifecycleState {
    Pending,
    Streaming,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageRoutingSeed {
    candidate_id: Option<String>,
    candidate_index: Option<u64>,
    key_name: Option<String>,
    planner_kind: Option<String>,
    route_family: Option<String>,
    route_kind: Option<String>,
    execution_path: Option<String>,
    local_execution_runtime_miss_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageBodyRefsSeed {
    request_body_ref: Option<String>,
    provider_request_body_ref: Option<String>,
    response_body_ref: Option<String>,
    client_response_body_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleUsageSeed {
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub provider_name: String,
    pub model: String,
    pub target_model: Option<String>,
    pub provider_id: Option<String>,
    pub provider_endpoint_id: Option<String>,
    pub provider_api_key_id: Option<String>,
    pub request_type: String,
    pub api_format: Option<String>,
    pub api_family: Option<String>,
    pub endpoint_kind: Option<String>,
    pub endpoint_api_format: Option<String>,
    pub provider_api_family: Option<String>,
    pub provider_endpoint_kind: Option<String>,
    pub has_format_conversion: Option<bool>,
    pub is_stream: bool,
    routing: UsageRoutingSeed,
    pub request_metadata: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageTerminalState {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct TerminalUsageContextSeed {
    pub client_contract: String,
    pub provider_contract: String,
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub provider_name: String,
    pub model: String,
    pub target_model: Option<String>,
    pub provider_id: Option<String>,
    pub provider_endpoint_id: Option<String>,
    pub provider_api_key_id: Option<String>,
    pub request_type: String,
    pub has_format_conversion: bool,
    pub is_stream: bool,
    pub request_headers: Option<Value>,
    pub request_body: Option<Value>,
    pub provider_request_headers: Option<Value>,
    pub provider_request: Option<Value>,
    body_refs: UsageBodyRefsSeed,
    routing: UsageRoutingSeed,
    pub request_metadata: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct SyncTerminalUsagePayloadSeed {
    pub report_kind: String,
    pub status_code: u16,
    pub response_time_ms: Option<u64>,
    pub first_byte_time_ms: Option<u64>,
    pub provider_response_headers: Option<Value>,
    pub provider_response_full: Option<Value>,
    pub client_response_headers: Option<Value>,
    pub client_response: Option<Value>,
    pub capture_metadata: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct StreamTerminalUsagePayloadSeed {
    pub report_kind: String,
    pub status_code: u16,
    pub response_time_ms: Option<u64>,
    pub first_byte_time_ms: Option<u64>,
    pub provider_response_headers: Option<Value>,
    pub provider_response_full: Option<Value>,
    pub client_response_headers: Option<Value>,
    pub client_response: Option<Value>,
    pub capture_metadata: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct TerminalUsageSeed {
    pub terminal_state: UsageTerminalState,
    pub client_contract: String,
    pub provider_contract: String,
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub provider_name: String,
    pub model: String,
    pub target_model: Option<String>,
    pub provider_id: Option<String>,
    pub provider_endpoint_id: Option<String>,
    pub provider_api_key_id: Option<String>,
    pub request_type: String,
    pub has_format_conversion: bool,
    pub is_stream: bool,
    pub status_code: u16,
    pub response_time_ms: Option<u64>,
    pub first_byte_time_ms: Option<u64>,
    pub request_headers: Option<Value>,
    pub request_body: Option<Value>,
    pub provider_request_headers: Option<Value>,
    pub provider_request: Option<Value>,
    body_refs: UsageBodyRefsSeed,
    pub provider_response_headers: Option<Value>,
    pub provider_response: Option<Value>,
    pub client_response_headers: Option<Value>,
    pub client_response: Option<Value>,
    routing: UsageRoutingSeed,
    pub request_metadata: Option<Value>,
    pub audit_payload: Option<Value>,
    pub standardized_usage: Option<StandardizedUsage>,
}

pub type TerminalUsageOutcome = TerminalUsageSeed;

struct LifecycleUsageRecordInput<'a> {
    seed: &'a LifecycleUsageSeed,
    lifecycle_state: UsageLifecycleState,
    status_code: Option<u16>,
    response_time_ms: Option<u64>,
    first_byte_time_ms: Option<u64>,
    response_headers: Option<Value>,
    client_response_headers: Option<Value>,
    updated_at_unix_secs: u64,
}

const MAX_USAGE_CAPTURE_DEPTH: usize = 64;
const MAX_USAGE_CAPTURE_NODES: usize = 20_000;
const MAX_USAGE_CAPTURE_BYTES: usize = 64 * 1024;

pub fn build_lifecycle_usage_seed(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> LifecycleUsageSeed {
    let context = report_context.and_then(Value::as_object);
    let api_format = context_string(context, "client_api_format")
        .or_else(|| non_empty_string(Some(plan.client_api_format.clone())));
    let endpoint_api_format = context_string(context, "provider_api_format")
        .or_else(|| non_empty_string(Some(plan.provider_api_format.clone())));
    let provider_name = context_string(context, "provider_name")
        .or_else(|| plan.provider_name.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let model = context_string(context, "model")
        .or_else(|| plan.model_name.clone())
        .unwrap_or_else(|| "unknown".to_string());

    LifecycleUsageSeed {
        request_id: plan.request_id.clone(),
        user_id: context_string(context, "user_id"),
        api_key_id: context_string(context, "api_key_id"),
        username: context_string(context, "username"),
        api_key_name: context_string(context, "api_key_name"),
        provider_name,
        model,
        target_model: context_string(context, "mapped_model"),
        provider_id: empty_to_none(
            context_string(context, "provider_id")
                .or_else(|| non_empty_string(Some(plan.provider_id.clone()))),
        ),
        provider_endpoint_id: empty_to_none(
            context_string(context, "endpoint_id")
                .or_else(|| non_empty_string(Some(plan.endpoint_id.clone()))),
        ),
        provider_api_key_id: empty_to_none(
            context_string(context, "key_id")
                .or_else(|| non_empty_string(Some(plan.key_id.clone()))),
        ),
        request_type: infer_request_type(api_format.as_deref()),
        api_format: api_format.clone(),
        api_family: api_format
            .as_deref()
            .and_then(infer_api_family)
            .map(ToOwned::to_owned),
        endpoint_kind: api_format
            .as_deref()
            .and_then(infer_endpoint_kind)
            .map(ToOwned::to_owned),
        endpoint_api_format: endpoint_api_format.clone(),
        provider_api_family: endpoint_api_format
            .as_deref()
            .and_then(infer_api_family)
            .map(ToOwned::to_owned),
        provider_endpoint_kind: endpoint_api_format
            .as_deref()
            .and_then(infer_endpoint_kind)
            .map(ToOwned::to_owned),
        has_format_conversion: context_bool(context, "needs_conversion"),
        is_stream: plan.stream,
        routing: build_runtime_routing_seed(plan, context),
        request_metadata: build_runtime_request_metadata_seed(plan, context),
    }
}

pub fn build_pending_usage_record(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    updated_at_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    let seed = build_lifecycle_usage_seed(plan, report_context);
    build_pending_usage_record_from_seed(&seed, updated_at_unix_secs)
}

pub fn build_pending_usage_record_from_seed(
    seed: &LifecycleUsageSeed,
    updated_at_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    build_lifecycle_usage_record(LifecycleUsageRecordInput {
        seed,
        lifecycle_state: UsageLifecycleState::Pending,
        status_code: None,
        response_time_ms: None,
        first_byte_time_ms: None,
        response_headers: None,
        client_response_headers: None,
        updated_at_unix_secs,
    })
}

pub fn build_streaming_usage_record(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    status_code: u16,
    telemetry: Option<&ExecutionTelemetry>,
    updated_at_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    let seed = build_lifecycle_usage_seed(plan, report_context);
    build_streaming_usage_record_from_seed(&seed, status_code, telemetry, updated_at_unix_secs)
}

pub fn build_streaming_usage_record_from_seed(
    seed: &LifecycleUsageSeed,
    status_code: u16,
    telemetry: Option<&ExecutionTelemetry>,
    updated_at_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    build_lifecycle_usage_record(LifecycleUsageRecordInput {
        seed,
        lifecycle_state: UsageLifecycleState::Streaming,
        status_code: Some(status_code),
        response_time_ms: telemetry.and_then(|value| value.elapsed_ms),
        first_byte_time_ms: telemetry.and_then(|value| value.ttfb_ms),
        response_headers: None,
        client_response_headers: None,
        updated_at_unix_secs,
    })
}

pub fn build_sync_terminal_usage_event(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    payload: &GatewaySyncReportRequest,
) -> Result<UsageEvent, DataLayerError> {
    let context_seed = build_terminal_usage_context_seed(plan, report_context);
    let payload_seed = build_sync_terminal_usage_payload_seed(payload);
    build_terminal_usage_event_from_seed(build_sync_terminal_usage_seed(context_seed, payload_seed))
}

pub fn build_stream_terminal_usage_event(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    payload: &GatewayStreamReportRequest,
) -> Result<UsageEvent, DataLayerError> {
    let context_seed = build_terminal_usage_context_seed(plan, report_context);
    let payload_seed = build_stream_terminal_usage_payload_seed(payload);
    build_terminal_usage_event_from_seed(build_stream_terminal_usage_seed(
        context_seed,
        payload_seed,
        false,
    ))
}

pub fn build_sync_terminal_usage_outcome(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    payload: &GatewaySyncReportRequest,
) -> TerminalUsageOutcome {
    let context_seed = build_terminal_usage_context_seed(plan, report_context);
    let payload_seed = build_sync_terminal_usage_payload_seed(payload);
    build_sync_terminal_usage_seed(context_seed, payload_seed)
}

pub fn build_stream_terminal_usage_outcome(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    payload: &GatewayStreamReportRequest,
) -> TerminalUsageOutcome {
    let context_seed = build_terminal_usage_context_seed(plan, report_context);
    let payload_seed = build_stream_terminal_usage_payload_seed(payload);
    build_stream_terminal_usage_seed(context_seed, payload_seed, false)
}

pub fn build_terminal_usage_event_from_outcome(
    outcome: TerminalUsageOutcome,
) -> Result<UsageEvent, DataLayerError> {
    build_terminal_usage_event_from_seed(outcome)
}

pub fn build_terminal_usage_event_from_seed(
    seed: TerminalUsageSeed,
) -> Result<UsageEvent, DataLayerError> {
    let event_type = match seed.terminal_state {
        UsageTerminalState::Completed => UsageEventType::Completed,
        UsageTerminalState::Failed => UsageEventType::Failed,
        UsageTerminalState::Cancelled => UsageEventType::Cancelled,
    };
    let provider_response = seed.provider_response;
    let client_response = seed.client_response;
    let routing = merge_routing_seed_with_metadata(&seed.routing, seed.request_metadata.as_ref());
    let body_refs =
        merge_body_refs_seed_with_metadata(&seed.body_refs, seed.request_metadata.as_ref());

    let mut data = UsageEventData {
        user_id: seed.user_id,
        api_key_id: seed.api_key_id,
        username: seed.username,
        api_key_name: seed.api_key_name,
        provider_name: seed.provider_name,
        model: seed.model,
        target_model: seed.target_model,
        provider_id: seed.provider_id,
        provider_endpoint_id: seed.provider_endpoint_id,
        provider_api_key_id: seed.provider_api_key_id,
        request_type: Some(seed.request_type),
        api_format: Some(seed.client_contract.clone()),
        api_family: infer_api_family(&seed.client_contract).map(ToOwned::to_owned),
        endpoint_kind: infer_endpoint_kind(&seed.client_contract).map(ToOwned::to_owned),
        endpoint_api_format: Some(seed.provider_contract.clone()),
        provider_api_family: infer_api_family(&seed.provider_contract).map(ToOwned::to_owned),
        provider_endpoint_kind: infer_endpoint_kind(&seed.provider_contract).map(ToOwned::to_owned),
        has_format_conversion: Some(seed.has_format_conversion),
        is_stream: Some(seed.is_stream),
        status_code: Some(seed.status_code),
        error_message: resolve_error_message(seed.status_code, provider_response.as_ref(), None),
        error_category: resolve_error_category(seed.status_code, event_type),
        response_time_ms: seed.response_time_ms,
        first_byte_time_ms: seed.first_byte_time_ms,
        request_headers: seed.request_headers,
        request_body: seed.request_body,
        request_body_ref: body_refs.request_body_ref,
        provider_request_headers: seed.provider_request_headers,
        provider_request_body: seed.provider_request,
        provider_request_body_ref: body_refs.provider_request_body_ref,
        response_headers: seed.provider_response_headers,
        response_body: provider_response.clone(),
        response_body_ref: body_refs.response_body_ref,
        client_response_headers: seed.client_response_headers,
        client_response_body: client_response.clone(),
        client_response_body_ref: body_refs.client_response_body_ref,
        candidate_id: routing.candidate_id,
        key_name: routing.key_name,
        planner_kind: routing.planner_kind,
        route_family: routing.route_family,
        route_kind: routing.route_kind,
        execution_path: routing.execution_path,
        local_execution_runtime_miss_reason: routing.local_execution_runtime_miss_reason,
        request_metadata: merge_usage_request_metadata(seed.request_metadata, seed.audit_payload),
        ..UsageEventData::default()
    };

    if let Some(usage) = seed.standardized_usage.as_ref() {
        apply_standardized_usage_seed(usage, &mut data);
    }

    if let Some(response_body) = provider_response.as_ref() {
        apply_standardized_usage(
            Some(seed.provider_contract.clone()),
            response_body,
            &mut data,
        );
    }
    if data.total_tokens.is_none() {
        if let Some(tokens) = provider_response
            .as_ref()
            .and_then(extract_token_counts_from_value)
        {
            data.input_tokens = Some(tokens.0);
            data.output_tokens = Some(tokens.1);
            data.total_tokens = Some(tokens.2);
        }
    }

    let data = sanitize_usage_event_data(data);

    Ok(UsageEvent::new(event_type, seed.request_id, data))
}

pub fn build_terminal_usage_context_seed(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> TerminalUsageContextSeed {
    let context = report_context.and_then(Value::as_object);
    let client_contract = context_string(context, "client_contract")
        .or_else(|| context_string(context, "client_api_format"))
        .or_else(|| non_empty_string(Some(plan.client_api_format.clone())))
        .unwrap_or_default();
    let provider_contract = context_string(context, "provider_contract")
        .or_else(|| context_string(context, "provider_api_format"))
        .or_else(|| non_empty_string(Some(plan.provider_api_format.clone())))
        .unwrap_or_default();

    TerminalUsageContextSeed {
        client_contract: client_contract.clone(),
        provider_contract: provider_contract.clone(),
        has_format_conversion: resolve_has_format_conversion(
            context,
            client_contract.as_str(),
            provider_contract.as_str(),
        ),
        request_id: plan.request_id.clone(),
        user_id: context_string(context, "user_id"),
        api_key_id: context_string(context, "api_key_id"),
        username: context_string(context, "username"),
        api_key_name: context_string(context, "api_key_name"),
        provider_name: context_string(context, "provider_name")
            .or_else(|| plan.provider_name.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string()),
        model: context_string(context, "model")
            .or_else(|| plan.model_name.clone())
            .unwrap_or_else(|| "unknown".to_string()),
        target_model: context_string(context, "mapped_model"),
        provider_id: context_string(context, "provider_id")
            .or_else(|| non_empty_string(Some(plan.provider_id.clone()))),
        provider_endpoint_id: context_string(context, "endpoint_id")
            .or_else(|| non_empty_string(Some(plan.endpoint_id.clone()))),
        provider_api_key_id: context_string(context, "key_id")
            .or_else(|| non_empty_string(Some(plan.key_id.clone()))),
        request_type: infer_request_type(Some(client_contract.as_str())),
        is_stream: plan.stream,
        routing: build_runtime_routing_seed(plan, context),
        request_headers: mask_sensitive_headers_in_json_value(context_value(
            context,
            "original_headers",
        )),
        request_body: context_body_value(context, "original_request_body"),
        provider_request_headers: mask_sensitive_headers_in_json_value(
            context_usage_value(context, "provider_request_headers")
                .or_else(|| Some(headers_to_json(&plan.headers))),
        ),
        provider_request: context_body_value(context, "provider_request_body")
            .or_else(|| plan_json_body_capture_for_usage(plan)),
        body_refs: build_runtime_body_refs_seed(plan, context),
        request_metadata: merge_usage_request_metadata(
            build_usage_request_metadata_seed(plan, context),
            build_plan_body_capture_metadata(plan),
        ),
    }
}

pub fn build_sync_terminal_usage_payload_seed(
    payload: &GatewaySyncReportRequest,
) -> SyncTerminalUsagePayloadSeed {
    SyncTerminalUsagePayloadSeed {
        report_kind: payload.report_kind.clone(),
        status_code: payload.status_code,
        response_time_ms: payload
            .telemetry
            .as_ref()
            .and_then(|value| value.elapsed_ms),
        first_byte_time_ms: payload.telemetry.as_ref().and_then(|value| value.ttfb_ms),
        provider_response_headers: Some(headers_to_json(&payload.headers)),
        provider_response_full: payload
            .body_json
            .as_ref()
            .cloned()
            .or_else(|| decode_body_for_storage(payload.body_base64.as_deref())),
        client_response_headers: Some(headers_to_json(&payload.headers)),
        client_response: payload.client_body_json.as_ref().cloned(),
        capture_metadata: build_payload_body_capture_metadata(payload.body_base64.as_deref(), None),
    }
}

pub fn build_stream_terminal_usage_payload_seed(
    payload: &GatewayStreamReportRequest,
) -> StreamTerminalUsagePayloadSeed {
    StreamTerminalUsagePayloadSeed {
        report_kind: payload.report_kind.clone(),
        status_code: payload.status_code,
        response_time_ms: payload
            .telemetry
            .as_ref()
            .and_then(|value| value.elapsed_ms),
        first_byte_time_ms: payload.telemetry.as_ref().and_then(|value| value.ttfb_ms),
        provider_response_headers: Some(headers_to_json(&payload.headers)),
        provider_response_full: decode_body_for_storage(payload.provider_body_base64.as_deref()),
        client_response_headers: Some(headers_to_json(&payload.headers)),
        client_response: decode_body_for_storage(payload.client_body_base64.as_deref()),
        capture_metadata: build_payload_body_capture_metadata(
            payload.provider_body_base64.as_deref(),
            payload.client_body_base64.as_deref(),
        ),
    }
}

pub fn build_sync_terminal_usage_seed(
    context_seed: TerminalUsageContextSeed,
    payload_seed: SyncTerminalUsagePayloadSeed,
) -> TerminalUsageSeed {
    let standardized_usage = payload_seed
        .provider_response_full
        .as_ref()
        .map(|response| map_usage_from_response(response, context_seed.provider_contract.as_str()));
    let terminal_state = infer_sync_terminal_state(
        payload_seed.report_kind.as_str(),
        payload_seed.status_code,
        payload_seed.provider_response_full.as_ref(),
    );

    TerminalUsageSeed {
        terminal_state,
        client_contract: context_seed.client_contract,
        provider_contract: context_seed.provider_contract,
        request_id: context_seed.request_id,
        user_id: context_seed.user_id,
        api_key_id: context_seed.api_key_id,
        username: context_seed.username,
        api_key_name: context_seed.api_key_name,
        provider_name: context_seed.provider_name,
        model: context_seed.model,
        target_model: context_seed.target_model,
        provider_id: context_seed.provider_id,
        provider_endpoint_id: context_seed.provider_endpoint_id,
        provider_api_key_id: context_seed.provider_api_key_id,
        request_type: context_seed.request_type,
        has_format_conversion: context_seed.has_format_conversion,
        is_stream: context_seed.is_stream,
        status_code: payload_seed.status_code,
        response_time_ms: payload_seed.response_time_ms,
        first_byte_time_ms: payload_seed.first_byte_time_ms,
        request_headers: context_seed.request_headers,
        request_body: context_seed.request_body,
        provider_request_headers: context_seed.provider_request_headers,
        provider_request: context_seed.provider_request,
        body_refs: context_seed.body_refs,
        routing: context_seed.routing,
        provider_response_headers: payload_seed.provider_response_headers,
        provider_response: payload_seed.provider_response_full,
        client_response_headers: payload_seed.client_response_headers,
        client_response: payload_seed.client_response,
        request_metadata: context_seed.request_metadata,
        audit_payload: payload_seed.capture_metadata,
        standardized_usage,
    }
}

pub fn build_stream_terminal_usage_seed(
    context_seed: TerminalUsageContextSeed,
    payload_seed: StreamTerminalUsagePayloadSeed,
    cancelled: bool,
) -> TerminalUsageSeed {
    let standardized_usage = payload_seed
        .provider_response_full
        .as_ref()
        .map(|response| map_usage_from_response(response, context_seed.provider_contract.as_str()));
    let terminal_state = infer_stream_terminal_state(
        payload_seed.report_kind.as_str(),
        payload_seed.status_code,
        cancelled,
    );

    TerminalUsageSeed {
        terminal_state,
        client_contract: context_seed.client_contract,
        provider_contract: context_seed.provider_contract,
        request_id: context_seed.request_id,
        user_id: context_seed.user_id,
        api_key_id: context_seed.api_key_id,
        username: context_seed.username,
        api_key_name: context_seed.api_key_name,
        provider_name: context_seed.provider_name,
        model: context_seed.model,
        target_model: context_seed.target_model,
        provider_id: context_seed.provider_id,
        provider_endpoint_id: context_seed.provider_endpoint_id,
        provider_api_key_id: context_seed.provider_api_key_id,
        request_type: context_seed.request_type,
        has_format_conversion: context_seed.has_format_conversion,
        is_stream: context_seed.is_stream,
        status_code: payload_seed.status_code,
        response_time_ms: payload_seed.response_time_ms,
        first_byte_time_ms: payload_seed.first_byte_time_ms,
        request_headers: context_seed.request_headers,
        request_body: context_seed.request_body,
        provider_request_headers: context_seed.provider_request_headers,
        provider_request: context_seed.provider_request,
        body_refs: context_seed.body_refs,
        routing: context_seed.routing,
        provider_response_headers: payload_seed.provider_response_headers,
        provider_response: payload_seed.provider_response_full,
        client_response_headers: payload_seed.client_response_headers,
        client_response: payload_seed.client_response,
        request_metadata: context_seed.request_metadata,
        audit_payload: payload_seed.capture_metadata,
        standardized_usage,
    }
}

fn infer_sync_terminal_state(
    report_kind: &str,
    status_code: u16,
    provider_response: Option<&Value>,
) -> UsageTerminalState {
    if status_code == 499 || report_kind.contains("cancel") {
        UsageTerminalState::Cancelled
    } else if status_code >= 400
        || provider_response
            .and_then(|value| value.get("error"))
            .is_some()
    {
        UsageTerminalState::Failed
    } else {
        UsageTerminalState::Completed
    }
}

fn infer_stream_terminal_state(
    report_kind: &str,
    status_code: u16,
    cancelled: bool,
) -> UsageTerminalState {
    if cancelled || status_code == 499 || report_kind.contains("cancel") {
        UsageTerminalState::Cancelled
    } else if status_code >= 400 {
        UsageTerminalState::Failed
    } else {
        UsageTerminalState::Completed
    }
}

fn resolve_has_format_conversion(
    context: Option<&Map<String, Value>>,
    client_contract: &str,
    provider_contract: &str,
) -> bool {
    match context_string(context, "conversion_mode").as_deref() {
        Some("none") => false,
        Some("request_only" | "response_only" | "bidirectional") => true,
        _ if context_bool(context, "needs_conversion").unwrap_or(false) => true,
        _ if client_contract != provider_contract => true,
        _ => false,
    }
}

fn build_lifecycle_usage_record(
    input: LifecycleUsageRecordInput<'_>,
) -> Result<UpsertUsageRecord, DataLayerError> {
    let LifecycleUsageRecordInput {
        seed,
        lifecycle_state,
        status_code,
        response_time_ms,
        first_byte_time_ms,
        response_headers,
        client_response_headers,
        updated_at_unix_secs,
    } = input;
    let (status, billing_status) = lifecycle_status_and_billing(lifecycle_state);
    let routing = merge_routing_seed_with_metadata(&seed.routing, seed.request_metadata.as_ref());
    let body_refs = merge_body_refs_seed_with_metadata(
        &UsageBodyRefsSeed::default(),
        seed.request_metadata.as_ref(),
    );

    Ok(UpsertUsageRecord {
        request_id: seed.request_id.clone(),
        user_id: seed.user_id.clone(),
        api_key_id: seed.api_key_id.clone(),
        username: seed.username.clone(),
        api_key_name: seed.api_key_name.clone(),
        provider_name: seed.provider_name.clone(),
        model: seed.model.clone(),
        target_model: seed.target_model.clone(),
        provider_id: seed.provider_id.clone(),
        provider_endpoint_id: seed.provider_endpoint_id.clone(),
        provider_api_key_id: seed.provider_api_key_id.clone(),
        request_type: Some(seed.request_type.clone()),
        api_format: seed.api_format.clone(),
        api_family: seed.api_family.clone(),
        endpoint_kind: seed.endpoint_kind.clone(),
        endpoint_api_format: seed.endpoint_api_format.clone(),
        provider_api_family: seed.provider_api_family.clone(),
        provider_endpoint_kind: seed.provider_endpoint_kind.clone(),
        has_format_conversion: seed.has_format_conversion,
        is_stream: Some(seed.is_stream),
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
        cache_creation_input_tokens: None,
        cache_creation_ephemeral_5m_input_tokens: None,
        cache_creation_ephemeral_1h_input_tokens: None,
        cache_read_input_tokens: None,
        cache_creation_cost_usd: None,
        cache_read_cost_usd: None,
        output_price_per_1m: None,
        total_cost_usd: None,
        actual_total_cost_usd: None,
        status_code,
        error_message: None,
        error_category: None,
        response_time_ms,
        first_byte_time_ms,
        status: status.to_string(),
        billing_status: billing_status.to_string(),
        request_headers: None,
        request_body: None,
        request_body_ref: body_refs.request_body_ref,
        provider_request_headers: None,
        provider_request_body: None,
        provider_request_body_ref: body_refs.provider_request_body_ref,
        response_headers: sanitize_usage_header_capture(response_headers),
        response_body: None,
        response_body_ref: body_refs.response_body_ref,
        client_response_headers: sanitize_usage_header_capture(client_response_headers),
        client_response_body: None,
        client_response_body_ref: body_refs.client_response_body_ref,
        candidate_id: routing.candidate_id,
        candidate_index: routing.candidate_index,
        key_name: routing.key_name,
        planner_kind: routing.planner_kind,
        route_family: routing.route_family,
        route_kind: routing.route_kind,
        execution_path: routing.execution_path,
        local_execution_runtime_miss_reason: routing.local_execution_runtime_miss_reason,
        request_metadata: sanitize_usage_request_metadata(seed.request_metadata.clone()),
        finalized_at_unix_secs: None,
        created_at_unix_ms: Some(updated_at_unix_secs),
        updated_at_unix_secs,
    })
}

pub fn build_usage_event_data_seed(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> UsageEventData {
    build_usage_event_data_seed_with_detail(plan, report_context)
}

fn build_usage_event_data_seed_with_detail(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> UsageEventData {
    let context = report_context.and_then(Value::as_object);
    let routing = build_runtime_routing_seed(plan, context);
    let body_refs = build_runtime_body_refs_seed(plan, context);
    let api_format = context_string(context, "client_api_format")
        .or_else(|| non_empty_string(Some(plan.client_api_format.clone())));
    let endpoint_api_format = context_string(context, "provider_api_format")
        .or_else(|| non_empty_string(Some(plan.provider_api_format.clone())));
    let model = context_string(context, "model")
        .or_else(|| plan.model_name.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let provider_name = context_string(context, "provider_name")
        .or_else(|| plan.provider_name.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    sanitize_usage_event_data(UsageEventData {
        user_id: context_string(context, "user_id"),
        api_key_id: context_string(context, "api_key_id"),
        username: context_string(context, "username"),
        api_key_name: context_string(context, "api_key_name"),
        provider_name,
        model,
        target_model: context_string(context, "mapped_model"),
        provider_id: context_string(context, "provider_id")
            .or_else(|| non_empty_string(Some(plan.provider_id.clone()))),
        provider_endpoint_id: context_string(context, "endpoint_id")
            .or_else(|| non_empty_string(Some(plan.endpoint_id.clone()))),
        provider_api_key_id: context_string(context, "key_id")
            .or_else(|| non_empty_string(Some(plan.key_id.clone()))),
        request_type: Some(infer_request_type(api_format.as_deref())),
        api_format: api_format.clone(),
        api_family: api_format
            .as_deref()
            .and_then(infer_api_family)
            .map(ToOwned::to_owned),
        endpoint_kind: api_format
            .as_deref()
            .and_then(infer_endpoint_kind)
            .map(ToOwned::to_owned),
        endpoint_api_format: endpoint_api_format.clone(),
        provider_api_family: endpoint_api_format
            .as_deref()
            .and_then(infer_api_family)
            .map(ToOwned::to_owned),
        provider_endpoint_kind: endpoint_api_format
            .as_deref()
            .and_then(infer_endpoint_kind)
            .map(ToOwned::to_owned),
        has_format_conversion: context_bool(context, "needs_conversion"),
        is_stream: Some(plan.stream),
        request_headers: context_usage_value(context, "original_headers"),
        request_body: context_body_value(context, "original_request_body"),
        request_body_ref: body_refs.request_body_ref,
        provider_request_headers: context_usage_value(context, "provider_request_headers")
            .or_else(|| Some(headers_to_json(&plan.headers))),
        provider_request_body: context_body_value(context, "provider_request_body")
            .or_else(|| plan_json_body_capture_for_usage(plan)),
        provider_request_body_ref: body_refs.provider_request_body_ref,
        response_body_ref: body_refs.response_body_ref,
        client_response_body_ref: body_refs.client_response_body_ref,
        candidate_id: routing.candidate_id,
        candidate_index: routing.candidate_index,
        key_name: routing.key_name,
        planner_kind: routing.planner_kind,
        route_family: routing.route_family,
        route_kind: routing.route_kind,
        execution_path: routing.execution_path,
        local_execution_runtime_miss_reason: routing.local_execution_runtime_miss_reason,
        request_metadata: build_runtime_request_metadata_seed(plan, context),
        ..UsageEventData::default()
    })
}

fn lifecycle_status_and_billing(state: UsageLifecycleState) -> (&'static str, &'static str) {
    match state {
        UsageLifecycleState::Pending => ("pending", "pending"),
        UsageLifecycleState::Streaming => ("streaming", "pending"),
    }
}

fn build_runtime_routing_seed(
    plan: &ExecutionPlan,
    context: Option<&Map<String, Value>>,
) -> UsageRoutingSeed {
    UsageRoutingSeed {
        candidate_id: context_string(context, "candidate_id")
            .or_else(|| non_empty_string(plan.candidate_id.clone())),
        candidate_index: context_u64(context, "candidate_index"),
        key_name: context_string(context, "key_name"),
        planner_kind: context_string(context, "planner_kind"),
        route_family: context_string(context, "route_family"),
        route_kind: context_string(context, "route_kind"),
        execution_path: context_string(context, "execution_path"),
        local_execution_runtime_miss_reason: context_string(
            context,
            "local_execution_runtime_miss_reason",
        ),
    }
}

fn routing_string_from_metadata(value: Option<&Value>, key: &str) -> Option<String> {
    value
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn merge_routing_seed_with_metadata(
    routing: &UsageRoutingSeed,
    metadata: Option<&Value>,
) -> UsageRoutingSeed {
    UsageRoutingSeed {
        candidate_id: routing
            .candidate_id
            .clone()
            .or_else(|| routing_string_from_metadata(metadata, "candidate_id")),
        candidate_index: routing
            .candidate_index
            .or_else(|| routing_u64_from_metadata(metadata, "candidate_index")),
        key_name: routing
            .key_name
            .clone()
            .or_else(|| routing_string_from_metadata(metadata, "key_name")),
        planner_kind: routing
            .planner_kind
            .clone()
            .or_else(|| routing_string_from_metadata(metadata, "planner_kind")),
        route_family: routing
            .route_family
            .clone()
            .or_else(|| routing_string_from_metadata(metadata, "route_family")),
        route_kind: routing
            .route_kind
            .clone()
            .or_else(|| routing_string_from_metadata(metadata, "route_kind")),
        execution_path: routing
            .execution_path
            .clone()
            .or_else(|| routing_string_from_metadata(metadata, "execution_path")),
        local_execution_runtime_miss_reason: routing
            .local_execution_runtime_miss_reason
            .clone()
            .or_else(|| {
                routing_string_from_metadata(metadata, "local_execution_runtime_miss_reason")
            }),
    }
}

fn context_string(context: Option<&Map<String, Value>>, key: &str) -> Option<String> {
    context
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn context_u64(context: Option<&Map<String, Value>>, key: &str) -> Option<u64> {
    context.and_then(|value| {
        value.get(key).and_then(|raw| {
            raw.as_u64()
                .or_else(|| raw.as_i64().and_then(|number| u64::try_from(number).ok()))
        })
    })
}

fn routing_u64_from_metadata(value: Option<&Value>, key: &str) -> Option<u64> {
    value
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(|raw| {
            raw.as_u64()
                .or_else(|| raw.as_i64().and_then(|number| u64::try_from(number).ok()))
        })
}

fn context_bool(context: Option<&Map<String, Value>>, key: &str) -> Option<bool> {
    context
        .and_then(|value| value.get(key))
        .and_then(Value::as_bool)
}

fn context_value_ref<'a>(context: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a Value> {
    context.and_then(|value| value.get(key))
}

fn context_value(context: Option<&Map<String, Value>>, key: &str) -> Option<Value> {
    context.and_then(|value| value.get(key)).cloned()
}

fn context_usage_value(context: Option<&Map<String, Value>>, key: &str) -> Option<Value> {
    match context_value_ref(context, key) {
        Some(Value::Null) | None => None,
        Some(value) => clone_usage_capture_value(Some(value)),
    }
}

fn context_body_value(context: Option<&Map<String, Value>>, key: &str) -> Option<Value> {
    match context_value_ref(context, key) {
        Some(Value::Null) | None => None,
        Some(value) => clone_usage_body_value(Some(value)),
    }
}

fn build_runtime_request_metadata_seed(
    plan: &ExecutionPlan,
    context: Option<&Map<String, Value>>,
) -> Option<Value> {
    let mut metadata = Map::new();
    if let Some(trace_id) = context_string(context, "trace_id") {
        metadata.insert("trace_id".to_string(), Value::String(trace_id));
    }
    append_plan_body_capture_metadata(plan, &mut metadata);

    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

fn capture_usage_storage_value(value: Value) -> Value {
    if usage_capture_within_limits(&value) {
        return value;
    }
    json!({
        "truncated": true,
        "reason": "usage_capture_limits_exceeded",
        "max_depth": MAX_USAGE_CAPTURE_DEPTH,
        "max_nodes": MAX_USAGE_CAPTURE_NODES,
        "max_bytes": MAX_USAGE_CAPTURE_BYTES,
        "value_kind": usage_value_kind(&value),
    })
}

fn build_plan_body_capture_metadata(plan: &ExecutionPlan) -> Option<Value> {
    let mut metadata = Map::new();
    append_plan_body_capture_metadata(plan, &mut metadata);
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

fn append_plan_body_capture_metadata(plan: &ExecutionPlan, metadata: &mut Map<String, Value>) {
    if let Some(body_bytes_b64) = plan.body.body_bytes_b64.as_deref() {
        if let Some(decoded_len) = decoded_base64_len_hint(body_bytes_b64) {
            metadata.insert(
                "provider_request_body_base64_bytes".to_string(),
                Value::Number(decoded_len.into()),
            );
        }
    }
}

fn build_runtime_body_refs_seed(
    plan: &ExecutionPlan,
    context: Option<&Map<String, Value>>,
) -> UsageBodyRefsSeed {
    UsageBodyRefsSeed {
        request_body_ref: context_string(context, "request_body_ref"),
        provider_request_body_ref: context_string(context, "provider_request_body_ref")
            .or_else(|| non_empty_string(plan.body.body_ref.clone())),
        response_body_ref: context_string(context, "response_body_ref"),
        client_response_body_ref: context_string(context, "client_response_body_ref"),
    }
}

fn merge_body_refs_seed_with_metadata(
    seed: &UsageBodyRefsSeed,
    metadata: Option<&Value>,
) -> UsageBodyRefsSeed {
    let object = metadata.and_then(Value::as_object);
    UsageBodyRefsSeed {
        request_body_ref: seed
            .request_body_ref
            .clone()
            .or_else(|| context_string(object, "request_body_ref")),
        provider_request_body_ref: seed
            .provider_request_body_ref
            .clone()
            .or_else(|| context_string(object, "provider_request_body_ref")),
        response_body_ref: seed
            .response_body_ref
            .clone()
            .or_else(|| context_string(object, "response_body_ref")),
        client_response_body_ref: seed
            .client_response_body_ref
            .clone()
            .or_else(|| context_string(object, "client_response_body_ref")),
    }
}

fn build_payload_body_capture_metadata(
    provider_body_base64: Option<&str>,
    client_body_base64: Option<&str>,
) -> Option<Value> {
    let mut metadata = Map::new();
    if let Some(decoded_len) = provider_body_base64.and_then(decoded_base64_len_hint) {
        metadata.insert(
            "provider_response_body_base64_bytes".to_string(),
            Value::Number(decoded_len.into()),
        );
    }
    if let Some(decoded_len) = client_body_base64.and_then(decoded_base64_len_hint) {
        metadata.insert(
            "client_response_body_base64_bytes".to_string(),
            Value::Number(decoded_len.into()),
        );
    }
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

fn plan_json_body_capture_for_usage(plan: &ExecutionPlan) -> Option<Value> {
    if plan.body.body_ref.is_some() || plan.body.body_bytes_b64.is_some() {
        return None;
    }
    clone_usage_body_value(plan.body.json_body.as_ref())
}

fn clone_usage_capture_value(value: Option<&Value>) -> Option<Value> {
    value.cloned().map(capture_usage_storage_value)
}

fn clone_usage_body_value(value: Option<&Value>) -> Option<Value> {
    value.cloned()
}

fn sanitize_usage_event_data(mut data: UsageEventData) -> UsageEventData {
    data.request_headers = sanitize_usage_header_capture(data.request_headers);
    data.request_body_ref = sanitize_usage_body_ref(data.request_body_ref);
    data.provider_request_headers = sanitize_usage_header_capture(data.provider_request_headers);
    data.provider_request_body_ref = sanitize_usage_body_ref(data.provider_request_body_ref);
    data.response_headers = sanitize_usage_header_capture(data.response_headers);
    data.response_body_ref = sanitize_usage_body_ref(data.response_body_ref);
    data.client_response_headers = sanitize_usage_header_capture(data.client_response_headers);
    data.client_response_body_ref = sanitize_usage_body_ref(data.client_response_body_ref);
    data.request_metadata = sanitize_usage_request_metadata(data.request_metadata);
    data
}

fn sanitize_usage_header_capture(value: Option<Value>) -> Option<Value> {
    mask_sensitive_headers_in_json_value(value).map(capture_usage_storage_value)
}

fn sanitize_usage_body_ref(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn usage_capture_within_limits(value: &Value) -> bool {
    let mut nodes = 0usize;
    let mut estimated_bytes = 0usize;
    let mut stack = vec![(value, 1usize)];

    while let Some((current, depth)) = stack.pop() {
        nodes = nodes.saturating_add(1);
        estimated_bytes = estimated_bytes.saturating_add(usage_value_size_hint(current));
        if depth > MAX_USAGE_CAPTURE_DEPTH
            || nodes > MAX_USAGE_CAPTURE_NODES
            || estimated_bytes > MAX_USAGE_CAPTURE_BYTES
        {
            return false;
        }
        match current {
            Value::Array(items) => {
                estimated_bytes = estimated_bytes.saturating_add(items.len().saturating_mul(2));
                for item in items.iter().rev() {
                    stack.push((item, depth + 1));
                }
            }
            Value::Object(object) => {
                estimated_bytes = estimated_bytes
                    .saturating_add(object.len().saturating_mul(3))
                    .saturating_add(
                        object
                            .keys()
                            .map(|key| key.len().saturating_add(2))
                            .sum::<usize>(),
                    );
                for item in object.values() {
                    stack.push((item, depth + 1));
                }
            }
            _ => {}
        }
    }

    true
}

fn usage_value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn usage_value_size_hint(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(false) => 5,
        Value::Bool(true) => 4,
        Value::Number(number) => number.to_string().len(),
        Value::String(text) => text.len().saturating_add(2),
        Value::Array(_) | Value::Object(_) => 2,
    }
}

fn non_empty_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn infer_request_type(api_format: Option<&str>) -> String {
    match infer_endpoint_kind(api_format.unwrap_or_default()) {
        Some("video") => "video".to_string(),
        Some("image") => "image".to_string(),
        _ => "chat".to_string(),
    }
}

fn infer_api_family(api_format: &str) -> Option<&str> {
    api_format.split_once(':').map(|(family, _)| family)
}

fn infer_endpoint_kind(api_format: &str) -> Option<&str> {
    api_format.split_once(':').map(|(_, kind)| kind)
}

fn apply_standardized_usage(
    api_format: Option<String>,
    response_body: &Value,
    data: &mut UsageEventData,
) {
    let Some(api_format) = api_format.as_deref() else {
        return;
    };
    if !response_body.is_object() {
        return;
    }
    let usage = map_usage_from_response(response_body, api_format);
    if usage.input_tokens > 0 {
        data.input_tokens = Some(usage.input_tokens as u64);
    }
    if usage.output_tokens > 0 {
        data.output_tokens = Some(usage.output_tokens as u64);
    }
    if usage.cache_creation_tokens > 0 {
        data.cache_creation_input_tokens = Some(usage.cache_creation_tokens as u64);
    }
    if usage.cache_creation_ephemeral_5m_tokens > 0 {
        data.cache_creation_ephemeral_5m_input_tokens =
            Some(usage.cache_creation_ephemeral_5m_tokens as u64);
    }
    if usage.cache_creation_ephemeral_1h_tokens > 0 {
        data.cache_creation_ephemeral_1h_input_tokens =
            Some(usage.cache_creation_ephemeral_1h_tokens as u64);
    }
    if usage.cache_read_tokens > 0 {
        data.cache_read_input_tokens = Some(usage.cache_read_tokens as u64);
    }
    let total_tokens = usage
        .input_tokens
        .saturating_add(usage.output_tokens)
        .max(0) as u64;
    if total_tokens > 0 {
        data.total_tokens = Some(total_tokens);
    }
}

fn apply_standardized_usage_seed(usage: &StandardizedUsage, data: &mut UsageEventData) {
    if usage.input_tokens > 0 {
        data.input_tokens = Some(usage.input_tokens as u64);
    }
    if usage.output_tokens > 0 {
        data.output_tokens = Some(usage.output_tokens as u64);
    }
    if usage.cache_creation_tokens > 0 {
        data.cache_creation_input_tokens = Some(usage.cache_creation_tokens as u64);
    }
    if usage.cache_creation_ephemeral_5m_tokens > 0 {
        data.cache_creation_ephemeral_5m_input_tokens =
            Some(usage.cache_creation_ephemeral_5m_tokens as u64);
    }
    if usage.cache_creation_ephemeral_1h_tokens > 0 {
        data.cache_creation_ephemeral_1h_input_tokens =
            Some(usage.cache_creation_ephemeral_1h_tokens as u64);
    }
    if usage.cache_read_tokens > 0 {
        data.cache_read_input_tokens = Some(usage.cache_read_tokens as u64);
    }
    let total_tokens = usage
        .input_tokens
        .saturating_add(usage.output_tokens)
        .max(0) as u64;
    if total_tokens > 0 {
        data.total_tokens = Some(total_tokens);
    }
}

fn headers_to_json(headers: &BTreeMap<String, String>) -> Value {
    Value::Object(Map::from_iter(headers.iter().map(|(key, value)| {
        (key.clone(), Value::String(mask_header_value(key, value)))
    })))
}

/// 默认敏感请求头清单。与
/// `apps/aether-gateway/src/handlers/admin/system/shared/configs.rs` 中
/// `sensitive_headers` 系统配置默认值保持一致。
const DEFAULT_SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "x-api-key",
    "api-key",
    "x-goog-api-key",
    "cookie",
    "set-cookie",
    "proxy-authorization",
];

/// 判断 header 名是否属于敏感字段（大小写不敏感）。
fn is_sensitive_header(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    DEFAULT_SENSITIVE_HEADERS.contains(&lower.as_str())
}

/// 对单个 header value 进行脱敏：保留前 4 + 后 4 字符，中间替换为 `****`。
/// 长度小于等于 8 时整体替换为 `****`。
fn mask_header_value(name: &str, value: &str) -> String {
    if !is_sensitive_header(name) {
        return value.to_string();
    }
    if value.len() <= 8 {
        return "****".to_string();
    }
    let prefix: String = value.chars().take(4).collect();
    let suffix: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}****{suffix}")
}

/// 对 JSON 形式的 headers 做就地脱敏。仅当 value 是 Object 时才会处理；
/// 其它形式的值保持不变。
fn mask_sensitive_headers_in_json_value(value: Option<Value>) -> Option<Value> {
    let value = value?;
    let Value::Object(map) = value else {
        return Some(value);
    };
    let masked = map
        .into_iter()
        .map(|(key, val)| {
            if !is_sensitive_header(&key) {
                return (key, val);
            }
            let masked_val = match val {
                Value::String(text) => Value::String(mask_header_value(&key, &text)),
                Value::Null => Value::Null,
                other => Value::String(mask_header_value(&key, &other.to_string())),
            };
            (key, masked_val)
        })
        .collect::<Map<_, _>>();
    Some(Value::Object(masked))
}

fn resolve_error_category(status_code: u16, event_type: UsageEventType) -> Option<String> {
    match event_type {
        UsageEventType::Cancelled => Some("cancelled".to_string()),
        UsageEventType::Failed if status_code >= 500 => Some("server_error".to_string()),
        UsageEventType::Failed if status_code >= 400 => Some("client_error".to_string()),
        _ => None,
    }
}

fn resolve_error_message(
    status_code: u16,
    body_json: Option<&Value>,
    body_base64: Option<&str>,
) -> Option<String> {
    let explicit_error_message = body_json
        .and_then(extract_explicit_error_message_from_json)
        .or_else(|| {
            body_base64
                .and_then(|value| decode_body_for_storage(Some(value)))
                .as_ref()
                .and_then(extract_explicit_error_message_from_json)
        });
    if explicit_error_message.is_some() {
        return explicit_error_message;
    }
    if status_code < 400 {
        return None;
    }

    body_json
        .and_then(extract_generic_error_message_from_json)
        .or_else(|| {
            body_base64
                .and_then(|value| decode_body_for_storage(Some(value)))
                .as_ref()
                .and_then(extract_generic_error_message_from_json)
        })
}

fn extract_explicit_error_message_from_json(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn extract_generic_error_message_from_json(value: &Value) -> Option<String> {
    extract_explicit_error_message_from_json(value).or_else(|| {
        value
            .get("message")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    })
}

fn decode_body_for_storage(body_base64: Option<&str>) -> Option<Value> {
    let body_base64 = body_base64?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body_base64)
        .ok()?;
    if let Ok(json_body) = serde_json::from_slice::<Value>(&bytes) {
        return Some(json_body);
    }
    if let Ok(text) = String::from_utf8(bytes) {
        if let Some(stream_body) = parse_sse_body_for_storage(&text) {
            return Some(stream_body);
        }
        return Some(Value::String(text));
    }
    Some(Value::String(body_base64.to_string()))
}

fn decoded_base64_len_hint(body_base64: &str) -> Option<u64> {
    let body_base64 = body_base64.trim();
    if body_base64.is_empty() {
        return None;
    }

    let usable_len = body_base64.len();
    if usable_len % 4 == 1 {
        return None;
    }

    let padding = body_base64
        .chars()
        .rev()
        .take_while(|char| *char == '=')
        .count();
    let full_quads = usable_len / 4;
    let remainder = usable_len % 4;
    let base_len = full_quads.saturating_mul(3);
    let remainder_len = match remainder {
        0 => 0,
        2 => 1,
        3 => 2,
        _ => return None,
    };
    let decoded_len = base_len
        .saturating_add(remainder_len)
        .saturating_sub(padding.min(2));

    Some(decoded_len as u64)
}

fn parse_sse_body_for_storage(text: &str) -> Option<Value> {
    if !text.contains("data:") {
        return None;
    }

    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut chunks = Vec::new();
    let mut total_chunks = 0_u64;
    let mut saw_done = false;

    for block in normalized.split("\n\n") {
        let data_lines = block
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("data:"))
            .map(|line| line.trim_start_matches("data:").trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();

        if data_lines.is_empty() {
            continue;
        }

        let payload = data_lines.join("\n");
        if payload == "[DONE]" {
            saw_done = true;
            continue;
        }

        total_chunks += 1;
        if let Ok(json_body) = serde_json::from_str::<Value>(&payload) {
            chunks.push(json_body);
        }
    }

    if total_chunks == 0 && !saw_done {
        return None;
    }

    let stored_chunks = chunks.len() as u64;
    let mut metadata = Map::from_iter([
        ("stream".to_string(), Value::Bool(true)),
        ("total_chunks".to_string(), json!(total_chunks)),
        ("stored_chunks".to_string(), json!(stored_chunks)),
        ("content_length".to_string(), json!(text.len())),
    ]);
    if saw_done {
        metadata.insert("has_completion".to_string(), Value::Bool(true));
    }
    if stored_chunks < total_chunks {
        metadata.insert(
            "dropped_chunks".to_string(),
            json!(total_chunks - stored_chunks),
        );
    }

    if chunks.is_empty() {
        metadata.insert(
            "parse_error".to_string(),
            Value::String("Failed to parse response as SSE JSON format".to_string()),
        );
        return Some(json!({
            "chunks": [],
            "raw_response": text,
            "metadata": metadata,
        }));
    }

    Some(json!({
        "chunks": chunks,
        "metadata": metadata,
    }))
}

fn extract_token_counts_from_sse_text(text: &str) -> Option<(u64, u64, u64)> {
    let mut last_seen = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with("data:") {
            continue;
        }
        let payload = line.trim_start_matches("data:").trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        if let Ok(json_body) = serde_json::from_str::<Value>(payload) {
            if let Some(tokens) = extract_token_counts_from_json(&json_body) {
                last_seen = Some(tokens);
            }
        }
    }
    last_seen
}

fn extract_token_counts_from_value(value: &Value) -> Option<(u64, u64, u64)> {
    match value {
        Value::String(text) => extract_token_counts_from_sse_text(text),
        _ => extract_token_counts_from_json(value),
    }
}

fn extract_token_counts_from_json(value: &Value) -> Option<(u64, u64, u64)> {
    if let Some(usage) = value.get("usage").and_then(Value::as_object) {
        let input = usage
            .get("input_tokens")
            .or_else(|| usage.get("prompt_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let output = usage
            .get("output_tokens")
            .or_else(|| usage.get("completion_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let total = usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(input + output);
        return Some((input, output, total));
    }

    if let Some(usage) = value.get("usageMetadata").and_then(Value::as_object) {
        let input = usage
            .get("promptTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let output = usage
            .get("candidatesTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let total = usage
            .get("totalTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(input + output);
        return Some((input, output, total));
    }

    if let Some(chunks) = value.get("chunks").and_then(Value::as_array) {
        let mut last_seen = None;
        for chunk in chunks {
            if let Some(tokens) = extract_token_counts_from_json(chunk) {
                last_seen = Some(tokens);
            }
        }
        if last_seen.is_some() {
            return last_seen;
        }
    }

    if let Some(response) = value.get("response") {
        return extract_token_counts_from_json(response);
    }

    None
}

fn empty_to_none(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        build_pending_usage_record, build_pending_usage_record_from_seed,
        build_stream_terminal_usage_event, build_streaming_usage_record,
        build_sync_terminal_usage_event, build_sync_terminal_usage_payload_seed,
        build_sync_terminal_usage_seed, build_terminal_usage_context_seed,
        build_terminal_usage_event_from_seed, build_usage_event_data_seed,
        extract_token_counts_from_json, headers_to_json, mask_header_value,
        mask_sensitive_headers_in_json_value, LifecycleUsageSeed, TerminalUsageSeed,
        UsageBodyRefsSeed, UsageRoutingSeed, UsageTerminalState, MAX_USAGE_CAPTURE_BYTES,
        MAX_USAGE_CAPTURE_DEPTH,
    };
    use crate::{
        build_upsert_usage_record_from_event, GatewayStreamReportRequest, GatewaySyncReportRequest,
        UsageEvent, UsageEventData, UsageEventType,
    };
    use aether_contracts::{ExecutionPlan, RequestBody};
    use base64::Engine as _;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    #[test]
    fn extracts_openai_usage_tokens() {
        let tokens = extract_token_counts_from_json(&json!({
            "usage": {
                "input_tokens": 3,
                "output_tokens": 5,
                "total_tokens": 8
            }
        }))
        .expect("tokens should exist");

        assert_eq!(tokens, (3, 5, 8));
    }

    #[test]
    fn builds_upsert_record_from_terminal_event() {
        let record = build_upsert_usage_record_from_event(&UsageEvent {
            event_type: UsageEventType::Completed,
            request_id: "req-1".to_string(),
            timestamp_ms: 1_700_000_000_000,
            data: UsageEventData {
                user_id: Some("user-1".to_string()),
                api_key_id: Some("key-1".to_string()),
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                api_format: Some("openai:chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                input_tokens: Some(10),
                output_tokens: Some(20),
                total_tokens: Some(30),
                status_code: Some(200),
                ..UsageEventData::default()
            },
        })
        .expect("record should build");

        assert_eq!(record.request_id, "req-1");
        assert_eq!(record.status, "completed");
        assert_eq!(record.billing_status, "pending");
        assert_eq!(record.total_tokens, Some(30));
    }

    #[test]
    fn pending_usage_records_stay_lightweight() {
        let plan = ExecutionPlan {
            request_id: "req-pending-usage-1".to_string(),
            candidate_id: Some("cand-pending-usage-1".to_string()),
            provider_name: Some("Codex Proxy".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/messages".to_string(),
            headers: BTreeMap::from([(
                "authorization".to_string(),
                "Bearer pending-secret".to_string(),
            )]),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.4",
                "messages": [{"role": "user", "content": "hello"}]
            })),
            stream: false,
            client_api_format: "claude:cli".to_string(),
            provider_api_format: "openai:cli".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        };

        let record = build_pending_usage_record(
            &plan,
            Some(&json!({
                "candidate_id": "cand-pending-usage-1",
                "candidate_index": 0,
                "key_name": "codex-upstream",
                "planner_kind": "claude_cli_sync",
                "route_family": "claude",
                "route_kind": "cli",
                "execution_path": "local_execution_runtime_miss",
                "local_execution_runtime_miss_reason": "all_candidates_skipped",
                "original_headers": {"authorization": "Bearer upstream-secret"},
                "original_request_body": {"messages": [{"content": "should not be persisted in pending"}]},
                "provider_request_headers": {"authorization": "Bearer provider-secret"},
                "provider_request_body": {"input": "should not be persisted in pending"}
            })),
            1_700_000_000,
        )
        .expect("pending usage should build");

        assert_eq!(record.status, "pending");
        assert_eq!(record.billing_status, "pending");
        assert!(record.request_headers.is_none());
        assert!(record.request_body.is_none());
        assert!(record.provider_request_headers.is_none());
        assert!(record.provider_request_body.is_none());
        assert_eq!(record.candidate_id.as_deref(), Some("cand-pending-usage-1"));
        assert_eq!(record.candidate_index, Some(0));
        assert_eq!(record.key_name.as_deref(), Some("codex-upstream"));
        assert_eq!(record.planner_kind.as_deref(), Some("claude_cli_sync"));
        assert_eq!(record.route_family.as_deref(), Some("claude"));
        assert_eq!(record.route_kind.as_deref(), Some("cli"));
        assert_eq!(
            record.execution_path.as_deref(),
            Some("local_execution_runtime_miss")
        );
        assert_eq!(
            record.local_execution_runtime_miss_reason.as_deref(),
            Some("all_candidates_skipped")
        );
        assert_eq!(record.request_metadata, None);
    }

    #[test]
    fn streaming_usage_records_stay_lightweight_by_default() {
        let plan = ExecutionPlan {
            request_id: "req-streaming-usage-1".to_string(),
            candidate_id: Some("cand-streaming-usage-1".to_string()),
            provider_name: Some("Codex Proxy".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/messages".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5.4"})),
            stream: true,
            client_api_format: "claude:cli".to_string(),
            provider_api_format: "openai:cli".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        };

        let record = build_streaming_usage_record(
            &plan,
            Some(&json!({
                "candidate_id": "cand-streaming-usage-1",
                "original_request_body": {"messages": [{"content": "omit me"}]},
                "provider_request_body": {"input": "omit me too"}
            })),
            200,
            None,
            1_700_000_010,
        )
        .expect("streaming usage should build");

        assert_eq!(record.status, "streaming");
        assert_eq!(record.billing_status, "pending");
        assert_eq!(record.status_code, Some(200));
        assert!(record.request_headers.is_none());
        assert!(record.request_body.is_none());
        assert!(record.provider_request_headers.is_none());
        assert!(record.provider_request_body.is_none());
        assert!(record.response_headers.is_none());
        assert!(record.client_response_headers.is_none());
    }

    #[test]
    fn sync_terminal_usage_preserves_overdeep_request_payloads_for_repo_body_storage() {
        const DEEP_NESTED_LEVELS: usize = MAX_USAGE_CAPTURE_DEPTH + 8;
        let mut nested = Value::String("leaf".to_string());
        for depth in 0..DEEP_NESTED_LEVELS {
            nested = json!({
                "depth": depth,
                "child": nested
            });
        }

        let plan = ExecutionPlan {
            request_id: "req-sync-usage-deep-1".to_string(),
            candidate_id: Some("cand-sync-usage-deep-1".to_string()),
            provider_name: Some("Codex Proxy".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/messages".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5.4"})),
            stream: false,
            client_api_format: "claude:cli".to_string(),
            provider_api_format: "openai:cli".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-usage-deep-1".to_string(),
            report_kind: "claude_cli_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "claude:cli",
                "provider_api_format": "openai:cli",
                "needs_conversion": true,
                "original_request_body": nested,
                "provider_request_body": {"input": "safe"}
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: Some(json!({"id": "resp-1"})),
            body_base64: None,
            client_body_json: Some(json!({"type": "message"})),
            telemetry: None,
        };

        let event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("terminal usage should build");

        assert_eq!(
            event.data.request_body,
            payload
                .report_context
                .as_ref()
                .and_then(|value| value.get("original_request_body"))
                .cloned()
        );
        assert_eq!(
            event.data.provider_request_body,
            Some(json!({"input": "safe"}))
        );
    }

    #[test]
    fn builds_stream_terminal_usage_from_provider_body_and_preserves_client_body() {
        let plan = ExecutionPlan {
            request_id: "req-stream-usage-1".to_string(),
            candidate_id: Some("cand-stream-usage-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody {
                json_body: None,
                body_bytes_b64: None,
                body_ref: None,
            },
            stream: true,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:cli".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        };
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-usage-1".to_string(),
            report_kind: "openai_chat_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:chat",
                "provider_api_format": "openai:cli",
                "needs_conversion": true
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            provider_body_base64: Some(
                base64::engine::general_purpose::STANDARD.encode(
                    serde_json::to_vec(&json!({
                        "usage": {
                            "prompt_tokens": 3,
                            "completion_tokens": 5,
                            "total_tokens": 8
                        }
                    }))
                    .expect("provider body should encode"),
                ),
            ),
            client_body_base64: Some(
                base64::engine::general_purpose::STANDARD
                    .encode("data: {\"id\":\"chatcmpl_123\"}\n\ndata: [DONE]\n"),
            ),
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.data.input_tokens, Some(3));
        assert_eq!(event.data.output_tokens, Some(5));
        assert_eq!(event.data.total_tokens, Some(8));
        assert_eq!(
            event.data.response_body,
            Some(json!({
                "usage": {
                    "prompt_tokens": 3,
                    "completion_tokens": 5,
                    "total_tokens": 8
                }
            }))
        );
        assert_eq!(
            event.data.client_response_body,
            Some(json!({
                "chunks": [
                    {
                        "id": "chatcmpl_123"
                    }
                ],
                "metadata": {
                    "stream": true,
                    "total_chunks": 1,
                    "stored_chunks": 1,
                    "content_length": 42,
                    "has_completion": true
                }
            }))
        );
    }

    #[test]
    fn builds_stream_terminal_usage_from_sse_chunks_and_extracts_usage() {
        let plan = ExecutionPlan {
            request_id: "req-stream-usage-2".to_string(),
            candidate_id: Some("cand-stream-usage-2".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody {
                json_body: None,
                body_bytes_b64: None,
                body_ref: None,
            },
            stream: true,
            client_api_format: "openai:cli".to_string(),
            provider_api_format: "openai:cli".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        };
        let sse_body = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"in_progress\"}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello from CLI stream\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello from CLI stream\"}]}],\"usage\":{\"input_tokens\":3,\"input_tokens_details\":{\"cached_tokens\":2,\"cached_creation_tokens\":1},\"output_tokens\":5,\"output_tokens_details\":{\"reasoning_tokens\":1},\"total_tokens\":8}}}\n\n",
            "data: [DONE]\n",
        );
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-usage-2".to_string(),
            report_kind: "openai_cli_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:cli",
                "provider_api_format": "openai:cli",
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            provider_body_base64: Some(base64::engine::general_purpose::STANDARD.encode(sse_body)),
            client_body_base64: None,
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.data.input_tokens, Some(3));
        assert_eq!(event.data.output_tokens, Some(5));
        assert_eq!(event.data.total_tokens, Some(8));
        assert_eq!(event.data.cache_creation_input_tokens, Some(1));
        assert_eq!(event.data.cache_read_input_tokens, Some(2));
        assert_eq!(
            event.data.response_body,
            Some(json!({
                "chunks": [
                    {
                        "type": "response.created",
                        "response": {
                            "id": "resp_123",
                            "object": "response",
                            "model": "gpt-5.4",
                            "status": "in_progress"
                        }
                    },
                    {
                        "type": "response.output_text.delta",
                        "delta": "Hello from CLI stream"
                    },
                    {
                        "type": "response.completed",
                        "response": {
                            "id": "resp_123",
                            "object": "response",
                            "model": "gpt-5.4",
                            "status": "completed",
                            "output": [
                                {
                                    "type": "message",
                                    "role": "assistant",
                                    "content": [
                                        {
                                            "type": "output_text",
                                            "text": "Hello from CLI stream"
                                        }
                                    ]
                                }
                            ],
                            "usage": {
                                "input_tokens": 3,
                                "input_tokens_details": {
                                    "cached_tokens": 2,
                                    "cached_creation_tokens": 1
                                },
                                "output_tokens": 5,
                                "output_tokens_details": {
                                    "reasoning_tokens": 1
                                },
                                "total_tokens": 8
                            }
                        }
                    }
                ],
                "metadata": {
                    "stream": true,
                    "total_chunks": 3,
                    "stored_chunks": 3,
                    "content_length": sse_body.len(),
                    "has_completion": true
                }
            }))
        );
    }

    #[test]
    fn builds_sync_terminal_usage_from_provider_body_and_preserves_client_body() {
        let plan = ExecutionPlan {
            request_id: "req-sync-usage-1".to_string(),
            candidate_id: Some("cand-sync-usage-1".to_string()),
            provider_name: Some("Gemini".to_string()),
            provider_id: "provider-2".to_string(),
            endpoint_id: "endpoint-2".to_string(),
            key_id: "key-2".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1beta/models/gemini:generateContent".to_string(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody {
                json_body: None,
                body_bytes_b64: None,
                body_ref: None,
            },
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "gemini:chat".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-usage-1".to_string(),
            report_kind: "openai_chat_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:chat",
                "provider_api_format": "gemini:chat",
                "needs_conversion": true
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: Some(json!({
                "usageMetadata": {
                    "promptTokenCount": 4,
                    "candidatesTokenCount": 6,
                    "totalTokenCount": 10
                }
            })),
            client_body_json: Some(json!({
                "id": "chatcmpl_456",
                "object": "chat.completion"
            })),
            body_base64: None,
            telemetry: None,
        };

        let event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.data.input_tokens, Some(4));
        assert_eq!(event.data.output_tokens, Some(6));
        assert_eq!(event.data.total_tokens, Some(10));
        assert_eq!(
            event.data.response_body,
            Some(json!({
                "usageMetadata": {
                    "promptTokenCount": 4,
                    "candidatesTokenCount": 6,
                    "totalTokenCount": 10
                }
            }))
        );
        assert_eq!(
            event.data.client_response_body,
            Some(json!({
                "id": "chatcmpl_456",
                "object": "chat.completion"
            }))
        );
    }

    #[test]
    fn sync_terminal_seed_path_matches_legacy_wrapper_event() {
        let plan = ExecutionPlan {
            request_id: "req-sync-seed-match-1".to_string(),
            candidate_id: Some("cand-sync-seed-match-1".to_string()),
            provider_name: Some("Gemini".to_string()),
            provider_id: "provider-2".to_string(),
            endpoint_id: "endpoint-2".to_string(),
            key_id: "key-2".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1beta/models/gemini:generateContent".to_string(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody {
                json_body: None,
                body_bytes_b64: None,
                body_ref: None,
            },
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "gemini:chat".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-seed-match-1".to_string(),
            report_kind: "openai_chat_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:chat",
                "provider_api_format": "gemini:chat",
                "needs_conversion": true
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: Some(json!({
                "usageMetadata": {
                    "promptTokenCount": 4,
                    "candidatesTokenCount": 6,
                    "totalTokenCount": 10
                }
            })),
            client_body_json: Some(json!({
                "id": "chatcmpl_456",
                "object": "chat.completion"
            })),
            body_base64: None,
            telemetry: None,
        };

        let legacy_event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("legacy terminal usage should build");
        let context_seed =
            build_terminal_usage_context_seed(&plan, payload.report_context.as_ref());
        let payload_seed = build_sync_terminal_usage_payload_seed(&payload);
        let seed_event = build_terminal_usage_event_from_seed(build_sync_terminal_usage_seed(
            context_seed,
            payload_seed,
        ))
        .expect("seed terminal usage should build");

        assert_eq!(seed_event.event_type, legacy_event.event_type);
        assert_eq!(seed_event.request_id, legacy_event.request_id);
        assert_eq!(seed_event.data, legacy_event.data);
    }

    #[test]
    fn sync_terminal_usage_exposes_provider_request_body_ref_as_typed_field() {
        let plan = ExecutionPlan {
            request_id: "req-sync-body-ref-1".to_string(),
            candidate_id: Some("cand-sync-body-ref-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody {
                json_body: Some(json!({
                    "model": "gpt-5.4",
                    "input": "this should stay out of usage storage"
                })),
                body_bytes_b64: None,
                body_ref: Some("blob://provider-request-1".to_string()),
            },
            stream: false,
            client_api_format: "openai:cli".to_string(),
            provider_api_format: "openai:cli".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-body-ref-1".to_string(),
            report_kind: "openai_cli_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:cli",
                "provider_api_format": "openai:cli",
                "trace_id": "trace-sync-body-ref-1"
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: Some(json!({"id": "resp_123"})),
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        };

        let event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert!(event.data.request_body.is_none());
        assert!(event.data.provider_request_body.is_none());
        assert_eq!(
            event.data.provider_request_body_ref.as_deref(),
            Some("blob://provider-request-1")
        );
        assert!(event
            .data
            .request_metadata
            .as_ref()
            .is_none_or(|value| { value.get("provider_request_body_ref").is_none() }));
    }

    #[test]
    fn stream_terminal_usage_records_base64_response_sizes_in_metadata() {
        let provider_bytes =
            b"{\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}";
        let client_bytes = b"data: {\"id\":\"chatcmpl_123\"}\n\ndata: [DONE]\n";
        let plan = ExecutionPlan {
            request_id: "req-stream-bytes-1".to_string(),
            candidate_id: Some("cand-stream-bytes-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody {
                json_body: None,
                body_bytes_b64: None,
                body_ref: None,
            },
            stream: true,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:cli".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        };
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-bytes-1".to_string(),
            report_kind: "openai_chat_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:chat",
                "provider_api_format": "openai:cli",
                "trace_id": "trace-stream-bytes-1"
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            provider_body_base64: Some(
                base64::engine::general_purpose::STANDARD.encode(provider_bytes),
            ),
            client_body_base64: Some(
                base64::engine::general_purpose::STANDARD.encode(client_bytes),
            ),
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("provider_response_body_base64_bytes"))
                .and_then(|value| value.as_u64()),
            Some(provider_bytes.len() as u64)
        );
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("client_response_body_base64_bytes"))
                .and_then(|value| value.as_u64()),
            Some(client_bytes.len() as u64)
        );
    }

    #[test]
    fn stream_terminal_usage_preserves_large_sse_response_for_repo_body_storage() {
        let large_delta = "x".repeat(MAX_USAGE_CAPTURE_BYTES);
        let sse_body = format!(
            concat!(
                "event: response.created\n",
                "data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"in_progress\"}}}}\n\n",
                "event: response.output_text.delta\n",
                "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{delta}\"}}\n\n",
                "event: response.completed\n",
                "data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"usage\":{{\"input_tokens\":3,\"input_tokens_details\":{{\"cached_tokens\":2,\"cached_creation_tokens\":1}},\"output_tokens\":5,\"output_tokens_details\":{{\"reasoning_tokens\":1}},\"total_tokens\":8}}}}}}\n\n",
                "data: [DONE]\n",
            ),
            delta = large_delta
        );
        let plan = ExecutionPlan {
            request_id: "req-stream-usage-large-1".to_string(),
            candidate_id: Some("cand-stream-usage-large-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody {
                json_body: None,
                body_bytes_b64: None,
                body_ref: None,
            },
            stream: true,
            client_api_format: "openai:cli".to_string(),
            provider_api_format: "openai:cli".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        };
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-usage-large-1".to_string(),
            report_kind: "openai_cli_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:cli",
                "provider_api_format": "openai:cli",
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            provider_body_base64: Some(base64::engine::general_purpose::STANDARD.encode(&sse_body)),
            client_body_base64: None,
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.data.input_tokens, Some(3));
        assert_eq!(event.data.output_tokens, Some(5));
        assert_eq!(event.data.total_tokens, Some(8));
        assert_eq!(event.data.cache_creation_input_tokens, Some(1));
        assert_eq!(event.data.cache_read_input_tokens, Some(2));
        assert_eq!(
            event
                .data
                .response_body
                .as_ref()
                .and_then(|value| value.get("chunks"))
                .and_then(Value::as_array)
                .and_then(|chunks| chunks.get(1))
                .and_then(|value| value.get("delta"))
                .and_then(Value::as_str),
            Some(large_delta.as_str())
        );
    }

    #[test]
    fn sync_terminal_usage_does_not_fallback_request_body_to_plan_when_client_echo_is_absent() {
        let plan = ExecutionPlan {
            request_id: "req-sync-no-client-echo-1".to_string(),
            candidate_id: Some("cand-sync-no-client-echo-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.4",
                "input": [{"role": "user", "content": "provider-side compiled body"}],
            })),
            stream: false,
            client_api_format: "claude:cli".to_string(),
            provider_api_format: "openai:cli".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-no-client-echo-1".to_string(),
            report_kind: "claude_cli_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "claude:cli",
                "provider_api_format": "openai:cli",
                "needs_conversion": true,
                "original_request_body": null,
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: Some(json!({"id": "resp_1"})),
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        };

        let event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert!(event.data.request_body.is_none());
        assert_eq!(
            event.data.provider_request_body,
            Some(json!({
                "model": "gpt-5.4",
                "input": [{"role": "user", "content": "provider-side compiled body"}],
            }))
        );
    }

    #[test]
    fn manual_terminal_seed_event_builder_sanitizes_headers_and_metadata_but_preserves_bodies() {
        let event = build_terminal_usage_event_from_seed(TerminalUsageSeed {
            terminal_state: UsageTerminalState::Completed,
            client_contract: "openai:chat".to_string(),
            provider_contract: "openai:chat".to_string(),
            request_id: "req-manual-seed-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            username: Some("alice".to_string()),
            api_key_name: Some("primary".to_string()),
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: Some("provider-1".to_string()),
            provider_endpoint_id: Some("endpoint-1".to_string()),
            provider_api_key_id: Some("upstream-key-1".to_string()),
            request_type: "chat".to_string(),
            has_format_conversion: false,
            is_stream: false,
            body_refs: UsageBodyRefsSeed::default(),
            routing: UsageRoutingSeed {
                candidate_id: Some("cand-1".to_string()),
                ..UsageRoutingSeed::default()
            },
            status_code: 200,
            response_time_ms: Some(123),
            first_byte_time_ms: Some(45),
            request_headers: Some(json!({
                "authorization": "Bearer very-secret-token",
                "accept": "application/json"
            })),
            request_body: Some(json!({
                "payload": "x".repeat(MAX_USAGE_CAPTURE_BYTES + 1)
            })),
            provider_request_headers: Some(json!({
                "x-api-key": "sk-proj-super-secret"
            })),
            provider_request: None,
            provider_response_headers: Some(json!({
                "set-cookie": "session=extremely-secret-cookie"
            })),
            provider_response: Some(json!({
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 2,
                    "total_tokens": 3
                }
            })),
            client_response_headers: Some(json!({
                "authorization": "Bearer client-secret"
            })),
            client_response: None,
            request_metadata: Some(json!({
                "candidate_id": "cand-1",
                "billing_snapshot": {
                    "payload": "x".repeat(32 * 1024)
                }
            })),
            audit_payload: None,
            standardized_usage: None,
        })
        .expect("usage event should build");

        assert_eq!(
            event.data.request_headers,
            Some(json!({
                "authorization": "Bear****oken",
                "accept": "application/json"
            }))
        );
        assert_eq!(
            event.data.provider_request_headers,
            Some(json!({
                "x-api-key": "sk-p****cret"
            }))
        );
        assert_eq!(
            event.data.response_headers,
            Some(json!({
                "set-cookie": "sess****okie"
            }))
        );
        assert_eq!(
            event.data.client_response_headers,
            Some(json!({
                "authorization": "Bear****cret"
            }))
        );
        assert_eq!(
            event.data.request_body,
            Some(json!({
                "payload": "x".repeat(MAX_USAGE_CAPTURE_BYTES + 1)
            }))
        );
        assert_eq!(event.data.input_tokens, Some(1));
        assert_eq!(event.data.output_tokens, Some(2));
        assert_eq!(event.data.total_tokens, Some(3));
        assert_eq!(event.data.candidate_id.as_deref(), Some("cand-1"));
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("candidate_id")),
            None
        );
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("billing_snapshot")),
            Some(&json!({
                "truncated": true,
                "reason": "usage_request_metadata_limits_exceeded",
                "max_depth": 32,
                "max_nodes": 4_000,
                "max_bytes": 16 * 1024,
                "value_kind": "object"
            }))
        );
    }

    #[test]
    fn lifecycle_seed_builder_sanitizes_manual_request_metadata() {
        let record = build_pending_usage_record_from_seed(
            &LifecycleUsageSeed {
                request_id: "req-lifecycle-manual-1".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("key-1".to_string()),
                username: Some("alice".to_string()),
                api_key_name: Some("primary".to_string()),
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                target_model: None,
                provider_id: Some("provider-1".to_string()),
                provider_endpoint_id: Some("endpoint-1".to_string()),
                provider_api_key_id: Some("upstream-key-1".to_string()),
                request_type: "chat".to_string(),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(false),
                is_stream: false,
                routing: UsageRoutingSeed {
                    candidate_id: Some("cand-1".to_string()),
                    ..UsageRoutingSeed::default()
                },
                request_metadata: Some(json!({
                    "billing_snapshot": {
                        "payload": "x".repeat(32 * 1024)
                    }
                })),
            },
            1_700_000_000,
        )
        .expect("pending record should build");

        assert_eq!(record.candidate_id.as_deref(), Some("cand-1"));
        assert_eq!(
            record.request_metadata,
            Some(json!({
                "billing_snapshot": {
                    "truncated": true,
                    "reason": "usage_request_metadata_limits_exceeded",
                    "max_depth": 32,
                    "max_nodes": 4_000,
                    "max_bytes": 16 * 1024,
                    "value_kind": "object"
                }
            }))
        );
    }

    #[test]
    fn usage_event_data_seed_masks_headers_before_outcome_paths_use_it() {
        let plan = ExecutionPlan {
            request_id: "req-seed-sanitize-1".to_string(),
            candidate_id: Some("cand-seed-sanitize-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/chat/completions".to_string(),
            headers: BTreeMap::new(),
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
        };

        let data = build_usage_event_data_seed(
            &plan,
            Some(&json!({
                "original_headers": {
                    "authorization": "Bearer outcome-secret",
                    "accept": "application/json"
                },
                "original_request_body": {
                    "payload": "x".repeat(MAX_USAGE_CAPTURE_BYTES + 1)
                }
            })),
        );

        assert_eq!(
            data.request_headers,
            Some(json!({
                "authorization": "Bear****cret",
                "accept": "application/json"
            }))
        );
        assert_eq!(
            data.request_body,
            Some(json!({
                "payload": "x".repeat(MAX_USAGE_CAPTURE_BYTES + 1)
            }))
        );
    }

    #[test]
    fn masks_known_sensitive_header_values() {
        let token = "Bearer eyJhbGciOiJSUzI1NiJ9.payload-here.signature-tail";
        let masked = mask_header_value("authorization", token);
        assert!(masked.starts_with("Bear"));
        assert!(masked.ends_with("tail"));
        assert!(masked.contains("****"));
        assert!(!masked.contains("payload-here"));

        // 大小写不敏感
        assert_eq!(
            mask_header_value("Authorization", "12345678"),
            "****",
            "短值整体替换为 ****",
        );
        assert_eq!(mask_header_value("X-Api-Key", "abcdefghij"), "abcd****ghij",);

        // 非敏感头保持原样
        assert_eq!(
            mask_header_value("user-agent", "codex-tui/0.1"),
            "codex-tui/0.1",
        );
    }

    #[test]
    fn headers_to_json_masks_sensitive_headers_at_source() {
        let mut headers = BTreeMap::new();
        headers.insert(
            "authorization".to_string(),
            "Bearer eyJhbGciOiJSUzI1NiJ9.body.signature".to_string(),
        );
        headers.insert("user-agent".to_string(), "codex-tui/0.1".to_string());
        headers.insert(
            "x-api-key".to_string(),
            "sk-proj-1234567890abcdef".to_string(),
        );

        let value = headers_to_json(&headers);
        let object = value.as_object().expect("expected object");

        let auth = object
            .get("authorization")
            .and_then(|v| v.as_str())
            .expect("authorization should be string");
        assert!(auth.starts_with("Bear"));
        assert!(auth.contains("****"));
        assert!(!auth.contains("eyJhbGciOiJSUzI1NiJ9"));

        let api_key = object
            .get("x-api-key")
            .and_then(|v| v.as_str())
            .expect("x-api-key should be string");
        assert!(api_key.starts_with("sk-p"));
        assert!(api_key.contains("****"));
        assert!(!api_key.contains("1234567890"));

        assert_eq!(
            object.get("user-agent").and_then(|v| v.as_str()),
            Some("codex-tui/0.1"),
        );
    }

    #[test]
    fn mask_sensitive_headers_in_json_value_handles_object_form() {
        let value = json!({
            "Authorization": "Bearer eyJhbGciOiJSUzI1NiJ9.body.signature",
            "Cookie": "session=verylongcookievalue1234",
            "Accept": "application/json",
        });
        let masked =
            mask_sensitive_headers_in_json_value(Some(value)).expect("masked value should exist");
        let object = masked.as_object().expect("expected object");

        let auth = object
            .get("Authorization")
            .and_then(|v| v.as_str())
            .expect("Authorization should be string");
        assert!(auth.contains("****"));
        assert!(!auth.contains("eyJhbGciOiJSUzI1NiJ9"));

        let cookie = object
            .get("Cookie")
            .and_then(|v| v.as_str())
            .expect("Cookie should be string");
        assert!(cookie.contains("****"));
        assert!(!cookie.contains("verylongcookievalue"));

        assert_eq!(
            object.get("Accept").and_then(|v| v.as_str()),
            Some("application/json"),
        );
    }

    #[test]
    fn mask_sensitive_headers_passthrough_for_non_object() {
        // None 输入返回 None
        assert!(mask_sensitive_headers_in_json_value(None).is_none());
        // 非 object 输入原样返回
        let masked = mask_sensitive_headers_in_json_value(Some(json!("not an object")));
        assert_eq!(masked, Some(json!("not an object")));
    }
}
