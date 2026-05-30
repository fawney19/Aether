use super::keys::{
    pool_cooldown_index_key, pool_cooldown_key, pool_cost_key, pool_latency_key, pool_lru_key,
    pool_sticky_key, pool_stream_timeout_key,
};
use crate::handlers::admin::provider::pool::config::admin_provider_pool_cache_affinity_enabled;
use crate::handlers::admin::provider::shared::support::{
    admin_provider_pool_quota_probe_active_members_key, AdminProviderPoolConfig,
    AdminProviderPoolUnschedulableRule,
};
use aether_runtime_state::RuntimeState;
use regex::Regex;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;
use uuid::Uuid;

const MAX_POOL_COOLDOWN_SECONDS: u64 = 32 * 60;

const ACCOUNT_DISABLE_PATTERNS: &[&str] = &[
    "organization has been disabled",
    "organization disabled",
    "organization_disabled",
    "account has been disabled",
    "account disabled",
    "account_disabled",
    "account has been deactivated",
    "account_deactivated",
    "account deactivated",
];

const WORKSPACE_DISABLE_PATTERNS: &[&str] = &[
    "deactivated_workspace",
    "workspace has been disabled",
    "workspace disabled",
    "workspace has been deactivated",
    "workspace deactivated",
    "workspace is disabled",
    "workspace is deactivated",
];

const FORBIDDEN_ACCOUNT_PATTERNS: &[&str] = &[
    "account suspended",
    "account suspend",
    "account banned",
    "account blocked",
    "account forbidden",
    "account deactivated",
    "account access denied",
    "subscription inactive",
    "suspended",
    "banned",
    "blocked",
    "deactivated",
    "access denied",
];

fn current_unix_secs_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn enabled_pool_presets(pool_config: &AdminProviderPoolConfig) -> impl Iterator<Item = &str> {
    pool_config
        .scheduling_presets
        .iter()
        .filter(|item| item.enabled)
        .map(|item| item.preset.as_str())
}

fn should_touch_lru(pool_config: &AdminProviderPoolConfig) -> bool {
    pool_config.lru_enabled || enabled_pool_presets(pool_config).next().is_some()
}

fn should_record_latency(pool_config: &AdminProviderPoolConfig) -> bool {
    enabled_pool_presets(pool_config).any(|preset| !preset.eq_ignore_ascii_case("lru"))
}

fn oauth_cache_key(key_id: &str) -> String {
    format!("provider_oauth_token_cache:{key_id}")
}

fn parse_retry_after_seconds(headers: Option<&BTreeMap<String, String>>) -> Option<u64> {
    let raw = headers.and_then(|headers| {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("retry-after"))
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
    })?;
    let seconds = raw.parse::<u64>().ok()?;
    Some(seconds.clamp(1, MAX_POOL_COOLDOWN_SECONDS))
}

fn parse_google_quota_duration_seconds(raw: &serde_json::Value) -> Option<u64> {
    match raw {
        serde_json::Value::Number(number) => {
            let seconds = number.as_f64()?;
            Some((seconds.max(1.0).floor() as u64).max(1))
        }
        serde_json::Value::String(text) => {
            let trimmed = text.trim().to_ascii_lowercase();
            if trimmed.is_empty() {
                return None;
            }
            let regex = Regex::new(r"(\d+(?:\.\d+)?)([dhms])").ok()?;
            let mut total_seconds = 0.0;
            let mut matched = false;
            for capture in regex.captures_iter(&trimmed) {
                let amount = capture.get(1)?.as_str().parse::<f64>().ok()?;
                let unit = capture.get(2)?.as_str();
                matched = true;
                total_seconds += match unit {
                    "d" => amount * 86_400.0,
                    "h" => amount * 3_600.0,
                    "m" => amount * 60.0,
                    "s" => amount,
                    _ => 0.0,
                };
            }
            matched.then(|| (total_seconds.max(1.0).floor() as u64).max(1))
        }
        _ => None,
    }
}

