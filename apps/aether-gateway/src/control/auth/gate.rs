use axum::body::Bytes;
use axum::http::Uri;

use super::super::GatewayControlDecision;
use super::credentials::{contains_string, extract_requested_model};
use super::GatewayControlAuthContext;
use crate::{AppState, GatewayError};

const DAILY_QUOTA_EPSILON_USD: f64 = 0.000_000_01;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GatewayLocalAuthRejection {
    InvalidApiKey,
    LockedApiKey,
    WalletUnavailable,
    BalanceDenied { remaining: Option<f64> },
    ProviderNotAllowed { provider: String },
    ApiFormatNotAllowed { api_format: String },
    ModelNotAllowed { model: String },
    IpNotAllowed { remote_ip: String },
}

pub(crate) fn trusted_auth_local_rejection(
    decision: Option<&GatewayControlDecision>,
    _headers: &http::HeaderMap,
) -> Option<GatewayLocalAuthRejection> {
    let decision = decision?;
    if decision.route_class.as_deref() != Some("ai_public") {
        return None;
    }

    decision
        .local_auth_rejection
        .clone()
        .or_else(|| decision.auth_context.as_ref()?.local_rejection.clone())
}

pub(crate) fn should_buffer_request_for_local_auth(
    decision: Option<&GatewayControlDecision>,
    headers: &http::HeaderMap,
) -> bool {
    let Some(decision) = decision else {
        return false;
    };
    decision.route_class.as_deref() == Some("ai_public")
        && decision.route_kind.as_deref() != Some("files")
        && crate::headers::is_json_request(headers)
}

pub(crate) async fn request_model_local_rejection(
    state: &AppState,
    decision: Option<&GatewayControlDecision>,
    uri: &Uri,
    headers: &http::HeaderMap,
    body: &Bytes,
) -> Result<Option<GatewayLocalAuthRejection>, GatewayError> {
    let Some(decision) = decision else {
        return Ok(None);
    };
    if decision.route_class.as_deref() != Some("ai_public") {
        return Ok(None);
    }
    let Some(auth_context) = decision.auth_context.as_ref() else {
        return Ok(None);
    };
    let requested_model = extract_requested_model(decision, uri, headers, body);
    if let (Some(allowed_models), Some(requested_model)) = (
        auth_context.allowed_models.as_deref(),
        requested_model.as_deref(),
    ) {
        if !contains_string(allowed_models, requested_model)
            && !model_directive_base_model_is_allowed_for_request(
                decision,
                requested_model,
                allowed_models,
            )
            && !request_model_resolves_to_allowed_model(
                state,
                decision,
                requested_model,
                allowed_models,
            )
            .await?
        {
            return Ok(Some(GatewayLocalAuthRejection::ModelNotAllowed {
                model: requested_model.to_string(),
            }));
        }
    }

    Ok(None)
}

pub(crate) async fn execution_plan_balance_capacity_rejection(
    state: &AppState,
    decision: &GatewayControlDecision,
    plan: &aether_contracts::ExecutionPlan,
    report_context: Option<&serde_json::Value>,
) -> Result<Option<GatewayLocalAuthRejection>, GatewayError> {
    let Some(auth_context) = decision.auth_context.as_ref() else {
        return Ok(None);
    };
    if auth_context.local_rejection.is_some() {
        return Ok(None);
    }
    if auth_context.api_key_is_standalone {
        validate_execution_plan_pricing_configuration_for_plan(state, plan, report_context).await?;
        return Ok(None);
    }
    let Some(available_usd) = available_balance_capacity_usd(state, auth_context).await? else {
        validate_execution_plan_pricing_configuration_for_plan(state, plan, report_context).await?;
        return Ok(None);
    };
    match estimate_execution_plan_cost_upper_bound_usd(state, plan, report_context).await? {
        Some(estimated_cost_usd)
            if estimated_cost_usd <= available_usd + DAILY_QUOTA_EPSILON_USD =>
        {
            Ok(None)
        }
        Some(_) | None if available_usd <= DAILY_QUOTA_EPSILON_USD => {
            Ok(Some(GatewayLocalAuthRejection::BalanceDenied {
                remaining: Some(0.0),
            }))
        }
        Some(_) => Ok(Some(GatewayLocalAuthRejection::BalanceDenied {
            remaining: Some(available_usd),
        })),
        None => Ok(None),
    }
}

async fn validate_execution_plan_pricing_configuration_for_plan(
    state: &AppState,
    plan: &aether_contracts::ExecutionPlan,
    report_context: Option<&serde_json::Value>,
) -> Result<(), GatewayError> {
    let model_id = report_context_string_field(report_context, "model_id");
    let global_model_name = report_context_string_field(report_context, "global_model_name");
    let requested_processing_tier =
        aether_data_contracts::repository::usage::extract_provider_service_tier_from_body(
            plan.body.json_body.as_ref(),
        );
    validate_execution_plan_pricing_for_unavailable_estimate(
        state,
        plan,
        model_id,
        global_model_name,
        requested_processing_tier.as_deref(),
    )
    .await
}

async fn available_balance_capacity_usd(
    state: &AppState,
    auth_context: &GatewayControlAuthContext,
) -> Result<Option<f64>, GatewayError> {
    let quota = state
        .find_user_daily_quota_availability_for_auth(&auth_context.user_id)
        .await?
        .filter(|quota| quota.has_active_daily_quota);
    let wallet = state
        .read_wallet_snapshot_for_auth(
            &auth_context.user_id,
            &auth_context.api_key_id,
            auth_context.api_key_is_standalone,
        )
        .await?;
    let wallet_available_usd = wallet.as_ref().and_then(wallet_finite_available_usd);
    let wallet_is_unlimited = wallet
        .as_ref()
        .is_some_and(|wallet| wallet.limit_mode.eq_ignore_ascii_case("unlimited"));
    Ok(match quota.as_ref() {
        Some(quota) if !quota.allow_wallet_overage => Some(quota.remaining_usd.max(0.0)),
        Some(_) if wallet_is_unlimited => None,
        Some(quota) => Some(quota.remaining_usd.max(0.0) + wallet_available_usd.unwrap_or(0.0)),
        None if wallet_is_unlimited => None,
        None => wallet_available_usd,
    })
}

