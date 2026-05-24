use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_cache::ExpiringMap;
use aether_runtime_state::{RateLimitCheck, RateLimitInput, RateLimitScope};
use tokio::sync::Mutex;
use tracing::warn;

use crate::control::GatewayControlDecision;
use crate::{AppState, GatewayError};

const SYSTEM_RPM_CONFIG_KEY: &str = "rate_limit_per_minute";
const SYSTEM_RPM_CONFIG_CACHE_TTL: Duration = Duration::from_secs(15);
const SYSTEM_RPM_CONFIG_CACHE_MAX_ENTRIES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontdoorUserRpmConfig {
    bucket_seconds: u64,
    key_ttl_seconds: u64,
    fail_open: bool,
    allow_local_fallback: bool,
}

impl Default for FrontdoorUserRpmConfig {
    fn default() -> Self {
        Self {
            bucket_seconds: 60,
            key_ttl_seconds: 120,
            fail_open: true,
            allow_local_fallback: true,
        }
    }
}

impl FrontdoorUserRpmConfig {
    pub fn new(bucket_seconds: u64, key_ttl_seconds: u64, fail_open: bool) -> Self {
        Self {
            bucket_seconds: bucket_seconds.max(1),
            key_ttl_seconds: key_ttl_seconds.max(1),
            fail_open,
            allow_local_fallback: true,
        }
    }

    pub(crate) fn bucket_seconds(&self) -> u64 {
        self.bucket_seconds
    }

    pub(crate) fn key_ttl_seconds(&self) -> u64 {
        self.key_ttl_seconds
    }

    pub(crate) fn fail_open(&self) -> bool {
        self.fail_open
    }

    pub fn allow_local_fallback(&self) -> bool {
        self.allow_local_fallback
    }

    pub fn with_local_fallback(mut self, allow_local_fallback: bool) -> Self {
        self.allow_local_fallback = allow_local_fallback;
        self
    }

    fn current_bucket(&self, now_ts: u64) -> u64 {
        now_ts / self.bucket_seconds
    }

