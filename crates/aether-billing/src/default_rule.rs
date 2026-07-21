use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::pricing::BillingPricingResolution;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VirtualBillingRule {
    pub id: String,
    pub name: String,
    pub task_type: String,
    pub expression: String,
    pub variables: BTreeMap<String, Value>,
    pub dimension_mappings: BTreeMap<String, Value>,
    pub scope: String,
}

pub struct DefaultBillingRuleGenerator;

impl DefaultBillingRuleGenerator {
    pub fn generate_for_pricing(
        global_model_name: &str,
        pricing: &BillingPricingResolution,
        task_type: &str,
    ) -> Option<VirtualBillingRule> {
        let pricing_config = pricing.tiered_pricing.as_ref();
        let tiers = pricing_config
            .and_then(|value| value.get("tiers"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let explicit_image_output_price_default =
            explicit_image_output_price_default(pricing_config);
        let image_output_price_default = explicit_image_output_price_default.unwrap_or(0.0);
        let has_image_output_matrix = explicit_image_output_price_entries(pricing_config)
            .is_some_and(|entries| !entries.is_empty());
        let has_image_output_ranges = explicit_image_output_price_ranges(pricing_config)
            .is_some_and(|ranges| !ranges.is_empty());
        let has_image_output_pricing = has_image_output_matrix
            || has_image_output_ranges
            || explicit_image_output_price_default.is_some();

        if tiers.is_empty() && pricing.price_per_request.is_none() && !has_image_output_pricing {
            return None;
        }

        let first_tier = tiers.first().cloned().unwrap_or_else(|| json!({}));
        let base_input_price = tier_value(&first_tier, "input_price_per_1m", 0.0);
        let base_output_price = tier_value(&first_tier, "output_price_per_1m", 0.0);
        let base_cache_creation_price =
            tier_value_with_fallback(&first_tier, "cache_creation_price_per_1m", 1.25);
        let base_cache_read_price =
            tier_value_with_fallback(&first_tier, "cache_read_price_per_1m", 0.1);
        let base_request_price = pricing.price_per_request.unwrap_or(0.0);

        let mut variables = BTreeMap::new();
        variables.insert("input_price_per_1m".to_string(), json!(base_input_price));
        variables.insert("output_price_per_1m".to_string(), json!(base_output_price));
        variables.insert(
            "cache_creation_price_per_1m".to_string(),
            json!(base_cache_creation_price),
        );
        variables.insert(
            "cache_creation_ephemeral_5m_price_per_1m".to_string(),
            json!(base_cache_creation_price),
        );
        variables.insert(
            "cache_creation_ephemeral_1h_price_per_1m".to_string(),
            json!(base_cache_creation_price),
        );
        variables.insert(
            "cache_read_price_per_1m".to_string(),
            json!(base_cache_read_price),
        );
        variables.insert("price_per_request".to_string(), json!(base_request_price));
        variables.insert(
            "image_output_price_per_image".to_string(),
            json!(image_output_price_default),
        );

        let mut dimension_mappings = BTreeMap::new();
        for (name, key, default) in [
            ("input_tokens", "input_tokens", json!(0)),
            ("output_tokens", "output_tokens", json!(0)),
            ("cache_creation_tokens", "cache_creation_tokens", json!(0)),
            (
                "cache_creation_ephemeral_5m_tokens",
                "cache_creation_ephemeral_5m_tokens",
                json!(0),
            ),
            (
                "cache_creation_ephemeral_1h_tokens",
                "cache_creation_ephemeral_1h_tokens",
                json!(0),
            ),
            (
                "cache_creation_uncategorized_tokens",
                "cache_creation_uncategorized_tokens",
                json!(0),
            ),
            ("cache_read_tokens", "cache_read_tokens", json!(0)),
            ("request_count", "request_count", json!(1)),
            ("image_count", "image_count", json!(0)),
            ("image_count_unmetered", "image_count_unmetered", json!(0)),
            ("image_price_key", "image_price_key", json!("default")),
            (
                "image_output_price_per_image",
                "image_output_price_per_image",
                json!(image_output_price_default),
            ),
        ] {
            dimension_mappings.insert(
                name.to_string(),
                json!({
                    "source": "dimension",
                    "key": key,
                    "required": false,
                    "allow_zero": true,
                    "default": default,
                }),
            );
        }

        for (name, expression) in [
            ("input_cost", "input_tokens * input_price_per_1m / 1000000"),
            (
                "output_cost",
                "output_tokens * output_price_per_1m / 1000000",
            ),
            (
                "cache_creation_uncategorized_cost",
                "cache_creation_uncategorized_tokens * cache_creation_price_per_1m / 1000000",
            ),
            (
                "cache_creation_ephemeral_5m_cost",
                "cache_creation_ephemeral_5m_tokens * cache_creation_ephemeral_5m_price_per_1m / 1000000",
            ),
            (
                "cache_creation_ephemeral_1h_cost",
                "cache_creation_ephemeral_1h_tokens * cache_creation_ephemeral_1h_price_per_1m / 1000000",
            ),
            (
                "cache_read_cost",
                "cache_read_tokens * cache_read_price_per_1m / 1000000",
            ),
            (
                "image_output_cost",
                "image_count_unmetered * image_output_price_per_image",
            ),
            ("request_cost", "request_count * price_per_request"),
        ] {
            dimension_mappings.insert(
                name.to_string(),
                json!({
                    "source": "computed",
                    "expression": expression,
                    "required": false,
                    "default": 0,
                }),
            );
        }

        if !tiers.is_empty() {
            dimension_mappings.insert(
                "input_price_per_1m".to_string(),
                json!({
                    "source": "tiered",
                    "tier_key": "total_input_context",
                    "allow_zero": true,
                    "tiers": build_tier_entries(&tiers, "input_price_per_1m", None, false),
                    "default": base_input_price,
                }),
            );
            dimension_mappings.insert(
                "output_price_per_1m".to_string(),
                json!({
                    "source": "tiered",
                    "tier_key": "total_input_context",
                    "allow_zero": true,
                    "tiers": build_tier_entries(&tiers, "output_price_per_1m", None, false),
                    "default": base_output_price,
                }),
            );
            dimension_mappings.insert(
                "cache_creation_price_per_1m".to_string(),
                json!({
                    "source": "tiered",
                    "tier_key": "total_input_context",
                    "allow_zero": true,
                    "ttl_key": "cache_ttl_minutes",
                    "ttl_value_key": "cache_creation_price_per_1m",
                    "tiers": build_tier_entries(&tiers, "cache_creation_price_per_1m", Some(1.25), true),
                    "default": base_cache_creation_price,
                }),
            );
            dimension_mappings.insert(
                "cache_creation_ephemeral_5m_price_per_1m".to_string(),
                json!({
                    "source": "tiered",
                    "tier_key": "total_input_context",
                    "allow_zero": true,
                    "ttl_key": "cache_creation_ephemeral_5m_ttl_minutes",
                    "ttl_value_key": "cache_creation_price_per_1m",
                    "tiers": build_tier_entries(&tiers, "cache_creation_price_per_1m", Some(1.25), true),
                    "default": base_cache_creation_price,
                }),
            );
            dimension_mappings.insert(
                "cache_creation_ephemeral_1h_price_per_1m".to_string(),
                json!({
                    "source": "tiered",
                    "tier_key": "total_input_context",
                    "allow_zero": true,
                    "ttl_key": "cache_creation_ephemeral_1h_ttl_minutes",
                    "ttl_value_key": "cache_creation_price_per_1m",
                    "tiers": build_tier_entries(&tiers, "cache_creation_price_per_1m", Some(1.25), true),
                    "default": base_cache_creation_price,
                }),
            );
            dimension_mappings.insert(
                "cache_read_price_per_1m".to_string(),
                json!({
                    "source": "tiered",
                    "tier_key": "total_input_context",
                    "allow_zero": true,
                    "ttl_key": "cache_ttl_minutes",
                    "ttl_value_key": "cache_read_price_per_1m",
                    "tiers": build_tier_entries(&tiers, "cache_read_price_per_1m", Some(0.1), true),
                    "default": base_cache_read_price,
                }),
            );
        }

        Some(VirtualBillingRule {
            id: "__default__".to_string(),
            name: format!("Default rule for {global_model_name}"),
            task_type: normalize_task_type(task_type).to_string(),
            expression: "input_cost + output_cost + cache_creation_uncategorized_cost + cache_creation_ephemeral_5m_cost + cache_creation_ephemeral_1h_cost + cache_read_cost + image_output_cost + request_cost".to_string(),
            variables,
            dimension_mappings,
            scope: "default".to_string(),
        })
    }
}

pub fn normalize_task_type(task_type: &str) -> &str {
    if task_type.trim().eq_ignore_ascii_case("cli") {
        "chat"
    } else {
        task_type.trim()
    }
}

fn tier_value(tier: &Value, key: &str, default: f64) -> f64 {
    tier.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn tier_value_with_fallback(tier: &Value, key: &str, default_multiplier: f64) -> f64 {
    if let Some(value) = tier.get(key).and_then(Value::as_f64) {
        return value;
    }
    tier.get("input_price_per_1m")
        .and_then(Value::as_f64)
        .map(|value| value * default_multiplier)
        .unwrap_or(0.0)
}

fn build_tier_entries(
    tiers: &[Value],
    key: &str,
    default_multiplier: Option<f64>,
    include_cache_ttl_pricing: bool,
) -> Vec<Value> {
    tiers
        .iter()
        .map(|tier| {
            let mut value = serde_json::Map::new();
            value.insert(
                "up_to".to_string(),
                tier.get("up_to").cloned().unwrap_or(Value::Null),
            );
            let resolved = match default_multiplier {
                Some(multiplier) => Value::from(tier_value_with_fallback(tier, key, multiplier)),
                None => Value::from(tier_value(tier, key, 0.0)),
            };
            value.insert("value".to_string(), resolved);
            if include_cache_ttl_pricing {
                if let Some(ttl_pricing) = tier.get("cache_ttl_pricing").cloned() {
                    value.insert("cache_ttl_pricing".to_string(), ttl_pricing);
                }
            }
            Value::Object(value)
        })
        .collect()
}

pub(crate) fn explicit_image_output_price_entries(
    pricing_config: Option<&Value>,
) -> Option<BTreeMap<String, Value>> {
    let pricing_config = pricing_config?;
    let mut entries = BTreeMap::new();
    for key in [
        "image_output_prices",
        "image_output_price_per_image",
        "image_output_price_matrix",
        "image_prices",
    ] {
        if let Some(value) = pricing_config.get(key) {
            collect_image_output_price_entries(value, &mut entries);
        }
    }
    Some(entries)
}

pub(crate) fn explicit_image_output_price_ranges(
    pricing_config: Option<&Value>,
) -> Option<Vec<Value>> {
    let pricing_config = pricing_config?;
    let Some(value) = pricing_config.get("image_output_price_ranges") else {
        return Some(Vec::new());
    };

    let mut ranges = Vec::new();
    match value {
        Value::Array(items) => {
            for item in items {
                let Some(object) = item.as_object() else {
                    continue;
                };
                let mut range = serde_json::Map::new();
                if let Some(up_to_pixels) = object
                    .get("up_to_pixels")
                    .or_else(|| object.get("up_to"))
                    .or_else(|| object.get("max_pixels"))
                {
                    range.insert("up_to_pixels".to_string(), up_to_pixels.clone());
                }
                if let Some(label) = object.get("label").cloned() {
                    range.insert("label".to_string(), label);
                }
                if let Some(prices) = object.get("prices") {
                    range.insert("prices".to_string(), prices.clone());
                } else {
                    let mut prices = serde_json::Map::new();
                    for quality in ["low", "medium", "high"] {
                        if let Some(price) = object.get(quality).and_then(Value::as_f64) {
                            prices.insert(quality.to_string(), json!(price));
                        }
                    }
                    if prices.is_empty() {
                        if let Some(price) = object
                            .get("price_per_image")
                            .or_else(|| object.get("price"))
                            .or_else(|| object.get("value"))
                            .and_then(Value::as_f64)
                        {
                            prices.insert("default".to_string(), json!(price));
                        }
                    }
                    if !prices.is_empty() {
                        range.insert("prices".to_string(), Value::Object(prices));
                    }
                }
                if !range.is_empty() {
                    ranges.push(Value::Object(range));
                }
            }
        }
        Value::Object(object) => {
            for (key, item) in object {
                let Some(entry) = item.as_object() else {
                    continue;
                };
                let mut range = serde_json::Map::new();
                if let Some(up_to_pixels) = entry
                    .get("up_to_pixels")
                    .or_else(|| entry.get("up_to"))
                    .or_else(|| entry.get("max_pixels"))
                {
                    range.insert("up_to_pixels".to_string(), up_to_pixels.clone());
                } else if let Ok(parsed) = key.parse::<u64>() {
                    range.insert("up_to_pixels".to_string(), json!(parsed));
                }
                if let Some(label) = entry.get("label").cloned() {
                    range.insert("label".to_string(), label);
                }
                if let Some(prices) = entry.get("prices") {
                    range.insert("prices".to_string(), prices.clone());
                }
                if !range.is_empty() {
                    ranges.push(Value::Object(range));
                }
            }
        }
        _ => {}
    }

    Some(ranges)
}

pub(crate) fn explicit_image_output_price_default(pricing_config: Option<&Value>) -> Option<f64> {
    let pricing_config = pricing_config?;
    pricing_config
        .get("image_output_price_default")
        .or_else(|| pricing_config.get("image_price_default"))
        .or_else(|| {
            pricing_config
                .get("image_output_prices")
                .and_then(|value| value.get("default"))
        })
        .and_then(Value::as_f64)
}

fn collect_image_output_price_entries(value: &Value, entries: &mut BTreeMap<String, Value>) {
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            if key.eq_ignore_ascii_case("default") {
                continue;
            }
            if let Some(price) = value.as_f64() {
                entries.insert(normalize_image_price_key(key), json!(price));
                continue;
            }
            let Some(nested) = value.as_object() else {
                continue;
            };
            let key_is_quality = matches_quality_key(key);
            for (nested_key, nested_value) in nested {
                let Some(price) = nested_value.as_f64() else {
                    continue;
                };
                let (size, quality) = if key_is_quality {
                    (nested_key.as_str(), key.as_str())
                } else {
                    (key.as_str(), nested_key.as_str())
                };
                entries.insert(image_price_key(size, quality), json!(price));
            }
        }
        return;
    }

    if let Some(items) = value.as_array() {
        for item in items.iter().filter_map(Value::as_object) {
            let Some(size) = item.get("size").and_then(Value::as_str) else {
                continue;
            };
            let quality = item
                .get("quality")
                .and_then(Value::as_str)
                .unwrap_or("medium");
            let Some(price) = item
                .get("price_per_image")
                .or_else(|| item.get("price"))
                .or_else(|| item.get("cost"))
                .and_then(Value::as_f64)
            else {
                continue;
            };
            entries.insert(image_price_key(size, quality), json!(price));
        }
    }
}

fn normalize_image_price_key(value: &str) -> String {
    if let Some((size, quality)) = value.split_once(':').or_else(|| value.split_once('|')) {
        return image_price_key(size, quality);
    }
    value.trim().to_ascii_lowercase().replace(' ', "")
}

fn image_price_key(size: &str, quality: &str) -> String {
    format!(
        "{}:{}",
        normalize_image_size(size),
        normalize_image_quality(quality)
    )
}

fn normalize_image_size(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(' ', "")
}

fn normalize_image_quality(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn matches_quality_key(value: &str) -> bool {
    matches!(
        normalize_image_quality(value).as_str(),
        "low" | "medium" | "high"
    )
}