fn parse_google_quota_cooldown_seconds_at(
    error_body: Option<&str>,
    now_unix_secs: u64,
) -> Option<u64> {
    let error_body = error_body
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let payload = serde_json::from_str::<serde_json::Value>(error_body).ok()?;
    let error = payload.get("error")?.as_object()?;

    if let Some(details) = error.get("details").and_then(serde_json::Value::as_array) {
        for detail in details {
            let Some(metadata) = detail
                .get("metadata")
                .and_then(serde_json::Value::as_object)
            else {
                continue;
            };

            if let Some(reset_at_text) = metadata
                .get("quotaResetTimeStamp")
                .or_else(|| metadata.get("quotaResetTimestamp"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(reset_at_text) {
                    let reset_at = parsed.timestamp().max(0) as u64;
                    return Some(reset_at.saturating_sub(now_unix_secs).max(1));
                }
            }

            if let Some(delay) = metadata
                .get("quotaResetDelay")
                .and_then(parse_google_quota_duration_seconds)
            {
                return Some(delay.max(1));
            }
        }
    }

    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let regex = Regex::new(r"(?i)reset after\s+([^.,;]+)").ok()?;
    let capture = regex.captures(message)?;
    parse_google_quota_duration_seconds(&serde_json::Value::String(
        capture.get(1)?.as_str().to_string(),
    ))
}

fn parse_google_quota_cooldown_seconds(error_body: Option<&str>) -> Option<u64> {
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    parse_google_quota_cooldown_seconds_at(error_body, now_unix_secs)
}

fn extract_error_message(error_body: Option<&str>) -> String {
    let Some(error_body) = error_body.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };

    serde_json::from_str::<serde_json::Value>(error_body)
        .ok()
        .and_then(|value| {
            value
                .as_object()
                .and_then(|object| object.get("error").or_else(|| object.get("message")))
                .and_then(|error| match error {
                    serde_json::Value::Object(object) => {
                        first_error_text(object, &["message", "detail", "reason", "code", "status"])
                    }
                    serde_json::Value::String(text) => Some(text.clone()),
                    _ => None,
                })
        })
        .unwrap_or_else(|| error_body.chars().take(500).collect())
}

fn first_error_text(
    object: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<String> {
    keys.iter().find_map(|key| {
        let text = object.get(*key).and_then(|value| match value {
            serde_json::Value::String(text) => Some(text.trim().to_string()),
            serde_json::Value::Number(number) => Some(number.to_string()),
            _ => None,
        })?;
        (!text.is_empty()).then_some(text)
    })
}

pub(crate) fn admin_provider_pool_key_terminal_error_reason(
    status_code: u16,
    error_body: Option<&str>,
) -> Option<String> {
    let error_message = extract_error_message(error_body).to_ascii_lowercase();
    if let Some(pattern) = WORKSPACE_DISABLE_PATTERNS
        .iter()
        .find(|pattern| error_message.contains(**pattern))
    {
        return Some(format!("workspace_deactivated_{status_code}:{pattern}"));
    }

    match status_code {
        401 if ACCOUNT_DISABLE_PATTERNS
            .iter()
            .any(|pattern| error_message.contains(pattern)) =>
        {
            Some("account_deactivated_401".to_string())
        }
        402 => Some("payment_required_402".to_string()),
        403 if FORBIDDEN_ACCOUNT_PATTERNS
            .iter()
            .any(|pattern| error_message.contains(pattern)) =>
        {
            Some("forbidden_403".to_string())
        }
        400 => ACCOUNT_DISABLE_PATTERNS
            .iter()
            .find(|pattern| error_message.contains(**pattern))
            .map(|pattern| format!("account_disabled_400:{pattern}")),
        423 if FORBIDDEN_ACCOUNT_PATTERNS
            .iter()
            .any(|pattern| error_message.contains(pattern)) =>
        {
            Some("account_locked_423".to_string())
        }
        _ => None,
    }
}

fn resolve_transient_cooldown_ttl(
    status_code: u16,
    retry_after_seconds: Option<u64>,
    pool_config: &AdminProviderPoolConfig,
) -> u64 {
    if matches!(status_code, 429 | 503) {
        if let Some(retry_after_seconds) = retry_after_seconds {
            return retry_after_seconds;
        }
    }
    if status_code == 429 {
        return pool_config.rate_limit_cooldown_seconds;
    }
    pool_config.overload_cooldown_seconds
}

async fn set_pool_cooldown(
    runtime: &RuntimeState,
    provider_id: &str,
    key_id: &str,
    reason: &str,
    ttl_seconds: u64,
) {
    if ttl_seconds == 0 {
        return;
    }
    let ttl_seconds = ttl_seconds.min(MAX_POOL_COOLDOWN_SECONDS);

    if let Err(err) = runtime
        .kv_set(
            &pool_cooldown_key(provider_id, key_id),
            reason.to_string(),
            Some(std::time::Duration::from_secs(ttl_seconds)),
        )
        .await
    {
        warn!(
            "gateway admin provider pool: failed to set cooldown for provider {provider_id} key {key_id}: {:?}",
            err
        );
    }
    let _ = runtime
        .set_add(&pool_cooldown_index_key(provider_id), key_id)
        .await;
    let _ = runtime
        .key_expire(
            &pool_cooldown_index_key(provider_id),
            std::time::Duration::from_secs(ttl_seconds.saturating_add(60)),
        )
        .await;
    spawn_remove_pool_active_probe_member(runtime, provider_id, key_id);
}

fn spawn_remove_pool_active_probe_member(runtime: &RuntimeState, provider_id: &str, key_id: &str) {
    let runtime = runtime.clone();
    let provider_id = provider_id.to_string();
    let key_id = key_id.to_string();
    tokio::spawn(async move {
        if let Err(err) = runtime
            .set_remove(
                &admin_provider_pool_quota_probe_active_members_key(&provider_id),
                &key_id,
            )
            .await
        {
            warn!(
                "gateway admin provider pool: failed to remove active probe member for provider {provider_id} key {key_id}: {:?}",
                err
            );
        }
    });
}

async fn invalidate_pool_oauth_cache(runtime: &RuntimeState, key_id: &str) {
    if let Err(err) = runtime.kv_delete(&oauth_cache_key(key_id)).await {
        warn!(
            "gateway admin provider pool: failed to invalidate oauth cache for key {key_id}: {:?}",
            err
        );
    }
}

fn matching_unschedulable_rule<'a>(
    rules: &'a [AdminProviderPoolUnschedulableRule],
    error_message: &str,
) -> Option<&'a AdminProviderPoolUnschedulableRule> {
    rules.iter().find(|rule| {
        let keyword = rule.keyword.trim().to_ascii_lowercase();
        !keyword.is_empty() && error_message.contains(keyword.as_str())
    })
}

