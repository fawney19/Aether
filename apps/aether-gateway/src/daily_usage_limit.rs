use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_cache::ExpiringMap;
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use aether_data_contracts::repository::usage::UsageDailyActualCostRollupQuery;
use aether_runtime_state::{
    DailyUsageLimitCountInput, DailyUsageLimitIncrementInput, DailyUsageLimitRestoreEntry,
    DailyUsageLimitRestoreInput, RuntimeState,
};
use chrono::{DateTime, SecondsFormat, Utc};
use tracing::warn;

use crate::app_timezone::{app_timezone, local_day_window};
use crate::control::GatewayControlDecision;
use crate::stage_metrics::observe_gateway_stage_ms;
use crate::{AppState, GatewayError};

const SYSTEM_DAILY_USAGE_LIMIT_CONFIG_KEY: &str = "daily_usage_limit_usd";
const SYSTEM_CONFIG_CACHE_TTL: Duration = Duration::from_secs(15);
const LIMIT_EPSILON_USD: f64 = 0.000_000_01;
const USD_UNITS_PER_DOLLAR: f64 = 100_000_000.0;
const COUNTER_EXPIRY_GRACE_SECONDS: u64 = 60;
const DAILY_USAGE_RUNTIME_STATE_KEY: &str = "daily_usage_limit:runtime_state";
const DAILY_USAGE_RECOVERY_LOCK_KEY: &str = "daily_usage_limit:recovery";
const DAILY_USAGE_RECOVERY_LOCK_OWNER: &str = "gateway-daily-usage-recovery";
const DAILY_USAGE_RECOVERY_LOCK_TTL: Duration = Duration::from_secs(600);
const DAILY_USAGE_RECOVERY_RETRY_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DailyUsageScopeStatus {
    pub(crate) scope: &'static str,
    pub(crate) limit_usd: f64,
    pub(crate) used_usd: f64,
    pub(crate) remaining_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FrontdoorDailyUsageStatus {
    pub(crate) available: bool,
    pub(crate) timezone: String,
    pub(crate) window_start: String,
    pub(crate) window_end: String,
    pub(crate) reset_at_unix_secs: u64,
    pub(crate) user: Option<DailyUsageScopeStatus>,
    pub(crate) key: Option<DailyUsageScopeStatus>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FrontdoorDailyUsageRejection {
    pub(crate) scope: &'static str,
    pub(crate) limit_usd: f64,
    pub(crate) used_usd: f64,
    pub(crate) remaining_usd: f64,
    pub(crate) retry_after: u64,
    pub(crate) reset_at_unix_secs: u64,
    pub(crate) timezone: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FrontdoorDailyUsageOutcome {
    NotApplicable,
    Allowed,
    Rejected(FrontdoorDailyUsageRejection),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DailyUsageLimitedResponse;

#[derive(Debug, Clone)]
pub(crate) struct FrontdoorDailyUsageLimiter {
    system_default_cache: Arc<ExpiringMap<String, f64>>,
    recovery_inflight: Arc<AtomicBool>,
    runtime_failures: Arc<AtomicU64>,
    #[cfg(test)]
    system_default_override: Arc<std::sync::Mutex<Option<f64>>>,
}

impl Default for FrontdoorDailyUsageLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl FrontdoorDailyUsageLimiter {
    pub(crate) fn new() -> Self {
        Self {
            system_default_cache: Arc::new(ExpiringMap::default()),
            recovery_inflight: Arc::new(AtomicBool::new(false)),
            runtime_failures: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            system_default_override: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub(crate) fn clear_system_default_cache(&self) {
        self.system_default_cache.clear();
    }

    pub(crate) fn runtime_failure_count(&self) -> u64 {
        self.runtime_failures.load(Ordering::Relaxed)
    }

    pub(crate) async fn check(
        &self,
        state: &AppState,
        decision: &GatewayControlDecision,
    ) -> FrontdoorDailyUsageOutcome {
        let started_at = Instant::now();
        let status_result = self.current_status(state, decision).await;
        observe_gateway_stage_ms(
            "daily_usage_limit_total",
            started_at.elapsed().as_millis() as u64,
        );
        let status = match status_result {
            Ok(Some(status)) => status,
            Ok(None) => return FrontdoorDailyUsageOutcome::NotApplicable,
            Err(err) => {
                let failure_count = self.runtime_failures.fetch_add(1, Ordering::Relaxed) + 1;
                let auth = decision.auth_context.as_ref();
                warn!(
                    event_name = "frontdoor_daily_usage_check_failed",
                    log_type = "ops",
                    error = ?err,
                    runtime_failures_total = failure_count,
                    user_id = auth.map(|auth| auth.user_id.as_str()).unwrap_or("-"),
                    api_key_id = auth.map(|auth| auth.api_key_id.as_str()).unwrap_or("-"),
                    "daily usage limit check failed; allowing request"
                );
                return FrontdoorDailyUsageOutcome::Allowed;
            }
        };
        if !status.available {
            return FrontdoorDailyUsageOutcome::Allowed;
        }
        let exceeded = status
            .user
            .as_ref()
            .filter(|scope| scope.used_usd + LIMIT_EPSILON_USD >= scope.limit_usd)
            .or_else(|| {
                status
                    .key
                    .as_ref()
                    .filter(|scope| scope.used_usd + LIMIT_EPSILON_USD >= scope.limit_usd)
            });
        let Some(exceeded) = exceeded else {
            return FrontdoorDailyUsageOutcome::Allowed;
        };
        let now = Utc::now().timestamp().max(0) as u64;
        FrontdoorDailyUsageOutcome::Rejected(FrontdoorDailyUsageRejection {
            scope: exceeded.scope,
            limit_usd: exceeded.limit_usd,
            used_usd: exceeded.used_usd,
            remaining_usd: exceeded.remaining_usd,
            retry_after: status.reset_at_unix_secs.saturating_sub(now).max(1),
            reset_at_unix_secs: status.reset_at_unix_secs,
            timezone: status.timezone,
        })
    }

    pub(crate) async fn current_status(
        &self,
        state: &AppState,
        decision: &GatewayControlDecision,
    ) -> Result<Option<FrontdoorDailyUsageStatus>, GatewayError> {
        let Some(auth) = decision.auth_context.as_ref() else {
            return Ok(None);
        };
        if decision.route_class.as_deref() != Some("ai_public")
            || auth.local_rejection.is_some()
            || auth.user_id.is_empty()
            || auth.api_key_id.is_empty()
            || auth.admin_bypass_limits
            || auth.ip_bypass_limits
        {
            return Ok(None);
        }

        let needs_system_default = if auth.api_key_is_standalone {
            auth.api_key_daily_usage_limit_usd.is_none()
        } else {
            auth.user_daily_usage_limit_usd.is_none()
        };
        let system_limit = if needs_system_default {
            let config_started_at = Instant::now();
            let result = self.resolve_system_default_limit(state).await;
            observe_gateway_stage_ms(
                "daily_usage_limit_system_default",
                config_started_at.elapsed().as_millis() as u64,
            );
            result?
        } else {
            0.0
        };
        let (user_limit, key_limit) = resolve_scope_limits(
            auth.api_key_is_standalone,
            auth.user_daily_usage_limit_usd,
            auth.api_key_daily_usage_limit_usd,
            system_limit,
        );
        if user_limit.is_none() && key_limit.is_none() {
            return Ok(None);
        }

        let timezone = app_timezone();
        let now = Utc::now();
        let (_, start, end) = local_day_window(now, timezone);
        let bucket = start.timestamp().max(0) as u64;
        let user_scope_key = daily_usage_user_scope_key(&auth.user_id, bucket);
        let key_scope_key = daily_usage_key_scope_key(&auth.api_key_id, bucket);
        let runtime_started_at = Instant::now();
        let counts_result = state
            .runtime_state
            .daily_usage_limit_counts(DailyUsageLimitCountInput {
                state_key: DAILY_USAGE_RUNTIME_STATE_KEY,
                user_key: (!auth.api_key_is_standalone).then_some(user_scope_key.as_str()),
                key_key: &key_scope_key,
                bucket,
            })
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()));
        observe_gateway_stage_ms(
            "daily_usage_limit_runtime_read",
            runtime_started_at.elapsed().as_millis() as u64,
        );
        let counts = counts_result?;
        if !counts.state_ready {
            self.trigger_runtime_recovery(state);
            return Ok(Some(FrontdoorDailyUsageStatus {
                available: false,
                timezone: timezone.name().to_string(),
                window_start: rfc3339(start),
                window_end: rfc3339(end),
                reset_at_unix_secs: end.timestamp().max(0) as u64,
                user: None,
                key: None,
            }));
        }
        let user = user_limit
            .map(|limit_usd| scope_status("user", limit_usd, units_to_usd(counts.user_units)));
        let key = key_limit
            .map(|limit_usd| scope_status("key", limit_usd, units_to_usd(counts.key_units)));
        Ok(Some(FrontdoorDailyUsageStatus {
            available: true,
            timezone: timezone.name().to_string(),
            window_start: rfc3339(start),
            window_end: rfc3339(end),
            reset_at_unix_secs: end.timestamp().max(0) as u64,
            user,
            key,
        }))
    }

    fn trigger_runtime_recovery(&self, state: &AppState) {
        if self
            .recovery_inflight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let limiter = self.clone();
        let state = state.clone();
        tokio::spawn(async move {
            let started_at = Instant::now();
            let result = recover_daily_usage_runtime(&state).await;
            observe_gateway_stage_ms(
                "daily_usage_limit_recovery",
                started_at.elapsed().as_millis() as u64,
            );
            match result {
                Ok(true) => {}
                Ok(false) => tokio::time::sleep(DAILY_USAGE_RECOVERY_RETRY_DELAY).await,
                Err(err) => {
                    let failure_count =
                        limiter.runtime_failures.fetch_add(1, Ordering::Relaxed) + 1;
                    warn!(
                        event_name = "frontdoor_daily_usage_recovery_failed",
                        log_type = "ops",
                        error = ?err,
                        runtime_failures_total = failure_count,
                        "daily usage runtime recovery failed; limits remain fail-open"
                    );
                    tokio::time::sleep(DAILY_USAGE_RECOVERY_RETRY_DELAY).await;
                }
            }
            limiter.recovery_inflight.store(false, Ordering::Release);
        });
    }

    async fn resolve_system_default_limit(&self, state: &AppState) -> Result<f64, GatewayError> {
        #[cfg(test)]
        if let Ok(guard) = self.system_default_override.lock() {
            if let Some(limit) = *guard {
                return Ok(limit);
            }
        }
        if let Some(limit) = self
            .system_default_cache
            .get_fresh(SYSTEM_DAILY_USAGE_LIMIT_CONFIG_KEY, SYSTEM_CONFIG_CACHE_TTL)
        {
            return Ok(limit);
        }
        let limit = parse_system_limit(
            state
                .read_system_config_json_value(SYSTEM_DAILY_USAGE_LIMIT_CONFIG_KEY)
                .await?,
        )?;
        self.system_default_cache.insert(
            SYSTEM_DAILY_USAGE_LIMIT_CONFIG_KEY.to_string(),
            limit,
            SYSTEM_CONFIG_CACHE_TTL,
            8,
        );
        Ok(limit)
    }

    #[cfg(test)]
    pub(crate) fn with_system_default_limit_for_tests(self, limit: f64) -> Self {
        if let Ok(mut guard) = self.system_default_override.lock() {
            *guard = Some(limit.max(0.0));
        }
        self
    }
}

async fn recover_daily_usage_runtime(state: &AppState) -> Result<bool, GatewayError> {
    let Some(lease) = state
        .runtime_state
        .lock_try_acquire(
            DAILY_USAGE_RECOVERY_LOCK_KEY,
            DAILY_USAGE_RECOVERY_LOCK_OWNER,
            DAILY_USAGE_RECOVERY_LOCK_TTL,
        )
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
    else {
        return Ok(false);
    };

    let recovery_result = async {
        state
            .runtime_state
            .kv_set(DAILY_USAGE_RUNTIME_STATE_KEY, "recovering", None)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;

        let timezone = app_timezone();
        let now = Utc::now();
        let (_, start, end) = local_day_window(now, timezone);
        let bucket = start.timestamp().max(0) as u64;
        let rollups = state
            .background_data
            .summarize_usage_daily_actual_cost_rollups(&UsageDailyActualCostRollupQuery {
                finalized_from_unix_secs: bucket,
                finalized_until_unix_secs: end.timestamp().max(0) as u64,
            })
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;

        let mut user_totals = HashMap::<String, f64>::new();
        let mut key_totals = HashMap::<String, (Option<String>, bool, f64)>::new();
        for rollup in rollups {
            let Some(api_key_id) = non_empty(&rollup.api_key_id) else {
                continue;
            };
            let amount = rollup.actual_total_cost_usd;
            if !amount.is_finite() || amount <= 0.0 {
                continue;
            }
            let user_id = rollup
                .user_id
                .as_deref()
                .and_then(non_empty)
                .map(ToOwned::to_owned);
            if !rollup.api_key_is_standalone {
                if let Some(user_id) = user_id.as_ref() {
                    *user_totals.entry(user_id.clone()).or_default() += amount;
                }
            }
            let key_total = key_totals.entry(api_key_id.to_string()).or_insert((
                user_id.clone(),
                rollup.api_key_is_standalone,
                0.0,
            ));
            key_total.2 += amount;
        }

        let entries = key_totals
            .into_iter()
            .map(|(api_key_id, (user_id, is_standalone, key_total))| {
                let user_id = (!is_standalone).then_some(user_id).flatten();
                let user_units = user_id
                    .as_ref()
                    .and_then(|user_id| user_totals.get(user_id))
                    .copied()
                    .map(usd_to_units)
                    .unwrap_or_default();
                DailyUsageLimitRestoreEntry {
                    user_key: user_id
                        .as_deref()
                        .map(|user_id| daily_usage_user_scope_key(user_id, bucket)),
                    key_key: daily_usage_key_scope_key(&api_key_id, bucket),
                    user_units,
                    key_units: usd_to_units(key_total),
                }
            })
            .collect::<Vec<_>>();
        let ttl_seconds = (end.timestamp().max(0) as u64)
            .saturating_sub(Utc::now().timestamp().max(0) as u64)
            .saturating_add(COUNTER_EXPIRY_GRACE_SECONDS)
            .max(1);
        state
            .runtime_state
            .restore_daily_usage_limits(DailyUsageLimitRestoreInput {
                entries: &entries,
                bucket,
                ttl_seconds,
            })
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        state
            .runtime_state
            .kv_set(DAILY_USAGE_RUNTIME_STATE_KEY, "ready", None)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        Ok::<(), GatewayError>(())
    }
    .await;

    if let Err(err) = state.runtime_state.lock_release(&lease).await {
        warn!(
            event_name = "frontdoor_daily_usage_recovery_lock_release_failed",
            log_type = "ops",
            error = ?err,
            "daily usage runtime recovery lock release failed"
        );
    }
    recovery_result.map(|()| true)
}

pub(crate) async fn record_finalized_daily_usage(
    runtime_state: &RuntimeState,
    usage: &StoredRequestUsageAudit,
) -> Result<(), aether_runtime_state::DataLayerError> {
    if usage.status != "completed" {
        return Ok(());
    }
    let amount_units = usd_to_units(usage.actual_total_cost_usd);
    if amount_units == 0 {
        return Ok(());
    }
    let Some(api_key_id) = usage.api_key_id.as_deref().and_then(non_empty) else {
        return Ok(());
    };
    let finalized_at = usage
        .finalized_at_unix_secs
        .unwrap_or(usage.updated_at_unix_secs);
    let Some(finalized_at) = DateTime::<Utc>::from_timestamp(finalized_at as i64, 0) else {
        return Ok(());
    };
    let timezone = app_timezone();
    let (_, start, end) = local_day_window(finalized_at, timezone);
    let bucket = start.timestamp().max(0) as u64;
    let key_scope_key = daily_usage_key_scope_key(api_key_id, bucket);
    let is_standalone = usage
        .request_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("api_key_is_standalone"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let user_scope_key = (!is_standalone)
        .then(|| usage.user_id.as_deref().and_then(non_empty))
        .flatten()
        .map(|user_id| daily_usage_user_scope_key(user_id, bucket));
    let now = Utc::now().timestamp().max(0) as u64;
    let ttl_seconds = (end.timestamp().max(0) as u64)
        .saturating_sub(now)
        .saturating_add(COUNTER_EXPIRY_GRACE_SECONDS)
        .max(1);
    runtime_state
        .increment_daily_usage_limit(DailyUsageLimitIncrementInput {
            user_key: user_scope_key.as_deref(),
            key_key: &key_scope_key,
            bucket,
            amount_units,
            ttl_seconds,
        })
        .await?;
    Ok(())
}

fn scope_status(scope: &'static str, limit_usd: f64, used_usd: f64) -> DailyUsageScopeStatus {
    DailyUsageScopeStatus {
        scope,
        limit_usd,
        used_usd,
        remaining_usd: (limit_usd - used_usd).max(0.0),
    }
}

fn daily_usage_user_scope_key(user_id: &str, bucket: u64) -> String {
    format!("daily_usage_limit:user:{user_id}:{bucket}")
}

fn daily_usage_key_scope_key(api_key_id: &str, bucket: u64) -> String {
    format!("daily_usage_limit:key:{api_key_id}:{bucket}")
}

fn usd_to_units(value: f64) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    (value * USD_UNITS_PER_DOLLAR)
        .round()
        .clamp(0.0, u64::MAX as f64) as u64
}

fn units_to_usd(value: u64) -> f64 {
    value as f64 / USD_UNITS_PER_DOLLAR
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

pub(crate) fn parse_system_limit(value: Option<serde_json::Value>) -> Result<f64, GatewayError> {
    let limit = match value {
        None | Some(serde_json::Value::Null) => 0.0,
        Some(serde_json::Value::Number(value)) => value.as_f64().ok_or_else(|| {
            GatewayError::Internal("invalid system config daily_usage_limit_usd".to_string())
        })?,
        Some(serde_json::Value::String(value)) => value.parse::<f64>().map_err(|_| {
            GatewayError::Internal("invalid system config daily_usage_limit_usd".to_string())
        })?,
        Some(_) => {
            return Err(GatewayError::Internal(
                "invalid system config daily_usage_limit_usd".to_string(),
            ))
        }
    };
    if !limit.is_finite() || limit < 0.0 {
        return Err(GatewayError::Internal(
            "invalid system config daily_usage_limit_usd".to_string(),
        ));
    }
    Ok(limit)
}

fn positive_limit(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then_some(value)
}

fn resolve_scope_limits(
    is_standalone: bool,
    user_limit: Option<f64>,
    key_limit: Option<f64>,
    system_limit: f64,
) -> (Option<f64>, Option<f64>) {
    if is_standalone {
        (None, positive_limit(key_limit.unwrap_or(system_limit)))
    } else {
        (
            positive_limit(user_limit.unwrap_or(system_limit)),
            key_limit.and_then(positive_limit),
        )
    }
}

fn rfc3339(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::{
        daily_usage_key_scope_key, daily_usage_user_scope_key, parse_system_limit, positive_limit,
        record_finalized_daily_usage, recover_daily_usage_runtime, resolve_scope_limits,
        usd_to_units, FrontdoorDailyUsageLimiter, FrontdoorDailyUsageOutcome,
        DAILY_USAGE_RUNTIME_STATE_KEY,
    };
    use crate::control::{GatewayControlAuthContext, GatewayControlDecision};
    use crate::AppState;
    use aether_data::repository::usage::InMemoryUsageReadRepository;
    use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
    use aether_runtime_state::DailyUsageLimitIncrementInput;
    use std::sync::Arc;

    fn sample_decision(user_limit: Option<f64>, key_limit: Option<f64>) -> GatewayControlDecision {
        let mut decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        );
        decision.auth_context = Some(GatewayControlAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "key-1".to_string(),
            username: Some("alice".to_string()),
            api_key_name: Some("default".to_string()),
            balance_remaining: None,
            access_allowed: true,
            user_rate_limit: None,
            api_key_rate_limit: None,
            user_daily_usage_limit_usd: user_limit,
            api_key_daily_usage_limit_usd: key_limit,
            api_key_is_standalone: false,
            admin_bypass_limits: false,
            ip_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
        });
        decision
    }

    async fn state_with_daily_usage(actual_cost_usd: f64) -> AppState {
        let state = AppState::new().expect("state should build");
        let now = chrono::Utc::now();
        let (_, start, end) =
            crate::app_timezone::local_day_window(now, crate::app_timezone::app_timezone());
        let bucket = start.timestamp().max(0) as u64;
        let user_key = daily_usage_user_scope_key("user-1", bucket);
        let key_key = daily_usage_key_scope_key("key-1", bucket);
        state
            .runtime_state
            .increment_daily_usage_limit(DailyUsageLimitIncrementInput {
                user_key: Some(&user_key),
                key_key: &key_key,
                bucket,
                amount_units: usd_to_units(actual_cost_usd),
                ttl_seconds: (end - now).num_seconds().max(1) as u64,
            })
            .await
            .expect("daily usage counter should update");
        state
            .runtime_state
            .kv_set(DAILY_USAGE_RUNTIME_STATE_KEY, "ready", None)
            .await
            .expect("daily usage runtime should be ready");
        state
    }

    fn finalized_usage(
        request_id: &str,
        user_id: &str,
        api_key_id: &str,
        actual_cost_usd: f64,
    ) -> StoredRequestUsageAudit {
        let now = chrono::Utc::now();
        StoredRequestUsageAudit::new(
            format!("usage-{request_id}"),
            request_id.to_string(),
            Some(user_id.to_string()),
            Some(api_key_id.to_string()),
            None,
            None,
            "OpenAI".to_string(),
            "gpt-5".to_string(),
            None,
            None,
            None,
            None,
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            false,
            false,
            10,
            10,
            20,
            actual_cost_usd,
            actual_cost_usd,
            Some(200),
            None,
            None,
            Some(100),
            Some(20),
            "completed".to_string(),
            "settled".to_string(),
            now.timestamp_millis(),
            now.timestamp(),
            Some(now.timestamp()),
        )
        .expect("usage should build")
    }

    #[test]
    fn system_limit_defaults_to_unlimited_and_accepts_numbers_or_strings() {
        assert_eq!(parse_system_limit(None).unwrap(), 0.0);
        assert_eq!(
            parse_system_limit(Some(serde_json::Value::Null)).unwrap(),
            0.0
        );
        assert_eq!(
            parse_system_limit(Some(serde_json::json!(12.5))).unwrap(),
            12.5
        );
        assert_eq!(
            parse_system_limit(Some(serde_json::json!("8.25"))).unwrap(),
            8.25
        );
    }

    #[test]
    fn system_limit_rejects_invalid_or_negative_values() {
        for value in [
            serde_json::json!(-1),
            serde_json::json!("invalid"),
            serde_json::json!({ "limit": 1 }),
        ] {
            assert!(parse_system_limit(Some(value)).is_err());
        }
    }

    #[test]
    fn zero_is_unlimited_and_positive_values_are_limits() {
        assert_eq!(positive_limit(0.0), None);
        assert_eq!(positive_limit(-1.0), None);
        assert_eq!(positive_limit(f64::NAN), None);
        assert_eq!(positive_limit(0.01), Some(0.01));
    }

    #[test]
    fn normal_key_limit_only_adds_a_narrower_key_scope() {
        assert_eq!(
            resolve_scope_limits(false, None, None, 10.0),
            (Some(10.0), None)
        );
        assert_eq!(
            resolve_scope_limits(false, Some(20.0), Some(5.0), 10.0),
            (Some(20.0), Some(5.0))
        );
        assert_eq!(
            resolve_scope_limits(false, Some(20.0), Some(0.0), 10.0),
            (Some(20.0), None)
        );
    }

    #[test]
    fn standalone_key_inherits_or_explicitly_overrides_system_limit() {
        assert_eq!(
            resolve_scope_limits(true, None, None, 10.0),
            (None, Some(10.0))
        );
        assert_eq!(
            resolve_scope_limits(true, None, Some(0.0), 10.0),
            (None, None)
        );
        assert_eq!(
            resolve_scope_limits(true, None, Some(3.0), 10.0),
            (None, Some(3.0))
        );
    }

    #[tokio::test]
    async fn daily_usage_below_limit_is_allowed_and_at_limit_is_rejected() {
        let decision = sample_decision(Some(1.0), None);

        assert_eq!(
            FrontdoorDailyUsageLimiter::new()
                .check(&state_with_daily_usage(0.99).await, &decision)
                .await,
            FrontdoorDailyUsageOutcome::Allowed
        );
        match FrontdoorDailyUsageLimiter::new()
            .check(&state_with_daily_usage(1.0).await, &decision)
            .await
        {
            FrontdoorDailyUsageOutcome::Rejected(rejection) => {
                assert_eq!(rejection.scope, "user");
                assert_eq!(rejection.limit_usd, 1.0);
                assert_eq!(rejection.used_usd, 1.0);
                assert_eq!(rejection.remaining_usd, 0.0);
            }
            other => panic!("expected daily usage rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn usage_is_accumulated_before_a_limit_is_enabled() {
        let state = AppState::new().expect("state should build");
        record_finalized_daily_usage(
            &state.runtime_state,
            &finalized_usage("request-before-limit", "user-1", "key-1", 1.0),
        )
        .await
        .expect("daily usage should be recorded while unlimited");
        state
            .runtime_state
            .kv_set(DAILY_USAGE_RUNTIME_STATE_KEY, "ready", None)
            .await
            .expect("daily usage runtime should be ready");

        match FrontdoorDailyUsageLimiter::new()
            .check(&state, &sample_decision(Some(1.0), None))
            .await
        {
            FrontdoorDailyUsageOutcome::Rejected(rejection) => {
                assert_eq!(rejection.scope, "user");
                assert_eq!(rejection.used_usd, 1.0);
            }
            other => panic!("expected accumulated usage rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn lost_daily_usage_runtime_is_restored_with_one_grouped_recovery() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::seed([finalized_usage(
            "request-restore",
            "user-1",
            "key-1",
            1.25,
        )]));
        let state = AppState::new()
            .expect("state should build")
            .with_usage_data_reader_for_tests(usage_repository);
        let decision = sample_decision(Some(1.0), None);

        assert!(recover_daily_usage_runtime(&state)
            .await
            .expect("daily usage runtime recovery"));

        match FrontdoorDailyUsageLimiter::new()
            .check(&state, &decision)
            .await
        {
            FrontdoorDailyUsageOutcome::Rejected(rejection) => {
                assert_eq!(rejection.scope, "user");
                assert_eq!(rejection.used_usd, 1.25);
            }
            other => panic!("expected restored daily usage rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn key_scope_can_narrow_a_normal_user_limit() {
        let limiter = FrontdoorDailyUsageLimiter::new();
        let decision = sample_decision(Some(10.0), Some(0.5));

        match limiter
            .check(&state_with_daily_usage(0.5).await, &decision)
            .await
        {
            FrontdoorDailyUsageOutcome::Rejected(rejection) => {
                assert_eq!(rejection.scope, "key");
                assert_eq!(rejection.limit_usd, 0.5);
            }
            other => panic!("expected key daily usage rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn admin_and_ip_bypass_skip_daily_usage_runtime_reads() {
        for field in ["admin", "ip"] {
            let limiter = FrontdoorDailyUsageLimiter::new();
            let mut decision = sample_decision(Some(1.0), None);
            let auth = decision.auth_context.as_mut().unwrap();
            auth.admin_bypass_limits = field == "admin";
            auth.ip_bypass_limits = field == "ip";

            assert_eq!(
                limiter
                    .check(&AppState::new().expect("state should build"), &decision)
                    .await,
                FrontdoorDailyUsageOutcome::NotApplicable
            );
        }
    }
}
