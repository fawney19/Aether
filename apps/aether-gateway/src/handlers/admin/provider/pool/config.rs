use crate::handlers::admin::provider::shared::support::{
    AdminProviderPoolConfig, AdminProviderPoolSchedulingPreset, AdminProviderPoolUnschedulableRule,
};
use aether_pool_core::{PoolMemberScoreRules, PoolMemberScoreWeights};
use serde_json::{Map, Value};

const POOL_ALLOWED_SCHEDULING_PRESETS: &[&str] = &[
    "lru",
    "cache_affinity",
    "load_balance",
    "single_account",
    "priority_first",
    "free_first",
    "team_first",
    "plus_first",
    "pro_first",
    "health_first",
    "latency_first",
    "cost_first",
    "quota_balanced",
    "recent_refresh",
];

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|raw| u64::try_from(raw).ok()))
}

fn json_f64(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| {
        value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<f64>().ok())
    })
}

fn parse_pool_probe_target_percent(pool_advanced: &Map<String, Value>) -> Option<f64> {
    pool_advanced
        .get("probing_target_percent")
        .or_else(|| pool_advanced.get("probing_active_target_percent"))
        .or_else(|| pool_advanced.get("active_probe_target_percent"))
        .and_then(json_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value.clamp(0.0, 100.0))
}

fn parse_pool_probe_target_count(pool_advanced: &Map<String, Value>) -> Option<u64> {
    pool_advanced
        .get("probing_target_count")
        .or_else(|| pool_advanced.get("probing_active_target_count"))
        .or_else(|| pool_advanced.get("active_probe_target_count"))
        .and_then(json_u64)
        .filter(|value| *value > 0)
        .map(|value| value.min(100_000))
}

fn pool_score_weight(object: &Map<String, Value>, names: &[&str], current: f64) -> f64 {
    names
        .iter()
        .find_map(|name| {
            object
                .get(*name)
                .and_then(json_f64)
                .filter(|value| value.is_finite() && *value >= 0.0)
        })
        .unwrap_or(current)
}

fn parse_pool_score_weights(
    raw_weights: Option<&Map<String, Value>>,
    current: PoolMemberScoreWeights,
) -> PoolMemberScoreWeights {
    let Some(raw_weights) = raw_weights else {
        return current;
    };
    PoolMemberScoreWeights {
        manual_priority: pool_score_weight(
            raw_weights,
            &["manual_priority", "priority", "internal_priority"],
            current.manual_priority,
        ),
        health: pool_score_weight(raw_weights, &["health"], current.health),
        probe_freshness: pool_score_weight(
            raw_weights,
            &["probe_freshness", "freshness", "probe"],
            current.probe_freshness,
        ),
        quota_remaining: pool_score_weight(
            raw_weights,
            &["quota_remaining", "quota", "quota_available"],
            current.quota_remaining,
        ),
        latency: pool_score_weight(raw_weights, &["latency"], current.latency),
        cost_lru: pool_score_weight(
            raw_weights,
            &["cost_lru", "cost_remaining", "cost", "lru"],
            current.cost_lru,
        ),
    }
}

fn parse_pool_score_rules(pool_advanced: &Map<String, Value>) -> PoolMemberScoreRules {
    let mut rules = PoolMemberScoreRules::default();

    for key in ["score_weights", "pool_score_weights", "scoring_weights"] {
        rules.weights = parse_pool_score_weights(
            pool_advanced.get(key).and_then(Value::as_object),
            rules.weights,
        );
    }

    if let Some(score_rules) = pool_advanced
        .get("score_rules")
        .or_else(|| pool_advanced.get("pool_score_rules"))
        .and_then(Value::as_object)
    {
        rules.weights = parse_pool_score_weights(
            score_rules.get("weights").and_then(Value::as_object),
            rules.weights,
        );
        if let Some(ttl_seconds) = score_rules
            .get("probe_freshness_ttl_seconds")
            .or_else(|| score_rules.get("score_probe_freshness_ttl_seconds"))
            .and_then(json_u64)
            .filter(|value| *value > 0)
        {
            rules.probe_freshness_ttl_seconds = ttl_seconds.min(7 * 24 * 3600);
        }
        if let Some(cap) = score_rules
            .get("unschedulable_score_cap")
            .or_else(|| score_rules.get("hard_state_score_cap"))
            .and_then(json_f64)
            .filter(|value| value.is_finite())
        {
            rules.unschedulable_score_cap = cap.clamp(0.0, 1.0);
        }
        if let Some(penalty) = score_rules
            .get("probe_failure_penalty")
            .and_then(json_f64)
            .filter(|value| value.is_finite())
        {
            rules.probe_failure_penalty = penalty.clamp(0.0, 1.0);
        }
        if let Some(penalty) = score_rules
            .get("request_failure_penalty")
            .or_else(|| score_rules.get("runtime_failure_penalty"))
            .and_then(json_f64)
            .filter(|value| value.is_finite())
        {
            rules.request_failure_penalty = penalty.clamp(0.0, 1.0);
        }
        if let Some(threshold) = score_rules
            .get("probe_failure_cooldown_threshold")
            .or_else(|| score_rules.get("probe_failure_hard_state_threshold"))
            .and_then(json_u64)
        {
            rules.probe_failure_cooldown_threshold = threshold.min(100);
        }
    }

    rules.effective()
}