    fn retry_after(&self, now_ts: u64) -> u64 {
        let elapsed = now_ts % self.bucket_seconds;
        (self.bucket_seconds - elapsed).max(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontdoorUserRpmRejection {
    pub(crate) scope: &'static str,
    pub(crate) limit: u32,
    pub(crate) retry_after: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrontdoorUserRpmOutcome {
    NotApplicable,
    Allowed,
    Rejected(FrontdoorUserRpmRejection),
}

#[derive(Debug, Clone)]
pub(crate) struct FrontdoorUserRpmLimiter {
    config: FrontdoorUserRpmConfig,
    memory_counts: Arc<Mutex<HashMap<String, (u64, u32)>>>,
    system_default_cache: Arc<ExpiringMap<String, u32>>,
    #[cfg(test)]
    system_default_override: Arc<StdMutex<Option<u32>>>,
}

impl FrontdoorUserRpmLimiter {
    pub(crate) fn new(config: FrontdoorUserRpmConfig) -> Self {
        Self {
            config,
            memory_counts: Arc::new(Mutex::new(HashMap::new())),
            system_default_cache: Arc::new(ExpiringMap::default()),
            #[cfg(test)]
            system_default_override: Arc::new(StdMutex::new(None)),
        }
    }

    pub(crate) fn config(&self) -> &FrontdoorUserRpmConfig {
        &self.config
    }

    pub(crate) async fn current_system_default_limit(
        &self,
        state: &AppState,
    ) -> Result<u32, GatewayError> {
        self.resolve_system_default_limit(state).await
    }

    pub(crate) fn clear_system_default_cache(&self) {
        self.system_default_cache.clear();
    }

    pub(crate) fn current_bucket(&self, now_ts: u64) -> u64 {
        self.config.current_bucket(now_ts)
    }

    pub(crate) fn retry_after(&self, now_ts: u64) -> u64 {
        self.config.retry_after(now_ts)
    }

    pub(crate) fn user_scope_key(&self, user_id: &str, bucket: u64) -> String {
        format!("rpm:user:{user_id}:{bucket}")
    }

    pub(crate) fn key_scope_key(&self, api_key_id: &str, bucket: u64) -> String {
        format!("rpm:key:{api_key_id}:{bucket}")
    }

    pub(crate) fn standalone_scope_key(&self, api_key_id: &str, bucket: u64) -> String {
        format!("rpm:ukey:{api_key_id}:{bucket}")
    }

    pub(crate) async fn get_scope_count(
        &self,
        state: &AppState,
        scope_key: &str,
        bucket: u64,
    ) -> Result<u32, GatewayError> {
        match state.runtime_state.kv_get(scope_key).await {
            Ok(raw) => return Ok(raw.and_then(|value| value.parse::<u32>().ok()).unwrap_or(0)),
            Err(err) if !self.config.allow_local_fallback() => {
                return Err(GatewayError::Internal(format!(
                    "frontdoor user rpm runtime read failed: {err}"
                )));
            }
            Err(err) => {
                warn!(
                    error = ?err,
                    scope_key = %scope_key,
                    "frontdoor user rpm runtime count read failed; using local fallback"
                );
            }
        }

        let counts = self.memory_counts.lock().await;
        Ok(counts
            .get(scope_key)
            .filter(|(stored_bucket, _)| *stored_bucket == bucket)
            .map(|(_, count)| *count)
            .unwrap_or(0))
    }

    pub(crate) async fn check_and_consume(
        &self,
        state: &AppState,
        decision: Option<&GatewayControlDecision>,
    ) -> Result<FrontdoorUserRpmOutcome, GatewayError> {
        let Some(decision) = decision else {
            return Ok(FrontdoorUserRpmOutcome::NotApplicable);
        };
        if decision.route_class.as_deref() != Some("ai_public") {
            return Ok(FrontdoorUserRpmOutcome::NotApplicable);
        }

        let system_default_limit = self.resolve_system_default_limit(state).await?;
        let Some(plan) = RpmPlan::from_decision(decision, &self.config, system_default_limit)
        else {
            return Ok(FrontdoorUserRpmOutcome::NotApplicable);
        };

        if plan.user_rpm_limit == 0 && plan.key_rpm_limit == 0 {
            return Ok(FrontdoorUserRpmOutcome::Allowed);
        }

        match self.check_and_consume_runtime(state, &plan).await {
            Ok(outcome) => return Ok(outcome),
            Err(err) => {
                warn!(
                    error = ?err,
                    user_rpm_key = %plan.user_rpm_key,
                    key_rpm_key = %plan.key_rpm_key,
                    "frontdoor user rpm runtime check failed"
                );
                if self.config.fail_open() {
                    return Ok(FrontdoorUserRpmOutcome::NotApplicable);
                }
                if !self.config.allow_local_fallback() {
                    return Err(GatewayError::Internal(
                        "frontdoor user rpm runtime backend is unavailable and local fallback is disabled for the current deployment mode".to_string(),
                    ));
                }
            }
        }

        if !self.config.allow_local_fallback() {
            if self.config.fail_open() {
                return Ok(FrontdoorUserRpmOutcome::NotApplicable);
            }
            return Err(GatewayError::Internal(
                "frontdoor user rpm requires shared runtime state in the current deployment mode"
                    .to_string(),
            ));
        }

        Ok(self.check_and_consume_memory(&plan).await)
    }

    async fn resolve_system_default_limit(&self, state: &AppState) -> Result<u32, GatewayError> {
        #[cfg(test)]
        if let Ok(guard) = self.system_default_override.lock() {
            if let Some(limit) = *guard {
                return Ok(limit);
            }
        }

        let cache_key = SYSTEM_RPM_CONFIG_KEY.to_string();
        if let Some(limit) = self
            .system_default_cache
            .get_fresh(&cache_key, SYSTEM_RPM_CONFIG_CACHE_TTL)
        {
            return Ok(limit);
        }

        let limit = parse_system_default_limit(
            state
                .read_system_config_json_value(SYSTEM_RPM_CONFIG_KEY)
                .await?,
        )?;
        self.system_default_cache.insert(
            cache_key,
            limit,
            SYSTEM_RPM_CONFIG_CACHE_TTL,
            SYSTEM_RPM_CONFIG_CACHE_MAX_ENTRIES,
        );
        Ok(limit)
    }

    #[cfg(test)]
    pub(crate) fn with_system_default_limit_for_tests(self, limit: u32) -> Self {
        if let Ok(mut guard) = self.system_default_override.lock() {
            *guard = Some(limit);
        }
        self
    }

    async fn check_and_consume_runtime(
        &self,
        state: &AppState,
        plan: &RpmPlan,
    ) -> Result<FrontdoorUserRpmOutcome, GatewayError> {
        let result = state
            .runtime_state
            .check_and_consume_rate_limit(RateLimitInput {
                user_key: &plan.user_rpm_key,
                key_key: &plan.key_rpm_key,
                bucket: plan.bucket,
                user_limit: plan.user_rpm_limit,
                key_limit: plan.key_rpm_limit,
                ttl_seconds: self.config.key_ttl_seconds(),
            })
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;

        if matches!(result, RateLimitCheck::Allowed { .. }) {
            return Ok(FrontdoorUserRpmOutcome::Allowed);
        }

        let RateLimitCheck::Rejected { scope, limit } = result else {
            unreachable!("allowed returned above");
        };
        Ok(FrontdoorUserRpmOutcome::Rejected(
            FrontdoorUserRpmRejection {
                scope: match scope {
                    RateLimitScope::User => "user",
                    RateLimitScope::Key => "key",
                },
                limit,
                retry_after: plan.retry_after,
            },
        ))
    }

    async fn check_and_consume_memory(&self, plan: &RpmPlan) -> FrontdoorUserRpmOutcome {
        let mut counts = self.memory_counts.lock().await;
        counts.retain(|_, (bucket, _)| *bucket >= plan.bucket);

        if plan.user_rpm_limit > 0 {
            let user_count = counts
                .get(&plan.user_rpm_key)
                .map(|(_, count)| *count)
                .unwrap_or_default();
            if user_count >= plan.user_rpm_limit {
                return FrontdoorUserRpmOutcome::Rejected(FrontdoorUserRpmRejection {
                    scope: "user",
                    limit: plan.user_rpm_limit,
                    retry_after: plan.retry_after,
                });
            }
        }

        if plan.key_rpm_limit > 0 {
            let key_count = counts
                .get(&plan.key_rpm_key)
                .map(|(_, count)| *count)
                .unwrap_or_default();
            if key_count >= plan.key_rpm_limit {
                return FrontdoorUserRpmOutcome::Rejected(FrontdoorUserRpmRejection {
                    scope: "key",
                    limit: plan.key_rpm_limit,
                    retry_after: plan.retry_after,
                });
            }
        }

        if plan.user_rpm_limit > 0 {
            let next = counts
                .get(&plan.user_rpm_key)
                .map(|(_, count)| count.saturating_add(1))
                .unwrap_or(1);
            counts.insert(plan.user_rpm_key.clone(), (plan.bucket, next));
        }
        if plan.key_rpm_limit > 0 {
            let next = counts
                .get(&plan.key_rpm_key)
                .map(|(_, count)| count.saturating_add(1))
                .unwrap_or(1);
            counts.insert(plan.key_rpm_key.clone(), (plan.bucket, next));
        }

        FrontdoorUserRpmOutcome::Allowed
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RpmPlan {
    bucket: u64,
    retry_after: u64,
    user_rpm_key: String,
    user_rpm_limit: u32,
    key_rpm_key: String,
    key_rpm_limit: u32,
}

impl RpmPlan {
    fn from_decision(
        decision: &GatewayControlDecision,
        config: &FrontdoorUserRpmConfig,
        system_default_limit: u32,
    ) -> Option<Self> {
        let auth = decision.auth_context.as_ref()?;
        if auth.local_rejection.is_some() || auth.user_id.is_empty() || auth.api_key_id.is_empty() {
            return None;
        }
        if auth.admin_bypass_limits {
            return None;
        }

        let now_ts = current_unix_secs();
        let bucket = config.current_bucket(now_ts);
        let retry_after = config.retry_after(now_ts);

        if auth.api_key_is_standalone {
            return Some(Self {
                bucket,
                retry_after,
                user_rpm_key: format!("rpm:ukey:{}:{bucket}", auth.api_key_id),
                user_rpm_limit: auth
                    .api_key_rate_limit
                    .map(|value| normalize_limit(Some(value)))
                    .unwrap_or(system_default_limit),
                key_rpm_key: format!("rpm:key:{}:{bucket}", auth.api_key_id),
                key_rpm_limit: 0,
            });
        }

        Some(Self {
            bucket,
            retry_after,
            user_rpm_key: format!("rpm:user:{}:{bucket}", auth.user_id),
            user_rpm_limit: auth
                .user_rate_limit
                .map(|value| normalize_limit(Some(value)))
                .unwrap_or(system_default_limit),
            key_rpm_key: format!("rpm:key:{}:{bucket}", auth.api_key_id),
            key_rpm_limit: normalize_limit(auth.api_key_rate_limit),
        })
    }
}

fn normalize_limit(value: Option<i32>) -> u32 {
    value
        .map(|value| value.max(0))
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_default()
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parse_system_default_limit(value: Option<serde_json::Value>) -> Result<u32, GatewayError> {
    let normalized = match value {
        None | Some(serde_json::Value::Null) => 0_i64,
        Some(serde_json::Value::Bool(value)) => {
            if value {
                1
            } else {
                0
            }
        }
        Some(serde_json::Value::Number(value)) => {
            if let Some(value) = value.as_i64() {
                value
            } else if let Some(value) = value.as_u64() {
                i64::try_from(value).unwrap_or(i64::MAX)
            } else if let Some(value) = value.as_f64() {
                if !value.is_finite() {
                    return Err(GatewayError::Internal(
                        "invalid system config rate_limit_per_minute".to_string(),
                    ));
                }
                value.trunc() as i64
            } else {
                return Err(GatewayError::Internal(
                    "invalid system config rate_limit_per_minute".to_string(),
                ));
            }
        }
        Some(serde_json::Value::String(value)) => value.parse::<i64>().map_err(|_| {
            GatewayError::Internal("invalid system config rate_limit_per_minute".to_string())
        })?,
        Some(_) => {
            return Err(GatewayError::Internal(
                "invalid system config rate_limit_per_minute".to_string(),
            ));
        }
    };

    Ok(u32::try_from(normalized.max(0)).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::{FrontdoorUserRpmConfig, FrontdoorUserRpmLimiter, FrontdoorUserRpmOutcome};
    use crate::control::GatewayControlAuthContext;
    use crate::control::GatewayControlDecision;
    use crate::AppState;

    fn sample_decision(auth_context: GatewayControlAuthContext) -> GatewayControlDecision {
        GatewayControlDecision {
            public_path: "/v1/chat/completions".to_string(),
            public_query_string: None,
            route_class: Some("ai_public".to_string()),
            route_family: Some("openai".to_string()),
            route_kind: Some("chat".to_string()),
            request_auth_channel: None,
            auth_endpoint_signature: Some("openai:chat".to_string()),
            execution_runtime_candidate: true,
            auth_context: Some(auth_context),
            admin_principal: None,
            local_auth_rejection: None,
        }
    }

    #[tokio::test]
    async fn limiter_uses_memory_fallback_for_explicit_user_rate_limit() {
        let limiter = FrontdoorUserRpmLimiter::new(FrontdoorUserRpmConfig::new(60, 120, false));
        let decision = sample_decision(GatewayControlAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "key-1".to_string(),
            username: None,
            api_key_name: None,
            balance_remaining: Some(10.0),
            access_allowed: true,
            user_rate_limit: Some(1),
            api_key_rate_limit: Some(10),
            api_key_is_standalone: false,
            admin_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
        });
        let state = AppState::new().expect("state should build for tests");

        let first = limiter
            .check_and_consume(&state, Some(&decision))
            .await
            .expect("check should succeed");
        assert_eq!(first, FrontdoorUserRpmOutcome::Allowed);

        let second = limiter
            .check_and_consume(&state, Some(&decision))
            .await
            .expect("check should succeed");
        match second {
            FrontdoorUserRpmOutcome::Rejected(rejection) => {
                assert_eq!(rejection.scope, "user");
                assert_eq!(rejection.limit, 1);
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn limiter_uses_system_default_for_missing_user_rate_limit() {
        let limiter = FrontdoorUserRpmLimiter::new(FrontdoorUserRpmConfig::new(60, 120, false))
            .with_system_default_limit_for_tests(1);
        let decision = sample_decision(GatewayControlAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "key-1".to_string(),
            username: None,
            api_key_name: None,
            balance_remaining: Some(10.0),
            access_allowed: true,
            user_rate_limit: None,
            api_key_rate_limit: Some(10),
            api_key_is_standalone: false,
            admin_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
        });
        let state = AppState::new().expect("state should build for tests");

        let first = limiter
            .check_and_consume(&state, Some(&decision))
            .await
            .expect("check should succeed");
        assert_eq!(first, FrontdoorUserRpmOutcome::Allowed);

        let second = limiter
            .check_and_consume(&state, Some(&decision))
            .await
            .expect("check should succeed");
        match second {
            FrontdoorUserRpmOutcome::Rejected(rejection) => {
                assert_eq!(rejection.scope, "user");
                assert_eq!(rejection.limit, 1);
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn limiter_skips_admin_bypass_context() {
        let limiter = FrontdoorUserRpmLimiter::new(FrontdoorUserRpmConfig::new(60, 120, false))
            .with_system_default_limit_for_tests(1);
        let decision = sample_decision(GatewayControlAuthContext {
            user_id: "admin-1".to_string(),
            api_key_id: "key-1".to_string(),
            username: None,
            api_key_name: None,
            balance_remaining: Some(10.0),
            access_allowed: true,
            user_rate_limit: Some(1),
            api_key_rate_limit: Some(1),
            api_key_is_standalone: false,
            admin_bypass_limits: true,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
        });
        let state = AppState::new().expect("state should build for tests");

        let first = limiter
            .check_and_consume(&state, Some(&decision))
            .await
            .expect("check should succeed");
        let second = limiter
            .check_and_consume(&state, Some(&decision))
            .await
            .expect("check should succeed");

        assert_eq!(first, FrontdoorUserRpmOutcome::NotApplicable);
        assert_eq!(second, FrontdoorUserRpmOutcome::NotApplicable);
    }

    #[test]
    fn config_normalizes_non_positive_values() {
        let config = FrontdoorUserRpmConfig::new(0, 0, true);
        assert_eq!(config.bucket_seconds(), 1);
        assert_eq!(config.key_ttl_seconds(), 1);
        assert!(config.fail_open());
        assert!(config.allow_local_fallback());
    }

    #[tokio::test]
    async fn limiter_uses_runtime_state_when_local_fallback_disabled() {
        let limiter = FrontdoorUserRpmLimiter::new(
            FrontdoorUserRpmConfig::new(60, 120, false).with_local_fallback(false),
        );
        let decision = sample_decision(GatewayControlAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "key-1".to_string(),
            username: None,
            api_key_name: None,
            balance_remaining: Some(10.0),
            access_allowed: true,
            user_rate_limit: Some(1),
            api_key_rate_limit: Some(10),
            api_key_is_standalone: false,
            admin_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
        });
        let state = AppState::new().expect("state should build for tests");

        let first = limiter
            .check_and_consume(&state, Some(&decision))
            .await
            .expect("runtime check should succeed");
        assert_eq!(first, FrontdoorUserRpmOutcome::Allowed);

        let second = limiter
            .check_and_consume(&state, Some(&decision))
            .await
            .expect("runtime check should succeed");
        match second {
            FrontdoorUserRpmOutcome::Rejected(rejection) => {
                assert_eq!(rejection.scope, "user");
                assert_eq!(rejection.limit, 1);
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }
}