pub(crate) async fn record_admin_provider_pool_success(
    runtime: &RuntimeState,
    provider_id: &str,
    key_id: &str,
    pool_config: &AdminProviderPoolConfig,
    sticky_session_token: Option<&str>,
    tokens_used: u64,
    ttfb_ms: Option<u64>,
) {
    let now = current_unix_secs_f64();

    if let Some(sticky_session_token) = sticky_session_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|_| pool_config.sticky_session_ttl_seconds > 0)
        .filter(|_| admin_provider_pool_cache_affinity_enabled(pool_config))
    {
        let _ = runtime
            .kv_set(
                &pool_sticky_key(provider_id, sticky_session_token),
                key_id.to_string(),
                Some(std::time::Duration::from_secs(
                    pool_config.sticky_session_ttl_seconds,
                )),
            )
            .await;
    }

    if should_touch_lru(pool_config) {
        let _ = runtime
            .score_set(&pool_lru_key(provider_id), key_id, now)
            .await;
    }

    if tokens_used > 0 && pool_config.cost_limit_per_key_tokens.is_some() {
        let cost_key = pool_cost_key(provider_id, key_id);
        let window_seconds = pool_config.cost_window_seconds.max(1);
        let member = format!("{}:{tokens_used}", Uuid::new_v4().simple());
        let _ = runtime.score_set(&cost_key, &member, now).await;
        let _ = runtime
            .score_remove_by_score(&cost_key, now - window_seconds as f64)
            .await;
        let _ = runtime
            .key_expire(
                &cost_key,
                std::time::Duration::from_secs(window_seconds.saturating_add(600)),
            )
            .await;
    }

    if let Some(ttfb_ms) = ttfb_ms
        .filter(|value| should_record_latency(pool_config))
        .filter(|_| pool_config.latency_window_seconds > 0)
    {
        let latency_key = pool_latency_key(provider_id, key_id);
        let window_seconds = pool_config.latency_window_seconds.max(1);
        let sample_limit = pool_config.latency_sample_limit.max(1);
        let member = format!("{}:{ttfb_ms}", Uuid::new_v4().simple());
        let _ = runtime.score_set(&latency_key, &member, now).await;
        let _ = runtime
            .score_remove_by_score(&latency_key, now - window_seconds as f64)
            .await;
        let _ = runtime
            .score_remove_by_rank(&latency_key, 0, -((sample_limit as i64) + 1))
            .await;
        let _ = runtime
            .key_expire(
                &latency_key,
                std::time::Duration::from_secs(window_seconds.saturating_add(600)),
            )
            .await;
    }
}

