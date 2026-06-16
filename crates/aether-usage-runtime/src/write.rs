use std::collections::BTreeMap;

use aether_ai_formats::UPSTREAM_IS_STREAM_KEY;
use aether_contracts::{ExecutionPlan, ExecutionTelemetry};
use aether_data_contracts::repository::usage::{UpsertUsageRecord, UsageBodyCaptureState};
use aether_data_contracts::DataLayerError;
use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::body_capture::{
    append_runtime_body_capture_metadata, build_payload_body_capture_metadata,
    build_plan_body_capture_metadata, build_runtime_body_capture_states, decoded_base64_len_hint,
    RuntimeBodyCaptureMetadataInput,
};
use crate::request_metadata::{
    attach_provider_request_body_metadata, build_usage_request_metadata_seed,
    merge_usage_request_metadata, merge_usage_request_metadata_owned,
    sanitize_usage_request_metadata, sanitize_usage_request_metadata_ref,
};
use crate::{
    map_usage_from_response, stream_capture_terminal_state, GatewayStreamReportRequest,
    GatewaySyncReportRequest, StandardizedUsage, StreamCapturedTerminalState, UsageEvent,
    UsageEventData, UsageEventType, STREAM_MISSING_TERMINAL_EVENT_CATEGORY,
    STREAM_MISSING_TERMINAL_EVENT_MESSAGE, STREAM_TERMINAL_ERROR_CATEGORY,
    STREAM_TERMINAL_ERROR_MESSAGE,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageBodyStatesSeed {
    request_body_state: Option<UsageBodyCaptureState>,
    provider_request_body_state: Option<UsageBodyCaptureState>,
    response_body_state: Option<UsageBodyCaptureState>,
    client_response_body_state: Option<UsageBodyCaptureState>,
}

#[derive(Debug, Clone)]
struct RuntimeRequestCaptureSeed {
    request_body: Option<Value>,
    request_body_ref: Option<String>,
    provider_request: Option<Value>,
    provider_request_body_ref: Option<String>,
    body_states: UsageBodyStatesSeed,
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
    pub model_id: Option<String>,
    pub global_model_id: Option<String>,
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
    body_states: UsageBodyStatesSeed,
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
    pub model_id: Option<String>,
    pub global_model_id: Option<String>,
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
    body_states: UsageBodyStatesSeed,
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
    pub client_response_headers: Option<Value>,
    pub provider_response_full: Option<Value>,
    pub provider_response_body_state: Option<UsageBodyCaptureState>,
    pub client_response: Option<Value>,
    pub client_response_body_state: Option<UsageBodyCaptureState>,
    pub standardized_usage: Option<StandardizedUsage>,
    pub capture_metadata: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct StreamTerminalUsagePayloadSeed {
    pub report_kind: String,
    pub status_code: u16,
    pub response_time_ms: Option<u64>,
    pub first_byte_time_ms: Option<u64>,
    pub provider_response_headers: Option<Value>,
    pub client_response_headers: Option<Value>,
    pub provider_response_full: Option<Value>,
    pub provider_response_body_state: Option<UsageBodyCaptureState>,
    pub client_response: Option<Value>,
    pub client_response_body_state: Option<UsageBodyCaptureState>,
    pub standardized_usage: Option<StandardizedUsage>,
    pub observed_stream_finish: Option<bool>,
    pub terminal_error_message: Option<String>,
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
    pub model_id: Option<String>,
    pub global_model_id: Option<String>,
    pub provider_id: Option<String>,
    pub provider_endpoint_id: Option<String>,
    pub provider_api_key_id: Option<String>,
    pub request_type: String,
    pub has_format_conversion: bool,
    pub is_stream: bool,
    pub status_code: u16,
    pub terminal_error_message: Option<String>,
    pub terminal_failure_category: Option<String>,
    pub response_time_ms: Option<u64>,
    pub first_byte_time_ms: Option<u64>,
    pub request_headers: Option<Value>,
    pub request_body: Option<Value>,
    pub provider_request_headers: Option<Value>,
    pub provider_request: Option<Value>,
    body_refs: UsageBodyRefsSeed,
    body_states: UsageBodyStatesSeed,
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
    options: LifecycleUsageRecordOptions,
}

struct OwnedLifecycleUsageRecordInput {
    seed: LifecycleUsageSeed,
    options: LifecycleUsageRecordOptions,
}

struct LifecycleUsageRecordOptions {
    lifecycle_state: UsageLifecycleState,
    status_code: Option<u16>,
    response_time_ms: Option<u64>,
    first_byte_time_ms: Option<u64>,
    response_headers: Option<Value>,
    client_response_headers: Option<Value>,
    updated_at_unix_secs: u64,
    trusted_request_metadata: bool,
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
        .or_else(|| non_empty_str(Some(plan.client_api_format.as_str())));
    let endpoint_api_format = context_string(context, "provider_api_format")
        .or_else(|| non_empty_str(Some(plan.provider_api_format.as_str())));
    let provider_name = context_string(context, "provider_name")
        .or_else(|| non_empty_str(plan.provider_name.as_deref()))
        .unwrap_or_else(|| "unknown".to_string());
    let model = context_string(context, "model")
        .or_else(|| non_empty_str(plan.model_name.as_deref()))
        .unwrap_or_else(|| "unknown".to_string());
    let request_type =
        infer_request_type_from_contracts(api_format.as_deref(), endpoint_api_format.as_deref());
    let api_family = api_format
        .as_deref()
        .and_then(infer_api_family)
        .map(ToOwned::to_owned);
    let endpoint_kind = api_format
        .as_deref()
        .and_then(infer_endpoint_kind)
        .map(ToOwned::to_owned);
    let provider_api_family = endpoint_api_format
        .as_deref()
        .and_then(infer_api_family)
        .map(ToOwned::to_owned);
    let provider_endpoint_kind = endpoint_api_format
        .as_deref()
        .and_then(infer_endpoint_kind)
        .map(ToOwned::to_owned);

    LifecycleUsageSeed {
        request_id: plan.request_id.clone(),
        user_id: context_string(context, "user_id"),
        api_key_id: context_string(context, "api_key_id"),
        username: context_string(context, "username"),
        api_key_name: context_string(context, "api_key_name"),
        provider_name,
        model,
        target_model: context_string(context, "mapped_model"),
        model_id: context_string(context, "model_id"),
        global_model_id: context_string(context, "global_model_id"),
        provider_id: empty_to_none(
            context_string(context, "provider_id")
                .or_else(|| non_empty_str(Some(plan.provider_id.as_str()))),
        ),
        provider_endpoint_id: empty_to_none(
            context_string(context, "endpoint_id")
                .or_else(|| non_empty_str(Some(plan.endpoint_id.as_str()))),
        ),
        provider_api_key_id: empty_to_none(
            context_string(context, "key_id").or_else(|| non_empty_str(Some(plan.key_id.as_str()))),
        ),
        request_type,
        api_format,
        api_family,
        endpoint_kind,
        endpoint_api_format,
        provider_api_family,
        provider_endpoint_kind,
        has_format_conversion: context_bool(context, "needs_conversion"),
        is_stream: plan.stream,
        routing: build_runtime_routing_seed(plan, context),
        body_states: build_runtime_body_states_seed(plan, context),
        request_metadata: build_runtime_request_metadata_seed(plan, context),
    }
}

pub fn build_pending_usage_record(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    updated_at_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    let seed = build_lifecycle_usage_seed(plan, report_context);
    build_lifecycle_usage_record_owned(OwnedLifecycleUsageRecordInput {
        seed,
        options: LifecycleUsageRecordOptions {
            lifecycle_state: UsageLifecycleState::Pending,
            status_code: None,
            response_time_ms: None,
            first_byte_time_ms: None,
            response_headers: None,
            client_response_headers: None,
            updated_at_unix_secs,
            trusted_request_metadata: false,
        },
    })
}

pub fn build_pending_usage_record_from_seed(
    seed: &LifecycleUsageSeed,
    updated_at_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    build_lifecycle_usage_record(LifecycleUsageRecordInput {
        seed,
        options: LifecycleUsageRecordOptions {
            lifecycle_state: UsageLifecycleState::Pending,
            status_code: None,
            response_time_ms: None,
            first_byte_time_ms: None,
            response_headers: None,
            client_response_headers: None,
            updated_at_unix_secs,
            trusted_request_metadata: false,
        },
    })
}

pub(crate) fn build_pending_usage_record_from_owned_seed(
    seed: LifecycleUsageSeed,
    updated_at_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    build_lifecycle_usage_record_owned(OwnedLifecycleUsageRecordInput {
        seed,
        options: LifecycleUsageRecordOptions {
            lifecycle_state: UsageLifecycleState::Pending,
            status_code: None,
            response_time_ms: None,
            first_byte_time_ms: None,
            response_headers: None,
            client_response_headers: None,
            updated_at_unix_secs,
            trusted_request_metadata: false,
        },
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
    build_lifecycle_usage_record_owned(OwnedLifecycleUsageRecordInput {
        seed,
        options: LifecycleUsageRecordOptions {
            lifecycle_state: UsageLifecycleState::Streaming,
            status_code: Some(status_code),
            response_time_ms: telemetry.and_then(|value| value.elapsed_ms),
            first_byte_time_ms: telemetry.and_then(|value| value.ttfb_ms),
            response_headers: None,
            client_response_headers: None,
            updated_at_unix_secs,
            trusted_request_metadata: false,
        },
    })
}

pub fn build_streaming_usage_record_from_seed(
    seed: &LifecycleUsageSeed,
    status_code: u16,
    telemetry: Option<&ExecutionTelemetry>,
    updated_at_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    build_lifecycle_usage_record(LifecycleUsageRecordInput {
        seed,
        options: LifecycleUsageRecordOptions {
            lifecycle_state: UsageLifecycleState::Streaming,
            status_code: Some(status_code),
            response_time_ms: telemetry.and_then(|value| value.elapsed_ms),
            first_byte_time_ms: telemetry.and_then(|value| value.ttfb_ms),
            response_headers: None,
            client_response_headers: None,
            updated_at_unix_secs,
            trusted_request_metadata: false,
        },
    })
}

pub(crate) fn build_streaming_usage_record_from_owned_seed(
    seed: LifecycleUsageSeed,
    status_code: u16,
    telemetry: Option<ExecutionTelemetry>,
    updated_at_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    build_lifecycle_usage_record_owned(OwnedLifecycleUsageRecordInput {
        seed,
        options: LifecycleUsageRecordOptions {
            lifecycle_state: UsageLifecycleState::Streaming,
            status_code: Some(status_code),
            response_time_ms: telemetry.as_ref().and_then(|value| value.elapsed_ms),
            first_byte_time_ms: telemetry.as_ref().and_then(|value| value.ttfb_ms),
            response_headers: None,
            client_response_headers: None,
            updated_at_unix_secs,
            trusted_request_metadata: false,
        },
    })
}

pub fn build_sync_terminal_usage_event(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    payload: &GatewaySyncReportRequest,
) -> Result<UsageEvent, DataLayerError> {
    let context_seed = build_terminal_usage_context_seed(plan, report_context);
    let payload_seed = build_sync_terminal_usage_payload_seed(payload);
    build_terminal_usage_event_from_seed_impl(
        build_sync_terminal_usage_seed(context_seed, payload_seed),
        true,
    )
}

pub fn build_stream_terminal_usage_event(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    payload: &GatewayStreamReportRequest,
) -> Result<UsageEvent, DataLayerError> {
    let context_seed = build_terminal_usage_context_seed(plan, report_context);
    let payload_seed = build_stream_terminal_usage_payload_seed(payload);
    build_terminal_usage_event_from_seed_impl(
        build_stream_terminal_usage_seed(context_seed, payload_seed, false),
        true,
    )
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
    build_terminal_usage_event_from_seed_impl(seed, false)
}

fn build_terminal_usage_event_from_seed_impl(
    seed: TerminalUsageSeed,
    trusted_request_metadata: bool,
) -> Result<UsageEvent, DataLayerError> {
    let TerminalUsageSeed {
        terminal_state,
        client_contract,
        provider_contract,
        request_id,
        user_id,
        api_key_id,
        username,
        api_key_name,
        provider_name,
        model,
        target_model,
        model_id,
        global_model_id,
        provider_id,
        provider_endpoint_id,
        provider_api_key_id,
        request_type,
        has_format_conversion,
        is_stream,
        status_code,
        terminal_error_message,
        terminal_failure_category,
        response_time_ms,
        first_byte_time_ms,
        request_headers,
        request_body,
        provider_request_headers,
        provider_request,
        body_refs,
        body_states,
        provider_response_headers,
        provider_response,
        client_response_headers,
        client_response,
        routing,
        request_metadata,
        audit_payload,
        standardized_usage,
    } = seed;
    let event_type = match terminal_state {
        UsageTerminalState::Completed => UsageEventType::Completed,
        UsageTerminalState::Failed => UsageEventType::Failed,
        UsageTerminalState::Cancelled => UsageEventType::Cancelled,
    };
    let routing = merge_routing_seed_with_metadata_owned(routing, request_metadata.as_ref());
    let body_refs = merge_body_refs_seed_with_metadata_owned(body_refs, request_metadata.as_ref());
    let error_message = terminal_error_message
        .or_else(|| resolve_error_message(status_code, provider_response.as_ref(), None))
        .or_else(|| resolve_error_message(status_code, client_response.as_ref(), None));
    let api_family = infer_api_family(&client_contract).map(ToOwned::to_owned);
    let endpoint_kind = infer_endpoint_kind(&client_contract).map(ToOwned::to_owned);
    let provider_api_family = infer_api_family(&provider_contract).map(ToOwned::to_owned);
    let provider_endpoint_kind = infer_endpoint_kind(&provider_contract).map(ToOwned::to_owned);
    let derived_standardized_usage = provider_response
        .as_ref()
        .filter(|response_body| response_body.is_object())
        .map(|response_body| map_usage_from_response(response_body, provider_contract.as_str()))
        .filter(StandardizedUsage::has_token_signal);
    let standardized_usage =
        StandardizedUsage::choose_more_complete(standardized_usage, derived_standardized_usage);
    let request_metadata = if trusted_request_metadata {
        merge_usage_request_metadata_owned(request_metadata, audit_payload)
    } else {
        merge_usage_request_metadata(request_metadata, audit_payload)
    };
    let request_metadata =
        attach_provider_request_body_metadata(request_metadata, provider_request.as_ref());

    let mut data = UsageEventData {
        user_id,
        api_key_id,
        username,
        api_key_name,
        provider_name,
        model,
        target_model,
        model_id,
        global_model_id,
        provider_id,
        provider_endpoint_id,
        provider_api_key_id,
        request_type: Some(request_type),
        api_format: Some(client_contract),
        api_family,
        endpoint_kind,
        endpoint_api_format: Some(provider_contract),
        provider_api_family,
        provider_endpoint_kind,
        has_format_conversion: Some(has_format_conversion),
        is_stream: Some(is_stream),
        status_code: Some(status_code),
        error_message,
        error_category: resolve_error_category(
            status_code,
            event_type,
            is_stream,
            terminal_failure_category.as_deref(),
        ),
        response_time_ms,
        first_byte_time_ms,
        request_headers,
        request_body,
        request_body_ref: body_refs.request_body_ref,
        request_body_state: body_states.request_body_state,
        provider_request_headers,
        provider_request_body: provider_request,
        provider_request_body_ref: body_refs.provider_request_body_ref,
        provider_request_body_state: body_states.provider_request_body_state,
        response_headers: provider_response_headers,
        response_body: provider_response,
        response_body_ref: body_refs.response_body_ref,
        response_body_state: body_states.response_body_state,
        client_response_headers,
        client_response_body: client_response,
        client_response_body_ref: body_refs.client_response_body_ref,
        client_response_body_state: body_states.client_response_body_state,
        candidate_id: routing.candidate_id,
        key_name: routing.key_name,
        planner_kind: routing.planner_kind,
        route_family: routing.route_family,
        route_kind: routing.route_kind,
        execution_path: routing.execution_path,
        local_execution_runtime_miss_reason: routing.local_execution_runtime_miss_reason,
        request_metadata,
        ..UsageEventData::default()
    };

    if let Some(usage) = standardized_usage.as_ref() {
        apply_standardized_usage_seed(usage, &mut data);
    }

    if data.total_tokens.is_none() {
        if let Some(tokens) = data
            .response_body
            .as_ref()
            .and_then(extract_token_counts_from_value)
        {
            data.input_tokens = Some(tokens.0);
            data.output_tokens = Some(tokens.1);
            data.total_tokens = Some(tokens.2);
        }
    }

    if matches!(event_type, UsageEventType::Completed) {
        apply_completed_image_usage_estimate(&mut data);
    }

    let data = if trusted_request_metadata {
        sanitize_usage_event_capture_fields_trusted(data)
    } else {
        sanitize_usage_event_data(data)
    };

    Ok(UsageEvent::new(event_type, request_id, data))
}

pub fn build_terminal_usage_context_seed(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> TerminalUsageContextSeed {
    let context = report_context.and_then(Value::as_object);
    let request_capture = build_runtime_request_capture_seed(plan, context);
    let client_contract = context_string(context, "client_contract")
        .or_else(|| context_string(context, "client_api_format"))
        .or_else(|| non_empty_str(Some(plan.client_api_format.as_str())))
        .unwrap_or_default();
    let provider_contract = context_string(context, "provider_contract")
        .or_else(|| context_string(context, "provider_api_format"))
        .or_else(|| non_empty_str(Some(plan.provider_api_format.as_str())))
        .unwrap_or_default();
    let request_type = infer_request_type_from_contracts(
        Some(client_contract.as_str()),
        Some(provider_contract.as_str()),
    );
    let has_format_conversion = resolve_has_format_conversion(
        context,
        client_contract.as_str(),
        provider_contract.as_str(),
    );

    TerminalUsageContextSeed {
        client_contract,
        provider_contract,
        has_format_conversion,
        request_id: plan.request_id.clone(),
        user_id: context_string(context, "user_id"),
        api_key_id: context_string(context, "api_key_id"),
        username: context_string(context, "username"),
        api_key_name: context_string(context, "api_key_name"),
        provider_name: context_string(context, "provider_name")
            .or_else(|| non_empty_str(plan.provider_name.as_deref()))
            .unwrap_or_else(|| "unknown".to_string()),
        model: context_string(context, "model")
            .or_else(|| non_empty_str(plan.model_name.as_deref()))
            .unwrap_or_else(|| "unknown".to_string()),
        target_model: context_string(context, "mapped_model"),
        model_id: context_string(context, "model_id"),
        global_model_id: context_string(context, "global_model_id"),
        provider_id: context_string(context, "provider_id")
            .or_else(|| non_empty_str(Some(plan.provider_id.as_str()))),
        provider_endpoint_id: context_string(context, "endpoint_id")
            .or_else(|| non_empty_str(Some(plan.endpoint_id.as_str()))),
        provider_api_key_id: context_string(context, "key_id")
            .or_else(|| non_empty_str(Some(plan.key_id.as_str()))),
        request_type,
        is_stream: plan.stream,
        routing: build_runtime_routing_seed(plan, context),
        request_headers: mask_sensitive_headers_in_json_value(context_value(
            context,
            "original_headers",
        )),
        request_body: request_capture.request_body,
        provider_request_headers: mask_sensitive_headers_in_json_value(
            context_usage_value(context, "provider_request_headers")
                .or_else(|| headers_to_json(&plan.headers)),
        ),
        provider_request: request_capture.provider_request,
        body_refs: UsageBodyRefsSeed {
            request_body_ref: request_capture.request_body_ref,
            provider_request_body_ref: request_capture.provider_request_body_ref,
            response_body_ref: context_string(context, "response_body_ref"),
            client_response_body_ref: context_string(context, "client_response_body_ref"),
        },
        body_states: request_capture.body_states,
        request_metadata: merge_usage_request_metadata_owned(
            merge_usage_request_metadata_owned(
                build_usage_request_metadata_seed(plan, context),
                build_plan_body_capture_metadata(plan.body.body_bytes_b64.as_deref()),
            ),
            build_runtime_body_size_request_metadata(plan, context),
        ),
    }
}

pub fn build_sync_terminal_usage_payload_seed(
    payload: &GatewaySyncReportRequest,
) -> SyncTerminalUsagePayloadSeed {
    let upstream_is_stream = payload
        .report_context
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|context| context.get(UPSTREAM_IS_STREAM_KEY))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let provider_response_full = if upstream_is_stream
        && payload.body_base64.is_some()
        && !body_json_has_terminal_error(payload.body_json.as_ref())
    {
        decode_body_for_storage(payload.body_base64.as_deref())
            .or_else(|| payload.body_json.as_ref().cloned())
    } else {
        payload
            .body_json
            .as_ref()
            .cloned()
            .or_else(|| decode_body_for_storage(payload.body_base64.as_deref()))
    };
    let has_provider_response = provider_response_full.is_some();
    let client_response = payload.client_body_json.as_ref().cloned();
    let has_client_response = client_response.is_some();
    let provider_response_body_state = Some(UsageBodyCaptureState::from_capture_parts(
        has_provider_response,
        false,
        false,
    ));
    let client_response_body_state = Some(UsageBodyCaptureState::from_capture_parts(
        has_client_response,
        false,
        false,
    ));
    let context = payload.report_context.as_ref().and_then(Value::as_object);
    let provider_response_headers = context_usage_value(context, "provider_response_headers")
        .or_else(|| headers_to_json(&payload.headers));
    let client_response_headers = context_usage_value(context, "client_response_headers")
        .or_else(|| headers_to_json(&payload.headers));
    let standardized_usage = kiro_simulated_cache_standardized_usage_from_context(context);
    SyncTerminalUsagePayloadSeed {
        report_kind: payload.report_kind.clone(),
        status_code: payload.status_code,
        response_time_ms: payload
            .telemetry
            .as_ref()
            .and_then(|value| value.elapsed_ms),
        first_byte_time_ms: payload.telemetry.as_ref().and_then(|value| value.ttfb_ms),
        provider_response_headers,
        client_response_headers,
        provider_response_full,
        provider_response_body_state,
        client_response,
        client_response_body_state,
        standardized_usage,
        capture_metadata: build_payload_body_capture_metadata(
            payload.body_base64.as_deref(),
            None,
            provider_response_body_state,
            client_response_body_state,
        ),
    }
}

fn body_json_has_terminal_error(body_json: Option<&Value>) -> bool {
    body_json
        .and_then(|value| value.get("error"))
        .is_some_and(|error| !error.is_null())
}

pub fn build_stream_terminal_usage_payload_seed(
    payload: &GatewayStreamReportRequest,
) -> StreamTerminalUsagePayloadSeed {
    let context = payload.report_context.as_ref().and_then(Value::as_object);
    let provider_response_headers = context_usage_value(context, "provider_response_headers")
        .or_else(|| headers_to_json(&payload.headers));
    let client_response_headers = headers_to_json(&payload.headers);
    let observed_stream_finish = payload
        .terminal_summary
        .as_ref()
        .map(|summary| summary.observed_finish);
    let terminal_error_message = payload
        .terminal_summary
        .as_ref()
        .and_then(|summary| summary.parser_error.clone())
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty());
    StreamTerminalUsagePayloadSeed {
        report_kind: payload.report_kind.clone(),
        status_code: payload.status_code,
        response_time_ms: payload
            .telemetry
            .as_ref()
            .and_then(|value| value.elapsed_ms),
        first_byte_time_ms: payload.telemetry.as_ref().and_then(|value| value.ttfb_ms),
        provider_response_headers,
        client_response_headers,
        provider_response_full: decode_body_for_storage(payload.provider_body_base64.as_deref()),
        provider_response_body_state: payload.provider_body_state,
        client_response: decode_body_for_storage(payload.client_body_base64.as_deref()),
        client_response_body_state: payload.client_body_state,
        standardized_usage: payload
            .terminal_summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.clone()),
        observed_stream_finish,
        terminal_error_message,
        capture_metadata: build_payload_body_capture_metadata(
            payload.provider_body_base64.as_deref(),
            payload.client_body_base64.as_deref(),
            payload.provider_body_state,
            payload.client_body_state,
        ),
    }
}

pub fn build_sync_terminal_usage_seed(
    context_seed: TerminalUsageContextSeed,
    payload_seed: SyncTerminalUsagePayloadSeed,
) -> TerminalUsageSeed {
    let SyncTerminalUsagePayloadSeed {
        report_kind,
        status_code,
        response_time_ms,
        first_byte_time_ms,
        provider_response_headers,
        client_response_headers,
        provider_response_full,
        provider_response_body_state,
        client_response,
        client_response_body_state,
        standardized_usage,
        capture_metadata,
    } = payload_seed;
    let derived_standardized_usage = provider_response_full
        .as_ref()
        .map(|response| map_usage_from_response(response, context_seed.provider_contract.as_str()));
    let standardized_usage =
        merge_standardized_usage_with_context_cache(standardized_usage, derived_standardized_usage);
    let terminal_state = infer_sync_terminal_state(
        report_kind.as_str(),
        status_code,
        provider_response_full.as_ref(),
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
        model_id: context_seed.model_id,
        global_model_id: context_seed.global_model_id,
        provider_id: context_seed.provider_id,
        provider_endpoint_id: context_seed.provider_endpoint_id,
        provider_api_key_id: context_seed.provider_api_key_id,
        request_type: context_seed.request_type,
        has_format_conversion: context_seed.has_format_conversion,
        is_stream: context_seed.is_stream,
        status_code,
        terminal_error_message: None,
        terminal_failure_category: None,
        response_time_ms,
        first_byte_time_ms,
        request_headers: context_seed.request_headers,
        request_body: context_seed.request_body,
        provider_request_headers: context_seed.provider_request_headers,
        provider_request: context_seed.provider_request,
        body_refs: context_seed.body_refs,
        body_states: UsageBodyStatesSeed {
            request_body_state: context_seed.body_states.request_body_state,
            provider_request_body_state: context_seed.body_states.provider_request_body_state,
            response_body_state: provider_response_body_state,
            client_response_body_state,
        },
        routing: context_seed.routing,
        provider_response_headers,
        provider_response: provider_response_full,
        client_response_headers,
        client_response,
        request_metadata: context_seed.request_metadata,
        audit_payload: capture_metadata,
        standardized_usage,
    }
}

fn merge_standardized_usage_with_context_cache(
    context_usage: Option<StandardizedUsage>,
    derived_usage: Option<StandardizedUsage>,
) -> Option<StandardizedUsage> {
    let Some(context_usage) = context_usage else {
        return derived_usage;
    };

    let mut usage = derived_usage.unwrap_or_default();
    usage.input_tokens = context_usage.input_tokens;
    if context_usage.cache_creation_tokens > 0 {
        usage.cache_creation_tokens = context_usage.cache_creation_tokens;
    }
    if context_usage.cache_read_tokens > 0 {
        usage.cache_read_tokens = context_usage.cache_read_tokens;
    }
    Some(usage)
}

pub fn build_stream_terminal_usage_seed(
    context_seed: TerminalUsageContextSeed,
    payload_seed: StreamTerminalUsagePayloadSeed,
    cancelled: bool,
) -> TerminalUsageSeed {
    let StreamTerminalUsagePayloadSeed {
        report_kind,
        status_code,
        response_time_ms,
        first_byte_time_ms,
        provider_response_headers,
        client_response_headers,
        provider_response_full,
        provider_response_body_state,
        mut client_response,
        mut client_response_body_state,
        standardized_usage,
        observed_stream_finish,
        terminal_error_message,
        capture_metadata,
    } = payload_seed;
    let standardized_usage = standardized_usage.or_else(|| {
        provider_response_full.as_ref().map(|response| {
            map_usage_from_response(response, context_seed.provider_contract.as_str())
        })
    });
    let captured_terminal_state = captured_stream_terminal_state(
        report_kind.as_str(),
        context_seed.client_contract.as_str(),
        context_seed.provider_contract.as_str(),
        provider_response_full.as_ref(),
        provider_response_body_state,
        client_response.as_ref(),
        client_response_body_state,
    );
    let requires_observed_terminal_event = stream_usage_requires_observed_terminal_event(
        report_kind.as_str(),
        context_seed.client_contract.as_str(),
        context_seed.provider_contract.as_str(),
    );
    let empty_required_capture_missing_terminal = stream_empty_required_captures_missing_terminal(
        report_kind.as_str(),
        context_seed.client_contract.as_str(),
        context_seed.provider_contract.as_str(),
        provider_response_full.as_ref(),
        provider_response_body_state,
        client_response.as_ref(),
        client_response_body_state,
    );
    let observed_stream_finish = observed_stream_finish
        .or_else(|| {
            captured_terminal_state.map(|state| state != StreamCapturedTerminalState::Missing)
        })
        .or_else(|| empty_required_capture_missing_terminal.then_some(false));
    let missing_observed_finish = matches!(observed_stream_finish, Some(false))
        && (requires_observed_terminal_event
            || !standardized_usage
                .as_ref()
                .is_some_and(StandardizedUsage::has_token_signal));
    let captured_terminal_failure = matches!(
        captured_terminal_state,
        Some(StreamCapturedTerminalState::Failed)
    );
    let terminal_error_message = terminal_error_message
        .or_else(|| {
            provider_response_full
                .as_ref()
                .and_then(extract_explicit_error_message_from_json)
        })
        .or_else(|| {
            client_response
                .as_ref()
                .and_then(extract_explicit_error_message_from_json)
        })
        .or_else(|| captured_terminal_failure.then(|| STREAM_TERMINAL_ERROR_MESSAGE.to_string()));
    let terminal_failure_category = if terminal_error_message.is_some() {
        Some(STREAM_TERMINAL_ERROR_CATEGORY.to_string())
    } else if missing_observed_finish {
        Some(STREAM_MISSING_TERMINAL_EVENT_CATEGORY.to_string())
    } else {
        None
    };
    let terminal_error_message = terminal_error_message.or_else(|| {
        missing_observed_finish.then(|| STREAM_MISSING_TERMINAL_EVENT_MESSAGE.to_string())
    });
    if client_response.is_none() {
        if let (Some(message), Some(category)) = (
            terminal_error_message.as_deref(),
            terminal_failure_category.as_deref(),
        ) {
            client_response = Some(build_stream_terminal_error_client_response(
                category,
                message,
                status_code,
                provider_response_full.as_ref(),
            ));
            client_response_body_state = Some(UsageBodyCaptureState::Inline);
        }
    }
    let terminal_state = infer_stream_terminal_state(
        report_kind.as_str(),
        status_code,
        cancelled,
        missing_observed_finish,
        terminal_error_message.is_some(),
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
        model_id: context_seed.model_id,
        global_model_id: context_seed.global_model_id,
        provider_id: context_seed.provider_id,
        provider_endpoint_id: context_seed.provider_endpoint_id,
        provider_api_key_id: context_seed.provider_api_key_id,
        request_type: context_seed.request_type,
        has_format_conversion: context_seed.has_format_conversion,
        is_stream: context_seed.is_stream,
        status_code,
        terminal_error_message,
        terminal_failure_category,
        response_time_ms,
        first_byte_time_ms,
        request_headers: context_seed.request_headers,
        request_body: context_seed.request_body,
        provider_request_headers: context_seed.provider_request_headers,
        provider_request: context_seed.provider_request,
        body_refs: context_seed.body_refs,
        body_states: UsageBodyStatesSeed {
            request_body_state: context_seed.body_states.request_body_state,
            provider_request_body_state: context_seed.body_states.provider_request_body_state,
            response_body_state: provider_response_body_state,
            client_response_body_state,
        },
        routing: context_seed.routing,
        provider_response_headers,
        provider_response: provider_response_full,
        client_response_headers,
        client_response,
        request_metadata: context_seed.request_metadata,
        audit_payload: capture_metadata,
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
    } else if !(200..300).contains(&status_code)
        || provider_response
            .and_then(|value| value.get("error"))
            .is_some_and(|value| !value.is_null())
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
    missing_observed_finish: bool,
    terminal_error: bool,
) -> UsageTerminalState {
    if cancelled || status_code == 499 || report_kind.contains("cancel") {
        UsageTerminalState::Cancelled
    } else if !(200..300).contains(&status_code) || missing_observed_finish || terminal_error {
        UsageTerminalState::Failed
    } else {
        UsageTerminalState::Completed
    }
}

fn captured_stream_terminal_state(
    report_kind: &str,
    client_contract: &str,
    provider_contract: &str,
    provider_response: Option<&Value>,
    provider_response_body_state: Option<UsageBodyCaptureState>,
    client_response: Option<&Value>,
    client_response_body_state: Option<UsageBodyCaptureState>,
) -> Option<StreamCapturedTerminalState> {
    let report_kind_requires_terminal_event =
        stream_report_kind_requires_observed_terminal_event(report_kind);
    let provider_contract_requires_terminal_event =
        is_openai_responses_family_format_alias(provider_contract);
    let client_contract_requires_terminal_event =
        is_openai_responses_family_format_alias(client_contract);
    let fallback_requires_terminal_event = report_kind_requires_terminal_event
        && !provider_contract_requires_terminal_event
        && !client_contract_requires_terminal_event;

    combine_stream_capture_terminal_states(
        (provider_contract_requires_terminal_event || fallback_requires_terminal_event)
            .then(|| {
                captured_stream_terminal_state_from_body(
                    provider_response,
                    provider_response_body_state,
                )
            })
            .flatten(),
        (client_contract_requires_terminal_event || fallback_requires_terminal_event)
            .then(|| {
                captured_stream_terminal_state_from_body(
                    client_response,
                    client_response_body_state,
                )
            })
            .flatten(),
    )
}

fn captured_stream_terminal_state_from_body(
    response: Option<&Value>,
    body_state: Option<UsageBodyCaptureState>,
) -> Option<StreamCapturedTerminalState> {
    let state = response.and_then(stream_capture_terminal_state);
    match state {
        Some(StreamCapturedTerminalState::Missing)
            if !stream_body_capture_can_prove_missing_terminal(body_state) =>
        {
            None
        }
        other => other,
    }
}

fn stream_body_capture_can_prove_missing_terminal(
    body_state: Option<UsageBodyCaptureState>,
) -> bool {
    matches!(
        body_state,
        None | Some(UsageBodyCaptureState::Inline) | Some(UsageBodyCaptureState::None)
    )
}

fn combine_stream_capture_terminal_states(
    current: Option<StreamCapturedTerminalState>,
    next: Option<StreamCapturedTerminalState>,
) -> Option<StreamCapturedTerminalState> {
    match (current, next) {
        (Some(StreamCapturedTerminalState::Failed), _)
        | (_, Some(StreamCapturedTerminalState::Failed)) => {
            Some(StreamCapturedTerminalState::Failed)
        }
        (Some(StreamCapturedTerminalState::Missing), _)
        | (_, Some(StreamCapturedTerminalState::Missing)) => {
            Some(StreamCapturedTerminalState::Missing)
        }
        (Some(StreamCapturedTerminalState::Completed), _)
        | (_, Some(StreamCapturedTerminalState::Completed)) => {
            Some(StreamCapturedTerminalState::Completed)
        }
        (None, None) => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn stream_empty_required_captures_missing_terminal(
    report_kind: &str,
    client_contract: &str,
    provider_contract: &str,
    provider_response: Option<&Value>,
    provider_response_body_state: Option<UsageBodyCaptureState>,
    client_response: Option<&Value>,
    client_response_body_state: Option<UsageBodyCaptureState>,
) -> bool {
    let report_kind_requires_terminal_event =
        stream_report_kind_requires_observed_terminal_event(report_kind);
    let provider_contract_requires_terminal_event =
        is_openai_responses_family_format_alias(provider_contract);
    let client_contract_requires_terminal_event =
        is_openai_responses_family_format_alias(client_contract);
    let fallback_requires_terminal_event = report_kind_requires_terminal_event
        && !provider_contract_requires_terminal_event
        && !client_contract_requires_terminal_event;
    let provider_requires_terminal =
        provider_contract_requires_terminal_event || fallback_requires_terminal_event;
    let client_requires_terminal =
        client_contract_requires_terminal_event || fallback_requires_terminal_event;

    let mut has_required_capture = false;
    let mut all_required_captures_empty = true;

    if provider_requires_terminal {
        has_required_capture = true;
        all_required_captures_empty &= provider_response.is_none()
            && provider_response_body_state == Some(UsageBodyCaptureState::None);
    }

    if client_requires_terminal {
        has_required_capture = true;
        all_required_captures_empty &= client_response.is_none()
            && client_response_body_state == Some(UsageBodyCaptureState::None);
    }

    has_required_capture && all_required_captures_empty
}

fn stream_report_kind_requires_observed_terminal_event(report_kind: &str) -> bool {
    let report_kind = report_kind.trim().to_ascii_lowercase();
    report_kind.starts_with("openai_responses_")
        || report_kind.starts_with("openai_compact_")
        || report_kind.starts_with("openai_cli_")
}

fn stream_usage_requires_observed_terminal_event(
    report_kind: &str,
    client_contract: &str,
    provider_contract: &str,
) -> bool {
    stream_report_kind_requires_observed_terminal_event(report_kind)
        || is_openai_responses_family_format_alias(client_contract)
        || is_openai_responses_family_format_alias(provider_contract)
}

fn is_openai_responses_family_format_alias(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase().replace('_', ":");
    aether_ai_formats::is_openai_responses_family_format(normalized.as_str())
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
    let LifecycleUsageRecordInput { seed, options } = input;
    build_lifecycle_usage_record_impl(seed, options)
}

fn build_lifecycle_usage_record_owned(
    input: OwnedLifecycleUsageRecordInput,
) -> Result<UpsertUsageRecord, DataLayerError> {
    let OwnedLifecycleUsageRecordInput { seed, options } = input;
    let LifecycleUsageRecordOptions {
        lifecycle_state,
        status_code,
        response_time_ms,
        first_byte_time_ms,
        response_headers,
        client_response_headers,
        updated_at_unix_secs,
        trusted_request_metadata,
    } = options;
    let (status, billing_status) = lifecycle_status_and_billing(lifecycle_state);
    let LifecycleUsageSeed {
        request_id,
        user_id,
        api_key_id,
        username,
        api_key_name,
        provider_name,
        model,
        target_model,
        provider_id,
        provider_endpoint_id,
        provider_api_key_id,
        request_type,
        api_format,
        api_family,
        endpoint_kind,
        endpoint_api_format,
        provider_api_family,
        provider_endpoint_kind,
        has_format_conversion,
        is_stream,
        routing,
        body_states,
        request_metadata,
        ..
    } = seed;
    let routing = merge_routing_seed_with_metadata_owned(routing, request_metadata.as_ref());
    let body_refs = merge_body_refs_seed_with_metadata_owned(
        UsageBodyRefsSeed::default(),
        request_metadata.as_ref(),
    );
    let request_metadata = if trusted_request_metadata {
        request_metadata
    } else {
        sanitize_usage_request_metadata(request_metadata)
    };

    Ok(UpsertUsageRecord {
        request_id,
        user_id,
        api_key_id,
        username,
        api_key_name,
        provider_name,
        model,
        target_model,
        provider_id,
        provider_endpoint_id,
        provider_api_key_id,
        request_type: Some(request_type),
        api_format,
        api_family,
        endpoint_kind,
        endpoint_api_format,
        provider_api_family,
        provider_endpoint_kind,
        has_format_conversion,
        is_stream: Some(is_stream),
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
        request_body_state: body_states.request_body_state,
        provider_request_headers: None,
        provider_request_body: None,
        provider_request_body_ref: body_refs.provider_request_body_ref,
        provider_request_body_state: body_states.provider_request_body_state,
        response_headers: sanitize_usage_header_capture(response_headers),
        response_body: None,
        response_body_ref: body_refs.response_body_ref,
        response_body_state: Some(UsageBodyCaptureState::None),
        client_response_headers: sanitize_usage_header_capture(client_response_headers),
        client_response_body: None,
        client_response_body_ref: body_refs.client_response_body_ref,
        client_response_body_state: Some(UsageBodyCaptureState::None),
        candidate_id: routing.candidate_id,
        candidate_index: routing.candidate_index,
        key_name: routing.key_name,
        planner_kind: routing.planner_kind,
        route_family: routing.route_family,
        route_kind: routing.route_kind,
        execution_path: routing.execution_path,
        local_execution_runtime_miss_reason: routing.local_execution_runtime_miss_reason,
        request_metadata,
        finalized_at_unix_secs: None,
        created_at_unix_ms: Some(updated_at_unix_secs),
        updated_at_unix_secs,
    })
}

fn build_lifecycle_usage_record_impl(
    seed: &LifecycleUsageSeed,
    options: LifecycleUsageRecordOptions,
) -> Result<UpsertUsageRecord, DataLayerError> {
    let LifecycleUsageRecordOptions {
        lifecycle_state,
        status_code,
        response_time_ms,
        first_byte_time_ms,
        response_headers,
        client_response_headers,
        updated_at_unix_secs,
        trusted_request_metadata,
    } = options;
    let (status, billing_status) = lifecycle_status_and_billing(lifecycle_state);
    let routing = merge_routing_seed_with_metadata(&seed.routing, seed.request_metadata.as_ref());
    let body_refs = merge_body_refs_seed_with_metadata(
        &UsageBodyRefsSeed::default(),
        seed.request_metadata.as_ref(),
    );
    let request_metadata = if trusted_request_metadata {
        seed.request_metadata.clone()
    } else {
        sanitize_usage_request_metadata_ref(seed.request_metadata.as_ref())
    };

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
        request_body_state: seed.body_states.request_body_state,
        provider_request_headers: None,
        provider_request_body: None,
        provider_request_body_ref: body_refs.provider_request_body_ref,
        provider_request_body_state: seed.body_states.provider_request_body_state,
        response_headers: sanitize_usage_header_capture(response_headers),
        response_body: None,
        response_body_ref: body_refs.response_body_ref,
        response_body_state: Some(UsageBodyCaptureState::None),
        client_response_headers: sanitize_usage_header_capture(client_response_headers),
        client_response_body: None,
        client_response_body_ref: body_refs.client_response_body_ref,
        client_response_body_state: Some(UsageBodyCaptureState::None),
        candidate_id: routing.candidate_id,
        candidate_index: routing.candidate_index,
        key_name: routing.key_name,
        planner_kind: routing.planner_kind,
        route_family: routing.route_family,
        route_kind: routing.route_kind,
        execution_path: routing.execution_path,
        local_execution_runtime_miss_reason: routing.local_execution_runtime_miss_reason,
        request_metadata,
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
    let request_capture = build_runtime_request_capture_seed(plan, context);
    let api_format = context_string(context, "client_api_format")
        .or_else(|| non_empty_str(Some(plan.client_api_format.as_str())));
    let endpoint_api_format = context_string(context, "provider_api_format")
        .or_else(|| non_empty_str(Some(plan.provider_api_format.as_str())));
    let model = context_string(context, "model")
        .or_else(|| non_empty_str(plan.model_name.as_deref()))
        .unwrap_or_else(|| "unknown".to_string());
    let provider_name = context_string(context, "provider_name")
        .or_else(|| non_empty_str(plan.provider_name.as_deref()))
        .unwrap_or_else(|| "unknown".to_string());
    let request_type = Some(infer_request_type_from_contracts(
        api_format.as_deref(),
        endpoint_api_format.as_deref(),
    ));
    let api_family = api_format
        .as_deref()
        .and_then(infer_api_family)
        .map(ToOwned::to_owned);
    let endpoint_kind = api_format
        .as_deref()
        .and_then(infer_endpoint_kind)
        .map(ToOwned::to_owned);
    let provider_api_family = endpoint_api_format
        .as_deref()
        .and_then(infer_api_family)
        .map(ToOwned::to_owned);
    let provider_endpoint_kind = endpoint_api_format
        .as_deref()
        .and_then(infer_endpoint_kind)
        .map(ToOwned::to_owned);
    let request_metadata = build_runtime_request_metadata_seed_from_parts(
        plan,
        context,
        request_capture.request_body.is_some(),
        request_capture.request_body_ref.as_deref(),
        request_capture.provider_request.is_some(),
        request_capture.provider_request_body_ref.as_deref(),
        plan.body.body_bytes_b64.as_deref(),
    );
    sanitize_usage_event_data(UsageEventData {
        user_id: context_string(context, "user_id"),
        api_key_id: context_string(context, "api_key_id"),
        username: context_string(context, "username"),
        api_key_name: context_string(context, "api_key_name"),
        provider_name,
        model,
        target_model: context_string(context, "mapped_model"),
        model_id: context_string(context, "model_id"),
        global_model_id: context_string(context, "global_model_id"),
        provider_id: context_string(context, "provider_id")
            .or_else(|| non_empty_str(Some(plan.provider_id.as_str()))),
        provider_endpoint_id: context_string(context, "endpoint_id")
            .or_else(|| non_empty_str(Some(plan.endpoint_id.as_str()))),
        provider_api_key_id: context_string(context, "key_id")
            .or_else(|| non_empty_str(Some(plan.key_id.as_str()))),
        request_type,
        api_format,
        api_family,
        endpoint_kind,
        endpoint_api_format,
        provider_api_family,
        provider_endpoint_kind,
        has_format_conversion: context_bool(context, "needs_conversion"),
        is_stream: Some(plan.stream),
        request_headers: context_usage_value(context, "original_headers"),
        request_body: request_capture.request_body,
        request_body_ref: request_capture.request_body_ref,
        request_body_state: request_capture.body_states.request_body_state,
        provider_request_headers: context_usage_value(context, "provider_request_headers")
            .or_else(|| headers_to_json(&plan.headers)),
        provider_request_body: request_capture.provider_request,
        provider_request_body_ref: request_capture.provider_request_body_ref,
        provider_request_body_state: request_capture.body_states.provider_request_body_state,
        response_body_ref: context_string(context, "response_body_ref"),
        response_body_state: request_capture.body_states.response_body_state,
        client_response_body_ref: context_string(context, "client_response_body_ref"),
        client_response_body_state: request_capture.body_states.client_response_body_state,
        candidate_id: routing.candidate_id,
        candidate_index: routing.candidate_index,
        key_name: routing.key_name,
        planner_kind: routing.planner_kind,
        route_family: routing.route_family,
        route_kind: routing.route_kind,
        execution_path: routing.execution_path,
        local_execution_runtime_miss_reason: routing.local_execution_runtime_miss_reason,
        request_metadata,
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
            .or_else(|| non_empty_str(plan.candidate_id.as_deref())),
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

fn merge_routing_seed_with_metadata_owned(
    routing: UsageRoutingSeed,
    metadata: Option<&Value>,
) -> UsageRoutingSeed {
    UsageRoutingSeed {
        candidate_id: routing
            .candidate_id
            .or_else(|| routing_string_from_metadata(metadata, "candidate_id")),
        candidate_index: routing
            .candidate_index
            .or_else(|| routing_u64_from_metadata(metadata, "candidate_index")),
        key_name: routing
            .key_name
            .or_else(|| routing_string_from_metadata(metadata, "key_name")),
        planner_kind: routing
            .planner_kind
            .or_else(|| routing_string_from_metadata(metadata, "planner_kind")),
        route_family: routing
            .route_family
            .or_else(|| routing_string_from_metadata(metadata, "route_family")),
        route_kind: routing
            .route_kind
            .or_else(|| routing_string_from_metadata(metadata, "route_kind")),
        execution_path: routing
            .execution_path
            .or_else(|| routing_string_from_metadata(metadata, "execution_path")),
        local_execution_runtime_miss_reason: routing.local_execution_runtime_miss_reason.or_else(
            || routing_string_from_metadata(metadata, "local_execution_runtime_miss_reason"),
        ),
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

fn kiro_simulated_cache_standardized_usage_from_context(
    context: Option<&Map<String, Value>>,
) -> Option<StandardizedUsage> {
    let enabled = context_bool(context, "kiro_simulated_cache_enabled").unwrap_or(false);
    if !enabled {
        return None;
    }

    let input_tokens = context_u64(context, "input_tokens")?;
    let cache_creation_tokens = context_u64(context, "cache_creation_input_tokens").unwrap_or(0);
    let cache_read_tokens = context_u64(context, "cache_read_input_tokens").unwrap_or(0);
    if cache_creation_tokens == 0 && cache_read_tokens == 0 {
        return None;
    }

    let billed_input_tokens = input_tokens
        .saturating_sub(cache_creation_tokens)
        .saturating_sub(cache_read_tokens);
    let mut usage = StandardizedUsage::new();
    usage.input_tokens = billed_input_tokens as i64;
    usage.cache_creation_tokens = cache_creation_tokens as i64;
    usage.cache_read_tokens = cache_read_tokens as i64;
    Some(usage)
}

fn context_has_inline_body(context: Option<&Map<String, Value>>, key: &str) -> bool {
    matches!(context_value_ref(context, key), Some(value) if !value.is_null())
}

fn plan_has_inline_json_body_for_usage(plan: &ExecutionPlan) -> bool {
    plan.body.body_ref.is_none()
        && plan.body.body_bytes_b64.is_none()
        && plan.body.json_body.is_some()
}

fn build_runtime_request_capture_seed(
    plan: &ExecutionPlan,
    context: Option<&Map<String, Value>>,
) -> RuntimeRequestCaptureSeed {
    let request_body = context_body_value(context, "original_request_body");
    let request_body_ref = context_string(context, "request_body_ref");
    let provider_request = context_body_value(context, "provider_request_body")
        .or_else(|| plan_json_body_capture_for_usage(plan));
    let provider_request_body_ref = context_string(context, "provider_request_body_ref")
        .or_else(|| non_empty_str(plan.body.body_ref.as_deref()));
    let body_states = build_runtime_body_states_seed_from_parts(
        request_body.is_some(),
        request_body_ref.as_deref(),
        provider_request.is_some(),
        provider_request_body_ref.as_deref(),
        plan.body.body_bytes_b64.is_some(),
    );

    RuntimeRequestCaptureSeed {
        request_body,
        request_body_ref,
        provider_request,
        provider_request_body_ref,
        body_states,
    }
}

fn build_runtime_request_metadata_seed(
    plan: &ExecutionPlan,
    context: Option<&Map<String, Value>>,
) -> Option<Value> {
    let request_body_ref = context_string(context, "request_body_ref");
    let provider_request_body_ref = context_string(context, "provider_request_body_ref")
        .or_else(|| non_empty_str(plan.body.body_ref.as_deref()));
    let request_has_inline_body = context_has_inline_body(context, "original_request_body");
    let provider_request_has_inline_body =
        context_has_inline_body(context, "provider_request_body")
            || plan_has_inline_json_body_for_usage(plan);
    let mut metadata = build_runtime_request_metadata_seed_from_parts(
        plan,
        context,
        request_has_inline_body,
        request_body_ref.as_deref(),
        provider_request_has_inline_body,
        provider_request_body_ref.as_deref(),
        plan.body.body_bytes_b64.as_deref(),
    );
    if let Some(proxy) = plan.proxy.as_ref() {
        if let Some(node_id) = proxy
            .node_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            let mode = proxy.mode.as_deref().unwrap_or("").trim();
            let mut proxy_obj = serde_json::Map::new();
            proxy_obj.insert("node_id".to_string(), Value::String(node_id.to_string()));
            if !mode.is_empty() {
                proxy_obj.insert("mode".to_string(), Value::String(mode.to_string()));
            }
            let obj = metadata.get_or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(map) = obj {
                map.insert("proxy".to_string(), Value::Object(proxy_obj));
            }
        }
    }
    metadata
}

fn build_runtime_request_metadata_seed_from_parts(
    plan: &ExecutionPlan,
    context: Option<&Map<String, Value>>,
    request_has_inline_body: bool,
    request_body_ref: Option<&str>,
    provider_request_has_inline_body: bool,
    provider_request_body_ref: Option<&str>,
    provider_request_body_base64: Option<&str>,
) -> Option<Value> {
    let mut metadata = Map::new();
    if let Some(trace_id) = context_string(context, "trace_id") {
        metadata.insert("trace_id".to_string(), Value::String(trace_id));
    }
    if let Some(client_ip) = context_string(context, "client_ip") {
        metadata.insert("client_ip".to_string(), Value::String(client_ip));
    }
    if let Some(user_agent) = context_string(context, "user_agent") {
        metadata.insert("user_agent".to_string(), Value::String(user_agent));
    }
    if let Some(client_requested_stream) = context_bool(context, "client_requested_stream") {
        metadata.insert(
            "client_requested_stream".to_string(),
            Value::Bool(client_requested_stream),
        );
    }
    if let Some(upstream_is_stream) = context_bool(context, UPSTREAM_IS_STREAM_KEY) {
        metadata.insert(
            UPSTREAM_IS_STREAM_KEY.to_string(),
            Value::Bool(upstream_is_stream),
        );
    }
    if let Some(api_key_is_standalone) = context_bool(context, "api_key_is_standalone") {
        metadata.insert(
            "api_key_is_standalone".to_string(),
            Value::Bool(api_key_is_standalone),
        );
    }
    let provider_source_bytes = provider_request_body_base64.and_then(decoded_base64_len_hint);
    append_runtime_body_capture_metadata(
        &mut metadata,
        RuntimeBodyCaptureMetadataInput {
            request_has_inline_body,
            request_body_ref,
            provider_request_has_inline_body,
            provider_request_body_ref,
            provider_request_source_bytes: provider_source_bytes,
            provider_request_unavailable: provider_request_body_base64.is_some(),
            provider_request_unavailable_reason: provider_request_body_base64
                .as_ref()
                .map(|_| "body_bytes_base64_only"),
        },
    );
    crate::body_capture::append_plan_body_capture_metadata(
        &mut metadata,
        provider_request_body_base64,
    );
    if let Some(body_size) = build_runtime_body_size_metadata(plan, context) {
        metadata.insert("body_size".to_string(), body_size);
    }

    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

fn build_runtime_body_size_metadata(
    plan: &ExecutionPlan,
    context: Option<&Map<String, Value>>,
) -> Option<Value> {
    let client_body_bytes =
        context_value_ref(context, "original_request_body").and_then(captured_body_size_bytes);
    let provider_body_bytes = plan_body_size_bytes(plan);
    if client_body_bytes.is_none() && provider_body_bytes.is_none() {
        return None;
    }

    let mut metadata = Map::new();
    if let Some(bytes) = client_body_bytes {
        metadata.insert(
            "client_request_body".to_string(),
            Value::String(format_data_size(bytes)),
        );
    }
    if let Some(bytes) = provider_body_bytes {
        metadata.insert(
            "provider_request_body".to_string(),
            Value::String(format_data_size(bytes)),
        );
    }
    if let (Some(client_bytes), Some(provider_bytes)) = (client_body_bytes, provider_body_bytes) {
        if let Some(ratio) = format_size_ratio(provider_bytes, client_bytes) {
            metadata.insert("provider_over_client".to_string(), Value::String(ratio));
        }
    }
    metadata.insert(
        "basis".to_string(),
        Value::String("serialized gateway request bodies after normalization".to_string()),
    );

    Some(Value::Object(metadata))
}

fn build_runtime_body_size_request_metadata(
    plan: &ExecutionPlan,
    context: Option<&Map<String, Value>>,
) -> Option<Value> {
    let body_size = build_runtime_body_size_metadata(plan, context)?;
    let mut metadata = Map::new();
    metadata.insert("body_size".to_string(), body_size);
    Some(Value::Object(metadata))
}

fn captured_body_size_bytes(value: &Value) -> Option<u64> {
    if value.is_null() {
        return None;
    }
    if let Some(body_base64) = value
        .as_object()
        .and_then(|object| object.get("body_bytes_b64"))
        .and_then(Value::as_str)
    {
        return decoded_base64_len_hint(body_base64);
    }
    json_serialized_len(value)
}

fn plan_body_size_bytes(plan: &ExecutionPlan) -> Option<u64> {
    if let Some(body_base64) = plan.body.body_bytes_b64.as_deref() {
        return decoded_base64_len_hint(body_base64);
    }
    plan.body.json_body.as_ref().and_then(json_serialized_len)
}

fn json_serialized_len(value: &Value) -> Option<u64> {
    serde_json::to_vec(value)
        .ok()
        .and_then(|bytes| u64::try_from(bytes.len()).ok())
}

fn format_data_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    let bytes = bytes as f64;
    let (value, unit) = if bytes >= GIB {
        (bytes / GIB, "GB")
    } else if bytes >= MIB {
        (bytes / MIB, "MB")
    } else {
        (bytes / KIB, "KB")
    };
    format!("{} {}", format_compact_decimal(value), unit)
}

fn format_size_ratio(numerator: u64, denominator: u64) -> Option<String> {
    if denominator == 0 {
        return None;
    }
    Some(format!(
        "{}x",
        format_compact_decimal(numerator as f64 / denominator as f64)
    ))
}

fn format_compact_decimal(value: f64) -> String {
    let digits = if value >= 100.0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    };
    let formatted = format!("{value:.digits$}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
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

fn build_runtime_body_states_seed(
    plan: &ExecutionPlan,
    context: Option<&Map<String, Value>>,
) -> UsageBodyStatesSeed {
    let request_body_ref = context_string(context, "request_body_ref");
    let provider_request_body_ref = context_string(context, "provider_request_body_ref")
        .or_else(|| non_empty_str(plan.body.body_ref.as_deref()));
    build_runtime_body_states_seed_from_parts(
        context_has_inline_body(context, "original_request_body"),
        request_body_ref.as_deref(),
        context_has_inline_body(context, "provider_request_body")
            || plan_has_inline_json_body_for_usage(plan),
        provider_request_body_ref.as_deref(),
        plan.body.body_bytes_b64.is_some(),
    )
}

fn build_runtime_body_states_seed_from_parts(
    request_has_inline_body: bool,
    request_body_ref: Option<&str>,
    provider_request_has_inline_body: bool,
    provider_request_body_ref: Option<&str>,
    provider_request_unavailable: bool,
) -> UsageBodyStatesSeed {
    let states = build_runtime_body_capture_states(
        request_has_inline_body,
        request_body_ref,
        provider_request_has_inline_body,
        provider_request_body_ref,
        provider_request_unavailable,
    );

    UsageBodyStatesSeed {
        request_body_state: Some(states.request),
        provider_request_body_state: Some(states.provider_request),
        response_body_state: Some(UsageBodyCaptureState::None),
        client_response_body_state: Some(UsageBodyCaptureState::None),
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

fn merge_body_refs_seed_with_metadata_owned(
    seed: UsageBodyRefsSeed,
    metadata: Option<&Value>,
) -> UsageBodyRefsSeed {
    let object = metadata.and_then(Value::as_object);
    UsageBodyRefsSeed {
        request_body_ref: seed
            .request_body_ref
            .or_else(|| context_string(object, "request_body_ref")),
        provider_request_body_ref: seed
            .provider_request_body_ref
            .or_else(|| context_string(object, "provider_request_body_ref")),
        response_body_ref: seed
            .response_body_ref
            .or_else(|| context_string(object, "response_body_ref")),
        client_response_body_ref: seed
            .client_response_body_ref
            .or_else(|| context_string(object, "client_response_body_ref")),
    }
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
    value.cloned().map(mask_sensitive_body_fields)
}

fn sanitize_usage_event_capture_fields(mut data: UsageEventData) -> UsageEventData {
    data.request_headers = sanitize_usage_header_capture(data.request_headers);
    data.request_body_ref = sanitize_usage_body_ref(data.request_body_ref);
    data.provider_request_headers = sanitize_usage_header_capture(data.provider_request_headers);
    data.provider_request_body_ref = sanitize_usage_body_ref(data.provider_request_body_ref);
    data.response_headers = sanitize_usage_header_capture(data.response_headers);
    data.response_body_ref = sanitize_usage_body_ref(data.response_body_ref);
    data.client_response_headers = sanitize_usage_header_capture(data.client_response_headers);
    data.client_response_body_ref = sanitize_usage_body_ref(data.client_response_body_ref);
    data
}

fn sanitize_usage_event_capture_fields_trusted(mut data: UsageEventData) -> UsageEventData {
    data.request_headers = capture_usage_header_capture(data.request_headers);
    data.provider_request_headers = capture_usage_header_capture(data.provider_request_headers);
    data.response_headers = capture_usage_header_capture(data.response_headers);
    data.client_response_headers = capture_usage_header_capture(data.client_response_headers);
    data
}

fn sanitize_usage_event_data(mut data: UsageEventData) -> UsageEventData {
    data = sanitize_usage_event_capture_fields(data);
    data.request_metadata = sanitize_usage_request_metadata(data.request_metadata);
    data
}

fn capture_usage_header_capture(value: Option<Value>) -> Option<Value> {
    value.map(capture_usage_storage_value)
}

fn sanitize_usage_header_capture(value: Option<Value>) -> Option<Value> {
    mask_sensitive_headers_in_json_value(value).map(capture_usage_storage_value)
}

fn trim_owned_non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() == value.len() {
        return Some(value);
    }
    Some(trimmed.to_string())
}

fn sanitize_usage_body_ref(value: Option<String>) -> Option<String> {
    value.and_then(trim_owned_non_empty_string)
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

fn non_empty_str(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn infer_request_type(api_format: Option<&str>) -> String {
    match infer_endpoint_kind(api_format.unwrap_or_default()) {
        Some("video") => "video".to_string(),
        Some("image") => "image".to_string(),
        _ => "chat".to_string(),
    }
}

fn infer_request_type_from_contracts(
    client_api_format: Option<&str>,
    provider_api_format: Option<&str>,
) -> String {
    if matches!(
        infer_endpoint_kind(provider_api_format.unwrap_or_default()),
        Some("image")
    ) {
        return "image".to_string();
    }
    infer_request_type(client_api_format)
}

fn infer_api_family(api_format: &str) -> Option<&str> {
    api_format.split_once(':').map(|(family, _)| family)
}

fn infer_endpoint_kind(api_format: &str) -> Option<&str> {
    api_format.split_once(':').map(|(_, kind)| kind)
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
    let total_tokens = standardized_usage_total_tokens(usage);
    if total_tokens > 0 {
        data.total_tokens = Some(total_tokens);
    }
    apply_standardized_usage_dimensions_seed(usage, data);
}

fn apply_standardized_usage_dimensions_seed(usage: &StandardizedUsage, data: &mut UsageEventData) {
    if usage.dimensions.is_empty() && usage.request_count <= 0 {
        return;
    }

    let mut dimensions = usage
        .dimensions
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<String, Value>>();
    if usage.request_count > 0 {
        dimensions
            .entry("request_count".to_string())
            .or_insert_with(|| json!(usage.request_count));
    }
    if dimensions.is_empty() {
        return;
    }

    let mut metadata = match data.request_metadata.take() {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    let mut existing_dimensions = match metadata.remove("dimensions") {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    for (key, value) in dimensions {
        existing_dimensions.insert(key, value);
    }
    metadata.insert("dimensions".to_string(), Value::Object(existing_dimensions));
    data.request_metadata = Some(Value::Object(metadata));
}

fn standardized_usage_total_tokens(usage: &StandardizedUsage) -> u64 {
    if let Some(total_tokens) = standardized_usage_explicit_total_tokens(usage) {
        return total_tokens;
    }

    positive_usage_component(usage.input_tokens)
        .saturating_add(positive_usage_component(usage.output_tokens))
        .saturating_add(positive_usage_component(usage.reasoning_tokens))
}

fn standardized_usage_explicit_total_tokens(usage: &StandardizedUsage) -> Option<u64> {
    usage
        .dimensions
        .get("total_tokens")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|v| u64::try_from(v).ok()))
        })
        .filter(|value| *value > 0)
}

fn positive_usage_component(value: i64) -> u64 {
    value.max(0) as u64
}

fn headers_to_json(headers: &BTreeMap<String, String>) -> Option<Value> {
    if headers.is_empty() {
        return None;
    }
    Some(Value::Object(Map::from_iter(headers.iter().map(
        |(key, value)| (key.clone(), Value::String(mask_header_value(key, value))),
    ))))
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
    let trimmed = name.trim();
    DEFAULT_SENSITIVE_HEADERS
        .iter()
        .any(|candidate| trimmed.eq_ignore_ascii_case(candidate))
}

/// 对单个 header value 进行脱敏：保留前 4 + 后 4 字符，中间替换为 `****`。
/// 长度小于等于 8 时整体替换为 `****`。
fn mask_header_value(name: &str, value: &str) -> String {
    if !is_sensitive_header(name) {
        return value.to_string();
    }
    mask_sensitive_header_value(value)
}

fn mask_sensitive_header_value(value: &str) -> String {
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
    let mut value = value?;
    let Value::Object(map) = &mut value else {
        return Some(value);
    };
    for (key, val) in map.iter_mut() {
        if !is_sensitive_header(key) {
            continue;
        }
        match val {
            Value::String(text) => {
                *text = mask_sensitive_header_value(text);
            }
            Value::Null => {}
            other => {
                *other = Value::String(mask_sensitive_header_value(&other.to_string()));
            }
        }
    }
    Some(value)
}

fn mask_sensitive_body_fields(mut value: Value) -> Value {
    mask_sensitive_body_fields_in_place(&mut value);
    value
}

fn mask_sensitive_body_fields_in_place(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                if is_sensitive_body_key(key) {
                    let replacement = if value.is_null() {
                        Value::Null
                    } else if let Some(text) = value.as_str() {
                        Value::String(mask_sensitive_header_value(text))
                    } else {
                        Value::String(mask_sensitive_header_value(&value.to_string()))
                    };
                    *value = replacement;
                } else {
                    mask_sensitive_body_fields_in_place(value);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                mask_sensitive_body_fields_in_place(item);
            }
        }
        _ => {}
    }
}

fn is_sensitive_body_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    normalized.contains("token")
        || normalized.contains("apikey")
        || normalized.contains("password")
        || normalized.contains("authorization")
        || normalized.contains("secret")
        || normalized == "cookie"
}

fn resolve_error_category(
    status_code: u16,
    event_type: UsageEventType,
    is_stream: bool,
    terminal_failure_category: Option<&str>,
) -> Option<String> {
    match event_type {
        UsageEventType::Cancelled => Some("cancelled".to_string()),
        UsageEventType::Failed if status_code >= 500 => Some("server_error".to_string()),
        UsageEventType::Failed if status_code >= 400 => Some("client_error".to_string()),
        UsageEventType::Failed if status_code >= 300 => Some("redirect".to_string()),
        UsageEventType::Failed if (200..300).contains(&status_code) => terminal_failure_category
            .map(ToOwned::to_owned)
            .or_else(|| is_stream.then(|| "stream_terminal_error".to_string()))
            .or_else(|| Some("non_success_status".to_string())),
        UsageEventType::Failed => Some("non_success_status".to_string()),
        _ => None,
    }
}

fn build_stream_terminal_error_client_response(
    category: &str,
    message: &str,
    status_code: u16,
    provider_response: Option<&Value>,
) -> Value {
    let mut error = provider_response
        .and_then(extract_error_object_from_json)
        .unwrap_or_default();
    error
        .entry("type".to_string())
        .or_insert_with(|| Value::String(category.to_string()));
    error
        .entry("message".to_string())
        .or_insert_with(|| Value::String(message.to_string()));
    error
        .entry("upstream_status".to_string())
        .or_insert_with(|| Value::from(status_code));

    json!({ "error": Value::Object(error) })
}

fn extract_error_object_from_json(value: &Value) -> Option<Map<String, Value>> {
    value
        .get("error")
        .and_then(value_to_error_object)
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("error"))
                .and_then(value_to_error_object)
        })
        .or_else(|| {
            value
                .get("chunks")
                .and_then(Value::as_array)
                .and_then(|chunks| chunks.iter().find_map(extract_error_object_from_json))
        })
}

fn value_to_error_object(value: &Value) -> Option<Map<String, Value>> {
    match value {
        Value::Object(object) => Some(object.clone()),
        Value::String(message) if !message.trim().is_empty() => Some(Map::from_iter([(
            "message".to_string(),
            Value::String(message.trim().to_string()),
        )])),
        _ => None,
    }
}

fn resolve_error_message(
    status_code: u16,
    body_json: Option<&Value>,
    body_base64: Option<&str>,
) -> Option<String> {
    let decoded_body = body_json
        .is_none()
        .then(|| decode_body_for_storage(body_base64))
        .flatten();
    let error_body = body_json.or(decoded_body.as_ref());
    let explicit_error_message = error_body.and_then(extract_explicit_error_message_from_json);
    if explicit_error_message.is_some() {
        return explicit_error_message;
    }
    if (200..300).contains(&status_code) {
        return None;
    }

    error_body.and_then(extract_generic_error_message_from_json)
}

fn extract_explicit_error_message_from_json(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("error"))
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("incomplete_details"))
                .and_then(|details| details.get("reason"))
                .and_then(Value::as_str)
                .map(|reason| format!("Response incomplete: {reason}"))
        })
        .or_else(|| extract_stream_error_message_from_chunks(value))
}

