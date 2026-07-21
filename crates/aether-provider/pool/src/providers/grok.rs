use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint;
use serde_json::{Map, Value};

use crate::capability::ProviderPoolCapabilities;
use crate::provider::{
    provider_pool_endpoint_format_matches, provider_pool_matching_endpoint, ProviderPoolAdapter,
    ProviderPoolMemberInput,
};
use crate::quota::{
    provider_pool_current_unix_secs, provider_pool_json_bool, provider_pool_json_f64,
    provider_pool_metadata_bucket, provider_pool_quota_snapshot_exhausted_decision,
    provider_pool_reset_deadline_elapsed, provider_pool_timestamp_unix_secs,
};

pub const GROK_QUOTA_WINDOWS_BASIC: &[(&str, &str)] = &[("quota_fast", "fast")];
pub const GROK_QUOTA_WINDOWS_SUPER: &[(&str, &str)] = &[
    ("quota_auto", "auto"),
    ("quota_fast", "fast"),
    ("quota_expert", "expert"),
    ("quota_grok_4_3", "grok-420-computer-use-sa"),
];
pub const GROK_QUOTA_WINDOWS_HEAVY: &[(&str, &str)] = &[
    ("quota_auto", "auto"),
    ("quota_fast", "fast"),
    ("quota_expert", "expert"),
    ("quota_heavy", "heavy"),
    ("quota_grok_4_3", "grok-420-computer-use-sa"),
];

#[derive(Debug, Clone, Default)]
pub struct GrokProviderPoolAdapter;

impl ProviderPoolAdapter for GrokProviderPoolAdapter {
    fn provider_type(&self) -> &'static str {
        "grok"
    }

    fn capabilities(&self) -> ProviderPoolCapabilities {
        ProviderPoolCapabilities {
            plan_tier: true,
            quota_reset: true,
            quota_refresh: true,
        }
    }

    fn quota_exhausted(&self, input: &ProviderPoolMemberInput<'_>) -> bool {
        if let Some(exhausted) =
            provider_pool_quota_snapshot_exhausted_decision(input.key, input.provider_type)
        {
            return exhausted;
        }
        provider_pool_metadata_bucket(input.key.upstream_metadata.as_ref(), input.provider_type)
            .is_some_and(quota_exhausted_from_bucket)
    }

    fn quota_refresh_endpoint(
        &self,
        endpoints: &[StoredProviderCatalogEndpoint],
        include_inactive: bool,
    ) -> Option<StoredProviderCatalogEndpoint> {
        provider_pool_matching_endpoint(endpoints, include_inactive, |endpoint| {
            provider_pool_endpoint_format_matches(endpoint, "openai:chat")
        })
        .or_else(|| provider_pool_matching_endpoint(endpoints, include_inactive, |_| true))
    }

    fn quota_refresh_missing_endpoint_message(&self) -> String {
        "找不到有效的 Grok 端点".to_string()
    }
}

pub fn grok_supported_quota_windows_for_tier(
    tier: Option<&str>,
) -> &'static [(&'static str, &'static str)] {
    match grok_normalize_pool_tier(tier) {
        Some("basic") => GROK_QUOTA_WINDOWS_BASIC,
        Some("super") => GROK_QUOTA_WINDOWS_SUPER,
        Some("heavy") => GROK_QUOTA_WINDOWS_HEAVY,
        _ => GROK_QUOTA_WINDOWS_HEAVY,
    }
}

pub fn grok_pool_tier_from_quota_bucket(bucket: &Map<String, Value>) -> Option<&'static str> {
    if let Some(tier) = grok_normalize_pool_tier(
        grok_bucket_string(bucket, &["pool_tier", "tier", "plan_type", "plan"]).as_deref(),
    ) {
        return Some(tier);
    }

    if let Some(auto_total) = grok_quota_total(bucket, "quota_auto") {
        if (auto_total - 50.0).abs() < f64::EPSILON {
            return Some("super");
        }
        if (auto_total - 150.0).abs() < f64::EPSILON {
            return Some("heavy");
        }
    }

    if let Some(fast_total) = grok_quota_total(bucket, "quota_fast") {
        if (fast_total - 30.0).abs() < f64::EPSILON {
            return Some("basic");
        }
        if (fast_total - 140.0).abs() < f64::EPSILON {
            return Some("super");
        }
        if (fast_total - 400.0).abs() < f64::EPSILON {
            return Some("heavy");
        }
    }

    None
}

