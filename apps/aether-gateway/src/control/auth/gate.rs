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
                state,
                decision,
                requested_model,
                allowed_models,
            )
            .await
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

    balance_capacity_rejection(
        state,
        decision,
        auth_context,
        requested_model.as_deref(),
        headers,
        body,
    )
    .await
}

async fn balance_capacity_rejection(
    state: &AppState,
    decision: &GatewayControlDecision,
    auth_context: &GatewayControlAuthContext,
    requested_model: Option<&str>,
    headers: &http::HeaderMap,
    body: &Bytes,
) -> Result<Option<GatewayLocalAuthRejection>, GatewayError> {
    if auth_context.api_key_is_standalone {
        return Ok(None);
    }
    if auth_context.local_rejection.is_some() {
        return Ok(None);
    }
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
    let available_usd = match quota.as_ref() {
        Some(quota) if !quota.allow_wallet_overage => Some(quota.remaining_usd.max(0.0)),
        Some(_) if wallet_is_unlimited => None,
        Some(quota) => Some(quota.remaining_usd.max(0.0) + wallet_available_usd.unwrap_or(0.0)),
        None if wallet_is_unlimited => None,
        None => wallet_available_usd,
    };
    let Some(available_usd) = available_usd else {
        return Ok(None);
    };
    if available_usd <= DAILY_QUOTA_EPSILON_USD {
        return Ok(Some(GatewayLocalAuthRejection::BalanceDenied {
            remaining: Some(0.0),
        }));
    }
    let Some(requested_model) = requested_model else {
        return Ok(None);
    };
    let Some(estimated_cost_usd) =
        estimate_request_cost_upper_bound_usd(state, decision, requested_model, headers, body)
            .await?
    else {
        return Ok(None);
    };
    if estimated_cost_usd > available_usd + DAILY_QUOTA_EPSILON_USD {
        return Ok(Some(GatewayLocalAuthRejection::BalanceDenied {
            remaining: Some(available_usd),
        }));
    }
    Ok(None)
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

async fn estimate_request_cost_upper_bound_usd(
    state: &AppState,
    decision: &GatewayControlDecision,
    requested_model: &str,
    headers: &http::HeaderMap,
    body: &Bytes,
) -> Result<Option<f64>, GatewayError> {
    let Some(api_format) = decision
        .auth_endpoint_signature
        .as_deref()
        .map(crate::ai_serving::normalize_api_format_alias)
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let body = crate::headers::decoded_request_body_bytes(headers, body.as_ref()).ok();
    let Some(body) = body else {
        return Ok(None);
    };
    let body_json = serde_json::from_slice::<serde_json::Value>(body.as_ref()).ok();
    let Some(input_tokens) = body_json
        .as_ref()
        .map(estimate_json_tokens)
        .filter(|value| *value > 0)
    else {
        return Ok(None);
    };
    let max_output_tokens = body_json.as_ref().and_then(max_output_tokens_from_request);
    let cache_key = auth_request_cost_upper_bound_cache_key(
        &api_format,
        requested_model,
        input_tokens,
        max_output_tokens,
    );
    let ttl = state.frontdoor_runtime_guards.auth_capacity_cache_ttl;
    if ttl.is_zero() {
        return estimate_request_cost_upper_bound_for_tokens(
            state,
            &api_format,
            requested_model,
            input_tokens,
            max_output_tokens,
        )
        .await;
    }
    state
        .auth_request_cost_upper_bound_cache
        .get_or_load(cache_key, ttl, || async {
            estimate_request_cost_upper_bound_for_tokens(
                state,
                &api_format,
                requested_model,
                input_tokens,
                max_output_tokens,
            )
            .await
        })
        .await
}

async fn estimate_request_cost_upper_bound_for_tokens(
    state: &AppState,
    api_format: &str,
    requested_model: &str,
    input_tokens: u64,
    max_output_tokens: Option<u64>,
) -> Result<Option<f64>, GatewayError> {
    let candidates = state
        .list_minimal_candidate_selection_rows_for_api_format_and_requested_model(
            api_format,
            requested_model,
        )
        .await?;
    let mut max_estimate = None::<f64>;
    for candidate in candidates {
        let context = state
            .data
            .find_billing_model_context_by_model_id(
                &candidate.provider_id,
                Some(&candidate.key_id),
                &candidate.model_id,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let Some(context) = context else {
            continue;
        };
        let Some(estimate) = estimate_cost_from_billing_context(
            &context,
            api_format,
            input_tokens,
            max_output_tokens,
        ) else {
            return Ok(None);
        };
        max_estimate = Some(max_estimate.map_or(estimate, |current| current.max(estimate)));
    }
    Ok(max_estimate.filter(|value| value.is_finite() && *value >= 0.0))
}

fn auth_request_cost_upper_bound_cache_key(
    api_format: &str,
    requested_model: &str,
    input_tokens: u64,
    max_output_tokens: Option<u64>,
) -> String {
    format!(
        "{}\x1f{}\x1f{}\x1f{}",
        api_format,
        requested_model,
        input_tokens,
        max_output_tokens
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string())
    )
}

fn estimate_cost_from_billing_context(
    context: &aether_data_contracts::repository::billing::StoredBillingModelContext,
    api_format: &str,
    input_tokens: u64,
    max_output_tokens: Option<u64>,
) -> Option<f64> {
    if context
        .provider_billing_type
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("free_tier"))
    {
        return Some(0.0);
    }
    let price_per_request = context
        .model_price_per_request
        .or(context.default_price_per_request)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(0.0);
    let tiered_pricing = effective_tiered_pricing(context);
    let input_price_per_1m = tiered_price_per_1m(tiered_pricing, "input_price_per_1m")
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(0.0);
    let output_price_per_1m = tiered_price_per_1m(tiered_pricing, "output_price_per_1m")
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(0.0);
    let output_tokens = if output_price_per_1m > 0.0 {
        max_output_tokens?
    } else {
        0
    };
    let estimate = price_per_request
        + (input_tokens as f64 * input_price_per_1m / 1_000_000.0)
        + (output_tokens as f64 * output_price_per_1m / 1_000_000.0);
    let rate_multiplier = rate_multiplier_for_api_format(context, api_format);
    Some(estimate * rate_multiplier)
}