fn extract_stream_error_message_from_chunks(value: &Value) -> Option<String> {
    value
        .get("chunks")
        .and_then(Value::as_array)?
        .iter()
        .find_map(extract_explicit_error_message_from_json)
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
    if let Some(error_body) =
        aether_ai_formats::api::extract_provider_private_stream_error_body(None, &bytes)
    {
        return Some(error_body);
    }
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

fn flush_sse_payload<F>(payload: &mut String, has_payload: &mut bool, on_payload: &mut F)
where
    F: FnMut(&str),
{
    if !*has_payload {
        return;
    }
    on_payload(payload);
    payload.clear();
    *has_payload = false;
}

fn for_each_sse_payload<F>(text: &str, mut on_payload: F)
where
    F: FnMut(&str),
{
    let mut payload = String::new();
    let mut has_payload = false;
    let bytes = text.as_bytes();
    let mut line_start = 0usize;
    let mut cursor = 0usize;

    while cursor <= bytes.len() {
        if cursor < bytes.len() && bytes[cursor] != b'\n' && bytes[cursor] != b'\r' {
            cursor += 1;
            continue;
        }

        let line = text[line_start..cursor].trim();
        if line.is_empty() {
            flush_sse_payload(&mut payload, &mut has_payload, &mut on_payload);
        } else if let Some(data) = line.strip_prefix("data:").map(str::trim) {
            if !data.is_empty() {
                if has_payload {
                    payload.push('\n');
                }
                payload.push_str(data);
                has_payload = true;
            }
        }

        if cursor == bytes.len() {
            break;
        }
        if bytes[cursor] == b'\r' && bytes.get(cursor + 1) == Some(&b'\n') {
            cursor += 2;
        } else {
            cursor += 1;
        }
        line_start = cursor;
    }
    flush_sse_payload(&mut payload, &mut has_payload, &mut on_payload);
}

fn parse_sse_body_for_storage(text: &str) -> Option<Value> {
    let mut chunks = Vec::new();
    let mut total_chunks = 0_u64;
    let mut saw_done = false;
    for_each_sse_payload(text, |payload| {
        if payload == "[DONE]" {
            saw_done = true;
            return;
        }
        total_chunks += 1;
        if let Ok(json_body) = serde_json::from_str::<Value>(payload) {
            chunks.push(json_body);
        }
    });
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
    for_each_sse_payload(text, |payload| {
        if payload == "[DONE]" {
            return;
        }
        if let Ok(json_body) = serde_json::from_str::<Value>(payload) {
            if let Some(tokens) = extract_token_counts_from_json(&json_body) {
                last_seen = Some(tokens);
            }
        }
    });
    last_seen
}

fn extract_token_counts_from_value(value: &Value) -> Option<(u64, u64, u64)> {
    match value {
        Value::String(text) => extract_token_counts_from_sse_text(text),
        _ => extract_token_counts_from_json(value),
    }
}

fn apply_completed_image_usage_estimate(data: &mut UsageEventData) {
    if !usage_event_data_is_image(data) {
        return;
    }
    apply_completed_image_dimensions(data);
    if data
        .response_body
        .as_ref()
        .and_then(extract_token_counts_from_value)
        .is_some()
    {
        return;
    }
    let request_usage = data
        .provider_request_body
        .as_ref()
        .or(data.request_body.as_ref())
        .and_then(estimate_request_usage);

    if positive_tokens(data.input_tokens) == 0 {
        if let Some(usage) = request_usage.as_ref() {
            data.input_tokens = Some(usage.input_tokens);
        }
    }
    apply_request_cache_usage_estimate(data, request_usage.as_ref());
    if positive_tokens(data.total_tokens) == 0 {
        let total_tokens =
            positive_tokens(data.input_tokens).saturating_add(positive_tokens(data.output_tokens));
        if total_tokens > 0 {
            data.total_tokens = Some(total_tokens);
        }
    }
}

fn apply_completed_image_dimensions(data: &mut UsageEventData) {
    let image_count = usage_dimension_i64(data.request_metadata.as_ref(), "image_count")
        .or_else(|| image_response_count(data.response_body.as_ref()))
        .or_else(|| image_request_count(data.provider_request_body.as_ref()))
        .or_else(|| image_request_count(data.request_body.as_ref()));

    if let Some(image_count) = image_count.filter(|value| *value > 0) {
        set_usage_dimension_if_absent(data, "image_count", json!(image_count));
    }

    for (dimension, request_key) in [
        ("image_size", "size"),
        ("image_quality", "quality"),
        ("image_output_format", "output_format"),
    ] {
        if usage_dimension_string(data.request_metadata.as_ref(), dimension).is_some() {
            continue;
        }
        if let Some(value) = image_request_string(data.provider_request_body.as_ref(), request_key)
            .or_else(|| image_request_string(data.request_body.as_ref(), request_key))
        {
            set_usage_dimension_if_absent(data, dimension, json!(value));
        }
    }
}

fn usage_dimension_i64(metadata: Option<&Value>, key: &str) -> Option<i64> {
    metadata
        .and_then(Value::as_object)
        .and_then(|object| object.get("dimensions"))
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        })
}

