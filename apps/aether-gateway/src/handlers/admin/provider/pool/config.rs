use crate::handlers::admin::provider::shared::support::{
    AdminProviderPoolConfig, AdminProviderPoolPreProbeConfig, AdminProviderPoolSchedulingPreset,
    AdminProviderPoolUnschedulableRule,
};
use serde_json::{Map, Value};
use tracing::warn;

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

fn json_u32(value: &Value) -> Option<u32> {
    json_u64(value).and_then(|raw| u32::try_from(raw).ok())
}

fn json_usize(value: &Value) -> Option<usize> {
    json_u64(value).and_then(|raw| usize::try_from(raw).ok())
}

const POOL_PREHEAT_ENABLED_ENV: &str = "POOL_PREHEAT_ENABLED";
const POOL_PREHEAT_TOP_N_ENV: &str = "POOL_PREHEAT_TOP_N";
const POOL_PREHEAT_REQUIRED_HEALTHY_ENV: &str = "POOL_PREHEAT_REQUIRED_HEALTHY";
const POOL_PREHEAT_CACHE_TTL_SECONDS_ENV: &str = "POOL_PREHEAT_CACHE_TTL_SECONDS";
const POOL_PREHEAT_CACHE_MAX_ENTRIES_ENV: &str = "POOL_PREHEAT_CACHE_MAX_ENTRIES";
const POOL_PREHEAT_PROBE_TIMEOUT_SECONDS_ENV: &str = "POOL_PREHEAT_PROBE_TIMEOUT_SECONDS";
const POOL_PREHEAT_PER_KEY_DEDUP_SECONDS_ENV: &str = "POOL_PREHEAT_PER_KEY_DEDUP_SECONDS";
const POOL_PREHEAT_PER_PROVIDER_RATE_LIMIT_PER_MINUTE_ENV: &str =
    "POOL_PREHEAT_PER_PROVIDER_RATE_LIMIT_PER_MINUTE";
const POOL_PREHEAT_GROUP_LOCK_TTL_SECONDS_ENV: &str = "POOL_PREHEAT_GROUP_LOCK_TTL_SECONDS";
const POOL_PREHEAT_CIRCUIT_FAILURE_RATE_THRESHOLD_ENV: &str =
    "POOL_PREHEAT_CIRCUIT_FAILURE_RATE_THRESHOLD";
const POOL_PREHEAT_CIRCUIT_SAMPLE_WINDOW_SECONDS_ENV: &str =
    "POOL_PREHEAT_CIRCUIT_SAMPLE_WINDOW_SECONDS";
const POOL_PREHEAT_CIRCUIT_SUSPEND_SECONDS_ENV: &str = "POOL_PREHEAT_CIRCUIT_SUSPEND_SECONDS";
const POOL_PREHEAT_5XX_STREAK_THRESHOLD_ENV: &str = "POOL_PREHEAT_5XX_STREAK_THRESHOLD";
const POOL_PREHEAT_REQUIRED_HEALTHY_MAX: u32 = 8;
const POOL_PREHEAT_CIRCUIT_FAILURE_RATE_MAX: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PoolPreheatEnvConfig {
    pre_probe: AdminProviderPoolPreProbeConfig,
}