fn normalize_pool_preset_mode(preset: &str, raw_mode: Option<&Value>) -> Option<String> {
    match preset {
        "free_first" | "team_first" | "plus_first" | "pro_first" => {
            let default_mode = match preset {
                "free_first" => "free_only",
                "team_first" => "team_only",
                "plus_first" => "plus_only",
                "pro_first" => "pro_only",
                _ => unreachable!("preset covered by outer match"),
            };
            let normalized = raw_mode
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.to_ascii_lowercase())
                .filter(|value| match preset {
                    "free_first" => value == "free_only",
                    "team_first" => value == "team_only",
                    "plus_first" => value == "plus_only",
                    "pro_first" => value == "pro_only",
                    _ => false,
                })
                .unwrap_or_else(|| default_mode.to_string());
            Some(normalized)
        }
        _ => None,
    }
}

fn parse_object_style_pool_scheduling_presets(
    presets: &[Value],
) -> Vec<AdminProviderPoolSchedulingPreset> {
    let mut normalized = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    for item in presets {
        let Some(object) = item.as_object() else {
            continue;
        };
        let Some(preset) = object
            .get("preset")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase())
        else {
            continue;
        };
        if !POOL_ALLOWED_SCHEDULING_PRESETS.contains(&preset.as_str())
            || !seen.insert(preset.clone())
        {
            continue;
        }
        normalized.push(AdminProviderPoolSchedulingPreset {
            mode: normalize_pool_preset_mode(&preset, object.get("mode")),
            preset,
            enabled: object
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        });
    }

    if normalized.is_empty() {
        vec![AdminProviderPoolSchedulingPreset {
            preset: "lru".to_string(),
            enabled: true,
            mode: None,
        }]
    } else {
        normalized
    }
}

fn parse_legacy_string_style_pool_scheduling_presets(
    raw_pool_advanced: &Map<String, Value>,
    presets: &[Value],
) -> Vec<AdminProviderPoolSchedulingPreset> {
    let lru_enabled = raw_pool_advanced
        .get("lru_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut normalized = vec![AdminProviderPoolSchedulingPreset {
        preset: "lru".to_string(),
        enabled: lru_enabled,
        mode: None,
    }];
    let mut seen = std::collections::BTreeSet::from(["lru".to_string()]);

    for item in presets {
        let Some(preset) = item
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase())
        else {
            continue;
        };
        if preset == "lru"
            || !POOL_ALLOWED_SCHEDULING_PRESETS.contains(&preset.as_str())
            || !seen.insert(preset.clone())
        {
            continue;
        }
        normalized.push(AdminProviderPoolSchedulingPreset {
            preset,
            enabled: true,
            mode: None,
        });
    }

    normalized
}

fn parse_pool_scheduling_presets_from_legacy_fields(
    raw_pool_advanced: &Map<String, Value>,
) -> Vec<AdminProviderPoolSchedulingPreset> {
    if raw_pool_advanced
        .get("lru_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        vec![AdminProviderPoolSchedulingPreset {
            preset: "lru".to_string(),
            enabled: true,
            mode: None,
        }]
    } else {
        vec![AdminProviderPoolSchedulingPreset {
            preset: "cache_affinity".to_string(),
            enabled: true,
            mode: None,
        }]
    }
}