fn wallet_finite_available_usd(
    wallet: &aether_data::repository::wallet::StoredWalletSnapshot,
) -> Option<f64> {
    if !wallet.status.eq_ignore_ascii_case("active")
        || wallet.limit_mode.eq_ignore_ascii_case("unlimited")
    {
        return None;
    }
    Some(wallet.balance.max(0.0) + wallet.gift_balance.max(0.0))
}

async fn estimate_execution_plan_cost_upper_bound_usd(
    state: &AppState,
    plan: &aether_contracts::ExecutionPlan,
    report_context: Option<&serde_json::Value>,
) -> Result<Option<f64>, GatewayError> {
    let api_format = crate::ai_serving::normalize_api_format_alias(&plan.provider_api_format);
    let body_json = plan.body.json_body.as_ref();
    let requested_processing_tier =
        aether_data_contracts::repository::usage::extract_provider_service_tier_from_body(
            body_json,
        );
    let model_id = report_context_string_field(report_context, "model_id");
    let global_model_name = report_context_string_field(report_context, "global_model_name");
    let Some(task_type) = authorization_task_type(&api_format, report_context) else {
        validate_execution_plan_pricing_for_unavailable_estimate(
            state,
            plan,
            model_id,
            global_model_name,
            requested_processing_tier.as_deref(),
        )
        .await?;
        return Ok(None);
    };
    let Some(body_json) = body_json else {
        validate_execution_plan_pricing_for_unavailable_estimate(
            state,
            plan,
            model_id,
            global_model_name,
            requested_processing_tier.as_deref(),
        )
        .await?;
        return Ok(None);
    };
    if !openai_request_input_is_self_contained(&api_format, body_json) {
        validate_execution_plan_pricing_for_unavailable_estimate(
            state,
            plan,
            model_id,
            global_model_name,
            requested_processing_tier.as_deref(),
        )
        .await?;
        return Ok(None);
    }
    let input_tokens = json_token_count_upper_bound(body_json);
    let Ok(input_tokens) = i64::try_from(input_tokens) else {
        validate_execution_plan_pricing_for_unavailable_estimate(
            state,
            plan,
            model_id,
            global_model_name,
            requested_processing_tier.as_deref(),
        )
        .await?;
        return Ok(None);
    };
    let max_output_tokens = max_output_tokens_from_request(body_json)
        .map(|value| value.saturating_mul(output_choice_count_upper_bound(&api_format, body_json)))
        .and_then(|value| i64::try_from(value).ok());
    let cache_ttl_minutes =
        aether_data_contracts::repository::usage::resolve_provider_cache_ttl_minutes(
            Some(&api_format),
            plan.model_name.as_deref(),
            global_model_name,
            Some(body_json),
        );
    if model_id.is_none() && global_model_name.is_none() {
        return Ok(None);
    }
    let cache_key = execution_plan_cost_upper_bound_cache_key(
        plan,
        model_id,
        global_model_name,
        &api_format,
        input_tokens,
        max_output_tokens,
        requested_processing_tier.as_deref(),
        cache_ttl_minutes,
    );
    let ttl = state.frontdoor_runtime_guards.auth_capacity_cache_ttl;
    if ttl.is_zero() {
        let _permit = state.acquire_auth_snapshot_load_gate().await?;
        return calculate_execution_plan_cost_upper_bound(
            state,
            plan,
            model_id,
            global_model_name,
            &api_format,
            task_type,
            input_tokens,
            max_output_tokens,
            requested_processing_tier.as_deref(),
            cache_ttl_minutes,
        )
        .await;
    }
    state
        .auth_request_cost_upper_bound_cache
        .get_or_load(cache_key, ttl, || async {
            let _permit = state.acquire_auth_snapshot_load_gate().await?;
            calculate_execution_plan_cost_upper_bound(
                state,
                plan,
                model_id,
                global_model_name,
                &api_format,
                task_type,
                input_tokens,
                max_output_tokens,
                requested_processing_tier.as_deref(),
                cache_ttl_minutes,
            )
            .await
        })
        .await
}

#[allow(clippy::too_many_arguments)]
async fn calculate_execution_plan_cost_upper_bound(
    state: &AppState,
    plan: &aether_contracts::ExecutionPlan,
    model_id: Option<&str>,
    global_model_name: Option<&str>,
    api_format: &str,
    task_type: &str,
    input_tokens: i64,
    max_output_tokens: Option<i64>,
    requested_processing_tier: Option<&str>,
    cache_ttl_minutes: Option<i64>,
) -> Result<Option<f64>, GatewayError> {
    let context =
        load_execution_plan_billing_context(state, plan, model_id, global_model_name).await?;
    let Some(context) = context else {
        return Ok(None);
    };
    let mut estimate =
        aether_billing::BillingAuthorizationEstimateInput::new(task_type, input_tokens);
    estimate.api_format = Some(api_format.to_string());
    estimate.requested_processing_tier = requested_processing_tier.map(ToOwned::to_owned);
    estimate.cache_ttl_minutes = cache_ttl_minutes;
    estimate.max_output_tokens = max_output_tokens;
    aether_billing::BillingService::new()
        .estimate_authorization_cost_upper_bound(
            &aether_billing::BillingModelPricingSnapshot::from(context),
            &estimate,
        )
        .map_err(|err| GatewayError::Internal(err.to_string()))
}

async fn validate_execution_plan_pricing_for_unavailable_estimate(
    state: &AppState,
    plan: &aether_contracts::ExecutionPlan,
    model_id: Option<&str>,
    global_model_name: Option<&str>,
    requested_processing_tier: Option<&str>,
) -> Result<(), GatewayError> {
    if model_id.is_none() && global_model_name.is_none() {
        return Ok(());
    }
    let _permit = state.acquire_auth_snapshot_load_gate().await?;
    let Some(context) =
        load_execution_plan_billing_context(state, plan, model_id, global_model_name).await?
    else {
        return Ok(());
    };
    aether_billing::BillingModelPricingSnapshot::from(context)
        .validate_authorization_pricing_configuration(requested_processing_tier)
        .map_err(|err| GatewayError::Internal(err.to_string()))
}

async fn load_execution_plan_billing_context(
    state: &AppState,
    plan: &aether_contracts::ExecutionPlan,
    model_id: Option<&str>,
    global_model_name: Option<&str>,
) -> Result<
    Option<aether_data_contracts::repository::billing::StoredBillingModelContext>,
    GatewayError,