impl PoolPreheatEnvConfig {
    pub(crate) fn from_env() -> Self {
        let defaults = AdminProviderPoolPreProbeConfig::default();
        Self {
            pre_probe: AdminProviderPoolPreProbeConfig {
                enabled: pool_preheat_env_bool(POOL_PREHEAT_ENABLED_ENV, defaults.enabled),
                top_n: pool_preheat_env_u32(POOL_PREHEAT_TOP_N_ENV, defaults.top_n, 1, None),
                required_healthy: pool_preheat_env_u32(
                    POOL_PREHEAT_REQUIRED_HEALTHY_ENV,
                    defaults.required_healthy,
                    1,
                    Some(POOL_PREHEAT_REQUIRED_HEALTHY_MAX),
                ),
                dedup_window_secs: pool_preheat_env_u64(
                    POOL_PREHEAT_PER_KEY_DEDUP_SECONDS_ENV,
                    defaults.dedup_window_secs,
                    1,
                    None,
                ),
                cache_ttl_seconds: pool_preheat_env_u64(
                    POOL_PREHEAT_CACHE_TTL_SECONDS_ENV,
                    defaults.cache_ttl_seconds,
                    1,
                    None,
                ),
                cache_max_entries: pool_preheat_env_usize(
                    POOL_PREHEAT_CACHE_MAX_ENTRIES_ENV,
                    defaults.cache_max_entries,
                    1,
                ),
                probe_timeout_seconds: pool_preheat_env_u64(
                    POOL_PREHEAT_PROBE_TIMEOUT_SECONDS_ENV,
                    defaults.probe_timeout_seconds,
                    1,
                    None,
                ),
                per_provider_rate_limit_per_minute: pool_preheat_env_u32(
                    POOL_PREHEAT_PER_PROVIDER_RATE_LIMIT_PER_MINUTE_ENV,
                    defaults.per_provider_rate_limit_per_minute,
                    1,
                    None,
                ),
                group_lock_ttl_seconds: pool_preheat_env_u64(
                    POOL_PREHEAT_GROUP_LOCK_TTL_SECONDS_ENV,
                    defaults.group_lock_ttl_seconds,
                    1,
                    None,
                ),
                circuit_failure_rate_threshold: pool_preheat_env_u32(
                    POOL_PREHEAT_CIRCUIT_FAILURE_RATE_THRESHOLD_ENV,
                    defaults.circuit_failure_rate_threshold,
                    1,
                    Some(POOL_PREHEAT_CIRCUIT_FAILURE_RATE_MAX),
                ),
                circuit_sample_window_seconds: pool_preheat_env_u64(
                    POOL_PREHEAT_CIRCUIT_SAMPLE_WINDOW_SECONDS_ENV,
                    defaults.circuit_sample_window_seconds,
                    1,
                    None,
                ),
                circuit_suspend_seconds: pool_preheat_env_u64(
                    POOL_PREHEAT_CIRCUIT_SUSPEND_SECONDS_ENV,
                    defaults.circuit_suspend_seconds,
                    1,
                    None,
                ),
                five_xx_streak_threshold: pool_preheat_env_u32(
                    POOL_PREHEAT_5XX_STREAK_THRESHOLD_ENV,
                    defaults.five_xx_streak_threshold,
                    1,
                    None,
                ),
            },
        }
    }
}

fn pool_preheat_env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn warn_invalid_pool_preheat_env(name: &str, value: &str) {
    warn!(
        env_key = name,
        env_value = %value,
        "invalid pool preheat env value, using default"
    );
}

fn pool_preheat_env_bool(name: &str, default_value: bool) -> bool {
    let Some(value) = pool_preheat_env_value(name) else {
        return default_value;
    };
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => true,
        "false" | "0" | "no" | "off" => false,
        _ => {
            warn_invalid_pool_preheat_env(name, &value);
            default_value
        }
    }
}

fn pool_preheat_env_u64(
    name: &str,
    default_value: u64,
    min_value: u64,
    max_value: Option<u64>,
) -> u64 {
    let Some(value) = pool_preheat_env_value(name) else {
        return default_value;
    };
    match value.parse::<u64>() {
        Ok(parsed) if parsed >= min_value => max_value.map_or(parsed, |max| parsed.min(max)),
        _ => {
            warn_invalid_pool_preheat_env(name, &value);
            default_value
        }
    }
}

fn pool_preheat_env_u32(
    name: &str,
    default_value: u32,
    min_value: u32,
    max_value: Option<u32>,
) -> u32 {
    let Some(value) = pool_preheat_env_value(name) else {
        return default_value;
    };
    match value.parse::<u32>() {
        Ok(parsed) if parsed >= min_value => max_value.map_or(parsed, |max| parsed.min(max)),
        _ => {
            warn_invalid_pool_preheat_env(name, &value);
            default_value
        }
    }
}

fn pool_preheat_env_usize(name: &str, default_value: usize, min_value: usize) -> usize {
    let Some(value) = pool_preheat_env_value(name) else {
        return default_value;
    };
    match value.parse::<usize>() {
        Ok(parsed) if parsed >= min_value => parsed,
        _ => {
            warn_invalid_pool_preheat_env(name, &value);
            default_value
        }
    }
}

fn pre_probe_value<'a>(pre_probe: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a Value> {
    pre_probe.and_then(|config| config.get(key))
}

fn pre_probe_bool(pre_probe: Option<&Map<String, Value>>, key: &str, default_value: bool) -> bool {
    pre_probe_value(pre_probe, key)
        .and_then(Value::as_bool)
        .unwrap_or(default_value)
}