fn parse_pool_scheduling_presets(
    raw_pool_advanced: &Map<String, Value>,
) -> Vec<AdminProviderPoolSchedulingPreset> {
    match raw_pool_advanced
        .get("scheduling_presets")
        .and_then(Value::as_array)
    {
        Some(presets) if !presets.is_empty() => match presets.first() {
            Some(Value::Object(_)) => parse_object_style_pool_scheduling_presets(presets),
            Some(Value::String(_)) => {
                parse_legacy_string_style_pool_scheduling_presets(raw_pool_advanced, presets)
            }
            _ => parse_pool_scheduling_presets_from_legacy_fields(raw_pool_advanced),
        },
        _ => parse_pool_scheduling_presets_from_legacy_fields(raw_pool_advanced),
    }
}

fn parse_pool_unschedulable_rules(
    raw_pool_advanced: &Map<String, Value>,
) -> Vec<AdminProviderPoolUnschedulableRule> {
    raw_pool_advanced
        .get("unschedulable_rules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let object = item.as_object()?;
            let keyword = object
                .get("keyword")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            Some(AdminProviderPoolUnschedulableRule {
                keyword: keyword.to_string(),
                duration_minutes: object
                    .get("duration_minutes")
                    .and_then(json_u64)
                    .filter(|value| *value > 0)
                    .unwrap_or(5),
            })
        })
        .collect()
}

fn admin_provider_pool_lru_enabled(
    scheduling_presets: &[AdminProviderPoolSchedulingPreset],
) -> bool {
    scheduling_presets
        .iter()
        .any(|item| item.enabled && item.preset.eq_ignore_ascii_case("lru"))
}

pub(crate) fn admin_provider_pool_cache_affinity_enabled(
    pool_config: &AdminProviderPoolConfig,
) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    for item in &pool_config.scheduling_presets {
        let preset = item.preset.trim().to_ascii_lowercase();
        if preset.is_empty() || !seen.insert(preset.clone()) {
            continue;
        }
        if !item.enabled {
            continue;
        }
        if matches!(
            preset.as_str(),
            "lru" | "cache_affinity" | "load_balance" | "single_account"
        ) {
            return preset == "cache_affinity";
        }
    }
    false
}

pub(crate) fn admin_provider_pool_config(
    provider: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
) -> Option<AdminProviderPoolConfig> {
    admin_provider_pool_config_from_config_value(provider.config.as_ref())
}