> {
    let context = match model_id {
        Some(model_id) => state
            .data
            .find_billing_model_context_by_model_id(&plan.provider_id, Some(&plan.key_id), model_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?,
        None => state
            .data
            .find_billing_model_context(
                &plan.provider_id,
                Some(&plan.key_id),
                global_model_name.expect("global model name should exist"),
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?,
    };
    Ok(context)
}

fn execution_plan_cost_upper_bound_cache_key(
    plan: &aether_contracts::ExecutionPlan,
    model_id: Option<&str>,
    global_model_name: Option<&str>,
    api_format: &str,
    input_tokens: i64,
    max_output_tokens: Option<i64>,
    requested_processing_tier: Option<&str>,
    cache_ttl_minutes: Option<i64>,
) -> String {
    format!(
        "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
        plan.provider_id,
        plan.key_id,
        model_id.unwrap_or(""),
        global_model_name.unwrap_or(""),
        api_format,
        input_tokens,
        max_output_tokens
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
        requested_processing_tier.unwrap_or("standard"),
        cache_ttl_minutes
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
    )
}

fn authorization_task_type<'a>(
    api_format: &str,
    report_context: Option<&'a serde_json::Value>,
) -> Option<&'a str> {
    if report_context
        .and_then(|context| context.get("image_request"))
        .is_some()
        || api_format == "openai:image"
    {
        return None;
    }
    if api_format.ends_with(":embedding") {
        return Some("embedding");
    }
    if api_format.ends_with(":rerank") {
        return Some("rerank");
    }
    Some("chat")
}

fn report_context_string_field<'a>(
    report_context: Option<&'a serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    report_context
        .and_then(|context| context.get(key))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn max_output_tokens_from_request(value: &serde_json::Value) -> Option<u64> {
    ["max_tokens", "max_completion_tokens", "max_output_tokens"]
        .iter()
        .filter_map(|field| value.get(*field).and_then(serde_json::Value::as_u64))
        .filter(|value| *value > 0)
        .max()
}

fn output_choice_count_upper_bound(api_format: &str, value: &serde_json::Value) -> u64 {
    if api_format != "openai:chat" {
        return 1;
    }
    value
        .get("n")
        .and_then(serde_json::Value::as_u64)
        .filter(|value| *value > 0)
        .unwrap_or(1)
}

fn openai_request_input_is_self_contained(api_format: &str, value: &serde_json::Value) -> bool {
    if !api_format.starts_with("openai:") {
        return false;
    }
    let Some(object) = value.as_object() else {
        return true;
    };
    if ["previous_response_id", "conversation"]
        .iter()
        .any(|key| object.get(*key).is_some_and(has_reference_value))
    {
        return false;
    }
    if object
        .get("prompt")
        .and_then(serde_json::Value::as_object)
        .and_then(|prompt| prompt.get("id"))
        .is_some_and(has_reference_value)
    {
        return false;
    }
    !contains_indirect_request_input(value)
}

fn contains_indirect_request_input(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => items.iter().any(contains_indirect_request_input),
        serde_json::Value::Object(object) => {
            let item_type = object
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(|value| value.trim().to_ascii_lowercase());
            if item_type.as_deref().is_some_and(|item_type| {
                matches!(
                    item_type,
                    "url"
                        | "item_reference"
                        | "input_file"
                        | "input_image"
                        | "input_audio"
                        | "image_url"
                        | "file_search"
                        | "web_search"
                        | "web_search_preview"
                        | "computer_use"
                        | "computer_use_preview"
                        | "code_interpreter"
                        | "mcp"
                        | "image_generation"
                )
            }) {
                return true;
            }
            if ["file_id", "file_uri", "fileUri", "vector_store_ids"]
                .iter()
                .any(|key| object.get(*key).is_some_and(has_reference_value))
            {
                return true;
            }
            object.values().any(contains_indirect_request_input)
        }
        _ => false,
    }
}

fn has_reference_value(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::String(value) => !value.trim().is_empty(),
        serde_json::Value::Array(values) => !values.is_empty(),
        serde_json::Value::Object(values) => !values.is_empty(),
        _ => true,
    }
}

