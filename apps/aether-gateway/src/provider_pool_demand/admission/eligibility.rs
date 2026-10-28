use super::*;
use aether_admin::provider::pool as pool_policy;
use aether_scheduler_core::{
    effective_provider_key_rpm_limit, is_provider_key_circuit_open_at, provider_key_health_score,
    provider_key_rpm_allows_request_since, should_skip_provider_quota,
};

pub(super) async fn current_skip_reason(
    state: &AppState,
    plan: &ExecutionPlan,
) -> Result<Option<&'static str>, GatewayError> {
    let (providers, keys, endpoints) = tokio::try_join!(
        state.read_provider_catalog_providers_by_ids(std::slice::from_ref(&plan.provider_id)),
        state.read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id)),
        state.read_provider_catalog_endpoints_by_ids(std::slice::from_ref(&plan.endpoint_id)),
    )?;
    let Some(provider) = providers
        .into_iter()
        .find(|provider| provider.id == plan.provider_id && provider.is_active)
    else {
        return Ok(Some("provider_inactive"));
    };
    let Some(key) = keys
        .into_iter()
        .find(|key| key.id == plan.key_id && key.provider_id == plan.provider_id && key.is_active)
    else {
        return Ok(Some("provider_key_inactive"));
    };
    if !endpoints.iter().any(|endpoint| {
        endpoint.id == plan.endpoint_id
            && endpoint.provider_id == plan.provider_id
            && endpoint.is_active
    }) {
        return Ok(Some("endpoint_inactive"));
    }
    let now = crate::clock::current_unix_secs();
    if !key.auth_type.eq_ignore_ascii_case("oauth")
        && key
            .expires_at_unix_secs
            .is_some_and(|expires| expires > 0 && expires <= now)
    {
        return Ok(Some("provider_key_expired"));
    }
    if state
        .read_provider_quota_snapshot(&plan.provider_id)
        .await?
        .as_ref()
        .is_some_and(|quota| should_skip_provider_quota(quota, now))
    {
        return Ok(Some("provider_quota_blocked"));
    }
    let pool = provider
        .config
        .as_ref()
        .and_then(|config| config.get("pool_advanced"));
    let model = plan.model_name.as_deref().unwrap_or_default();
    let exhausted =
        pool_policy::admin_pool_key_model_quota_exhausted(&key, &provider.provider_type, model)
            .unwrap_or_else(|| {
                pool_policy::admin_pool_key_account_quota_exhausted(&key, &provider.provider_type)
            });
    let skip_exhausted = pool
        .and_then(|config| config.get("ignore_exhausted_accounts"))
        .and_then(serde_json::Value::as_bool)
        .map(|ignore| !ignore)
        .unwrap_or(true);
    let reserve_reached = pool
        .and_then(|config| config.get("reserve_minimum_quota"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        && pool_policy::admin_pool_key_minimum_quota_reached(
            &key,
            &provider.provider_type,
            Some(model),
        );
    if pool_policy::admin_pool_key_model_quota_hard_blocked(&key, &provider.provider_type, model)
        || reserve_reached
        || (skip_exhausted && exhausted)
    {
        return Ok(Some("account_quota_exhausted"));
    }
    if pool.is_some() {
        if crate::dispatch::pool_scheduler::pool_key_requires_reauth_for_scheduling(&key, now) {
            return Ok(Some("oauth_invalid"));
        }
        if crate::handlers::shared::provider_pool::read_admin_provider_pool_key_cooldown_reason(
            state.runtime_state.as_ref(),
            &plan.provider_id,
            &plan.key_id,
        )
        .await
        .map_err(|error| GatewayError::Internal(error.to_string()))?
        .is_some()
        {
            return Ok(Some("pool_cooldown"));
        }
    } else {
        if crate::scheduler::candidate::key_requires_oauth_reauth(
            &key,
            &provider.provider_type,
            now,
        ) {
            return Ok(Some("oauth_invalid"));
        }
        if is_provider_key_circuit_open_at(&key, &plan.provider_api_format, now) {
            return Ok(Some("key_circuit_open"));
        }
        if provider_key_health_score(&key, &plan.provider_api_format)
            .is_some_and(|score| score <= 0.0)
        {
            return Ok(Some("key_health_score_zero"));
        }
        if effective_provider_key_rpm_limit(&key, now).is_some() {
            let mut recent = state.read_recent_request_candidates(128).await?;
            recent.retain(|candidate| candidate.request_id != plan.request_id);
            if !provider_key_rpm_allows_request_since(
                &key,
                &recent,
                now,
                false,
                state.provider_key_rpm_reset_at(&key.id, now),
            ) {
                return Ok(Some("key_rpm_exhausted"));
            }
        }
    }
    Ok(None)
}