fn usage_dimension_string(metadata: Option<&Value>, key: &str) -> Option<String> {
    metadata
        .and_then(Value::as_object)
        .and_then(|object| object.get("dimensions"))
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn set_usage_dimension_if_absent(data: &mut UsageEventData, key: &str, value: Value) {
    let mut metadata = match data.request_metadata.take() {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    let mut dimensions = match metadata.remove("dimensions") {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    dimensions.entry(key.to_string()).or_insert(value);
    metadata.insert("dimensions".to_string(), Value::Object(dimensions));
    data.request_metadata = Some(Value::Object(metadata));
}

fn image_response_count(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value
        .get("data")
        .and_then(Value::as_array)
        .map(|items| items.len() as i64)
        .filter(|count| *count > 0)
        .or_else(|| image_result_count(value.get("result")))
}

fn image_result_count(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Array(items) => Some(items.len() as i64).filter(|count| *count > 0),
        Value::Object(object) if !object.is_empty() => Some(1),
        Value::String(text) if !text.trim().is_empty() => Some(1),
        _ => None,
    }
}

fn image_request_count(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(|value| value.get("n"))
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        })
        .filter(|count| *count > 0)
}

fn image_request_string(value: Option<&Value>, key: &str) -> Option<String> {
    value
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn usage_event_data_is_image(data: &UsageEventData) -> bool {
    data.request_type
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("image"))
        || data
            .endpoint_kind
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("image"))
        || data
            .provider_endpoint_kind
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("image"))
        || data
            .endpoint_api_format
            .as_deref()
            .and_then(infer_endpoint_kind)
            .is_some_and(|value| value.eq_ignore_ascii_case("image"))
        || data
            .api_format
            .as_deref()
            .and_then(infer_endpoint_kind)
            .is_some_and(|value| value.eq_ignore_ascii_case("image"))
}