fn json_token_count_upper_bound(value: &serde_json::Value) -> u64 {
    serde_json::to_vec(value)
        .map(|bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
        .unwrap_or(u64::MAX)
}

fn model_directive_base_model_is_allowed_for_request(
    decision: &GatewayControlDecision,
    requested_model: &str,
    allowed_models: &[String],
) -> bool {
    let Some(client_api_format) = decision
        .auth_endpoint_signature
        .as_deref()
        .map(crate::ai_serving::normalize_api_format_alias)
        .filter(|value| !value.trim().is_empty())
    else {
        return false;
    };
    for api_format in candidate_api_formats_for_model_resolution(&client_api_format) {
        let resolution = decision
            .model_directive_policy
            .resolve_reasoning(&api_format, Some(requested_model));
        if resolution
            .base_model()
            .is_some_and(|base_model| contains_string(allowed_models, base_model))
        {
            return true;
        }
    }
    false
}

async fn request_model_resolves_to_allowed_model(
    state: &AppState,
    decision: &GatewayControlDecision,
    requested_model: &str,
    allowed_models: &[String],
) -> Result<bool, GatewayError> {
    let Some(client_api_format) = decision
        .auth_endpoint_signature
        .as_deref()
        .map(crate::ai_serving::normalize_api_format_alias)
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(false);
    };

    for api_format in candidate_api_formats_for_model_resolution(&client_api_format) {
        let resolution = decision
            .model_directive_policy
            .resolve_reasoning(&api_format, Some(requested_model));
        let routing_model = resolution.base_model().unwrap_or(requested_model);
        let rows = {
            // Model alias authorization runs before candidate planning, so its database read must
            // participate in the same foreground DB admission budget as the rest of auth. Keep
            // the permit scoped to this one read; callers do not hold this gate, and releasing it
            // here avoids carrying a DB permit through pure filtering or subsequent formats.
            let _permit = state.acquire_auth_snapshot_load_gate().await?;
            state
                .list_minimal_candidate_selection_rows_for_api_format(&api_format)
                .await?
        };
        let matching_rows = rows
            .into_iter()
            .filter(|row| {
                aether_scheduler_core::row_supports_requested_model_with_model_directives(
                    row,
                    routing_model,
                    &api_format,
                    false,
                )
            })
            .collect::<Vec<_>>();
        let Some(resolved_global_model) =
            aether_scheduler_core::resolve_requested_global_model_name_with_model_directives(
                &matching_rows,
                routing_model,
                &api_format,
                false,
            )
        else {
            continue;
        };
        if contains_string(allowed_models, &resolved_global_model) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn candidate_api_formats_for_model_resolution(client_api_format: &str) -> Vec<String> {
    let mut api_formats = Vec::new();
    push_unique_api_format(&mut api_formats, client_api_format);
    for api_format in crate::ai_serving::request_candidate_api_formats(client_api_format, false) {
        push_unique_api_format(&mut api_formats, api_format);
    }
    api_formats
}

fn push_unique_api_format(api_formats: &mut Vec<String>, api_format: &str) {
    let api_format = crate::ai_serving::normalize_api_format_alias(api_format);
    if api_format.is_empty() || api_formats.iter().any(|value| value == &api_format) {
        return;
    }
    api_formats.push(api_format);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use aether_contracts::{ExecutionPlan, RequestBody};
    use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
    use aether_data::repository::wallet::StoredWalletSnapshot;
    use aether_data_contracts::repository::billing::{
        BillingReadRepository, StoredBillingModelContext, UserDailyQuotaAvailabilityRecord,
    };
    use aether_data_contracts::repository::candidate_selection::{
        StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
    };
    use aether_data_contracts::DataLayerError;
    use aether_runtime::ConcurrencyGate;
    use async_trait::async_trait;
    use axum::body::Bytes;
    use axum::http::{HeaderMap, Uri};
    use serde_json::json;

    use super::{
        execution_plan_balance_capacity_rejection, execution_plan_cost_upper_bound_cache_key,
        max_output_tokens_from_request, openai_request_input_is_self_contained,
        output_choice_count_upper_bound, request_model_local_rejection, GatewayLocalAuthRejection,
    };
    use crate::control::{GatewayControlAuthContext, GatewayControlDecision};
    use crate::data::GatewayDataState;
    use crate::AppState;

    fn sample_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-1".to_string(),
            provider_name: "Provider 1".to_string(),
            provider_type: "openai".to_string(),
            provider_priority: 0,
            provider_is_active: true,
            endpoint_id: "endpoint-1".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: "key-1".to_string(),
            key_name: "key".to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 0,
            key_global_priority_by_format: None,
            model_id: "model-1".to_string(),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: Some(vec!["gpt-5(?:\\.\\d+)?".to_string()]),
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-5-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gpt-5-upstream".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:chat".to_string()]),
                endpoint_ids: None,
                operations: None,
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_row_for_api_format(api_format: &str) -> StoredMinimalCandidateSelectionRow {
        let mut row = sample_row();
        let api_family = api_format
            .split_once(':')
            .map(|(family, _)| family)
            .unwrap_or(api_format);
        row.provider_id = format!("provider-{api_family}");
        row.provider_name = format!("Provider {api_family}");
        row.provider_type = api_family.to_string();
        row.endpoint_id = format!("endpoint-{api_family}");
        row.endpoint_api_format = api_format.to_string();
        row.endpoint_api_family = Some(api_family.to_string());
        row.key_id = format!("key-{api_family}");
        row.key_api_formats = Some(vec![api_format.to_string()]);
        if let Some(mappings) = row.model_provider_model_mappings.as_mut() {
            for mapping in mappings {
                mapping.api_formats = Some(vec![api_format.to_string()]);
            }
        }
        row
    }

    fn decision_with_allowed_models(allowed_models: Vec<String>) -> GatewayControlDecision {
        let mut decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        );
        decision.auth_context = Some(GatewayControlAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "api-key-1".to_string(),
            username: None,
            api_key_name: None,
            balance_remaining: None,
            access_allowed: true,
            user_rate_limit: None,
            api_key_rate_limit: None,
            api_key_is_standalone: false,
            admin_bypass_limits: false,
            local_rejection: None,
            allowed_models: Some(allowed_models),
            ip_rules: None,
        });
        decision
    }

    fn state_with_rows(rows: Vec<StoredMinimalCandidateSelectionRow>) -> AppState {
        let repository = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(rows));
        let data = GatewayDataState::with_minimal_candidate_selection_reader_for_tests(repository);
        AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data)
    }

    fn state_with_quota_and_wallet(
        quota: UserDailyQuotaAvailabilityRecord,
        context: StoredBillingModelContext,
    ) -> AppState {
        let candidate_repository =
            Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
                sample_row(),
            ]));
        let billing_repository = Arc::new(FixedBillingReadRepository::new(quota, context));
        let data = GatewayDataState::with_minimal_candidate_selection_and_billing_for_tests(
            candidate_repository,
            billing_repository,
        );
        AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data)
            .with_auth_wallets_for_tests(vec![sample_wallet("user-1", 30.0)])
    }

    fn state_with_model_mapping() -> AppState {
        state_with_rows(vec![sample_row()])
    }

    fn execution_plan(body: serde_json::Value, api_format: &str) -> ExecutionPlan {
        ExecutionPlan {
            request_id: "request-1".to_string(),
            candidate_id: Some("candidate-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://api.openai.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(body),
            stream: false,
            client_api_format: api_format.to_string(),
            provider_api_format: api_format.to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn billing_report_context() -> serde_json::Value {
        json!({
            "model_id": "model-1",
            "global_model_name": "gpt-5"
        })
    }

    fn estimate_from_billing_context(
        context: &StoredBillingModelContext,
        api_format: &str,
        input_tokens: i64,
        max_output_tokens: Option<i64>,
    ) -> Option<f64> {
        let mut estimate =
            aether_billing::BillingAuthorizationEstimateInput::new("chat", input_tokens);
        estimate.api_format = Some(api_format.to_string());
        estimate.max_output_tokens = max_output_tokens;
        aether_billing::BillingService::new()
            .estimate_authorization_cost_upper_bound(
                &aether_billing::BillingModelPricingSnapshot::from(context),
                &estimate,
            )
            .expect("estimate should calculate")
    }

    fn json_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            "application/json"
                .parse()
                .expect("content type should parse"),
        );
        headers
    }

    fn billing_context_with_pricing(
        default_tiered_pricing: Option<serde_json::Value>,
        model_tiered_pricing: Option<serde_json::Value>,
        rate_multipliers: Option<serde_json::Value>,
        billing_type: Option<&str>,
    ) -> StoredBillingModelContext {
        StoredBillingModelContext::new(
            "provider-1".to_string(),
            billing_type.map(ToOwned::to_owned),
            Some("key-1".to_string()),
            rate_multipliers,
            Some(60),
            "global-model-1".to_string(),
            "gpt-5".to_string(),
            None,
            None,
            default_tiered_pricing,
            Some("model-1".to_string()),
            Some("gpt-5-upstream".to_string()),
            None,
            None,
            model_tiered_pricing,
        )
        .expect("billing context should build")
    }

    fn sample_wallet(user_id: &str, balance: f64) -> StoredWalletSnapshot {
        StoredWalletSnapshot::new(
            format!("wallet-{user_id}"),
            Some(user_id.to_string()),
            None,
            balance,
            0.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            balance,
            0.0,
            0.0,
            0.0,
            100,
        )
        .expect("wallet should build")
    }

    fn quota_availability(
        remaining_usd: f64,
        allow_wallet_overage: bool,
    ) -> UserDailyQuotaAvailabilityRecord {
        UserDailyQuotaAvailabilityRecord {
            has_active_daily_quota: true,
            total_quota_usd: remaining_usd,
            used_usd: 0.0,
            remaining_usd,
            allow_wallet_overage,
        }
    }

    #[derive(Debug)]
    struct FixedBillingReadRepository {
        quota: UserDailyQuotaAvailabilityRecord,
        context: StoredBillingModelContext,
        quota_calls: Arc<AtomicUsize>,
        model_context_by_model_id_calls: Arc<AtomicUsize>,
    }

    impl FixedBillingReadRepository {
        fn new(
            quota: UserDailyQuotaAvailabilityRecord,
            context: StoredBillingModelContext,
        ) -> Self {
            Self {
                quota,
                context,
                quota_calls: Arc::new(AtomicUsize::new(0)),
                model_context_by_model_id_calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_counters(
            quota: UserDailyQuotaAvailabilityRecord,
            context: StoredBillingModelContext,
            quota_calls: Arc<AtomicUsize>,
            model_context_by_model_id_calls: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                quota,
                context,
                quota_calls,
                model_context_by_model_id_calls,
            }
        }
    }

    #[async_trait]
    impl BillingReadRepository for FixedBillingReadRepository {
        async fn find_model_context(
            &self,
            _provider_id: &str,
            _provider_api_key_id: Option<&str>,
            _global_model_name: &str,
        ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
            Ok(Some(self.context.clone()))
        }

        async fn find_model_context_by_model_id(
            &self,
            _provider_id: &str,
            _provider_api_key_id: Option<&str>,
            _model_id: &str,
        ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
            self.model_context_by_model_id_calls
                .fetch_add(1, Ordering::AcqRel);
            Ok(Some(self.context.clone()))
        }

        async fn find_user_daily_quota_availability(
            &self,
            _user_id: &str,
        ) -> Result<Option<UserDailyQuotaAvailabilityRecord>, DataLayerError> {
            self.quota_calls.fetch_add(1, Ordering::AcqRel);
            Ok(Some(self.quota.clone()))
        }
    }

    #[tokio::test]
    async fn model_rejection_allows_requested_model_that_resolves_to_allowed_global_model() {
        let state = state_with_model_mapping();
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body = Bytes::from_static(br#"{"model":"gpt-5.2","messages":[]}"#);

        let rejection =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
                .await
                .expect("model rejection should resolve");

        assert_eq!(rejection, None);
    }

    #[tokio::test]
    async fn model_alias_resolution_waits_for_auth_database_gate() {
        let mut state = state_with_model_mapping();
        state.auth_snapshot_load_gate = Some(Arc::new(ConcurrencyGate::new(
            "test_auth_model_resolution",
            1,
        )));
        let held = state
            .acquire_auth_snapshot_load_gate()
            .await
            .expect("auth gate acquisition should succeed")
            .expect("auth gate should be configured");
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let headers = json_headers();
        let body = Bytes::from_static(br#"{"model":"gpt-5.2","messages":[]}"#);

        let blocked = tokio::time::timeout(
            Duration::from_millis(25),
            request_model_local_rejection(&state, Some(&decision), &uri, &headers, &body),
        )
        .await;
        assert!(
            blocked.is_err(),
            "model alias candidate reads must wait for the auth DB gate"
        );

        drop(held);
        let rejection = tokio::time::timeout(
            Duration::from_secs(1),
            request_model_local_rejection(&state, Some(&decision), &uri, &headers, &body),
        )
        .await
        .expect("model alias resolution should resume after releasing the auth gate")
        .expect("model rejection should resolve");
        assert_eq!(rejection, None);
    }

    #[tokio::test]
    async fn model_rejection_allows_cross_format_provider_mapping_to_allowed_global_model() {
        let mut row = sample_row_for_api_format("gemini:generate_content");
        row.model_provider_model_name = "gemini-2.5-pro-upstream".to_string();
        row.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
            name: "gemini-2.5-pro-alias".to_string(),
            priority: 1,
            api_formats: Some(vec!["gemini:generate_content".to_string()]),
            endpoint_ids: None,
            operations: None,
        }]);
        let state = state_with_rows(vec![row]);
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body = Bytes::from_static(br#"{"model":"gemini-2.5-pro-alias","messages":[]}"#);

        let rejection =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
                .await
                .expect("model rejection should resolve");

        assert_eq!(rejection, None);
    }

    #[tokio::test]
    async fn model_rejection_allows_cross_format_regex_mapping_to_allowed_global_model() {
        let state = state_with_rows(vec![sample_row_for_api_format("claude:messages")]);
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body = Bytes::from_static(br#"{"model":"gpt-5.2","messages":[]}"#);

        let rejection =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
                .await
                .expect("model rejection should resolve");

        assert_eq!(rejection, None);
    }

    #[tokio::test]
    async fn model_rejection_denies_requested_model_outside_allowed_global_models() {
        let state = state_with_model_mapping();
        let decision = decision_with_allowed_models(vec!["gpt-4.1".to_string()]);
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body = Bytes::from_static(br#"{"model":"gpt-5.2","messages":[]}"#);

        let rejection =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
                .await
                .expect("model rejection should resolve");

        assert_eq!(
            rejection,
            Some(GatewayLocalAuthRejection::ModelNotAllowed {
                model: "gpt-5.2".to_string(),
            })
        );
    }

    #[tokio::test]
    async fn model_rejection_reuses_request_policy_snapshot_for_directive_base_model() {
        let state = state_with_rows(Vec::new());
        let mut decision = decision_with_allowed_models(vec!["gpt-5.6-sol".to_string()]);
        decision.model_directive_policy =
            crate::system_features::ModelDirectivePolicySnapshot::from_config_values(
                Some(&json!(true)),
                None,
            );
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body = Bytes::from_static(br#"{"model":"gpt-5.6-sol-high","messages":[]}"#);

        let rejection =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
                .await
                .expect("model rejection should resolve");

        assert_eq!(rejection, None);
    }

    #[tokio::test]
    async fn model_rejection_uses_custom_policy_suffix_for_base_model_authorization() {
        let state = state_with_rows(Vec::new());
        let mut decision = decision_with_allowed_models(vec!["deployment-alias".to_string()]);
        decision.model_directive_policy =
            crate::system_features::ModelDirectivePolicySnapshot::from_config_values(
                Some(&json!(true)),
                Some(&json!({
                    "reasoning_effort": {
                        "api_formats": {
                            "openai:chat": {
                                "suffixes": ["VendorFuture"],
                                "mappings": {
                                    "VendorFuture": { "reasoning_effort": "high" }
                                }
                            }
                        }
                    }
                })),
            );
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body =
            Bytes::from_static(br#"{"model":"deployment-alias-VendorFuture","messages":[]}"#);

        let rejection =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
                .await
                .expect("model rejection should resolve");

        assert_eq!(rejection, None);
    }

    #[tokio::test]
    async fn positive_balance_allows_unbounded_output_request_without_cost_estimate() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 1.0,
                    "output_price_per_1m": 2.0
                }]
            })),
            None,
            None,
            None,
        );
        for allow_wallet_overage in [false, true] {
            let state = state_with_quota_and_wallet(
                quota_availability(50.0, allow_wallet_overage),
                context.clone(),
            );
            let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
            let plan = execution_plan(
                json!({
                    "model": "gpt-5",
                    "messages": [{"role": "user", "content": "hi"}],
                    "stream": true
                }),
                "openai:chat",
            );

            let rejection = execution_plan_balance_capacity_rejection(
                &state,
                &decision,
                &plan,
                Some(&billing_report_context()),
            )
            .await
            .expect("quota rejection should resolve");

            assert_eq!(rejection, None);
        }
    }

    #[tokio::test]
    async fn positive_balance_does_not_allow_historical_invalid_processing_pricing() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 1.0,
                    "output_price_per_1m": 2.0
                }],
                "processing_tiers": {
                    "priority": {
                        "tiers": [{}],
                        "price_multiplier": -1.0
                    }
                }
            })),
            None,
            None,
            None,
        );
        let state = state_with_quota_and_wallet(quota_availability(50.0, true), context);
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let plan = execution_plan(
            json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "hi"}],
                "service_tier": "priority",
                "max_completion_tokens": 1
            }),
            "openai:chat",
        );

        let error = execution_plan_balance_capacity_rejection(
            &state,
            &decision,
            &plan,
            Some(&billing_report_context()),
        )
        .await
        .expect_err("invalid configured processing pricing must stop authorization");

        assert!(error
            .into_message()
            .contains("explicit catalog contains malformed or unrecognized prices"));
    }

    #[tokio::test]
    async fn standalone_key_does_not_bypass_invalid_processing_pricing_validation() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 1.0}],
                "processing_tiers": {
                    "priority": {"tiers": [{}], "price_multiplier": 2.0}
                }
            })),
            None,
            None,
            None,
        );
        let state = state_with_quota_and_wallet(quota_availability(50.0, true), context);
        let mut decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        decision
            .auth_context
            .as_mut()
            .expect("auth context should exist")
            .api_key_is_standalone = true;
        let plan = execution_plan(
            json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "hi"}],
                "service_tier": "priority",
                "max_completion_tokens": 1
            }),
            "openai:chat",
        );

        let error = execution_plan_balance_capacity_rejection(
            &state,
            &decision,
            &plan,
            Some(&billing_report_context()),
        )
        .await
        .expect_err("standalone keys must still validate configured pricing");
        assert!(error
            .into_message()
            .contains("explicit catalog contains malformed or unrecognized prices"));
    }

    #[tokio::test]
    async fn unlimited_wallet_does_not_bypass_invalid_processing_pricing_validation() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 1.0}],
                "processing_tiers": {
                    "priority": {"tiers": [{}], "price_multiplier": 2.0}
                }
            })),
            None,
            None,
            None,
        );
        let state = state_with_quota_and_wallet(quota_availability(0.0, true), context);
        {
            let store = state
                .auth_wallet_store
                .as_ref()
                .expect("test wallet store should exist");
            let mut wallets = store.lock().expect("wallet store should lock");
            wallets
                .get_mut("wallet-user-1")
                .expect("test wallet should exist")
                .limit_mode = "unlimited".to_string();
        }
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let plan = execution_plan(
            json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "hi"}],
                "service_tier": "priority",
                "max_completion_tokens": 1
            }),
            "openai:chat",
        );

        let error = execution_plan_balance_capacity_rejection(
            &state,
            &decision,
            &plan,
            Some(&billing_report_context()),
        )
        .await
        .expect_err("unlimited wallets must still validate configured pricing");
        assert!(error
            .into_message()
            .contains("explicit catalog contains malformed or unrecognized prices"));
    }

    #[tokio::test]
    async fn standalone_and_unlimited_paths_keep_allowing_valid_pricing() {
        let valid_context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 1.0}]
            })),
            None,
            None,
            None,
        );
        let plan = execution_plan(
            json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "hi"}],
                "max_completion_tokens": 1
            }),
            "openai:chat",
        );
        let report_context = billing_report_context();

        let standalone_state =
            state_with_quota_and_wallet(quota_availability(50.0, true), valid_context.clone());
        let mut standalone_decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        standalone_decision
            .auth_context
            .as_mut()
            .expect("auth context should exist")
            .api_key_is_standalone = true;
        assert_eq!(
            execution_plan_balance_capacity_rejection(
                &standalone_state,
                &standalone_decision,
                &plan,
                Some(&report_context),
            )
            .await
            .expect("valid standalone pricing should resolve"),
            None
        );

        let unlimited_state =
            state_with_quota_and_wallet(quota_availability(0.0, true), valid_context);
        {
            let store = unlimited_state
                .auth_wallet_store
                .as_ref()
                .expect("test wallet store should exist");
            let mut wallets = store.lock().expect("wallet store should lock");
            wallets
                .get_mut("wallet-user-1")
                .expect("test wallet should exist")
                .limit_mode = "unlimited".to_string();
        }
        assert_eq!(
            execution_plan_balance_capacity_rejection(
                &unlimited_state,
                &decision_with_allowed_models(vec!["gpt-5".to_string()]),
                &plan,
                Some(&report_context),
            )
            .await
            .expect("valid unlimited-wallet pricing should resolve"),
            None
        );
    }

    #[tokio::test]
    async fn auth_capacity_reuses_quota_wallet_and_cost_estimate_within_ttl() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 0.0,
                    "output_price_per_1m": 60.0
                }]
            })),
            None,
            None,
            None,
        );
        let quota_calls = Arc::new(AtomicUsize::new(0));
        let model_context_calls = Arc::new(AtomicUsize::new(0));
        let candidate_repository =
            Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
                sample_row(),
            ]));
        let billing_repository = Arc::new(FixedBillingReadRepository::with_counters(
            quota_availability(1.0, true),
            context,
            Arc::clone(&quota_calls),
            Arc::clone(&model_context_calls),
        ));
        let data = GatewayDataState::with_minimal_candidate_selection_and_billing_for_tests(
            candidate_repository,
            billing_repository,
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data)
            .with_auth_wallets_for_tests(vec![sample_wallet("user-1", 30.0)]);
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let plan = execution_plan(
            json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "hi"}],
                "max_tokens": 100_000
            }),
            "openai:chat",
        );
        let report_context = billing_report_context();

        let first = execution_plan_balance_capacity_rejection(
            &state,
            &decision,
            &plan,
            Some(&report_context),
        )
        .await
        .expect("first auth capacity check should resolve");

        assert_eq!(first, None);
        assert_eq!(quota_calls.load(Ordering::Acquire), 1);
        assert_eq!(model_context_calls.load(Ordering::Acquire), 1);

        let store = state
            .auth_wallet_store
            .as_ref()
            .expect("test wallet store should exist");
        {
            let mut wallets = store.lock().expect("wallet store should lock");
            let wallet = wallets
                .get_mut("wallet-user-1")
                .expect("test wallet should exist");
            wallet.balance = 0.0;
            wallet.gift_balance = 0.0;
        }

        let cached = execution_plan_balance_capacity_rejection(
            &state,
            &decision,
            &plan,
            Some(&report_context),
        )
        .await
        .expect("cached auth capacity check should resolve");

        assert_eq!(cached, None);
        assert_eq!(quota_calls.load(Ordering::Acquire), 1);
        assert_eq!(model_context_calls.load(Ordering::Acquire), 1);

        state.invalidate_auth_context_cache();

        let refreshed = execution_plan_balance_capacity_rejection(
            &state,
            &decision,
            &plan,
            Some(&report_context),
        )
        .await
        .expect("refreshed auth capacity check should resolve");

        assert_eq!(
            refreshed,
            Some(GatewayLocalAuthRejection::BalanceDenied {
                remaining: Some(1.0),
            })
        );
        assert_eq!(quota_calls.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn admin_bypass_limits_does_not_skip_unbounded_zero_balance_capacity() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 1.0,
                    "output_price_per_1m": 2.0
                }]
            })),
            None,
            None,
            None,
        );
        let state = state_with_quota_and_wallet(quota_availability(0.0, false), context);
        let mut decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        if let Some(auth_context) = decision.auth_context.as_mut() {
            auth_context.admin_bypass_limits = true;
        }
        let plan = execution_plan(
            json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "hi"}],
                "stream": true
            }),
            "openai:chat",
        );
        let report_context = billing_report_context();

        let rejection = execution_plan_balance_capacity_rejection(
            &state,
            &decision,
            &plan,
            Some(&report_context),
        )
        .await
        .expect("quota rejection should resolve");

        assert_eq!(
            rejection,
            Some(GatewayLocalAuthRejection::BalanceDenied {
                remaining: Some(0.0),
            })
        );
    }

    #[tokio::test]
    async fn zero_balance_allows_a_proven_free_tier_execution_plan() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 100.0,
                    "output_price_per_1m": 100.0
                }]
            })),
            None,
            None,
            Some("free_tier"),
        );
        let state = state_with_quota_and_wallet(quota_availability(0.0, false), context);
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let plan = execution_plan(
            json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "hi"}],
                "max_completion_tokens": 1_000_000
            }),
            "openai:chat",
        );
        let report_context = billing_report_context();

        let rejection = execution_plan_balance_capacity_rejection(
            &state,
            &decision,
            &plan,
            Some(&report_context),
        )
        .await
        .expect("free tier capacity check should resolve");

        assert_eq!(rejection, None);
    }

    #[tokio::test]
    async fn finalized_chat_output_fields_and_choice_count_bound_capacity() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 0.0,
                    "output_price_per_1m": 20.0
                }]
            })),
            None,
            None,
            None,
        );
        let state = state_with_quota_and_wallet(quota_availability(50.0, false), context);
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let plan = execution_plan(
            json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "hi"}],
                "max_tokens": 1,
                "max_completion_tokens": 1_000_000,
                "n": 3
            }),
            "openai:chat",
        );

        let rejection = execution_plan_balance_capacity_rejection(
            &state,
            &decision,
            &plan,
            Some(&billing_report_context()),
        )
        .await
        .expect("quota rejection should resolve");

        assert_eq!(
            rejection,
            Some(GatewayLocalAuthRejection::BalanceDenied {
                remaining: Some(50.0),
            })
        );
    }

    #[tokio::test]
    async fn stateful_responses_request_skips_unprovable_capacity_rejection() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 100.0,
                    "output_price_per_1m": 100.0
                }]
            })),
            None,
            None,
            None,
        );
        let state = state_with_quota_and_wallet(quota_availability(0.01, false), context);
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let plan = execution_plan(
            json!({
                "model": "gpt-5",
                "input": "continue",
                "previous_response_id": "resp_123",
                "max_output_tokens": 1_000_000
            }),
            "openai:responses",
        );
        let report_context = billing_report_context();

        let rejection = execution_plan_balance_capacity_rejection(
            &state,
            &decision,
            &plan,
            Some(&report_context),
        )
        .await
        .expect("stateful request capacity check should resolve");

        assert_eq!(rejection, None);
    }

    #[tokio::test]
    async fn stateful_unavailable_estimate_still_rejects_invalid_processing_pricing() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 1.0,
                    "output_price_per_1m": 2.0
                }],
                "processing_tiers": {
                    "priority": {
                        "tiers": [{}],
                        "price_multiplier": 2.0
                    }
                }
            })),
            None,
            None,
            None,
        );
        let state = state_with_quota_and_wallet(quota_availability(1.0, false), context);
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let plan = execution_plan(
            json!({
                "model": "gpt-5",
                "input": "continue",
                "previous_response_id": "resp_123",
                "service_tier": "priority",
                "max_output_tokens": 1
            }),
            "openai:responses",
        );

        let error = execution_plan_balance_capacity_rejection(
            &state,
            &decision,
            &plan,
            Some(&billing_report_context()),
        )
        .await
        .expect_err("unavailable estimates must still validate configured processing pricing");

        assert!(error
            .into_message()
            .contains("explicit catalog contains malformed or unrecognized prices"));
    }

    #[tokio::test]
    async fn wallet_overage_policy_extends_known_cost_capacity_when_enabled() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 0.0,
                    "output_price_per_1m": 70.0
                }]
            })),
            None,
            None,
            None,
        );
        let state = state_with_quota_and_wallet(quota_availability(50.0, true), context);
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let plan = execution_plan(
            json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "hi"}],
                "max_tokens": 1_000_000
            }),
            "openai:chat",
        );

        let rejection = execution_plan_balance_capacity_rejection(
            &state,
            &decision,
            &plan,
            Some(&billing_report_context()),
        )
        .await
        .expect("quota rejection should resolve");

        assert_eq!(rejection, None);
    }

    #[test]
    fn daily_quota_estimate_falls_back_to_default_tiers_when_model_tiers_empty() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 3.0,
                    "output_price_per_1m": 15.0
                }]
            })),
            Some(json!({})),
            None,
            None,
        );

        let estimate =
            estimate_from_billing_context(&context, "openai:chat", 1_000_000, Some(1_000_000))
                .expect("estimate should be bounded");

        assert_eq!(estimate, 18.75);
    }

    #[test]
    fn daily_quota_estimate_applies_provider_key_rate_multiplier() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 1.0,
                    "output_price_per_1m": 2.0
                }]
            })),
            None,
            Some(json!({ "openai:chat": 2.0 })),
            None,
        );

        let estimate =
            estimate_from_billing_context(&context, "openai:chat", 1_000_000, Some(1_000_000))
                .expect("estimate should be bounded");

        assert_eq!(estimate, 6.5);
    }

    #[test]
    fn daily_quota_estimate_treats_free_tier_as_zero_cost() {
        let context = billing_context_with_pricing(
            Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 3.0,
                    "output_price_per_1m": 15.0
                }]
            })),
            None,
            Some(json!({ "openai:chat": 10.0 })),
            Some("free_tier"),
        );

        let estimate =
            estimate_from_billing_context(&context, "openai:chat", 1_000_000, Some(1_000_000))
                .expect("estimate should be bounded");

        assert_eq!(estimate, 0.0);
    }

    #[test]
    fn output_bound_uses_largest_supported_field_and_chat_choice_count() {
        let body = json!({
            "max_tokens": 1,
            "max_completion_tokens": 100_000,
            "max_output_tokens": 50_000,
            "n": 3
        });

        assert_eq!(max_output_tokens_from_request(&body), Some(100_000));
        assert_eq!(output_choice_count_upper_bound("openai:chat", &body), 3);
        assert_eq!(
            output_choice_count_upper_bound("openai:responses", &body),
            1
        );
    }

    #[test]
    fn authorization_cache_key_includes_effective_cache_ttl() {
        let plan = execution_plan(json!({"model": "gpt-5.6-sol"}), "openai:responses");
        let without_ttl = execution_plan_cost_upper_bound_cache_key(
            &plan,
            Some("model-1"),
            Some("gpt-5.6-sol"),
            "openai:responses",
            100,
            Some(10),
            Some("priority"),
            None,
        );
        let with_ttl = execution_plan_cost_upper_bound_cache_key(
            &plan,
            Some("model-1"),
            Some("gpt-5.6-sol"),
            "openai:responses",
            100,
            Some(10),
            Some("priority"),
            Some(30),
        );

        assert_ne!(without_ttl, with_ttl);
    }

    #[test]
    fn indirect_request_inputs_are_not_treated_as_body_bounded() {
        let self_contained = json!({
            "input": [{
                "role": "user",
                "content": [{"type": "input_text", "text": "hello"}]
            }],
            "tools": [{"type": "function", "name": "lookup", "parameters": {}}]
        });
        assert!(openai_request_input_is_self_contained(
            "openai:responses",
            &self_contained
        ));

        for indirect in [
            json!({"input": "continue", "previous_response_id": "resp_123"}),
            json!({"input": "hello", "conversation": "conv_123"}),
            json!({"prompt": {"id": "pmpt_123", "variables": {}}}),
            json!({"input": [{"type": "item_reference", "id": "item_123"}]}),
            json!({"input": [{"type": "input_file", "file_id": "file_123"}]}),
            json!({"input": [{"type": "input_image", "image_url": "https://example.test/a.png"}]}),
            json!({"input": [{"type": "url", "url": "https://example.test/document"}]}),
            json!({"input": [{"file_uri": "https://example.test/file"}]}),
            json!({"input": "search", "tools": [{"type": "file_search", "vector_store_ids": ["vs_123"]}]}),
        ] {
            assert!(!openai_request_input_is_self_contained(
                "openai:responses",
                &indirect
            ));
        }
        assert!(!openai_request_input_is_self_contained(
            "claude:messages",
            &self_contained
        ));
    }
}