fn pre_probe_u64(
    pre_probe: Option<&Map<String, Value>>,
    key: &str,
    default_value: u64,
    min_value: u64,
    max_value: Option<u64>,
) -> u64 {
    pre_probe_value(pre_probe, key)
        .and_then(json_u64)
        .filter(|value| *value >= min_value)
        .map(|value| max_value.map_or(value, |max| value.min(max)))
        .unwrap_or(default_value)
}

fn pre_probe_u32(
    pre_probe: Option<&Map<String, Value>>,
    key: &str,
    default_value: u32,
    min_value: u32,
    max_value: Option<u32>,
) -> u32 {
    pre_probe_value(pre_probe, key)
        .and_then(json_u32)
        .filter(|value| *value >= min_value)
        .map(|value| max_value.map_or(value, |max| value.min(max)))
        .unwrap_or(default_value)
}

fn pre_probe_usize(
    pre_probe: Option<&Map<String, Value>>,
    key: &str,
    default_value: usize,
    min_value: usize,
) -> usize {
    pre_probe_value(pre_probe, key)
        .and_then(json_usize)
        .filter(|value| *value >= min_value)
        .unwrap_or(default_value)
}

fn parse_pool_pre_probe_config(
    raw_pool_advanced: Option<&Map<String, Value>>,
    env_config: PoolPreheatEnvConfig,
) -> AdminProviderPoolPreProbeConfig {
    let pre_probe = raw_pool_advanced
        .and_then(|config| config.get("pre_probe"))
        .and_then(Value::as_object);
    let defaults = env_config.pre_probe;

    AdminProviderPoolPreProbeConfig {
        enabled: pre_probe_bool(pre_probe, "enabled", defaults.enabled),
        top_n: pre_probe_u32(pre_probe, "top_n", defaults.top_n, 1, None),
        required_healthy: pre_probe_u32(
            pre_probe,
            "required_healthy",
            defaults.required_healthy,
            1,
            Some(POOL_PREHEAT_REQUIRED_HEALTHY_MAX),
        ),
        dedup_window_secs: pre_probe_u64(
            pre_probe,
            "dedup_window_secs",
            defaults.dedup_window_secs,
            1,
            None,
        ),
        cache_ttl_seconds: pre_probe_u64(
            pre_probe,
            "cache_ttl_seconds",
            defaults.cache_ttl_seconds,
            1,
            None,
        ),
        cache_max_entries: pre_probe_usize(
            pre_probe,
            "cache_max_entries",
            defaults.cache_max_entries,
            1,
        ),
        probe_timeout_seconds: pre_probe_u64(
            pre_probe,
            "probe_timeout_seconds",
            defaults.probe_timeout_seconds,
            1,
            None,
        ),
        per_provider_rate_limit_per_minute: pre_probe_u32(
            pre_probe,
            "per_provider_rate_limit_per_minute",
            defaults.per_provider_rate_limit_per_minute,
            1,
            None,
        ),
        group_lock_ttl_seconds: pre_probe_u64(
            pre_probe,
            "group_lock_ttl_seconds",
            defaults.group_lock_ttl_seconds,
            1,
            None,
        ),
        circuit_failure_rate_threshold: pre_probe_u32(
            pre_probe,
            "circuit_failure_rate_threshold",
            defaults.circuit_failure_rate_threshold,
            1,
            Some(POOL_PREHEAT_CIRCUIT_FAILURE_RATE_MAX),
        ),
        circuit_sample_window_seconds: pre_probe_u64(
            pre_probe,
            "circuit_sample_window_seconds",
            defaults.circuit_sample_window_seconds,
            1,
            None,
        ),
        circuit_suspend_seconds: pre_probe_u64(
            pre_probe,
            "circuit_suspend_seconds",
            defaults.circuit_suspend_seconds,
            1,
            None,
        ),
        five_xx_streak_threshold: pre_probe_u32(
            pre_probe,
            "5xx_streak_threshold",
            defaults.five_xx_streak_threshold,
            1,
            None,
        ),
    }
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

pub(crate) fn admin_provider_pool_config(
    provider: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
) -> Option<AdminProviderPoolConfig> {
    admin_provider_pool_config_from_config_value(provider.config.as_ref())
}

pub(crate) fn admin_provider_pool_config_from_config_value(
    config: Option<&serde_json::Value>,
) -> Option<AdminProviderPoolConfig> {
    let env_config = PoolPreheatEnvConfig::from_env();
    let raw_pool_advanced = config
        .and_then(Value::as_object)
        .and_then(|config| config.get("pool_advanced"))?;

    let Some(pool_advanced) = raw_pool_advanced.as_object() else {
        let pre_probe = parse_pool_pre_probe_config(None, env_config);
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
            pre_probe,
            probing_enabled: false,
            probing_interval_minutes: 10,
            stream_timeout_threshold: 3,
            stream_timeout_window_seconds: 1800,
            stream_timeout_cooldown_seconds: 300,
        });
    };

    let scheduling_presets = parse_pool_scheduling_presets(pool_advanced);
    let unschedulable_rules = parse_pool_unschedulable_rules(pool_advanced);
    let pre_probe = parse_pool_pre_probe_config(Some(pool_advanced), env_config);

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
        pre_probe,
        probing_enabled: pool_advanced
            .get("probing_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        probing_interval_minutes: pool_advanced
            .get("probing_interval_minutes")
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .map(|value| value.min(1440))
            .unwrap_or(10),
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
        admin_provider_pool_config, admin_provider_pool_config_from_config_value,
        PoolPreheatEnvConfig, POOL_PREHEAT_5XX_STREAK_THRESHOLD_ENV,
        POOL_PREHEAT_CACHE_MAX_ENTRIES_ENV, POOL_PREHEAT_CACHE_TTL_SECONDS_ENV,
        POOL_PREHEAT_CIRCUIT_FAILURE_RATE_THRESHOLD_ENV,
        POOL_PREHEAT_CIRCUIT_SAMPLE_WINDOW_SECONDS_ENV, POOL_PREHEAT_CIRCUIT_SUSPEND_SECONDS_ENV,
        POOL_PREHEAT_ENABLED_ENV, POOL_PREHEAT_GROUP_LOCK_TTL_SECONDS_ENV,
        POOL_PREHEAT_PER_KEY_DEDUP_SECONDS_ENV,
        POOL_PREHEAT_PER_PROVIDER_RATE_LIMIT_PER_MINUTE_ENV,
        POOL_PREHEAT_PROBE_TIMEOUT_SECONDS_ENV, POOL_PREHEAT_REQUIRED_HEALTHY_ENV,
        POOL_PREHEAT_TOP_N_ENV,
    };
    use crate::handlers::admin::provider::shared::support::AdminProviderPoolPreProbeConfig;
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
    use serde_json::json;
    use std::ffi::OsString;
    use std::sync::Mutex;

    static POOL_PREHEAT_ENV_LOCK: Mutex<()> = Mutex::new(());

    const POOL_PREHEAT_ENV_KEYS: &[&str] = &[
        POOL_PREHEAT_ENABLED_ENV,
        POOL_PREHEAT_TOP_N_ENV,
        POOL_PREHEAT_REQUIRED_HEALTHY_ENV,
        POOL_PREHEAT_CACHE_TTL_SECONDS_ENV,
        POOL_PREHEAT_CACHE_MAX_ENTRIES_ENV,
        POOL_PREHEAT_PROBE_TIMEOUT_SECONDS_ENV,
        POOL_PREHEAT_PER_KEY_DEDUP_SECONDS_ENV,
        POOL_PREHEAT_PER_PROVIDER_RATE_LIMIT_PER_MINUTE_ENV,
        POOL_PREHEAT_GROUP_LOCK_TTL_SECONDS_ENV,
        POOL_PREHEAT_CIRCUIT_FAILURE_RATE_THRESHOLD_ENV,
        POOL_PREHEAT_CIRCUIT_SAMPLE_WINDOW_SECONDS_ENV,
        POOL_PREHEAT_CIRCUIT_SUSPEND_SECONDS_ENV,
        POOL_PREHEAT_5XX_STREAK_THRESHOLD_ENV,
    ];

    struct PoolPreheatEnvRestore {
        previous: Vec<(&'static str, Option<OsString>)>,
    }

    impl Drop for PoolPreheatEnvRestore {
        fn drop(&mut self) {
            for (key, value) in self.previous.drain(..) {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    fn with_pool_preheat_env<R>(
        values: &[(&'static str, &'static str)],
        run: impl FnOnce() -> R,
    ) -> R {
        let _guard = POOL_PREHEAT_ENV_LOCK
            .lock()
            .expect("pool preheat env lock should not be poisoned");
        let previous = POOL_PREHEAT_ENV_KEYS
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect::<Vec<_>>();
        for key in POOL_PREHEAT_ENV_KEYS {
            std::env::remove_var(key);
        }
        for (key, value) in values {
            std::env::set_var(key, value);
        }
        let _restore = PoolPreheatEnvRestore { previous };
        run()
    }

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
                "probing_interval_minutes": 20,
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
        assert_eq!(config.probing_interval_minutes, 20);
        assert_eq!(config.stream_timeout_threshold, 4);
        assert_eq!(config.stream_timeout_window_seconds, 900);
        assert_eq!(config.stream_timeout_cooldown_seconds, 180);
    }

    #[test]
    fn clamps_pool_quota_probe_interval_to_python_range() {
        let provider = sample_provider(json!({
            "pool_advanced": {
                "probing_enabled": true,
                "probing_interval_minutes": 2000,
            }
        }));
        let config = admin_provider_pool_config(&provider).expect("pool config should exist");
        assert_eq!(config.probing_interval_minutes, 1440);

        let provider = sample_provider(json!({
            "pool_advanced": {
                "probing_enabled": true,
                "probing_interval_minutes": 0,
            }
        }));
        let config = admin_provider_pool_config(&provider).expect("pool config should exist");
        assert_eq!(config.probing_interval_minutes, 10);
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
    fn pool_preheat_config_defaults_disabled() {
        with_pool_preheat_env(&[], || {
            let config = admin_provider_pool_config_from_config_value(Some(&json!({
                "pool_advanced": {}
            })))
            .expect("pool config should parse");

            // `pre_probe.enabled` gates only Layer 2/3 candidate preheat behavior.
            assert_eq!(config.pre_probe, AdminProviderPoolPreProbeConfig::default());
            assert!(!config.pre_probe.enabled);
        });
    }

    #[test]
    fn pool_preheat_config_parses_enabled_pre_probe() {
        with_pool_preheat_env(&[], || {
            let config = admin_provider_pool_config_from_config_value(Some(&json!({
                "pool_advanced": {
                    "pre_probe": {
                        "enabled": true,
                        "top_n": 12,
                        "required_healthy": 6,
                        "dedup_window_secs": 240,
                        "cache_ttl_seconds": 180,
                        "cache_max_entries": 2048,
                        "probe_timeout_seconds": 11,
                        "per_provider_rate_limit_per_minute": 45,
                        "group_lock_ttl_seconds": 12,
                        "circuit_failure_rate_threshold": 40,
                        "circuit_sample_window_seconds": 240,
                        "circuit_suspend_seconds": 480,
                        "5xx_streak_threshold": 4
                    }
                }
            })))
            .expect("pool config should parse");

            assert!(config.pre_probe.enabled);
            assert_eq!(config.pre_probe.top_n, 12);
            assert_eq!(config.pre_probe.required_healthy, 6);
            assert_eq!(config.pre_probe.dedup_window_secs, 240);
            assert_eq!(config.pre_probe.cache_ttl_seconds, 180);
            assert_eq!(config.pre_probe.cache_max_entries, 2048);
            assert_eq!(config.pre_probe.probe_timeout_seconds, 11);
            assert_eq!(config.pre_probe.per_provider_rate_limit_per_minute, 45);
            assert_eq!(config.pre_probe.group_lock_ttl_seconds, 12);
            assert_eq!(config.pre_probe.circuit_failure_rate_threshold, 40);
            assert_eq!(config.pre_probe.circuit_sample_window_seconds, 240);
            assert_eq!(config.pre_probe.circuit_suspend_seconds, 480);
            assert_eq!(config.pre_probe.five_xx_streak_threshold, 4);
        });
    }

    #[test]
    fn pool_preheat_config_clamps_invalid_numeric_values() {
        with_pool_preheat_env(&[], || {
            let config = admin_provider_pool_config_from_config_value(Some(&json!({
                "pool_advanced": {
                    "pre_probe": {
                        "enabled": true,
                        "top_n": 0,
                        "required_healthy": 99,
                        "dedup_window_secs": 0,
                        "cache_ttl_seconds": 0,
                        "cache_max_entries": 0,
                        "probe_timeout_seconds": 0,
                        "per_provider_rate_limit_per_minute": 0,
                        "group_lock_ttl_seconds": 0,
                        "circuit_failure_rate_threshold": 150,
                        "circuit_sample_window_seconds": 0,
                        "circuit_suspend_seconds": 0,
                        "5xx_streak_threshold": 0
                    }
                }
            })))
            .expect("pool config should parse");
            let defaults = AdminProviderPoolPreProbeConfig::default();

            assert!(config.pre_probe.enabled);
            assert_eq!(config.pre_probe.top_n, defaults.top_n);
            assert_eq!(config.pre_probe.required_healthy, 8);
            assert_eq!(
                config.pre_probe.dedup_window_secs,
                defaults.dedup_window_secs
            );
            assert_eq!(
                config.pre_probe.cache_ttl_seconds,
                defaults.cache_ttl_seconds
            );
            assert_eq!(
                config.pre_probe.cache_max_entries,
                defaults.cache_max_entries
            );
            assert_eq!(
                config.pre_probe.probe_timeout_seconds,
                defaults.probe_timeout_seconds
            );
            assert_eq!(
                config.pre_probe.per_provider_rate_limit_per_minute,
                defaults.per_provider_rate_limit_per_minute
            );
            assert_eq!(
                config.pre_probe.group_lock_ttl_seconds,
                defaults.group_lock_ttl_seconds
            );
            assert_eq!(config.pre_probe.circuit_failure_rate_threshold, 100);
            assert_eq!(
                config.pre_probe.circuit_sample_window_seconds,
                defaults.circuit_sample_window_seconds
            );
            assert_eq!(
                config.pre_probe.circuit_suspend_seconds,
                defaults.circuit_suspend_seconds
            );
            assert_eq!(
                config.pre_probe.five_xx_streak_threshold,
                defaults.five_xx_streak_threshold
            );
        });
    }

    #[test]
    fn pool_preheat_config_env_defaults_when_missing_or_invalid() {
        with_pool_preheat_env(&[], || {
            assert_eq!(
                PoolPreheatEnvConfig::from_env().pre_probe,
                AdminProviderPoolPreProbeConfig::default()
            );
        });
        with_pool_preheat_env(
            &[
                (POOL_PREHEAT_ENABLED_ENV, "sometimes"),
                (POOL_PREHEAT_TOP_N_ENV, "0"),
                (POOL_PREHEAT_REQUIRED_HEALTHY_ENV, "0"),
                (POOL_PREHEAT_CACHE_TTL_SECONDS_ENV, "nope"),
                (POOL_PREHEAT_CACHE_MAX_ENTRIES_ENV, "0"),
                (POOL_PREHEAT_PROBE_TIMEOUT_SECONDS_ENV, "0"),
                (POOL_PREHEAT_PER_KEY_DEDUP_SECONDS_ENV, "0"),
                (POOL_PREHEAT_PER_PROVIDER_RATE_LIMIT_PER_MINUTE_ENV, "0"),
                (POOL_PREHEAT_GROUP_LOCK_TTL_SECONDS_ENV, "0"),
                (POOL_PREHEAT_CIRCUIT_FAILURE_RATE_THRESHOLD_ENV, "0"),
                (POOL_PREHEAT_CIRCUIT_SAMPLE_WINDOW_SECONDS_ENV, "0"),
                (POOL_PREHEAT_CIRCUIT_SUSPEND_SECONDS_ENV, "0"),
                (POOL_PREHEAT_5XX_STREAK_THRESHOLD_ENV, "0"),
            ],
            || {
                assert_eq!(
                    PoolPreheatEnvConfig::from_env().pre_probe,
                    AdminProviderPoolPreProbeConfig::default()
                );
            },
        );
    }

    #[test]
    fn pool_preheat_config_provider_values_override_env_defaults() {
        with_pool_preheat_env(
            &[
                (POOL_PREHEAT_ENABLED_ENV, "true"),
                (POOL_PREHEAT_REQUIRED_HEALTHY_ENV, "6"),
                (POOL_PREHEAT_PER_PROVIDER_RATE_LIMIT_PER_MINUTE_ENV, "60"),
            ],
            || {
                let env_only = admin_provider_pool_config_from_config_value(Some(&json!({
                    "pool_advanced": {"pre_probe": {}}
                })))
                .expect("pool config should parse");
                assert!(env_only.pre_probe.enabled);
                assert_eq!(env_only.pre_probe.required_healthy, 6);
                assert_eq!(env_only.pre_probe.per_provider_rate_limit_per_minute, 60);

                let overridden = admin_provider_pool_config_from_config_value(Some(&json!({
                    "pool_advanced": {
                        "pre_probe": {
                            "enabled": false,
                            "required_healthy": 4,
                            "per_provider_rate_limit_per_minute": 45
                        }
                    }
                })))
                .expect("pool config should parse");
                assert!(!overridden.pre_probe.enabled);
                assert_eq!(overridden.pre_probe.required_healthy, 4);
                assert_eq!(overridden.pre_probe.per_provider_rate_limit_per_minute, 45);
            },
        );
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
}