fn apply_request_cache_usage_estimate(
    data: &mut UsageEventData,
    request_usage: Option<&EstimatedRequestUsage>,
) {
    let Some(request_usage) = request_usage else {
        return;
    };
    if positive_tokens(data.cache_read_input_tokens) == 0 && request_usage.cache_read_tokens > 0 {
        data.cache_read_input_tokens = Some(request_usage.cache_read_tokens);
    }
    if positive_tokens(data.cache_creation_input_tokens) == 0
        && request_usage.cache_creation_tokens > 0
    {
        data.cache_creation_input_tokens = Some(request_usage.cache_creation_tokens);
    }
    if positive_tokens(data.cache_creation_ephemeral_5m_input_tokens) == 0
        && request_usage.cache_creation_ephemeral_5m_tokens > 0
    {
        data.cache_creation_ephemeral_5m_input_tokens =
            Some(request_usage.cache_creation_ephemeral_5m_tokens);
    }
    if positive_tokens(data.cache_creation_ephemeral_1h_input_tokens) == 0
        && request_usage.cache_creation_ephemeral_1h_tokens > 0
    {
        data.cache_creation_ephemeral_1h_input_tokens =
            Some(request_usage.cache_creation_ephemeral_1h_tokens);
    }
}

fn positive_tokens(value: Option<u64>) -> u64 {
    value.unwrap_or_default()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct EstimatedRequestUsage {
    input_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    cache_creation_ephemeral_5m_tokens: u64,
    cache_creation_ephemeral_1h_tokens: u64,
}

fn estimate_request_usage(value: &Value) -> Option<EstimatedRequestUsage> {
    let preferred_total = match value {
        Value::Object(object) => [
            "instructions",
            "input",
            "messages",
            "prompt",
            "contents",
            "system",
            "tools",
        ]
        .into_iter()
        .filter_map(|field| object.get(field))
        .map(estimate_json_tokens)
        .fold(0u64, u64::saturating_add),
        _ => 0,
    };
    let total = if preferred_total > 0 {
        preferred_total
    } else {
        estimate_json_tokens(value)
    };
    if total == 0 {
        return None;
    }

    let mut usage = EstimatedRequestUsage {
        input_tokens: total,
        ..EstimatedRequestUsage::default()
    };
    apply_explicit_request_cache_usage(value, &mut usage);
    Some(usage)
}

fn apply_explicit_request_cache_usage(value: &Value, usage: &mut EstimatedRequestUsage) {
    usage.cache_read_tokens = first_positive_u64_path(
        value,
        &[
            &["cache_read_input_tokens"],
            &["cache_read_tokens"],
            &["input_tokens_details", "cached_tokens"],
            &["prompt_tokens_details", "cached_tokens"],
        ],
    )
    .unwrap_or_default();
    usage.cache_creation_tokens = first_positive_u64_path(
        value,
        &[
            &["cache_creation_input_tokens"],
            &["cache_creation_tokens"],
            &["input_tokens_details", "cached_creation_tokens"],
            &["prompt_tokens_details", "cached_creation_tokens"],
        ],
    )
    .unwrap_or_default();
    usage.cache_creation_ephemeral_5m_tokens = first_positive_u64_path(
        value,
        &[
            &["cache_creation", "ephemeral_5m_input_tokens"],
            &["cache_creation_ephemeral_5m_input_tokens"],
        ],
    )
    .unwrap_or_default();
    usage.cache_creation_ephemeral_1h_tokens = first_positive_u64_path(
        value,
        &[
            &["cache_creation", "ephemeral_1h_input_tokens"],
            &["cache_creation_ephemeral_1h_input_tokens"],
        ],
    )
    .unwrap_or_default();
    if usage.cache_creation_tokens == 0 {
        usage.cache_creation_tokens = usage
            .cache_creation_ephemeral_5m_tokens
            .saturating_add(usage.cache_creation_ephemeral_1h_tokens);
    }
}

fn first_positive_u64_path(value: &Value, paths: &[&[&str]]) -> Option<u64> {
    paths
        .iter()
        .find_map(|path| value_at_path(value, path).and_then(value_as_positive_u64))
}

fn value_at_path<'a>(mut value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    for segment in path {
        value = value.get(*segment)?;
    }
    Some(value)
}

