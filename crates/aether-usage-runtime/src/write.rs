use std::collections::BTreeMap;

use aether_contracts::{ExecutionPlan, ExecutionTelemetry};
use aether_data_contracts::repository::usage::UpsertUsageRecord;
use aether_data_contracts::DataLayerError;
use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::{
    map_usage_from_response, GatewayStreamReportRequest, GatewaySyncReportRequest, UsageEvent,
    UsageEventData, UsageEventType,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageLifecycleState {
    Pending,
    Streaming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageTerminalState {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct TerminalUsageOutcome {
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
    pub provider_response_headers: Option<Value>,
    pub provider_response: Option<Value>,
    pub client_response_headers: Option<Value>,
    pub client_response: Option<Value>,
    pub request_metadata: Option<Value>,
    pub audit_payload: Option<Value>,
}

struct TerminalUsageOutcomeBaseInput<'a> {
    plan: &'a ExecutionPlan,
    report_context: Option<&'a Value>,
    terminal_state: UsageTerminalState,
    status_code: u16,
    telemetry: Option<&'a ExecutionTelemetry>,
    provider_response: Option<Value>,
    client_response: Option<Value>,
    provider_response_headers: Option<Value>,
    client_response_headers: Option<Value>,
}

pub fn build_pending_usage_record(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    updated_at_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    build_upsert_usage_record(
        plan,
        report_context,
        UsageEventData::default(),
        UsageLifecycleState::Pending,
        None,
        updated_at_unix_secs,
    )
}

pub fn build_streaming_usage_record(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    status_code: u16,
    response_headers: &BTreeMap<String, String>,
    telemetry: Option<&ExecutionTelemetry>,
    updated_at_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    build_upsert_usage_record(
        plan,
        report_context,
        UsageEventData {
            status_code: Some(status_code),
            response_time_ms: telemetry.and_then(|value| value.elapsed_ms),
            first_byte_time_ms: telemetry.and_then(|value| value.ttfb_ms),
            response_headers: Some(headers_to_json(response_headers)),
            client_response_headers: Some(headers_to_json(response_headers)),
            ..UsageEventData::default()
        },
        UsageLifecycleState::Streaming,
        None,
        updated_at_unix_secs,
    )
}

pub fn build_sync_terminal_usage_event(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    payload: &GatewaySyncReportRequest,
) -> Result<UsageEvent, DataLayerError> {
    build_terminal_usage_event_from_outcome(build_sync_terminal_usage_outcome(
        plan,
        report_context,
        payload,
    ))
}

pub fn build_stream_terminal_usage_event(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    payload: &GatewayStreamReportRequest,
) -> Result<UsageEvent, DataLayerError> {
    build_terminal_usage_event_from_outcome(build_stream_terminal_usage_outcome(
        plan,
        report_context,
        payload,
    ))
}

pub fn build_sync_terminal_usage_outcome(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    payload: &GatewaySyncReportRequest,
) -> TerminalUsageOutcome {
    let provider_response = payload
        .body_json
        .clone()
        .or_else(|| decode_body_for_storage(payload.body_base64.as_deref()));
    let client_response = payload.client_body_json.clone();
    build_terminal_usage_outcome_base(TerminalUsageOutcomeBaseInput {
        plan,
        report_context,
        terminal_state: infer_sync_terminal_state(payload, provider_response.as_ref()),
        status_code: payload.status_code,
        telemetry: payload.telemetry.as_ref(),
        provider_response,
        client_response,
        provider_response_headers: Some(headers_to_json(&payload.headers)),
        client_response_headers: Some(headers_to_json(&payload.headers)),
    })
}

pub fn build_stream_terminal_usage_outcome(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    payload: &GatewayStreamReportRequest,
) -> TerminalUsageOutcome {
    let provider_response = decode_body_for_storage(payload.provider_body_base64.as_deref());
    let client_response = decode_body_for_storage(payload.client_body_base64.as_deref());
    build_terminal_usage_outcome_base(TerminalUsageOutcomeBaseInput {
        plan,
        report_context,
        terminal_state: infer_stream_terminal_state(payload),
        status_code: payload.status_code,
        telemetry: payload.telemetry.as_ref(),
        provider_response,
        client_response,
        provider_response_headers: Some(headers_to_json(&payload.headers)),
        client_response_headers: Some(headers_to_json(&payload.headers)),
    })
}

pub fn build_terminal_usage_event_from_outcome(
    outcome: TerminalUsageOutcome,
) -> Result<UsageEvent, DataLayerError> {
    let event_type = match outcome.terminal_state {
        UsageTerminalState::Completed => UsageEventType::Completed,
        UsageTerminalState::Failed => UsageEventType::Failed,
        UsageTerminalState::Cancelled => UsageEventType::Cancelled,
    };

    let mut data = UsageEventData {
        user_id: outcome.user_id,
        api_key_id: outcome.api_key_id,
        username: outcome.username,
        api_key_name: outcome.api_key_name,
        provider_name: outcome.provider_name,
        model: outcome.model,
        target_model: outcome.target_model,
        provider_id: outcome.provider_id,
        provider_endpoint_id: outcome.provider_endpoint_id,
        provider_api_key_id: outcome.provider_api_key_id,
        request_type: Some(outcome.request_type),
        api_format: Some(outcome.client_contract.clone()),
        api_family: infer_api_family(&outcome.client_contract).map(ToOwned::to_owned),
        endpoint_kind: infer_endpoint_kind(&outcome.client_contract).map(ToOwned::to_owned),
        endpoint_api_format: Some(outcome.provider_contract.clone()),
        provider_api_family: infer_api_family(&outcome.provider_contract).map(ToOwned::to_owned),
        provider_endpoint_kind: infer_endpoint_kind(&outcome.provider_contract)
            .map(ToOwned::to_owned),
        has_format_conversion: Some(outcome.has_format_conversion),
        is_stream: Some(outcome.is_stream),
        status_code: Some(outcome.status_code),
        error_message: resolve_error_message(
            outcome.status_code,
            outcome.provider_response.as_ref(),
            None,
        ),
        error_category: resolve_error_category(outcome.status_code, event_type),
        response_time_ms: outcome.response_time_ms,
        first_byte_time_ms: outcome.first_byte_time_ms,
        request_headers: outcome.request_headers,
        request_body: outcome.request_body,
        provider_request_headers: outcome.provider_request_headers,
        provider_request_body: outcome.provider_request,
        response_headers: outcome.provider_response_headers,
        response_body: outcome.provider_response.clone(),
        client_response_headers: outcome.client_response_headers,
        client_response_body: outcome.client_response.clone(),
        request_metadata: merge_json_value(outcome.request_metadata, outcome.audit_payload),
        ..UsageEventData::default()
    };

    if let Some(response_body) = outcome.provider_response.as_ref() {
        apply_standardized_usage(
            Some(outcome.provider_contract.clone()),
            response_body,
            &mut data,
        );
    }
    if data.total_tokens.is_none() {
        if let Some(tokens) = outcome
            .provider_response
            .as_ref()
            .and_then(extract_token_counts_from_value)
        {
            data.input_tokens = Some(tokens.0);
            data.output_tokens = Some(tokens.1);
            data.total_tokens = Some(tokens.2);
        }
    }

    Ok(UsageEvent::new(event_type, outcome.request_id, data))
}

fn build_terminal_usage_outcome_base(
    input: TerminalUsageOutcomeBaseInput<'_>,
) -> TerminalUsageOutcome {
    let TerminalUsageOutcomeBaseInput {
        plan,
        report_context,
        terminal_state,
        status_code,
        telemetry,
        provider_response,
        client_response,
        provider_response_headers,
        client_response_headers,
    } = input;
    let context = report_context.and_then(Value::as_object);
    let client_contract = context_string(context, "client_contract")
        .or_else(|| context_string(context, "client_api_format"))
        .or_else(|| non_empty_string(Some(plan.client_api_format.clone())))
        .unwrap_or_default();
    let provider_contract = context_string(context, "provider_contract")
        .or_else(|| context_string(context, "provider_api_format"))
        .or_else(|| non_empty_string(Some(plan.provider_api_format.clone())))
        .unwrap_or_default();

    TerminalUsageOutcome {
        status_code,
        terminal_state,
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
        response_time_ms: telemetry.and_then(|value| value.elapsed_ms),
        first_byte_time_ms: telemetry.and_then(|value| value.ttfb_ms),
        request_headers: mask_sensitive_headers_in_json_value(context_value(
            context,
            "original_headers",
        )),
        request_body: context_value(context, "original_request_body")
            .or_else(|| plan.body.json_body.clone()),
        provider_request_headers: mask_sensitive_headers_in_json_value(
            context_value(context, "provider_request_headers")
                .or_else(|| Some(headers_to_json(&plan.headers))),
        ),
        provider_request: context_value(context, "provider_request_body")
            .or_else(|| plan.body.json_body.clone()),
        provider_response_headers,
        provider_response: provider_response.clone(),
        client_response_headers,
        client_response,
        request_metadata: Some(Value::Object(Map::from_iter([
            (
                "request_id".to_string(),
                Value::String(plan.request_id.clone()),
            ),
            (
                "candidate_id".to_string(),
                plan.candidate_id
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            ),
        ]))),
        audit_payload: report_context.cloned(),
    }
}

fn infer_sync_terminal_state(
    payload: &GatewaySyncReportRequest,
    provider_response: Option<&Value>,
) -> UsageTerminalState {
    if payload.status_code == 499 || payload.report_kind.contains("cancel") {
        UsageTerminalState::Cancelled
    } else if payload.status_code >= 400
        || provider_response
            .and_then(|value| value.get("error"))
            .is_some()
    {
        UsageTerminalState::Failed
    } else {
        UsageTerminalState::Completed
    }
}

fn infer_stream_terminal_state(payload: &GatewayStreamReportRequest) -> UsageTerminalState {
    if payload.status_code == 499 || payload.report_kind.contains("cancel") {
        UsageTerminalState::Cancelled
    } else if payload.status_code >= 400 {
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

fn build_upsert_usage_record(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    data: UsageEventData,
    lifecycle_state: UsageLifecycleState,
    finalized_at_unix_secs: Option<u64>,
    updated_at_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    let data = merge_usage_data(build_base_usage_data(plan, report_context), data);
    let (status, billing_status) = lifecycle_status_and_billing(lifecycle_state);

    Ok(UpsertUsageRecord {
        request_id: plan.request_id.clone(),
        user_id: data.user_id,
        api_key_id: data.api_key_id,
        username: data.username,
        api_key_name: data.api_key_name,
        provider_name: data.provider_name,
        model: data.model,
        target_model: data.target_model,
        provider_id: empty_to_none(data.provider_id),
        provider_endpoint_id: empty_to_none(data.provider_endpoint_id),
        provider_api_key_id: empty_to_none(data.provider_api_key_id),
        request_type: data.request_type,
        api_format: data.api_format,
        api_family: data.api_family,
        endpoint_kind: data.endpoint_kind,
        endpoint_api_format: data.endpoint_api_format,
        provider_api_family: data.provider_api_family,
        provider_endpoint_kind: data.provider_endpoint_kind,
        has_format_conversion: data.has_format_conversion,
        is_stream: data.is_stream,
        input_tokens: data.input_tokens,
        output_tokens: data.output_tokens,
        total_tokens: data.total_tokens,
        cache_creation_input_tokens: data.cache_creation_input_tokens,
        cache_read_input_tokens: data.cache_read_input_tokens,
        cache_creation_cost_usd: data.cache_creation_cost_usd,
        cache_read_cost_usd: data.cache_read_cost_usd,
        output_price_per_1m: data.output_price_per_1m,
        total_cost_usd: data.total_cost_usd,
        actual_total_cost_usd: data.actual_total_cost_usd,
        status_code: data.status_code,
        error_message: data.error_message,
        error_category: data.error_category,
        response_time_ms: data.response_time_ms,
        first_byte_time_ms: data.first_byte_time_ms,
        status: status.to_string(),
        billing_status: billing_status.to_string(),
        request_headers: data.request_headers,
        request_body: data.request_body,
        provider_request_headers: data.provider_request_headers,
        provider_request_body: data.provider_request_body,
        response_headers: data.response_headers,
        response_body: data.response_body,
        client_response_headers: data.client_response_headers,
        client_response_body: data.client_response_body,
        request_metadata: data.request_metadata,
        finalized_at_unix_secs,
        created_at_unix_secs: Some(updated_at_unix_secs),
        updated_at_unix_secs,
    })
}

fn build_base_usage_data(plan: &ExecutionPlan, report_context: Option<&Value>) -> UsageEventData {
    let context = report_context.and_then(Value::as_object);
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
    let mut request_metadata = Map::from_iter([
        (
            "request_id".to_string(),
            Value::String(plan.request_id.clone()),
        ),
        (
            "candidate_id".to_string(),
            plan.candidate_id
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
    ]);
    if let Some(key_name) = context_string(context, "key_name") {
        request_metadata.insert("key_name".to_string(), Value::String(key_name));
    }

    UsageEventData {
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
        request_headers: context_value(context, "original_headers"),
        request_body: context_value(context, "original_request_body")
            .or_else(|| plan.body.json_body.clone()),
        provider_request_headers: context_value(context, "provider_request_headers")
            .or_else(|| Some(headers_to_json(&plan.headers))),
        provider_request_body: context_value(context, "provider_request_body")
            .or_else(|| plan.body.json_body.clone()),
        request_metadata: Some(Value::Object(request_metadata)),
        ..UsageEventData::default()
    }
}

fn merge_usage_data(base: UsageEventData, override_data: UsageEventData) -> UsageEventData {
    UsageEventData {
        user_id: override_data.user_id.or(base.user_id),
        api_key_id: override_data.api_key_id.or(base.api_key_id),
        username: override_data.username.or(base.username),
        api_key_name: override_data.api_key_name.or(base.api_key_name),
        provider_name: if override_data.provider_name.trim().is_empty() {
            base.provider_name
        } else {
            override_data.provider_name
        },
        model: if override_data.model.trim().is_empty() {
            base.model
        } else {
            override_data.model
        },
        target_model: override_data.target_model.or(base.target_model),
        provider_id: override_data.provider_id.or(base.provider_id),
        provider_endpoint_id: override_data
            .provider_endpoint_id
            .or(base.provider_endpoint_id),
        provider_api_key_id: override_data
            .provider_api_key_id
            .or(base.provider_api_key_id),
        request_type: override_data.request_type.or(base.request_type),
        api_format: override_data.api_format.or(base.api_format),
        api_family: override_data.api_family.or(base.api_family),
        endpoint_kind: override_data.endpoint_kind.or(base.endpoint_kind),
        endpoint_api_format: override_data
            .endpoint_api_format
            .or(base.endpoint_api_format),
        provider_api_family: override_data
            .provider_api_family
            .or(base.provider_api_family),
        provider_endpoint_kind: override_data
            .provider_endpoint_kind
            .or(base.provider_endpoint_kind),
        has_format_conversion: override_data
            .has_format_conversion
            .or(base.has_format_conversion),
        is_stream: override_data.is_stream.or(base.is_stream),
        input_tokens: override_data.input_tokens.or(base.input_tokens),
        output_tokens: override_data.output_tokens.or(base.output_tokens),
        total_tokens: override_data.total_tokens.or(base.total_tokens),
        cache_creation_input_tokens: override_data
            .cache_creation_input_tokens
            .or(base.cache_creation_input_tokens),
        cache_read_input_tokens: override_data
            .cache_read_input_tokens
            .or(base.cache_read_input_tokens),
        cache_creation_cost_usd: override_data
            .cache_creation_cost_usd
            .or(base.cache_creation_cost_usd),
        cache_read_cost_usd: override_data
            .cache_read_cost_usd
            .or(base.cache_read_cost_usd),
        output_price_per_1m: override_data
            .output_price_per_1m
            .or(base.output_price_per_1m),
        total_cost_usd: override_data.total_cost_usd.or(base.total_cost_usd),
        actual_total_cost_usd: override_data
            .actual_total_cost_usd
            .or(base.actual_total_cost_usd),
        status_code: override_data.status_code.or(base.status_code),
        error_message: override_data.error_message.or(base.error_message),
        error_category: override_data.error_category.or(base.error_category),
        response_time_ms: override_data.response_time_ms.or(base.response_time_ms),
        first_byte_time_ms: override_data.first_byte_time_ms.or(base.first_byte_time_ms),
        request_headers: override_data.request_headers.or(base.request_headers),
        request_body: override_data.request_body.or(base.request_body),
        provider_request_headers: override_data
            .provider_request_headers
            .or(base.provider_request_headers),
        provider_request_body: override_data
            .provider_request_body
            .or(base.provider_request_body),
        response_headers: override_data.response_headers.or(base.response_headers),
        response_body: override_data.response_body.or(base.response_body),
        client_response_headers: override_data
            .client_response_headers
            .or(base.client_response_headers),
        client_response_body: override_data
            .client_response_body
            .or(base.client_response_body),
        request_metadata: merge_json_value(base.request_metadata, override_data.request_metadata),
    }
}

fn merge_json_value(base: Option<Value>, override_value: Option<Value>) -> Option<Value> {
    match (base, override_value) {
        (Some(Value::Object(mut base)), Some(Value::Object(override_object))) => {
            for (key, value) in override_object {
                base.insert(key, value);
            }
            Some(Value::Object(base))
        }
        (Some(base), None) => Some(base),
        (_, Some(override_value)) => Some(override_value),
        (None, None) => None,
    }
}

fn lifecycle_status_and_billing(state: UsageLifecycleState) -> (&'static str, &'static str) {
    match state {
        UsageLifecycleState::Pending => ("pending", "pending"),
        UsageLifecycleState::Streaming => ("streaming", "pending"),
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

fn context_bool(context: Option<&Map<String, Value>>, key: &str) -> Option<bool> {
    context
        .and_then(|value| value.get(key))
        .and_then(Value::as_bool)
}

fn context_value(context: Option<&Map<String, Value>>, key: &str) -> Option<Value> {
    context.and_then(|value| value.get(key)).cloned()
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
    DEFAULT_SENSITIVE_HEADERS
        .iter()
        .any(|candidate| *candidate == lower.as_str())
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
        build_stream_terminal_usage_event, build_sync_terminal_usage_event,
        extract_token_counts_from_json, headers_to_json, mask_header_value,
        mask_sensitive_headers_in_json_value,
    };
    use crate::{
        build_upsert_usage_record_from_event, GatewayStreamReportRequest, GatewaySyncReportRequest,
        UsageEvent, UsageEventData, UsageEventType,
    };
    use aether_contracts::{ExecutionPlan, RequestBody};
    use base64::Engine as _;
    use serde_json::json;
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
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello from CLI stream\"}]}],\"usage\":{\"input_tokens\":3,\"output_tokens\":5,\"total_tokens\":8}}}\n\n",
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
                                "output_tokens": 5,
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