pub fn grok_quota_window_key_for_model(model: Option<&str>) -> Option<&'static str> {
    Some(match grok_mode_id_for_model(model) {
        "fast" => "quota_fast",
        "auto" => "quota_auto",
        "expert" => "quota_expert",
        "heavy" => "quota_heavy",
        "grok-420-computer-use-sa" => "quota_grok_4_3",
        _ => return None,
    })
}

pub fn grok_mode_id_for_model(model: Option<&str>) -> &'static str {
    let model = model.unwrap_or_default().to_ascii_lowercase();
    if model.contains("4.3") || model.contains("computer") {
        "grok-420-computer-use-sa"
    } else if model.contains("multi-agent") {
        "heavy"
    } else if model.contains("non-reasoning") || model.contains("fast") || model.contains("lite") {
        "fast"
    } else if model.contains("expert") || model.contains("reasoning") {
        "expert"
    } else if model.contains("0309-heavy") {
        "auto"
    } else if model.contains("heavy") {
        "heavy"
    } else {
        "auto"
    }
}

fn grok_normalize_pool_tier(value: Option<&str>) -> Option<&'static str> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "basic" => Some("basic"),
        "super" => Some("super"),
        "heavy" => Some("heavy"),
        _ => None,
    }
}

fn grok_bucket_string(bucket: &Map<String, Value>, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        bucket
            .get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn grok_quota_total(quota_by_model: &Map<String, Value>, key: &str) -> Option<f64> {
    let models = quota_by_model
        .get("quota_by_model")
        .or_else(|| quota_by_model.get("models"))
        .and_then(Value::as_object)
        .unwrap_or(quota_by_model);
    models
        .get(key)
        .and_then(Value::as_object)
        .and_then(|quota| quota.get("total"))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
}

pub(crate) fn quota_exhausted_from_bucket(bucket: &Map<String, Value>) -> bool {
    let models = bucket
        .get("quota_by_model")
        .or_else(|| bucket.get("models"))
        .and_then(Value::as_object);
    let Some(models) = models else {
        return false;
    };

    let supported_mode_keys =
        grok_supported_quota_windows_for_tier(grok_pool_tier_from_quota_bucket(bucket))
            .iter()
            .map(|(quota_key, _)| *quota_key)
            .collect::<Vec<_>>();

    let mut model_count = 0usize;
    let mut exhausted_count = 0usize;
    let now_unix_secs = provider_pool_current_unix_secs();
    let bucket_updated_at = provider_pool_timestamp_unix_secs(bucket.get("updated_at"));
    for (model_key, item) in models.iter() {
        if !supported_mode_keys.is_empty() && !supported_mode_keys.contains(&model_key.as_str()) {
            continue;
        }

        let Some(item) = item.as_object() else {
            continue;
        };
        let has_quota_data = provider_pool_json_bool(item.get("is_exhausted")).is_some()
            || provider_pool_json_f64(item.get("used_percent")).is_some()
            || provider_pool_json_f64(item.get("remaining")).is_some()
            || provider_pool_json_f64(item.get("remaining_fraction")).is_some();
        if !has_quota_data {
            continue;
        }
        model_count += 1;
        let quota_exhausted = provider_pool_json_bool(item.get("is_exhausted")) == Some(true)
            || provider_pool_json_f64(item.get("used_percent")).is_some_and(|value| value >= 100.0)
            || provider_pool_json_f64(item.get("remaining")).is_some_and(|value| value <= 0.0)
            || provider_pool_json_f64(item.get("remaining_fraction"))
                .is_some_and(|value| value <= 0.0);
        let reset_elapsed = now_unix_secs
            .is_some_and(|now| provider_pool_reset_deadline_elapsed(item, bucket_updated_at, now));
        if quota_exhausted && !reset_elapsed {
            exhausted_count += 1;
        }
    }
    model_count > 0 && model_count == exhausted_count
}