fn value_as_positive_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .filter(|value| *value > 0)
}

fn estimate_json_tokens(value: &Value) -> u64 {
    match value {
        Value::String(text) => estimate_text_tokens(text),
        Value::Array(items) => items
            .iter()
            .map(estimate_json_tokens)
            .fold(0u64, u64::saturating_add),
        Value::Object(object) => object
            .iter()
            .map(|(key, value)| {
                estimate_text_tokens(key).saturating_add(estimate_json_tokens(value))
            })
            .fold(0u64, u64::saturating_add),
        Value::Null => 0,
        _ => 1,
    }
}

fn estimate_text_tokens(text: &str) -> u64 {
    let chars = text.chars().count() as u64;
    if chars == 0 {
        0
    } else {
        chars.div_ceil(4).max(1)
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
        let reasoning = usage
            .get("reasoning_tokens")
            .and_then(Value::as_u64)
            .or_else(|| {
                usage
                    .get("output_tokens_details")
                    .or_else(|| usage.get("completion_tokens_details"))
                    .and_then(Value::as_object)
                    .and_then(|details| details.get("reasoning_tokens"))
                    .and_then(Value::as_u64)
            })
            .unwrap_or_default();
        let total = usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| input.saturating_add(output).saturating_add(reasoning));
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
        let raw_total = usage
            .get("totalTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(input + output);
        return Some((input, output, raw_total));
    }

    if let Some(chunks) = value.get("chunks").and_then(Value::as_array) {
        if let Some(tokens) = chunks.iter().rev().find_map(extract_token_counts_from_json) {
            return Some(tokens);
        }
    }

    if let Some(response) = value.get("response") {
        return extract_token_counts_from_json(response);
    }

    None
}

fn empty_to_none(value: Option<String>) -> Option<String> {
    value.and_then(trim_owned_non_empty_string)
}