pub(crate) async fn record_admin_provider_pool_error(
    runtime: &RuntimeState,
    provider_id: &str,
    key_id: &str,
    pool_config: &AdminProviderPoolConfig,
    status_code: u16,
    error_body: Option<&str>,
    response_headers: Option<&BTreeMap<String, String>>,
) {
    if !pool_config.health_policy_enabled {
        return;
    }

    let error_message = extract_error_message(error_body).to_ascii_lowercase();

    if status_code == 401 {
        invalidate_pool_oauth_cache(runtime, key_id).await;
        spawn_remove_pool_active_probe_member(runtime, provider_id, key_id);
        return;
    }

    if status_code == 402 {
        spawn_remove_pool_active_probe_member(runtime, provider_id, key_id);
        return;
    }

    if status_code == 403 {
        if FORBIDDEN_ACCOUNT_PATTERNS
            .iter()
            .any(|pattern| error_message.contains(pattern))
        {
            spawn_remove_pool_active_probe_member(runtime, provider_id, key_id);
            return;
        }
        set_pool_cooldown(
            runtime,
            provider_id,
            key_id,
            "forbidden_403",
            pool_config.rate_limit_cooldown_seconds.max(300),
        )
        .await;
        return;
    }

    if status_code == 400 {
        // Bad Request is usually attributable to the caller payload, not key health.
        // Account-level 400s are handled by orchestration pool-score feedback.
        return;
    }

    if let Some(rule) =
        matching_unschedulable_rule(&pool_config.unschedulable_rules, &error_message)
    {
        let ttl_seconds = (rule.duration_minutes.max(1)).saturating_mul(60).max(60);
        set_pool_cooldown(
            runtime,
            provider_id,
            key_id,
            &format!("rule:{}", rule.keyword),
            ttl_seconds,
        )
        .await;
        return;
    }

    if status_code == 429 {
        let ttl_seconds = resolve_transient_cooldown_ttl(
            status_code,
            parse_retry_after_seconds(response_headers)
                .or_else(|| parse_google_quota_cooldown_seconds(error_body)),
            pool_config,
        );
        set_pool_cooldown(
            runtime,
            provider_id,
            key_id,
            "rate_limited_429",
            ttl_seconds,
        )
        .await;
        return;
    }

    if status_code == 529 {
        set_pool_cooldown(
            runtime,
            provider_id,
            key_id,
            "overloaded_529",
            pool_config.overload_cooldown_seconds,
        )
        .await;
        return;
    }

    let transient_reason = match status_code {
        408 => Some("request_timeout_408".to_string()),
        409 => Some("conflict_409".to_string()),
        423 => Some("locked_423".to_string()),
        425 => Some("too_early_425".to_string()),
        500 => Some("server_error_500".to_string()),
        502 => Some("bad_gateway_502".to_string()),
        503 => Some("service_unavailable_503".to_string()),
        504 => Some("gateway_timeout_504".to_string()),
        501 | 505..=599 => Some(format!("server_error_{status_code}")),
        _ => None,
    };

    if let Some(reason) = transient_reason {
        let ttl_seconds = resolve_transient_cooldown_ttl(
            status_code,
            parse_retry_after_seconds(response_headers),
            pool_config,
        );
        set_pool_cooldown(runtime, provider_id, key_id, &reason, ttl_seconds).await;
    }
}