fn effective_tiered_pricing(
    context: &aether_data_contracts::repository::billing::StoredBillingModelContext,
) -> Option<&serde_json::Value> {
    context
        .model_tiered_pricing
        .as_ref()
        .filter(|value| tiered_pricing_has_rates(value))
        .or(context.default_tiered_pricing.as_ref())
}

fn tiered_pricing_has_rates(value: &serde_json::Value) -> bool {
    value
        .get("tiers")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|tiers| !tiers.is_empty())
        || ["input_price_per_1m", "output_price_per_1m"]
            .iter()
            .any(|field| {
                value
                    .get(*field)
                    .and_then(serde_json::Value::as_f64)
                    .is_some()
            })
}

fn rate_multiplier_for_api_format(
    context: &aether_data_contracts::repository::billing::StoredBillingModelContext,
    api_format: &str,
) -> f64 {
    let normalized_api_format = api_format.trim().to_ascii_lowercase();
    let Some(mapping) = context
        .provider_api_key_rate_multipliers
        .as_ref()
        .and_then(serde_json::Value::as_object)
    else {
        return 1.0;
    };
    mapping
        .get(&normalized_api_format)
        .and_then(serde_json::Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(1.0)
}

fn tiered_price_per_1m(tiered_pricing: Option<&serde_json::Value>, field: &str) -> Option<f64> {
    let value = tiered_pricing?;
    value
        .get(field)
        .and_then(serde_json::Value::as_f64)
        .or_else(|| {
            value
                .get("tiers")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|tier| tier.get(field).and_then(serde_json::Value::as_f64))
                .filter(|price| price.is_finite() && *price >= 0.0)
                .max_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal))
        })
}

fn max_output_tokens_from_request(value: &serde_json::Value) -> Option<u64> {
    ["max_tokens", "max_completion_tokens", "max_output_tokens"]
        .iter()
        .find_map(|field| value.get(*field).and_then(serde_json::Value::as_u64))
        .filter(|value| *value > 0)
}

fn estimate_json_tokens(value: &serde_json::Value) -> u64 {
    match value {
        serde_json::Value::String(text) => estimate_text_tokens(text),
        serde_json::Value::Array(items) => items
            .iter()
            .map(estimate_json_tokens)
            .fold(0u64, u64::saturating_add),
        serde_json::Value::Object(object) => object
            .iter()
            .map(|(key, value)| {
                estimate_text_tokens(key).saturating_add(estimate_json_tokens(value))
            })
            .fold(0u64, u64::saturating_add),
        _ => 1,
    }
}

fn estimate_text_tokens(text: &str) -> u64 {
    let chars = text.chars().count() as u64;
    chars.div_ceil(4).max(1)
}