#[cfg(test)]
mod tests {
    use super::{
        build_pending_usage_record, build_pending_usage_record_from_seed,
        build_stream_terminal_usage_event, build_streaming_usage_record,
        build_sync_terminal_usage_event, build_sync_terminal_usage_payload_seed,
        build_sync_terminal_usage_seed, build_terminal_usage_context_seed,
        build_terminal_usage_event_from_seed, build_usage_event_data_seed, decode_body_for_storage,
        extract_token_counts_from_json, extract_token_counts_from_value, headers_to_json,
        mask_header_value, mask_sensitive_body_fields, mask_sensitive_headers_in_json_value,
        parse_sse_body_for_storage, resolve_error_message, trim_owned_non_empty_string,
        LifecycleUsageSeed, TerminalUsageSeed, UsageBodyRefsSeed, UsageBodyStatesSeed,
        UsageRoutingSeed, UsageTerminalState, MAX_USAGE_CAPTURE_BYTES, MAX_USAGE_CAPTURE_DEPTH,
    };
    use crate::{
        build_upsert_usage_record_from_event, GatewayStreamReportRequest, GatewaySyncReportRequest,
        UsageEvent, UsageEventData, UsageEventType,
    };
    use aether_contracts::{
        ExecutionPlan, ExecutionStreamTerminalSummary, RequestBody, StandardizedUsage,
    };
    use aether_data_contracts::repository::usage::UsageBodyCaptureState;
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
    fn extracts_openai_usage_tokens_with_cache_components() {
        let tokens = extract_token_counts_from_json(&json!({
            "usage": {
                "input_tokens": 3,
                "output_tokens": 5,
                "total_tokens": 8,
                "input_tokens_details": {
                    "cached_tokens": 2,
                    "cached_creation_tokens": 1
                }
            }
        }))
        .expect("tokens should exist");

        assert_eq!(tokens, (3, 5, 8));
    }

    #[test]
    fn extracts_openai_usage_tokens_with_prompt_token_details() {
        let tokens = extract_token_counts_from_json(&json!({
            "usage": {
                "prompt_tokens": 3,
                "completion_tokens": 5,
                "total_tokens": 8,
                "prompt_tokens_details": {
                    "cached_tokens": 2,
                    "cached_creation_tokens": 1
                }
            }
        }))
        .expect("tokens should exist");

        assert_eq!(tokens, (3, 5, 8));
    }

    #[test]
    fn extracts_claude_usage_tokens_with_cache_components() {
        let tokens = extract_token_counts_from_json(&json!({
            "usage": {
                "input_tokens": 6,
                "output_tokens": 20,
                "cache_creation_input_tokens": 41857,
                "cache_read_input_tokens": 0
            }
        }))
        .expect("tokens should exist");

        assert_eq!(tokens, (6, 20, 26));
    }

    #[test]
    fn extracts_gemini_usage_tokens_without_adding_cached_content_twice() {
        let tokens = extract_token_counts_from_json(&json!({
            "usageMetadata": {
                "promptTokenCount": 14,
                "candidatesTokenCount": 6,
                "cachedContentTokenCount": 2,
                "totalTokenCount": 20
            }
        }))
        .expect("tokens should exist");

        assert_eq!(tokens, (14, 6, 20));
    }

    #[test]
    fn extracts_usage_tokens_from_last_matching_chunk() {
        let tokens = extract_token_counts_from_json(&json!({
            "chunks": [
                {
                    "response": {
                        "usage": {
                            "input_tokens": 1,
                            "output_tokens": 2,
                            "total_tokens": 3
                        }
                    }
                },
                {
                    "response": {
                        "usage": {
                            "input_tokens": 9,
                            "output_tokens": 4,
                            "total_tokens": 13,
                            "input_tokens_details": {
                                "cached_tokens": 5,
                                "cached_creation_tokens": 2
                            }
                        }
                    }
                }
            ]
        }))
        .expect("tokens should exist");

        assert_eq!(tokens, (9, 4, 13));
    }

    #[test]
    fn trim_owned_non_empty_string_preserves_clean_values_and_drops_blank_ones() {
        assert_eq!(
            trim_owned_non_empty_string("body-ref-1".to_string()),
            Some("body-ref-1".to_string()),
        );
        assert_eq!(
            trim_owned_non_empty_string("  body-ref-1  ".to_string()),
            Some("body-ref-1".to_string()),
        );
        assert_eq!(trim_owned_non_empty_string("   ".to_string()), None);
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
            client_api_format: "claude:messages".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
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
        assert_eq!(record.provider_id.as_deref(), Some("provider-1"));
        assert_eq!(record.provider_endpoint_id.as_deref(), Some("endpoint-1"));
        assert_eq!(record.provider_api_key_id.as_deref(), Some("key-1"));
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
        let metadata = record
            .request_metadata
            .as_ref()
            .and_then(Value::as_object)
            .expect("pending usage should only keep lightweight request metadata");
        assert_eq!(metadata.len(), 1);
        let body_size = metadata
            .get("body_size")
            .and_then(Value::as_object)
            .expect("pending usage should keep request body size metadata");
        assert_eq!(
            body_size.get("basis"),
            Some(&json!(
                "serialized gateway request bodies after normalization"
            ))
        );
        assert!(body_size.get("client_request_body").is_some());
        assert!(body_size.get("provider_request_body").is_some());
        assert!(body_size.get("provider_over_client").is_some());
    }

    #[test]
    fn pending_usage_record_preserves_standalone_key_metadata() {
        let plan = ExecutionPlan {
            request_id: "req-pending-standalone-1".to_string(),
            candidate_id: None,
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5.4"})),
            stream: false,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };

        let record = build_pending_usage_record(
            &plan,
            Some(&json!({
                "api_key_is_standalone": true,
                "client_ip": "203.0.113.8",
                "user_agent": "Claude-Code/1.0"
            })),
            1_700_000_000,
        )
        .expect("pending usage should build");

        let metadata = record
            .request_metadata
            .as_ref()
            .and_then(Value::as_object)
            .expect("pending usage should keep request metadata");
        assert_eq!(metadata.get("api_key_is_standalone"), Some(&json!(true)));
        assert_eq!(metadata.get("client_ip"), Some(&json!("203.0.113.8")));
        assert_eq!(metadata.get("user_agent"), Some(&json!("Claude-Code/1.0")));
        let body_size = metadata
            .get("body_size")
            .and_then(Value::as_object)
            .expect("pending usage should keep request body size metadata");
        assert_eq!(
            body_size.get("basis"),
            Some(&json!(
                "serialized gateway request bodies after normalization"
            ))
        );
        assert!(body_size.get("provider_request_body").is_some());
        assert!(body_size.get("client_request_body").is_none());
        assert!(body_size.get("provider_over_client").is_none());
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
            client_api_format: "claude:messages".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
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
            client_api_format: "claude:messages".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-usage-deep-1".to_string(),
            report_kind: "claude_cli_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "claude:messages",
                "provider_api_format": "openai:responses",
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
    fn usage_body_capture_redacts_nested_provider_secrets() {
        let masked = mask_sensitive_body_fields(json!({
            "metadata": {
                "apiKey": "devin-session-token$secret-value",
                "nested": {
                    "sessionToken": "session-token-secret"
                }
            },
            "password": "plain-password",
            "messages": [{"content": "safe text"}]
        }));

        assert_ne!(
            masked.pointer("/metadata/apiKey").and_then(Value::as_str),
            Some("devin-session-token$secret-value")
        );
        assert_ne!(
            masked
                .pointer("/metadata/nested/sessionToken")
                .and_then(Value::as_str),
            Some("session-token-secret")
        );
        assert_ne!(
            masked.get("password").and_then(Value::as_str),
            Some("plain-password")
        );
        assert_eq!(
            masked
                .pointer("/messages/0/content")
                .and_then(Value::as_str),
            Some("safe text")
        );
    }

    #[test]
    fn sync_terminal_usage_redacts_provider_request_body_secrets_from_context() {
        let plan = ExecutionPlan {
            request_id: "req-sync-redact-provider-request-1".to_string(),
            candidate_id: Some("cand-sync-redact-provider-request-1".to_string()),
            provider_name: Some("Windsurf".to_string()),
            provider_id: "provider-windsurf".to_string(),
            endpoint_id: "endpoint-windsurf".to_string(),
            key_id: "key-windsurf".to_string(),
            method: "POST".to_string(),
            url: "https://server.codeium.com/exa.api_server_pb.ApiServerService/GetChatMessage"
                .to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "windsurf-model"})),
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("windsurf-model".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-redact-provider-request-1".to_string(),
            report_kind: "openai_chat_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:chat",
                "provider_api_format": "openai:chat",
                "provider_request_body": {
                    "metadata": {
                        "apiKey": "devin-session-token$abc",
                        "sessionToken": "session-token-secret"
                    },
                    "message": "safe prompt"
                }
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: Some(json!({"id": "resp_1", "choices": []})),
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        };

        let event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("terminal usage should build");
        let provider_request = event
            .data
            .provider_request_body
            .as_ref()
            .expect("provider request body should be captured");