pub(crate) fn admin_provider_pool_config_from_config_value(
    config: Option<&serde_json::Value>,
) -> Option<AdminProviderPoolConfig> {
    let raw_pool_advanced = config
        .and_then(Value::as_object)
        .and_then(|config| config.get("pool_advanced"))?;

    let Some(pool_advanced) = raw_pool_advanced.as_object() else {
        return Some(AdminProviderPoolConfig {
            scheduling_presets: vec![AdminProviderPoolSchedulingPreset {
                preset: "cache_affinity".to_string(),
                enabled: true,
                mode: None,
            }],
            unschedulable_rules: Vec::new(),
            lru_enabled: false,
            skip_exhausted_accounts: false,
            sticky_session_ttl_seconds: 3600,
            latency_window_seconds: 3600,
            latency_sample_limit: 50,
            cost_window_seconds: 18_000,
            cost_limit_per_key_tokens: None,
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
            score_rules: PoolMemberScoreRules::default(),
            stream_timeout_threshold: 3,
            stream_timeout_window_seconds: 1800,
            stream_timeout_cooldown_seconds: 300,
        });
    };

    let scheduling_presets = parse_pool_scheduling_presets(pool_advanced);
    let unschedulable_rules = parse_pool_unschedulable_rules(pool_advanced);
    let score_rules = parse_pool_score_rules(pool_advanced);

    Some(AdminProviderPoolConfig {
        lru_enabled: admin_provider_pool_lru_enabled(&scheduling_presets),
        scheduling_presets,
        unschedulable_rules,
        skip_exhausted_accounts: pool_advanced
            .get("skip_exhausted_accounts")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        sticky_session_ttl_seconds: pool_advanced
            .get("sticky_session_ttl_seconds")
            .and_then(json_u64)
            .unwrap_or(3600),
        latency_window_seconds: pool_advanced
            .get("latency_window_seconds")
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .unwrap_or(3600),
        latency_sample_limit: pool_advanced
            .get("latency_sample_limit")
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .unwrap_or(50),
        cost_window_seconds: pool_advanced
            .get("cost_window_seconds")
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .unwrap_or(18_000),
        cost_limit_per_key_tokens: pool_advanced
            .get("cost_limit_per_key_tokens")
            .and_then(json_u64),
        rate_limit_cooldown_seconds: pool_advanced
            .get("rate_limit_cooldown_seconds")
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .unwrap_or(300),
        overload_cooldown_seconds: pool_advanced
            .get("overload_cooldown_seconds")
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .unwrap_or(30),
        health_policy_enabled: pool_advanced
            .get("health_policy_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        probing_enabled: pool_advanced
            .get("probing_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        probing_target_percent: parse_pool_probe_target_percent(pool_advanced),
        probing_target_count: parse_pool_probe_target_count(pool_advanced),
        probe_concurrency: pool_advanced
            .get("probe_concurrency")
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .map(|value| value.min(64))
            .unwrap_or(4),
        account_self_check_enabled: pool_advanced
            .get("account_self_check_enabled")
            .or_else(|| pool_advanced.get("self_check_enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        account_self_check_interval_minutes: pool_advanced
            .get("account_self_check_interval_minutes")
            .or_else(|| pool_advanced.get("self_check_interval_minutes"))
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .map(|value| value.min(1440))
            .unwrap_or(60),
        account_self_check_concurrency: pool_advanced
            .get("account_self_check_concurrency")
            .or_else(|| pool_advanced.get("self_check_concurrency"))
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .map(|value| value.min(64))
            .unwrap_or(4),
        score_top_n: pool_advanced
            .get("score_top_n")
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .map(|value| value.min(4096))
            .unwrap_or(128),
        score_fallback_scan_limit: pool_advanced
            .get("score_fallback_scan_limit")
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .map(|value| value.min(50_000))
            .unwrap_or(4096),
        score_rules,
        stream_timeout_threshold: pool_advanced
            .get("stream_timeout_threshold")
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .unwrap_or(3),
        stream_timeout_window_seconds: pool_advanced
            .get("stream_timeout_window_seconds")
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .unwrap_or(1800),
        stream_timeout_cooldown_seconds: pool_advanced
            .get("stream_timeout_cooldown_seconds")
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .unwrap_or(300),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        admin_provider_pool_cache_affinity_enabled, admin_provider_pool_config,
        admin_provider_pool_config_from_config_value,
    };
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
    use serde_json::json;

    fn sample_provider(config: serde_json::Value) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "provider-1".to_string(),
            Some("https://example.com".to_string()),
            "codex".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            Some(config),
        )
    }

    #[test]
    fn defaults_skip_exhausted_accounts_to_false() {
        let provider = sample_provider(json!({ "pool_advanced": {} }));
        let config = admin_provider_pool_config(&provider).expect("pool config should exist");

        assert!(!config.skip_exhausted_accounts);
    }

    #[test]
    fn parses_skip_exhausted_accounts_from_pool_advanced() {
        let provider = sample_provider(json!({
            "pool_advanced": {
                "skip_exhausted_accounts": true,
                "lru_enabled": true,
                "sticky_session_ttl_seconds": 600,
                "latency_window_seconds": 900,
                "latency_sample_limit": 75,
                "cost_window_seconds": 7200,
                "cost_limit_per_key_tokens": 12000,
                "rate_limit_cooldown_seconds": 420,
                "overload_cooldown_seconds": 45,
                "health_policy_enabled": false,
                "probing_enabled": true,
                "probing_target_percent": 25,
                "probing_target_count": 3,
                "probe_concurrency": 6,
                "account_self_check_enabled": true,
                "account_self_check_interval_minutes": 90,
                "account_self_check_concurrency": 5,
                "score_top_n": 256,
                "score_fallback_scan_limit": 2048,
                "score_rules": {
                    "weights": {
                        "manual_priority": 0.4,
                        "health": 0.2,
                        "probe_freshness": 0.2,
                        "quota_remaining": 0.1,
                        "latency": 0.05,
                        "cost_lru": 0.05
                    },
                    "probe_freshness_ttl_seconds": 1200,
                    "unschedulable_score_cap": 0.03,
                    "probe_failure_penalty": 0.08,
                    "request_failure_penalty": 0.01,
                    "probe_failure_cooldown_threshold": 2
                },
                "stream_timeout_threshold": 4,
                "stream_timeout_window_seconds": 900,
                "stream_timeout_cooldown_seconds": 180
            }
        }));
        let config = admin_provider_pool_config(&provider).expect("pool config should exist");

        assert!(config.skip_exhausted_accounts);
        assert!(config.lru_enabled);
        assert_eq!(config.sticky_session_ttl_seconds, 600);
        assert_eq!(config.latency_window_seconds, 900);
        assert_eq!(config.latency_sample_limit, 75);
        assert_eq!(config.cost_window_seconds, 7200);
        assert_eq!(config.cost_limit_per_key_tokens, Some(12_000));
        assert_eq!(config.rate_limit_cooldown_seconds, 420);
        assert_eq!(config.overload_cooldown_seconds, 45);
        assert!(!config.health_policy_enabled);
        assert!(config.probing_enabled);
        assert_eq!(config.probing_target_percent, Some(25.0));
        assert_eq!(config.probing_target_count, Some(3));
        assert_eq!(config.probe_concurrency, 6);
        assert!(config.account_self_check_enabled);
        assert_eq!(config.account_self_check_interval_minutes, 90);
        assert_eq!(config.account_self_check_concurrency, 5);
        assert_eq!(config.score_top_n, 256);
        assert_eq!(config.score_fallback_scan_limit, 2048);
        assert_eq!(config.score_rules.weights.manual_priority, 0.4);
        assert_eq!(config.score_rules.weights.health, 0.2);
        assert_eq!(config.score_rules.probe_freshness_ttl_seconds, 1200);
        assert_eq!(config.score_rules.unschedulable_score_cap, 0.03);
        assert_eq!(config.score_rules.probe_failure_penalty, 0.08);
        assert_eq!(config.score_rules.request_failure_penalty, 0.01);
        assert_eq!(config.score_rules.probe_failure_cooldown_threshold, 2);
        assert_eq!(config.stream_timeout_threshold, 4);
        assert_eq!(config.stream_timeout_window_seconds, 900);
        assert_eq!(config.stream_timeout_cooldown_seconds, 180);
    }

    #[test]
    fn ignores_legacy_pool_quota_probe_interval() {
        let provider = sample_provider(json!({
            "pool_advanced": {
                "probing_enabled": true,
                "probing_interval_minutes": 2000,
            }
        }));
        let config = admin_provider_pool_config(&provider).expect("pool config should exist");
        assert!(config.probing_enabled);

        let provider = sample_provider(json!({
            "pool_advanced": {
                "probing_enabled": true,
                "probing_interval_minutes": 0,
            }
        }));
        let config = admin_provider_pool_config(&provider).expect("pool config should exist");
        assert!(config.probing_enabled);
    }

    #[test]
    fn parses_zero_sticky_session_ttl_to_disable_sticky_sessions() {
        let config = admin_provider_pool_config_from_config_value(Some(&json!({
            "pool_advanced": {
                "sticky_session_ttl_seconds": 0
            }
        })))
        .expect("pool config should parse");

        assert_eq!(config.sticky_session_ttl_seconds, 0);
    }

    #[test]
    fn parses_legacy_pool_score_weights_from_pool_advanced() {
        let config = admin_provider_pool_config_from_config_value(Some(&json!({
            "pool_advanced": {
                "scoring_weights": {
                    "manual_priority": 0,
                    "health": 2,
                    "probe": 1,
                    "quota_remaining": 0,
                    "latency": 0,
                    "cost_remaining": 1
                }
            }
        })))
        .expect("pool config should parse");

        assert_eq!(config.score_rules.weights.health, 0.5);
        assert_eq!(config.score_rules.weights.probe_freshness, 0.25);
        assert_eq!(config.score_rules.weights.cost_lru, 0.25);
    }

    #[test]
    fn parses_pool_config_from_generic_config_value() {
        let config = admin_provider_pool_config_from_config_value(Some(&json!({
            "pool_advanced": {
                "scheduling_presets": [{"preset": "lru", "enabled": true}],
                "cost_limit_per_key_tokens": 4096
            }
        })))
        .expect("pool config should parse");

        assert!(config.lru_enabled);
        assert_eq!(config.scheduling_presets.len(), 1);
        assert_eq!(config.scheduling_presets[0].preset, "lru");
        assert_eq!(config.cost_limit_per_key_tokens, Some(4096));
    }

    #[test]
    fn defaults_empty_pool_advanced_to_cache_affinity_preset() {
        let config = admin_provider_pool_config_from_config_value(Some(&json!({
            "pool_advanced": {}
        })))
        .expect("pool config should parse");

        assert!(!config.lru_enabled);
        assert_eq!(config.scheduling_presets.len(), 1);
        assert_eq!(config.scheduling_presets[0].preset, "cache_affinity");
        assert!(config.scheduling_presets[0].enabled);
    }

    #[test]
    fn parses_object_style_scheduling_presets_with_modes() {
        let config = admin_provider_pool_config_from_config_value(Some(&json!({
            "pool_advanced": {
                "scheduling_presets": [
                    {"preset": "cache_affinity", "enabled": false},
                    {"preset": "plus_first", "enabled": true, "mode": "plus_only"},
                    {"preset": "pro_first", "enabled": true, "mode": "pro_only"}
                ]
            }
        })))
        .expect("pool config should parse");

        assert!(!config.lru_enabled);
        assert_eq!(config.scheduling_presets.len(), 3);
        assert_eq!(config.scheduling_presets[0].preset, "cache_affinity");
        assert!(!config.scheduling_presets[0].enabled);
        assert_eq!(config.scheduling_presets[1].preset, "plus_first");
        assert_eq!(
            config.scheduling_presets[1].mode.as_deref(),
            Some("plus_only")
        );
        assert_eq!(config.scheduling_presets[2].preset, "pro_first");
        assert_eq!(
            config.scheduling_presets[2].mode.as_deref(),
            Some("pro_only")
        );
    }

    #[test]
    fn parses_legacy_string_style_scheduling_presets() {
        let config = admin_provider_pool_config_from_config_value(Some(&json!({
            "pool_advanced": {
                "lru_enabled": false,
                "scheduling_presets": [
                    "free_first",
                    "recent_refresh",
                    "free_first"
                ]
            }
        })))
        .expect("pool config should parse");

        assert!(!config.lru_enabled);
        assert_eq!(config.scheduling_presets.len(), 3);
        assert_eq!(config.scheduling_presets[0].preset, "lru");
        assert!(!config.scheduling_presets[0].enabled);
        assert_eq!(config.scheduling_presets[1].preset, "free_first");
        assert_eq!(config.scheduling_presets[2].preset, "recent_refresh");
    }

    #[test]
    fn retired_free_team_first_preset_is_rejected() {
        let config = admin_provider_pool_config_from_config_value(Some(&json!({
            "pool_advanced": {
                "scheduling_presets": [
                    {"preset": "free_team_first", "enabled": true, "mode": "team_only"}
                ]
            }
        })))
        .expect("pool config should parse");

        assert_eq!(config.scheduling_presets.len(), 1);
        assert_eq!(config.scheduling_presets[0].preset, "lru");
        assert_eq!(config.scheduling_presets[0].mode, None);
    }

    #[test]
    fn parses_unschedulable_rules_from_pool_advanced() {
        let config = admin_provider_pool_config_from_config_value(Some(&json!({
            "pool_advanced": {
                "unschedulable_rules": [
                    {"keyword": "suspended", "duration_minutes": 15},
                    {"keyword": "review_required"}
                ]
            }
        })))
        .expect("pool config should parse");

        assert_eq!(config.unschedulable_rules.len(), 2);
        assert_eq!(config.unschedulable_rules[0].keyword, "suspended");
        assert_eq!(config.unschedulable_rules[0].duration_minutes, 15);
        assert_eq!(config.unschedulable_rules[1].keyword, "review_required");
        assert_eq!(config.unschedulable_rules[1].duration_minutes, 5);
    }

    #[test]
    fn cache_affinity_enabled_only_when_it_is_distribution_mode() {
        let cache_affinity = admin_provider_pool_config_from_config_value(Some(&json!({
            "pool_advanced": {
                "scheduling_presets": [
                    {"preset": "cache_affinity", "enabled": true},
                    {"preset": "priority_first", "enabled": true}
                ]
            }
        })))
        .expect("pool config should parse");
        assert!(admin_provider_pool_cache_affinity_enabled(&cache_affinity));

        let load_balance = admin_provider_pool_config_from_config_value(Some(&json!({
            "pool_advanced": {
                "scheduling_presets": [
                    {"preset": "load_balance", "enabled": true},
                    {"preset": "cache_affinity", "enabled": true}
                ]
            }
        })))
        .expect("pool config should parse");
        assert!(!admin_provider_pool_cache_affinity_enabled(&load_balance));
    }
}