pub(crate) async fn record_admin_provider_pool_stream_timeout(
    runtime: &RuntimeState,
    provider_id: &str,
    key_id: &str,
    pool_config: &AdminProviderPoolConfig,
) {
    if !pool_config.health_policy_enabled || pool_config.stream_timeout_threshold == 0 {
        return;
    }

    let timeout_key = pool_stream_timeout_key(provider_id, key_id);
    let now = current_unix_secs_f64();
    let window_seconds = pool_config.stream_timeout_window_seconds.max(1);
    let member = Uuid::new_v4().simple().to_string();
    let _ = runtime
        .score_remove_by_score(&timeout_key, now - window_seconds as f64)
        .await;
    let _ = runtime.score_set(&timeout_key, &member, now).await;
    let count = runtime.score_len(&timeout_key).await.unwrap_or(0) as u64;
    let _ = runtime
        .key_expire(
            &timeout_key,
            std::time::Duration::from_secs(window_seconds.saturating_add(60)),
        )
        .await;

    if count >= pool_config.stream_timeout_threshold {
        set_pool_cooldown(
            runtime,
            provider_id,
            key_id,
            &format!("stream_timeout_x{count}"),
            pool_config.stream_timeout_cooldown_seconds.max(1),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        admin_provider_pool_key_terminal_error_reason, parse_google_quota_cooldown_seconds_at,
        record_admin_provider_pool_error, record_admin_provider_pool_stream_timeout,
        record_admin_provider_pool_success,
    };
    use crate::handlers::admin::provider::pool::runtime::reads::read_admin_provider_pool_runtime_state;
    use crate::handlers::admin::provider::shared::support::{
        admin_provider_pool_quota_probe_active_members_key, AdminProviderPoolConfig,
        AdminProviderPoolSchedulingPreset, AdminProviderPoolUnschedulableRule,
    };
    use crate::AppState;
    use aether_runtime_state::{RedisClientConfig, RuntimeState, RuntimeStateConfig};
    use aether_testkit::ManagedRedisServer;
    use std::collections::BTreeMap;

    async fn start_managed_redis_or_skip() -> Option<ManagedRedisServer> {
        match ManagedRedisServer::start().await {
            Ok(server) => Some(server),
            Err(err) if err.to_string().contains("No such file or directory") => {
                eprintln!("skipping redis-backed pool runtime test: {err}");
                None
            }
            Err(err) => panic!("redis server should start: {err}"),
        }
    }

    fn sample_pool_config() -> AdminProviderPoolConfig {
        AdminProviderPoolConfig {
            scheduling_presets: vec![
                AdminProviderPoolSchedulingPreset {
                    preset: "cache_affinity".to_string(),
                    enabled: true,
                    mode: None,
                },
                AdminProviderPoolSchedulingPreset {
                    preset: "latency_first".to_string(),
                    enabled: true,
                    mode: None,
                },
            ],
            unschedulable_rules: Vec::new(),
            lru_enabled: true,
            skip_exhausted_accounts: false,
            sticky_session_ttl_seconds: 120,
            latency_window_seconds: 600,
            latency_sample_limit: 10,
            cost_window_seconds: 600,
            cost_limit_per_key_tokens: Some(10_000),
            rate_limit_cooldown_seconds: 300,
            overload_cooldown_seconds: 30,
            health_policy_enabled: true,
            probing_enabled: false,
            probing_target_percent: None,
            probing_target_count: None,
            probe_concurrency: 4,
            account_self_check_enabled: false,
            account_self_check_interval_minutes: 60,
            account_self_check_concurrency: 4,
            score_top_n: 128,
            score_fallback_scan_limit: 4096,
            score_rules: aether_pool_core::PoolMemberScoreRules::default(),
            stream_timeout_threshold: 3,
            stream_timeout_window_seconds: 1800,
            stream_timeout_cooldown_seconds: 300,
        }
    }

    async fn build_runner_app(redis_url: &str, key_prefix: &str) -> AppState {
        let runtime_state =
            RuntimeState::from_config(RuntimeStateConfig::redis(RedisClientConfig {
                url: redis_url.to_string(),
                key_prefix: Some(key_prefix.to_string()),
            }))
            .await
            .expect("runtime state should build");
        AppState::new()
            .expect("app state should build")
            .with_runtime_state(std::sync::Arc::new(runtime_state))
    }

    async fn wait_for_active_probe_members_empty(runtime: &RuntimeState, set_key: &str) {
        for _ in 0..20 {
            let members = runtime
                .set_members(set_key)
                .await
                .expect("active members should read");
            if members.is_empty() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let members = runtime
            .set_members(set_key)
            .await
            .expect("active members should read");
        assert!(members.is_empty());
    }

    #[test]
    fn parses_google_quota_cooldown_from_reset_timestamp() {
        let now_unix_secs = chrono::DateTime::parse_from_rfc3339("2026-04-17T10:00:00Z")
            .expect("timestamp should parse")
            .timestamp()
            .max(0) as u64;
        let cooldown = parse_google_quota_cooldown_seconds_at(
            Some(
                r#"{
                    "error": {
                        "message": "Quota exhausted.",
                        "details": [{
                            "metadata": {
                                "quotaResetTimeStamp": "2026-04-17T10:01:30Z"
                            }
                        }]
                    }
                }"#,
            ),
            now_unix_secs,
        );

        assert_eq!(cooldown, Some(90));
    }

    #[test]
    fn parses_google_quota_cooldown_from_reset_delay_and_message() {
        let now_unix_secs = chrono::DateTime::parse_from_rfc3339("2026-04-17T10:00:00Z")
            .expect("timestamp should parse")
            .timestamp()
            .max(0) as u64;
        let delay_cooldown = parse_google_quota_cooldown_seconds_at(
            Some(
                r#"{
                    "error": {
                        "message": "Quota exhausted.",
                        "details": [{
                            "metadata": {
                                "quotaResetDelay": "1h30m15s"
                            }
                        }]
                    }
                }"#,
            ),
            now_unix_secs,
        );
        let message_cooldown = parse_google_quota_cooldown_seconds_at(
            Some(
                r#"{
                    "error": {
                        "message": "Too many requests, reset after 45m."
                    }
                }"#,
            ),
            now_unix_secs,
        );

        assert_eq!(delay_cooldown, Some(5_415));
        assert_eq!(message_cooldown, Some(2_700));
    }

    #[test]
    fn terminal_error_reason_detects_workspace_deactivated_errors() {
        assert_eq!(
            admin_provider_pool_key_terminal_error_reason(
                402,
                Some(r#"{"error":{"message":"workspace has been deactivated"}}"#),
            )
            .as_deref(),
            Some("workspace_deactivated_402:workspace has been deactivated")
        );
        assert_eq!(
            admin_provider_pool_key_terminal_error_reason(
                400,
                Some(r#"{"error":{"message":"deactivated_workspace"}}"#),
            )
            .as_deref(),
            Some("workspace_deactivated_400:deactivated_workspace")
        );
    }

    #[test]
    fn terminal_error_reason_detects_account_ban_errors() {
        assert_eq!(
            admin_provider_pool_key_terminal_error_reason(
                403,
                Some(r#"{"error":{"message":"AccountSuspendedException: account suspended"}}"#),
            )
            .as_deref(),
            Some("forbidden_403")
        );
        assert_eq!(
            admin_provider_pool_key_terminal_error_reason(
                423,
                Some(r#"{"error":{"message":"account access denied"}}"#),
            )
            .as_deref(),
            Some("account_locked_423")
        );
    }

    #[tokio::test]
    async fn success_feedback_writes_sticky_lru_cost_and_latency() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let app = build_runner_app(redis.redis_url(), "pool_runtime_success_feedback").await;
        let runtime = app.runtime_state.as_ref();
        let pool_config = sample_pool_config();
        let key_ids = vec!["key-1".to_string()];

        record_admin_provider_pool_success(
            runtime,
            "provider-1",
            "key-1",
            &pool_config,
            Some("session-1"),
            120,
            Some(80),
        )
        .await;

        let runtime = read_admin_provider_pool_runtime_state(
            runtime,
            "provider-1",
            &key_ids,
            &pool_config,
            Some("session-1"),
        )
        .await;

        assert_eq!(runtime.total_sticky_sessions, 1);
        assert_eq!(runtime.sticky_bound_key_id.as_deref(), Some("key-1"));
        assert_eq!(runtime.sticky_sessions_by_key.get("key-1"), Some(&1));
        assert_eq!(runtime.cost_window_usage_by_key.get("key-1"), Some(&120));
        assert_eq!(runtime.latency_avg_ms_by_key.get("key-1"), Some(&80.0));
        assert!(runtime.lru_score_by_key.contains_key("key-1"));
    }

    #[tokio::test]
    async fn success_feedback_does_not_write_sticky_when_ttl_is_zero() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let app =
            build_runner_app(redis.redis_url(), "pool_runtime_no_sticky_without_affinity").await;
        let runtime = app.runtime_state.as_ref();
        let mut pool_config = sample_pool_config();
        pool_config.sticky_session_ttl_seconds = 0;
        let key_ids = vec!["key-1".to_string()];

        record_admin_provider_pool_success(
            runtime,
            "provider-1",
            "key-1",
            &pool_config,
            Some("session-1"),
            120,
            Some(80),
        )
        .await;

        let runtime = read_admin_provider_pool_runtime_state(
            runtime,
            "provider-1",
            &key_ids,
            &pool_config,
            Some("session-1"),
        )
        .await;

        assert_eq!(runtime.total_sticky_sessions, 0);
        assert_eq!(runtime.sticky_bound_key_id, None);
        assert_eq!(runtime.sticky_sessions_by_key.get("key-1"), None);
        assert_eq!(runtime.cost_window_usage_by_key.get("key-1"), Some(&120));
        assert!(runtime.lru_score_by_key.contains_key("key-1"));
    }

    #[tokio::test]
    async fn success_feedback_does_not_write_sticky_without_cache_affinity() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let app = build_runner_app(redis.redis_url(), "pool_runtime_no_sticky_load_balance").await;
        let runtime = app.runtime_state.as_ref();
        let mut pool_config = sample_pool_config();
        pool_config.scheduling_presets = vec![AdminProviderPoolSchedulingPreset {
            preset: "load_balance".to_string(),
            enabled: true,
            mode: None,
        }];
        pool_config.lru_enabled = false;
        let key_ids = vec!["key-1".to_string()];

        record_admin_provider_pool_success(
            runtime,
            "provider-1",
            "key-1",
            &pool_config,
            Some("session-1"),
            120,
            Some(80),
        )
        .await;

        let runtime = read_admin_provider_pool_runtime_state(
            runtime,
            "provider-1",
            &key_ids,
            &pool_config,
            Some("session-1"),
        )
        .await;

        assert_eq!(runtime.total_sticky_sessions, 0);
        assert_eq!(runtime.sticky_bound_key_id, None);
        assert_eq!(runtime.sticky_sessions_by_key.get("key-1"), None);
        assert_eq!(runtime.cost_window_usage_by_key.get("key-1"), Some(&120));
        assert_eq!(runtime.latency_avg_ms_by_key.get("key-1"), Some(&80.0));
        assert!(runtime.lru_score_by_key.contains_key("key-1"));
    }

    #[tokio::test]
    async fn error_feedback_respects_retry_after_for_rate_limits() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let app = build_runner_app(redis.redis_url(), "pool_runtime_error_feedback").await;
        let runtime = app.runtime_state.as_ref();
        let pool_config = sample_pool_config();
        let key_ids = vec!["key-2".to_string()];

        record_admin_provider_pool_error(
            runtime,
            "provider-1",
            "key-2",
            &pool_config,
            429,
            Some(r#"{"error":{"message":"rate limited"}}"#),
            Some(&BTreeMap::from([(
                "Retry-After".to_string(),
                "120".to_string(),
            )])),
        )
        .await;

        let runtime = read_admin_provider_pool_runtime_state(
            runtime,
            "provider-1",
            &key_ids,
            &pool_config,
            None,
        )
        .await;

        assert_eq!(
            runtime
                .cooldown_reason_by_key
                .get("key-2")
                .map(String::as_str),
            Some("rate_limited_429")
        );
        assert!(runtime
            .cooldown_ttl_by_key
            .get("key-2")
            .is_some_and(|ttl| *ttl <= 120 && *ttl >= 100));
    }

    #[tokio::test]
    async fn error_feedback_removes_active_probe_member_when_key_becomes_unschedulable() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let app = build_runner_app(redis.redis_url(), "pool_runtime_evict_active_probe").await;
        let runtime = app.runtime_state.as_ref();
        let pool_config = sample_pool_config();
        let set_key = admin_provider_pool_quota_probe_active_members_key("provider-1");
        runtime
            .set_add(&set_key, "key-2")
            .await
            .expect("active member should insert");

        record_admin_provider_pool_error(
            runtime,
            "provider-1",
            "key-2",
            &pool_config,
            429,
            Some(r#"{"error":{"message":"rate limited"}}"#),
            None,
        )
        .await;

        wait_for_active_probe_members_empty(runtime, &set_key).await;

        runtime
            .set_add(&set_key, "key-402")
            .await
            .expect("active member should insert");
        record_admin_provider_pool_error(
            runtime,
            "provider-1",
            "key-402",
            &pool_config,
            402,
            Some(r#"{"error":{"message":"quota exhausted"}}"#),
            None,
        )
        .await;
        wait_for_active_probe_members_empty(runtime, &set_key).await;
    }

    #[tokio::test]
    async fn error_feedback_uses_google_quota_cooldown_when_retry_after_missing() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let app = build_runner_app(redis.redis_url(), "pool_runtime_google_quota_cooldown").await;
        let runtime = app.runtime_state.as_ref();
        let pool_config = sample_pool_config();
        let key_ids = vec!["key-google-429".to_string()];

        record_admin_provider_pool_error(
            runtime,
            "provider-1",
            "key-google-429",
            &pool_config,
            429,
            Some(
                r#"{
                    "error": {
                        "message": "Quota exhausted. reset after 45s.",
                        "status": "RESOURCE_EXHAUSTED",
                        "details": [{
                            "metadata": {
                                "quotaResetDelay": "45s"
                            }
                        }]
                    }
                }"#,
            ),
            None,
        )
        .await;

        let runtime = read_admin_provider_pool_runtime_state(
            runtime,
            "provider-1",
            &key_ids,
            &pool_config,
            None,
        )
        .await;

        assert_eq!(
            runtime
                .cooldown_reason_by_key
                .get("key-google-429")
                .map(String::as_str),
            Some("rate_limited_429")
        );
        assert!(runtime
            .cooldown_ttl_by_key
            .get("key-google-429")
            .is_some_and(|ttl| *ttl <= 45 && *ttl >= 30));
    }

    #[tokio::test]
    async fn error_feedback_caps_long_retry_after_cooldowns_at_32_minutes() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let app = build_runner_app(redis.redis_url(), "pool_runtime_capped_cooldown").await;
        let runtime = app.runtime_state.as_ref();
        let pool_config = sample_pool_config();
        let key_ids = vec!["key-long-cooldown".to_string()];

        record_admin_provider_pool_error(
            runtime,
            "provider-1",
            "key-long-cooldown",
            &pool_config,
            429,
            Some(r#"{"error":{"message":"rate limited"}}"#),
            Some(&BTreeMap::from([(
                "Retry-After".to_string(),
                "3600".to_string(),
            )])),
        )
        .await;

        let runtime = read_admin_provider_pool_runtime_state(
            runtime,
            "provider-1",
            &key_ids,
            &pool_config,
            None,
        )
        .await;

        assert_eq!(
            runtime
                .cooldown_reason_by_key
                .get("key-long-cooldown")
                .map(String::as_str),
            Some("rate_limited_429")
        );
        assert!(runtime
            .cooldown_ttl_by_key
            .get("key-long-cooldown")
            .is_some_and(|ttl| *ttl <= 32 * 60 && *ttl >= 31 * 60));
    }

    #[tokio::test]
    async fn severe_account_errors_do_not_use_pool_cooldown() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let app = build_runner_app(redis.redis_url(), "pool_runtime_circuit_no_cooldown").await;
        let runtime = app.runtime_state.as_ref();
        let pool_config = sample_pool_config();
        let key_ids = vec!["key-account-disabled".to_string()];

        assert_eq!(
            admin_provider_pool_key_terminal_error_reason(
                401,
                Some(r#"{"error":{"message":"account has been deactivated"}}"#),
            )
            .as_deref(),
            Some("account_deactivated_401")
        );
        record_admin_provider_pool_error(
            runtime,
            "provider-1",
            "key-account-disabled",
            &pool_config,
            401,
            Some(r#"{"error":{"message":"account has been deactivated"}}"#),
            None,
        )
        .await;

        let runtime = read_admin_provider_pool_runtime_state(
            runtime,
            "provider-1",
            &key_ids,
            &pool_config,
            None,
        )
        .await;

        assert!(!runtime
            .cooldown_reason_by_key
            .contains_key("key-account-disabled"));
        assert!(!runtime
            .cooldown_ttl_by_key
            .contains_key("key-account-disabled"));
    }

    #[tokio::test]
    async fn error_feedback_applies_unschedulable_rule_cooldown() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let app = build_runner_app(redis.redis_url(), "pool_runtime_unschedulable_rule").await;
        let runtime = app.runtime_state.as_ref();
        let mut pool_config = sample_pool_config();
        pool_config.unschedulable_rules = vec![AdminProviderPoolUnschedulableRule {
            keyword: "review required".to_string(),
            duration_minutes: 7,
        }];
        let key_ids = vec!["key-3".to_string()];

        record_admin_provider_pool_error(
            runtime,
            "provider-1",
            "key-3",
            &pool_config,
            418,
            Some(r#"{"error":{"message":"manual review required before reuse"}}"#),
            None,
        )
        .await;

        let runtime = read_admin_provider_pool_runtime_state(
            runtime,
            "provider-1",
            &key_ids,
            &pool_config,
            None,
        )
        .await;

        assert_eq!(
            runtime
                .cooldown_reason_by_key
                .get("key-3")
                .map(String::as_str),
            Some("rule:review required")
        );
        assert!(runtime
            .cooldown_ttl_by_key
            .get("key-3")
            .is_some_and(|ttl| *ttl <= 420 && *ttl >= 380));
    }

    #[tokio::test]
    async fn error_feedback_ignores_client_bad_request_for_cooldown() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let app = build_runner_app(redis.redis_url(), "pool_runtime_ignore_400").await;
        let runtime = app.runtime_state.as_ref();
        let mut pool_config = sample_pool_config();
        pool_config.unschedulable_rules = vec![AdminProviderPoolUnschedulableRule {
            keyword: "review required".to_string(),
            duration_minutes: 7,
        }];
        let key_ids = vec!["key-client-400".to_string()];

        record_admin_provider_pool_error(
            runtime,
            "provider-1",
            "key-client-400",
            &pool_config,
            400,
            Some(r#"{"error":{"message":"manual review required before reuse"}}"#),
            None,
        )
        .await;

        let runtime = read_admin_provider_pool_runtime_state(
            runtime,
            "provider-1",
            &key_ids,
            &pool_config,
            None,
        )
        .await;

        assert!(!runtime
            .cooldown_reason_by_key
            .contains_key("key-client-400"));
        assert!(!runtime.cooldown_ttl_by_key.contains_key("key-client-400"));
    }

    #[tokio::test]
    async fn stream_timeout_policy_cools_down_after_threshold() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let app = build_runner_app(redis.redis_url(), "pool_runtime_stream_timeout").await;
        let runtime_state = app.runtime_state.as_ref();
        let mut pool_config = sample_pool_config();
        pool_config.stream_timeout_threshold = 2;
        pool_config.stream_timeout_window_seconds = 300;
        pool_config.stream_timeout_cooldown_seconds = 90;
        let key_ids = vec!["key-4".to_string()];

        record_admin_provider_pool_stream_timeout(
            runtime_state,
            "provider-1",
            "key-4",
            &pool_config,
        )
        .await;
        record_admin_provider_pool_stream_timeout(
            runtime_state,
            "provider-1",
            "key-4",
            &pool_config,
        )
        .await;

        let mut runtime = read_admin_provider_pool_runtime_state(
            runtime_state,
            "provider-1",
            &key_ids,
            &pool_config,
            None,
        )
        .await;
        for _ in 0..20 {
            if runtime
                .cooldown_reason_by_key
                .get("key-4")
                .map(String::as_str)
                == Some("stream_timeout_x2")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            runtime = read_admin_provider_pool_runtime_state(
                runtime_state,
                "provider-1",
                &key_ids,
                &pool_config,
                None,
            )
            .await;
        }

        assert_eq!(
            runtime
                .cooldown_reason_by_key
                .get("key-4")
                .map(String::as_str),
            Some("stream_timeout_x2")
        );
        assert!(runtime
            .cooldown_ttl_by_key
            .get("key-4")
            .is_some_and(|ttl| *ttl <= 90 && *ttl >= 70));
    }
}