async fn model_directive_base_model_is_allowed_for_request(
    state: &AppState,
    decision: &GatewayControlDecision,
    requested_model: &str,
    allowed_models: &[String],
) -> bool {
    let Some(base_model) = crate::ai_serving::model_directive_base_model(requested_model) else {
        return false;
    };
    if !contains_string(allowed_models, &base_model) {
        return false;
    }
    let Some(client_api_format) = decision
        .auth_endpoint_signature
        .as_deref()
        .map(crate::ai_serving::normalize_api_format_alias)
        .filter(|value| !value.trim().is_empty())
    else {
        return false;
    };
    for api_format in candidate_api_formats_for_model_resolution(&client_api_format) {
        if crate::system_features::reasoning_model_directive_enabled_for_api_format_and_model(
            state,
            &api_format,
            Some(requested_model),
        )
        .await
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
        let enable_model_directives =
            crate::system_features::reasoning_model_directive_enabled_for_api_format_and_model(
                state,
                &api_format,
                Some(requested_model),
            )
            .await;
        let rows = state
            .list_minimal_candidate_selection_rows_for_api_format(&api_format)
            .await?;
        let matching_rows = rows
            .into_iter()
            .filter(|row| {
                aether_scheduler_core::row_supports_requested_model_with_model_directives(
                    row,
                    requested_model,
                    &api_format,
                    enable_model_directives,
                )
            })
            .collect::<Vec<_>>();
        let Some(resolved_global_model) =
            aether_scheduler_core::resolve_requested_global_model_name_with_model_directives(
                &matching_rows,
                requested_model,
                &api_format,
                enable_model_directives,
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
    use aether_data::repository::wallet::StoredWalletSnapshot;
    use aether_data_contracts::repository::billing::{
        BillingReadRepository, StoredBillingModelContext, UserDailyQuotaAvailabilityRecord,
    };
    use aether_data_contracts::repository::candidate_selection::{
        StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
    };
    use aether_data_contracts::DataLayerError;
    use async_trait::async_trait;
    use axum::body::Bytes;
    use axum::http::{HeaderMap, Uri};
    use serde_json::json;

    use super::{
        estimate_cost_from_billing_context, request_model_local_rejection,
        GatewayLocalAuthRejection,
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
    async fn model_rejection_allows_cross_format_provider_mapping_to_allowed_global_model() {
        let mut row = sample_row_for_api_format("gemini:generate_content");
        row.model_provider_model_name = "gemini-2.5-pro-upstream".to_string();
        row.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
            name: "gemini-2.5-pro-alias".to_string(),
            priority: 1,
            api_formats: Some(vec!["gemini:generate_content".to_string()]),
            endpoint_ids: None,
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
            let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
            let body = Bytes::from_static(
                br#"{"model":"gpt-5","messages":[{"role":"user","content":"hi"}],"stream":true}"#,
            );

            let rejection = request_model_local_rejection(
                &state,
                Some(&decision),
                &uri,
                &json_headers(),
                &body,
            )
            .await
            .expect("quota rejection should resolve");

            assert_eq!(rejection, None);
        }
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
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body = Bytes::from_static(
            br#"{"model":"gpt-5","messages":[{"role":"user","content":"hi"}],"max_tokens":100000}"#,
        );

        let first =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
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

        let cached =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
                .await
                .expect("cached auth capacity check should resolve");

        assert_eq!(cached, None);
        assert_eq!(quota_calls.load(Ordering::Acquire), 1);
        assert_eq!(model_context_calls.load(Ordering::Acquire), 1);

        state.invalidate_auth_context_cache();

        let refreshed =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
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
    async fn admin_bypass_limits_does_not_skip_exhausted_daily_quota_capacity() {
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
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body = Bytes::from_static(
            br#"{"model":"gpt-5","messages":[{"role":"user","content":"hi"}],"stream":true}"#,
        );

        let rejection =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
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
    async fn positive_balance_still_denies_known_cost_above_available_capacity() {
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
        let state = state_with_quota_and_wallet(quota_availability(50.0, false), context);
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body = Bytes::from_static(
            br#"{"model":"gpt-5","messages":[{"role":"user","content":"hi"}],"max_tokens":1000000}"#,
        );

        let rejection =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
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
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body = Bytes::from_static(
            br#"{"model":"gpt-5","messages":[{"role":"user","content":"hi"}],"max_tokens":1000000}"#,
        );

        let rejection =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
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
            estimate_cost_from_billing_context(&context, "openai:chat", 1_000_000, Some(1_000_000))
                .expect("estimate should resolve");

        assert_eq!(estimate, 18.0);
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
            estimate_cost_from_billing_context(&context, "openai:chat", 1_000_000, Some(1_000_000))
                .expect("estimate should resolve");

        assert_eq!(estimate, 6.0);
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
            estimate_cost_from_billing_context(&context, "openai:chat", 1_000_000, Some(1_000_000))
                .expect("estimate should resolve");

        assert_eq!(estimate, 0.0);
    }
}