        assert_ne!(
            provider_request
                .pointer("/metadata/apiKey")
                .and_then(Value::as_str),
            Some("devin-session-token$abc")
        );
        assert_ne!(
            provider_request
                .pointer("/metadata/sessionToken")
                .and_then(Value::as_str),
            Some("session-token-secret")
        );
        assert_eq!(
            provider_request.pointer("/message").and_then(Value::as_str),
            Some("safe prompt")
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
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-usage-1".to_string(),
            report_kind: "openai_chat_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:chat",
                "provider_api_format": "openai:responses",
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
            provider_body_state: Some(UsageBodyCaptureState::Inline),
            client_body_base64: Some(
                base64::engine::general_purpose::STANDARD
                    .encode("data: {\"id\":\"chatcmpl_123\"}\n\ndata: [DONE]\n"),
            ),
            client_body_state: Some(UsageBodyCaptureState::Inline),
            terminal_summary: None,
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
    fn cancelled_stream_usage_does_not_estimate_tokens_from_request_or_partial_response() {
        let plan = ExecutionPlan {
            request_id: "req-stream-cancelled-estimated-usage-1".to_string(),
            candidate_id: Some("cand-stream-cancelled-estimated-usage-1".to_string()),
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
                "input": "Write a short greeting for a usage test",
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let sse_body = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_cancel_123\",\"model\":\"gpt-5.4\"}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello there friend\"}\n\n"
        );
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-cancelled-estimated-usage-1".to_string(),
            report_kind: "openai_responses_stream_cancelled".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "provider_request_body": {
                    "model": "gpt-5.4",
                    "input": "Write a short greeting for a usage test",
                    "stream": true
                }
            })),
            status_code: 499,
            headers: BTreeMap::new(),
            provider_body_base64: Some(base64::engine::general_purpose::STANDARD.encode(sse_body)),
            provider_body_state: Some(UsageBodyCaptureState::Inline),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: None,
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Cancelled);
        assert_eq!(event.data.input_tokens, None);
        assert_eq!(event.data.output_tokens, None);
        assert_eq!(event.data.total_tokens, None);
        assert_eq!(event.data.cache_read_input_tokens, None);
    }

    #[test]
    fn cancelled_stream_usage_does_not_infer_cache_or_token_estimates_from_prompt_cache_key() {
        let request_body = json!({
            "model": "gpt-5.4",
            "input": "Use the cached project context and answer briefly",
            "prompt_cache_key": "prompt-session-1",
            "stream": true
        });
        let plan = ExecutionPlan {
            request_id: "req-stream-cancelled-cache-estimated-usage-1".to_string(),
            candidate_id: Some("cand-stream-cancelled-cache-estimated-usage-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(request_body.clone()),
            stream: true,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-cancelled-cache-estimated-usage-1".to_string(),
            report_kind: "openai_responses_stream_cancelled".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "provider_request_body": request_body
            })),
            status_code: 499,
            headers: BTreeMap::new(),
            provider_body_base64: Some(base64::engine::general_purpose::STANDARD.encode(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"cached answer\"}\n\n",
            )),
            provider_body_state: Some(UsageBodyCaptureState::Inline),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: None,
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Cancelled);
        assert_eq!(event.data.input_tokens, None);
        assert_eq!(event.data.output_tokens, None);
        assert_eq!(event.data.total_tokens, None);
        assert_eq!(event.data.cache_read_input_tokens, None);
    }

    #[test]
    fn cancelled_stream_usage_preserves_terminal_summary_usage() {
        let plan = ExecutionPlan {
            request_id: "req-stream-cancelled-summary-usage-1".to_string(),
            candidate_id: Some("cand-stream-cancelled-summary-usage-1".to_string()),
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
                "input": "This cancelled request has terminal upstream usage",
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let mut standardized_usage = StandardizedUsage::new();
        standardized_usage.input_tokens = 13;
        standardized_usage.output_tokens = 21;
        standardized_usage.cache_creation_tokens = 2;
        standardized_usage.cache_read_tokens = 3;
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-cancelled-summary-usage-1".to_string(),
            report_kind: "openai_responses_stream_cancelled".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses"
            })),
            status_code: 499,
            headers: BTreeMap::new(),
            provider_body_base64: None,
            provider_body_state: Some(UsageBodyCaptureState::None),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: Some(ExecutionStreamTerminalSummary {
                standardized_usage: Some(standardized_usage),
                finish_reason: None,
                response_id: Some("resp_cancel_summary_1".to_string()),
                model: Some("gpt-5.4".to_string()),
                observed_finish: true,
                unknown_event_count: 0,
                parser_error: None,
            }),
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Cancelled);
        assert_eq!(event.data.input_tokens, Some(13));
        assert_eq!(event.data.output_tokens, Some(21));
        assert_eq!(event.data.total_tokens, Some(34));
        assert_eq!(event.data.cache_creation_input_tokens, Some(2));
        assert_eq!(event.data.cache_read_input_tokens, Some(3));
    }

    #[test]
    fn failed_stream_usage_does_not_estimate_partial_response_tokens() {
        let plan = ExecutionPlan {
            request_id: "req-stream-failed-estimated-usage-1".to_string(),
            candidate_id: Some("cand-stream-failed-estimated-usage-1".to_string()),
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
                "input": "This failed request should not be billed",
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-failed-estimated-usage-1".to_string(),
            report_kind: "openai_responses_stream_failed".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "provider_request_body": {
                    "model": "gpt-5.4",
                    "input": "This failed request should not be billed",
                    "stream": true
                }
            })),
            status_code: 500,
            headers: BTreeMap::new(),
            provider_body_base64: Some(base64::engine::general_purpose::STANDARD.encode(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial text\"}\n\n",
            )),
            provider_body_state: Some(UsageBodyCaptureState::Inline),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: None,
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Failed);
        assert_eq!(event.data.input_tokens, None);
        assert_eq!(event.data.output_tokens, None);
        assert_eq!(event.data.total_tokens, None);
    }

    #[test]
    fn stream_terminal_usage_marks_redirect_status_as_failed() {
        let plan = ExecutionPlan {
            request_id: "req-stream-redirect-usage".to_string(),
            candidate_id: Some("cand-stream-redirect-usage".to_string()),
            provider_name: Some("ChatGPTWeb".to_string()),
            provider_id: "provider-redirect".to_string(),
            endpoint_id: "endpoint-redirect".to_string(),
            key_id: "key-redirect".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1beta/models/gemini:streamGenerateContent".to_string(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody {
                json_body: None,
                body_bytes_b64: None,
                body_ref: None,
            },
            stream: true,
            client_api_format: "gemini:generate_content".to_string(),
            provider_api_format: "gemini:generate_content".to_string(),
            model_name: Some("gemini".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let client_body = json!({
            "error": {
                "type": "execution_runtime_non_success_status",
                "message": "execution runtime stream returned non-success status 302",
                "code": 302,
                "upstream_status": 302,
                "location": "/"
            }
        });
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-redirect-usage".to_string(),
            report_kind: "gemini_chat_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "gemini:generate_content",
                "provider_api_format": "gemini:generate_content"
            })),
            status_code: 302,
            headers: BTreeMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                ("x-aether-upstream-status".to_string(), "302".to_string()),
            ]),
            provider_body_base64: Some(
                base64::engine::general_purpose::STANDARD
                    .encode(br#"{"error":{"message":"raw redirect body"}}"#),
            ),
            provider_body_state: Some(UsageBodyCaptureState::Inline),
            client_body_base64: Some(
                base64::engine::general_purpose::STANDARD
                    .encode(serde_json::to_vec(&client_body).expect("body should encode")),
            ),
            client_body_state: Some(UsageBodyCaptureState::Inline),
            terminal_summary: None,
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Failed);
        assert_eq!(event.data.status_code, Some(302));
        assert_eq!(event.data.error_category.as_deref(), Some("redirect"));
        assert_eq!(
            event.data.error_message.as_deref(),
            Some("raw redirect body")
        );
    }

    #[test]
    fn builds_stream_terminal_usage_from_terminal_summary_usage_without_decoding_bodies() {
        let plan = ExecutionPlan {
            request_id: "req-stream-summary-usage-1".to_string(),
            candidate_id: Some("cand-stream-summary-usage-1".to_string()),
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
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let mut standardized_usage = StandardizedUsage::new();
        standardized_usage.input_tokens = 13;
        standardized_usage.output_tokens = 21;
        standardized_usage.cache_creation_tokens = 2;
        standardized_usage.cache_read_tokens = 3;
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-summary-usage-1".to_string(),
            report_kind: "openai_chat_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:chat",
                "provider_api_format": "openai:responses",
                "needs_conversion": true
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            provider_body_base64: None,
            provider_body_state: Some(UsageBodyCaptureState::None),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: Some(ExecutionStreamTerminalSummary {
                standardized_usage: Some(standardized_usage),
                finish_reason: Some("stop".to_string()),
                response_id: Some("resp_summary_1".to_string()),
                model: Some("gpt-5.4".to_string()),
                observed_finish: true,
                unknown_event_count: 0,
                parser_error: None,
            }),
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.data.input_tokens, Some(13));
        assert_eq!(event.data.output_tokens, Some(21));
        assert_eq!(event.data.total_tokens, Some(34));
        assert_eq!(event.data.cache_creation_input_tokens, Some(2));
        assert_eq!(event.data.cache_read_input_tokens, Some(3));
        assert!(event.data.response_body.is_none());
        assert!(event.data.client_response_body.is_none());
    }

    #[test]
    fn stream_terminal_usage_marks_missing_observed_finish_as_failed() {
        let plan = ExecutionPlan {
            request_id: "req-stream-missing-finish-1".to_string(),
            candidate_id: Some("cand-stream-missing-finish-1".to_string()),
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
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-missing-finish-1".to_string(),
            report_kind: "openai_responses_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses"
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            provider_body_base64: None,
            provider_body_state: Some(UsageBodyCaptureState::None),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: Some(ExecutionStreamTerminalSummary {
                response_id: Some("resp_missing_finish".to_string()),
                model: Some("gpt-5.5".to_string()),
                observed_finish: false,
                ..ExecutionStreamTerminalSummary::default()
            }),
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Failed);
        assert_eq!(event.data.status_code, Some(200));
        assert_eq!(
            event.data.error_category.as_deref(),
            Some("stream_missing_terminal_event")
        );
        assert_eq!(
            event.data.error_message.as_deref(),
            Some("execution runtime stream ended before provider terminal event")
        );
        assert_eq!(event.data.input_tokens, None);
        assert_eq!(event.data.output_tokens, None);
    }

    #[test]
    fn stream_terminal_usage_marks_empty_openai_responses_capture_as_missing_terminal() {
        let plan = ExecutionPlan {
            request_id: "req-stream-empty-capture-1".to_string(),
            candidate_id: Some("cand-stream-empty-capture-1".to_string()),
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
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-empty-capture-1".to_string(),
            report_kind: "openai_responses_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses"
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            provider_body_base64: None,
            provider_body_state: Some(UsageBodyCaptureState::None),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: None,
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Failed);
        assert_eq!(event.data.status_code, Some(200));
        assert_eq!(
            event.data.error_category.as_deref(),
            Some("stream_missing_terminal_event")
        );
        assert_eq!(
            event.data.error_message.as_deref(),
            Some("execution runtime stream ended before provider terminal event")
        );
        assert_eq!(
            event
                .data
                .client_response_body
                .as_ref()
                .and_then(|body| body.get("error"))
                .and_then(|error| error.get("type"))
                .and_then(Value::as_str),
            Some("stream_missing_terminal_event")
        );
        assert_eq!(
            event.data.client_response_body_state,
            Some(UsageBodyCaptureState::Inline)
        );
    }

    #[test]
    fn stream_terminal_usage_marks_missing_captured_openai_responses_terminal_as_failed() {
        let plan = ExecutionPlan {
            request_id: "req-stream-missing-captured-finish-1".to_string(),
            candidate_id: Some("cand-stream-missing-captured-finish-1".to_string()),
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
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let provider_sse = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n"
        );
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-missing-captured-finish-1".to_string(),
            report_kind: "openai_responses_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses"
            })),
            status_code: 200,
            headers: BTreeMap::from([(
                "content-type".to_string(),
                "text/event-stream".to_string(),
            )]),
            provider_body_base64: Some(
                base64::engine::general_purpose::STANDARD.encode(provider_sse.as_bytes()),
            ),
            provider_body_state: Some(UsageBodyCaptureState::Inline),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: None,
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Failed);
        assert_eq!(event.data.status_code, Some(200));
        assert_eq!(
            event.data.error_category.as_deref(),
            Some("stream_missing_terminal_event")
        );
        assert_eq!(
            event.data.error_message.as_deref(),
            Some("execution runtime stream ended before provider terminal event")
        );
    }

    #[test]
    fn stream_terminal_usage_ignores_missing_terminal_from_truncated_capture() {
        let plan = ExecutionPlan {
            request_id: "req-stream-truncated-captured-finish-1".to_string(),
            candidate_id: Some("cand-stream-truncated-captured-finish-1".to_string()),
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
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let provider_sse = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n"
        );
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-truncated-captured-finish-1".to_string(),
            report_kind: "openai_responses_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses"
            })),
            status_code: 200,
            headers: BTreeMap::from([(
                "content-type".to_string(),
                "text/event-stream".to_string(),
            )]),
            provider_body_base64: Some(
                base64::engine::general_purpose::STANDARD.encode(provider_sse.as_bytes()),
            ),
            provider_body_state: Some(UsageBodyCaptureState::Truncated),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: None,
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Completed);
        assert_eq!(event.data.status_code, Some(200));
        assert_eq!(event.data.error_category, None);
        assert_eq!(event.data.error_message, None);
    }

    #[test]
    fn stream_terminal_usage_marks_http_200_response_failed_as_stream_terminal_error() {
        let plan = ExecutionPlan {
            request_id: "req-stream-response-failed-1".to_string(),
            candidate_id: Some("cand-stream-response-failed-1".to_string()),
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
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let message = "This content was flagged for possible cybersecurity risk";
        let provider_sse = format!(
            concat!(
                "event: response.failed\n",
                "data: {{\"type\":\"response.failed\",\"response\":{{\"status\":\"failed\",\"error\":{{\"message\":\"{}\",\"code\":\"cyber_policy\"}}}}}}\n\n"
            ),
            message
        );
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-response-failed-1".to_string(),
            report_kind: "openai_responses_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses"
            })),
            status_code: 200,
            headers: BTreeMap::from([(
                "content-type".to_string(),
                "text/event-stream".to_string(),
            )]),
            provider_body_base64: Some(
                base64::engine::general_purpose::STANDARD.encode(provider_sse),
            ),
            provider_body_state: Some(UsageBodyCaptureState::Inline),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: Some(ExecutionStreamTerminalSummary {
                response_id: Some("resp_failed".to_string()),
                model: Some("gpt-5.5".to_string()),
                observed_finish: true,
                parser_error: Some(message.to_string()),
                ..ExecutionStreamTerminalSummary::default()
            }),
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Failed);
        assert_eq!(event.data.status_code, Some(200));
        assert_eq!(event.data.error_message.as_deref(), Some(message));
        assert_eq!(
            event.data.error_category.as_deref(),
            Some("stream_terminal_error")
        );
        assert_eq!(
            event
                .data
                .client_response_body
                .as_ref()
                .and_then(|body| body.get("error"))
                .and_then(|error| error.get("code"))
                .and_then(Value::as_str),
            Some("cyber_policy")
        );
        assert_eq!(
            event
                .data
                .client_response_body
                .as_ref()
                .and_then(|body| body.get("error"))
                .and_then(|error| error.get("type"))
                .and_then(Value::as_str),
            Some("stream_terminal_error")
        );
    }

    #[test]
    fn completed_image_usage_estimates_request_tokens_when_provider_usage_is_missing() {
        let plan = ExecutionPlan {
            request_id: "req-image-completed-estimate-1".to_string(),
            candidate_id: Some("cand-image-completed-estimate-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/images/generations".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-image-2",
                "prompt": "draw a small red cube on a clean desk",
                "size": "1024x1024",
                "quality": "medium"
            })),
            stream: true,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:image".to_string(),
            model_name: Some("gpt-image-2".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let mut standardized_usage = StandardizedUsage::new();
        standardized_usage.request_count = 1;
        standardized_usage
            .dimensions
            .insert("image_count".to_string(), json!(1));
        standardized_usage
            .dimensions
            .insert("image_size".to_string(), json!("1024x1024"));
        standardized_usage
            .dimensions
            .insert("image_quality".to_string(), json!("medium"));
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-image-completed-estimate-1".to_string(),
            report_kind: "openai_chat_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:chat",
                "provider_api_format": "openai:image",
                "image_request": {
                    "size": "1024x1024",
                    "quality": "medium"
                }
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            provider_body_base64: None,
            provider_body_state: Some(UsageBodyCaptureState::None),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: Some(ExecutionStreamTerminalSummary {
                standardized_usage: Some(standardized_usage),
                finish_reason: Some("stop".to_string()),
                response_id: Some("resp_image_estimate_1".to_string()),
                model: Some("gpt-image-2".to_string()),
                observed_finish: true,
                unknown_event_count: 0,
                parser_error: None,
            }),
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Completed);
        assert!(event.data.input_tokens.unwrap_or_default() > 0);
        assert_eq!(event.data.output_tokens.unwrap_or_default(), 0);
        assert_eq!(event.data.total_tokens, event.data.input_tokens);
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("dimensions"))
                .and_then(|dimensions| dimensions.get("image_count"))
                .and_then(Value::as_i64),
            Some(1)
        );
    }

    #[test]
    fn sync_completed_openai_image_usage_infers_image_dimensions_from_response() {
        let request_body = json!({
            "model": "gpt-image-2",
            "prompt": "draw a small red cube on a clean desk",
            "size": "1024x1024",
            "quality": "medium",
            "output_format": "png"
        });
        let plan = ExecutionPlan {
            request_id: "req-image-sync-completed-dimensions-1".to_string(),
            candidate_id: Some("cand-image-sync-completed-dimensions-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/images/generations".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(request_body.clone()),
            stream: false,
            client_api_format: "openai:image".to_string(),
            provider_api_format: "openai:image".to_string(),
            model_name: Some("gpt-image-2".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-image-sync-completed-dimensions-1".to_string(),
            report_kind: "openai_image_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:image",
                "provider_api_format": "openai:image",
                "provider_request_body": request_body
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: Some(json!({
                "created": 1_700_000_000,
                "data": [{ "b64_json": "abc" }]
            })),
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        };

        let event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        let dimensions = event
            .data
            .request_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("dimensions"))
            .expect("dimensions should exist");
        assert_eq!(event.event_type, UsageEventType::Completed);
        assert!(event.data.input_tokens.unwrap_or_default() > 0);
        assert_eq!(dimensions.get("image_count"), Some(&json!(1)));
        assert_eq!(dimensions.get("image_size"), Some(&json!("1024x1024")));
        assert_eq!(dimensions.get("image_quality"), Some(&json!("medium")));
        assert_eq!(dimensions.get("image_output_format"), Some(&json!("png")));
    }

    #[test]
    fn sync_completed_openai_image_usage_preserves_native_usage_with_image_count() {
        let request_body = json!({
            "model": "gpt-image-2",
            "prompt": "draw a small red cube on a clean desk",
            "size": "1024x1024",
            "quality": "medium"
        });
        let plan = ExecutionPlan {
            request_id: "req-image-sync-native-usage-1".to_string(),
            candidate_id: Some("cand-image-sync-native-usage-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/images/generations".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(request_body.clone()),
            stream: false,
            client_api_format: "openai:image".to_string(),
            provider_api_format: "openai:image".to_string(),
            model_name: Some("gpt-image-2".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-image-sync-native-usage-1".to_string(),
            report_kind: "openai_image_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:image",
                "provider_api_format": "openai:image",
                "provider_request_body": request_body
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: Some(json!({
                "usage": {
                    "input_tokens": 11,
                    "output_tokens": 22,
                    "total_tokens": 33
                },
                "data": [{ "b64_json": "abc" }]
            })),
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        };

        let event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Completed);
        assert_eq!(event.data.input_tokens, Some(11));
        assert_eq!(event.data.output_tokens, Some(22));
        assert_eq!(event.data.total_tokens, Some(33));
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("dimensions"))
                .and_then(|dimensions| dimensions.get("image_count")),
            Some(&json!(1))
        );
    }

    #[test]
    fn stream_terminal_usage_prefers_more_complete_provider_chunks_usage() {
        let plan = ExecutionPlan {
            request_id: "req-stream-provider-chunks-usage-1".to_string(),
            candidate_id: Some("cand-stream-provider-chunks-usage-1".to_string()),
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
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let mut partial_summary_usage = StandardizedUsage::new();
        partial_summary_usage.output_tokens = 148;
        let provider_body = json!({
            "chunks": [
                {
                    "type": "response.created",
                    "response": {
                        "id": "resp_123",
                        "object": "response",
                        "model": "gpt-5.5",
                        "status": "in_progress",
                        "usage": null
                    }
                },
                {
                    "type": "response.completed",
                    "response": {
                        "id": "resp_123",
                        "object": "response",
                        "model": "gpt-5.5",
                        "status": "completed",
                        "usage": {
                            "input_tokens": 26,
                            "input_tokens_details": {
                                "cached_tokens": 0
                            },
                            "output_tokens": 148,
                            "output_tokens_details": {
                                "reasoning_tokens": 10
                            },
                            "total_tokens": 174
                        }
                    },
                    "sequence_number": 141
                }
            ],
            "metadata": {
                "stream": true,
                "total_chunks": 142,
                "stored_chunks": 142
            }
        });
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-provider-chunks-usage-1".to_string(),
            report_kind: "openai_responses_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            provider_body_base64: Some(
                base64::engine::general_purpose::STANDARD.encode(provider_body.to_string()),
            ),
            provider_body_state: Some(UsageBodyCaptureState::Inline),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: Some(ExecutionStreamTerminalSummary {
                standardized_usage: Some(partial_summary_usage),
                finish_reason: None,
                response_id: Some("resp_123".to_string()),
                model: Some("gpt-5.5".to_string()),
                observed_finish: true,
                unknown_event_count: 0,
                parser_error: None,
            }),
            telemetry: None,
        };

        let event =
            build_stream_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.data.input_tokens, Some(26));
        assert_eq!(event.data.output_tokens, Some(148));
        assert_eq!(event.data.total_tokens, Some(174));
        assert_eq!(event.data.cache_read_input_tokens, None);
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
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let sse_body = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"in_progress\"}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello from Responses stream\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello from Responses stream\"}]}],\"usage\":{\"input_tokens\":3,\"input_tokens_details\":{\"cached_tokens\":2,\"cached_creation_tokens\":1},\"output_tokens\":5,\"output_tokens_details\":{\"reasoning_tokens\":1},\"total_tokens\":8}}}\n\n",
            "data: [DONE]\n",
        );
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-usage-2".to_string(),
            report_kind: "openai_responses_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            provider_body_base64: Some(base64::engine::general_purpose::STANDARD.encode(sse_body)),
            provider_body_state: Some(UsageBodyCaptureState::Inline),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: None,
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
                        "delta": "Hello from Responses stream"
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
                                            "text": "Hello from Responses stream"
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
            provider_api_format: "gemini:generate_content".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-usage-1".to_string(),
            report_kind: "openai_chat_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:chat",
                "provider_api_format": "gemini:generate_content",
                "needs_conversion": true
            })),
            status_code: 200,
            headers: BTreeMap::from([
                (
                    "authorization".to_string(),
                    "Bearer very-secret-token".to_string(),
                ),
                ("content-type".to_string(), "application/json".to_string()),
            ]),
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
            event.data.response_headers,
            Some(json!({
                "authorization": "Bear****oken",
                "content-type": "application/json"
            }))
        );
        assert_eq!(
            event.data.client_response_headers,
            Some(json!({
                "authorization": "Bear****oken",
                "content-type": "application/json"
            }))
        );
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
    fn sync_terminal_usage_prefers_upstream_stream_body_over_aggregated_sync_body() {
        let sse_body = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_sync_stream_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello from upstream stream\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_sync_stream_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":3,\"output_tokens\":5,\"total_tokens\":8}}}\n\n",
            "data: [DONE]\n",
        );
        let plan = ExecutionPlan {
            request_id: "req-sync-upstream-stream-1".to_string(),
            candidate_id: Some("cand-sync-upstream-stream-1".to_string()),
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
            stream: false,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-upstream-stream-1".to_string(),
            report_kind: "openai_responses_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "upstream_is_stream": true
            })),
            status_code: 200,
            headers: BTreeMap::from([(
                "content-type".to_string(),
                "text/event-stream".to_string(),
            )]),
            body_json: Some(json!({
                "id": "resp_sync_stream_123",
                "object": "response",
                "status": "completed",
                "output": [],
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 1,
                    "total_tokens": 2
                }
            })),
            client_body_json: Some(json!({
                "id": "resp_sync_stream_123",
                "object": "response",
                "status": "completed",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": "Hello from upstream stream"
                    }]
                }],
                "usage": {
                    "input_tokens": 3,
                    "output_tokens": 5,
                    "total_tokens": 8
                }
            })),
            body_base64: Some(base64::engine::general_purpose::STANDARD.encode(sse_body)),
            telemetry: None,
        };

        let event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.data.input_tokens, Some(3));
        assert_eq!(event.data.output_tokens, Some(5));
        assert_eq!(event.data.total_tokens, Some(8));
        assert_eq!(
            event
                .data
                .response_body
                .as_ref()
                .and_then(|value| value.get("chunks"))
                .and_then(Value::as_array)
                .and_then(|chunks| chunks.get(1))
                .and_then(|chunk| chunk.get("type"))
                .and_then(Value::as_str),
            Some("response.output_text.delta")
        );
        assert_eq!(
            event.data.client_response_body.as_ref().and_then(|value| {
                value
                    .get("output")
                    .and_then(Value::as_array)
                    .and_then(|output| output.first())
                    .and_then(|item| item.get("content"))
                    .and_then(Value::as_array)
                    .and_then(|content| content.first())
                    .and_then(|part| part.get("text"))
                    .and_then(Value::as_str)
            }),
            Some("Hello from upstream stream")
        );
    }

    #[test]
    fn sync_terminal_usage_prefers_error_body_over_partial_upstream_stream_body() {
        let partial_sse_body = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_partial_123\",\"object\":\"response\",\"model\":\"gpt-5.5\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
            "event: response.output_item.added\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"name\":\"exec_command\"}}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\\\"cmd\\\":\"}\n\n",
        );
        let plan = ExecutionPlan {
            request_id: "req-sync-upstream-stream-error-1".to_string(),
            candidate_id: Some("cand-sync-upstream-stream-error-1".to_string()),
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
            stream: false,
            client_api_format: "claude:messages".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-upstream-stream-error-1".to_string(),
            report_kind: "openai_responses_sync_error".to_string(),
            report_context: Some(json!({
                "client_api_format": "claude:messages",
                "provider_api_format": "openai:responses",
                "upstream_is_stream": true,
                "needs_conversion": true
            })),
            status_code: 200,
            headers: BTreeMap::from([(
                "content-type".to_string(),
                "text/event-stream".to_string(),
            )]),
            body_json: Some(json!({
                "error": {
                    "type": "internal",
                    "message": "error decoding response body: stream error received"
                }
            })),
            client_body_json: None,
            body_base64: Some(base64::engine::general_purpose::STANDARD.encode(partial_sse_body)),
            telemetry: None,
        };

        let event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Failed);
        assert_eq!(event.data.status_code, Some(200));
        assert_eq!(
            event.data.error_message.as_deref(),
            Some("error decoding response body: stream error received")
        );
        assert_eq!(
            event
                .data
                .response_body
                .as_ref()
                .and_then(|value| value.pointer("/error/type"))
                .and_then(Value::as_str),
            Some("internal")
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
            provider_api_format: "gemini:generate_content".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-seed-match-1".to_string(),
            report_kind: "openai_chat_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:chat",
                "provider_api_format": "gemini:generate_content",
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
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-body-ref-1".to_string(),
            report_kind: "openai_responses_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
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
    fn sync_terminal_usage_records_human_readable_request_body_sizes_in_metadata() {
        let client_body_bytes = vec![b'c'; 1024];
        let provider_body_bytes = vec![b'p'; 4096];
        let client_body_base64 =
            base64::engine::general_purpose::STANDARD.encode(client_body_bytes);
        let provider_body_base64 =
            base64::engine::general_purpose::STANDARD.encode(provider_body_bytes);
        let plan = ExecutionPlan {
            request_id: "req-sync-body-size-1".to_string(),
            candidate_id: Some("cand-sync-body-size-1".to_string()),
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
                json_body: None,
                body_bytes_b64: Some(provider_body_base64),
                body_ref: None,
            },
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-body-size-1".to_string(),
            report_kind: "openai_chat_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:chat",
                "provider_api_format": "openai:responses",
                "needs_conversion": true,
                "original_request_body": {
                    "body_bytes_b64": client_body_base64
                }
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: Some(json!({"id": "resp_123"})),
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        };

        let context_seed =
            build_terminal_usage_context_seed(&plan, payload.report_context.as_ref());
        assert_request_body_size_metadata(
            context_seed.request_metadata.as_ref(),
            "context seed should include body size metadata",
        );

        let payload_seed = build_sync_terminal_usage_payload_seed(&payload);
        let seed_event = build_terminal_usage_event_from_seed(build_sync_terminal_usage_seed(
            context_seed,
            payload_seed,
        ))
        .expect("seed usage event should build");
        assert_request_body_size_metadata(
            seed_event.data.request_metadata.as_ref(),
            "seed event should include body size metadata",
        );

        let wrapper_event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("wrapper usage event should build");
        assert_request_body_size_metadata(
            wrapper_event.data.request_metadata.as_ref(),
            "wrapper event should include body size metadata",
        );
    }

    fn assert_request_body_size_metadata(metadata: Option<&Value>, message: &str) {
        let body_size = metadata
            .and_then(|metadata| metadata.get("body_size"))
            .unwrap_or_else(|| panic!("{message}: {metadata:?}"));

        assert_eq!(body_size.get("client_request_body"), Some(&json!("1 KB")));
        assert_eq!(body_size.get("provider_request_body"), Some(&json!("4 KB")));
        assert_eq!(body_size.get("provider_over_client"), Some(&json!("4x")));
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
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-bytes-1".to_string(),
            report_kind: "openai_chat_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:chat",
                "provider_api_format": "openai:responses",
                "trace_id": "trace-stream-bytes-1"
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            provider_body_base64: Some(
                base64::engine::general_purpose::STANDARD.encode(provider_bytes),
            ),
            provider_body_state: Some(UsageBodyCaptureState::Inline),
            client_body_base64: Some(
                base64::engine::general_purpose::STANDARD.encode(client_bytes),
            ),
            client_body_state: Some(UsageBodyCaptureState::Inline),
            terminal_summary: None,
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
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-stream-usage-large-1".to_string(),
            report_kind: "openai_responses_stream_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            provider_body_base64: Some(base64::engine::general_purpose::STANDARD.encode(&sse_body)),
            provider_body_state: Some(UsageBodyCaptureState::Truncated),
            client_body_base64: None,
            client_body_state: Some(UsageBodyCaptureState::None),
            terminal_summary: None,
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
            client_api_format: "claude:messages".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-no-client-echo-1".to_string(),
            report_kind: "claude_cli_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "claude:messages",
                "provider_api_format": "openai:responses",
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
        let body_size = event
            .data
            .request_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("body_size"))
            .and_then(Value::as_object)
            .expect("provider body size metadata should exist");
        assert!(body_size.get("provider_request_body").is_some());
        assert!(body_size.get("client_request_body").is_none());
        assert!(body_size.get("provider_over_client").is_none());
    }

    #[test]
    fn openai_image_sync_terminal_usage_captures_request_and_response_bodies() {
        let plan = ExecutionPlan {
            request_id: "req-openai-image-sync-body-1".to_string(),
            candidate_id: Some("cand-openai-image-sync-body-1".to_string()),
            provider_name: Some("Upstream Aether".to_string()),
            provider_id: "provider-aether-1".to_string(),
            endpoint_id: "endpoint-aether-1".to_string(),
            key_id: "key-aether-1".to_string(),
            method: "POST".to_string(),
            url: "https://upstream-aether.example/v1/images/generations".to_string(),
            headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-image-2-upstream",
                "prompt": "Draw a red kite",
                "size": "1024x1024",
                "n": 1,
                "response_format": "b64_json"
            })),
            stream: false,
            client_api_format: "openai:image".to_string(),
            provider_api_format: "openai:image".to_string(),
            model_name: Some("gpt-image-2".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-openai-image-sync-body-1".to_string(),
            report_kind: "openai_image_sync_finalize".to_string(),
            report_context: Some(json!({
                "client_api_format": "openai:image",
                "provider_api_format": "openai:image",
                "needs_conversion": true,
                "original_request_body": {
                    "model": "gpt-image-2",
                    "prompt": "Draw a red kite",
                    "size": "1024x1024",
                    "response_format": "b64_json"
                },
                "provider_request_body": {
                    "model": "gpt-image-2-upstream",
                    "prompt": "Draw a red kite",
                    "size": "1024x1024",
                    "n": 1,
                    "response_format": "b64_json"
                }
            })),
            status_code: 200,
            headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
            body_json: Some(json!({
                "created": 1776839946,
                "data": [{
                    "b64_json": "aGVsbG8=",
                    "revised_prompt": "red kite"
                }],
                "usage": {
                    "input_tokens": 11,
                    "output_tokens": 22,
                    "total_tokens": 33
                }
            })),
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        };

        let event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

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
            payload
                .report_context
                .as_ref()
                .and_then(|value| value.get("provider_request_body"))
                .cloned()
        );
        assert_eq!(event.data.response_body, payload.body_json);
        assert!(event.data.client_response_body.is_none());
        assert_eq!(
            event.data.request_body_state,
            Some(UsageBodyCaptureState::Inline)
        );
        assert_eq!(
            event.data.provider_request_body_state,
            Some(UsageBodyCaptureState::Inline)
        );
        assert_eq!(
            event.data.response_body_state,
            Some(UsageBodyCaptureState::Inline)
        );
    }

    #[test]
    fn sync_terminal_usage_applies_kiro_simulated_cache_context() {
        let plan = ExecutionPlan {
            request_id: "req-sync-kiro-cache-context-1".to_string(),
            candidate_id: Some("cand-sync-kiro-cache-context-1".to_string()),
            provider_name: Some("Kiro".to_string()),
            provider_id: "provider-kiro-1".to_string(),
            endpoint_id: "endpoint-kiro-1".to_string(),
            key_id: "key-kiro-1".to_string(),
            method: "POST".to_string(),
            url: "https://kiro.example/generateAssistantResponse".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "claude-sonnet-4",
                "messages": [{"role": "user", "content": "hello kiro"}],
            })),
            stream: false,
            client_api_format: "claude:messages".to_string(),
            provider_api_format: "claude:messages".to_string(),
            model_name: Some("claude-sonnet-4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-kiro-cache-context-1".to_string(),
            report_kind: "claude_cli_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "claude:messages",
                "provider_api_format": "claude:messages",
                "provider_name": "Kiro",
                "model": "claude-sonnet-4",
                "input_tokens": 1800,
                "kiro_simulated_cache_enabled": true,
                "cache_creation_input_tokens": 1200,
                "cache_read_input_tokens": 300,
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: Some(json!({"id": "kiro-sync-response-1"})),
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        };

        let event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Completed);
        assert_eq!(event.data.input_tokens, Some(300));
        assert_eq!(event.data.cache_creation_input_tokens, Some(1200));
        assert_eq!(event.data.cache_read_input_tokens, Some(300));
        assert_eq!(event.data.total_tokens, Some(300));
    }

    #[test]
    fn sync_terminal_usage_treats_null_error_field_as_success() {
        let plan = ExecutionPlan {
            request_id: "req-sync-null-error-1".to_string(),
            candidate_id: Some("cand-sync-null-error-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
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
            client_api_format: "claude:messages".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-sync-null-error-1".to_string(),
            report_kind: "claude_cli_sync_success".to_string(),
            report_context: Some(json!({
                "client_api_format": "claude:messages",
                "provider_api_format": "openai:responses",
                "needs_conversion": true,
            })),
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: Some(json!({
                "id": "resp_1",
                "status": "completed",
                "error": null,
                "usage": {
                    "input_tokens": 24,
                    "output_tokens": 11,
                    "total_tokens": 35
                }
            })),
            client_body_json: Some(json!({
                "type": "message",
                "usage": {
                    "input_tokens": 24,
                    "output_tokens": 11
                }
            })),
            body_base64: None,
            telemetry: None,
        };

        let event =
            build_sync_terminal_usage_event(&plan, payload.report_context.as_ref(), &payload)
                .expect("usage event should build");

        assert_eq!(event.event_type, UsageEventType::Completed);
        assert_eq!(event.data.status_code, Some(200));
        assert_eq!(event.data.input_tokens, Some(24));
        assert_eq!(event.data.output_tokens, Some(11));
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
            model_id: None,
            global_model_id: None,
            provider_id: Some("provider-1".to_string()),
            provider_endpoint_id: Some("endpoint-1".to_string()),
            provider_api_key_id: Some("upstream-key-1".to_string()),
            request_type: "chat".to_string(),
            has_format_conversion: false,
            is_stream: false,
            body_refs: UsageBodyRefsSeed::default(),
            body_states: UsageBodyStatesSeed::default(),
            routing: UsageRoutingSeed {
                candidate_id: Some("cand-1".to_string()),
                ..UsageRoutingSeed::default()
            },
            status_code: 200,
            terminal_error_message: None,
            terminal_failure_category: None,
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
                model_id: None,
                global_model_id: None,
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
                body_states: UsageBodyStatesSeed::default(),
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
            transport_profile: None,
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

        let value = headers_to_json(&headers).expect("expected object");
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
    fn headers_to_json_returns_none_for_empty_headers() {
        assert!(headers_to_json(&BTreeMap::new()).is_none());
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

    #[test]
    fn resolve_error_message_extracts_generic_message_from_base64_body() {
        let body_base64 = base64::engine::general_purpose::STANDARD.encode(
            serde_json::to_vec(&json!({
                "message": "upstream exploded"
            }))
            .expect("json should serialize"),
        );

        assert_eq!(
            resolve_error_message(500, None, Some(body_base64.as_str())),
            Some("upstream exploded".to_string()),
        );
    }

    #[test]
    fn decode_body_for_storage_extracts_connect_json_error_frames() {
        let payload = br#"{"error":{"code":"resource_exhausted","message":"quota exhausted"}}"#;
        let mut framed = Vec::new();
        framed.push(2);
        framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        framed.extend_from_slice(payload);
        let body_base64 = base64::engine::general_purpose::STANDARD.encode(framed);

        assert_eq!(
            decode_body_for_storage(Some(body_base64.as_str())),
            Some(json!({
                "error": {
                    "code": "resource_exhausted",
                    "message": "quota exhausted",
                    "type": "resource_exhausted"
                }
            }))
        );
    }

    #[test]
    fn parse_sse_body_for_storage_handles_crlf_and_cr_line_endings() {
        let sse_body = concat!(
            "data: {\"id\":\"chunk-1\"}\r\n",
            "\r\n",
            "data: {\"id\":\"chunk-2\"}\r",
            "\r",
            "data: [DONE]\r",
        );

        assert_eq!(
            parse_sse_body_for_storage(sse_body),
            Some(json!({
                "chunks": [
                    {"id": "chunk-1"},
                    {"id": "chunk-2"}
                ],
                "metadata": {
                    "stream": true,
                    "total_chunks": 2,
                    "stored_chunks": 2,
                    "content_length": sse_body.len(),
                    "has_completion": true
                }
            })),
        );
    }

    #[test]
    fn extract_token_counts_from_value_handles_crlf_and_cr_sse_text() {
        let sse_body = concat!(
            "event: response.created\r\n",
            "data: {\"type\":\"response.created\"}\r\n",
            "\r\n",
            "event: response.completed\r",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":3,\"input_tokens_details\":{\"cached_tokens\":2,\"cached_creation_tokens\":1},\"output_tokens\":5,\"total_tokens\":8}}}\r",
            "\r",
            "data: [DONE]\r",
        );

        assert_eq!(
            extract_token_counts_from_value(&Value::String(sse_body.to_string())),
            Some((3, 5, 8)),
        );
    }
}
