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
        let video_pricing = video_pricing_state(pricing.video_pricing.as_ref());

        if tiers.is_empty()
            && pricing.price_per_request.is_none()
            && !has_image_output_pricing
            && !video_pricing.per_second_enabled
            && !video_pricing.per_token_by_resolution_enabled
        {
            return None;
        }

        let first_tier = tiers.first().cloned().unwrap_or_else(|| json!({}));
        // Per-second video pricing replaces token pricing rather than adding to
        // it, so the token rates are zeroed here. Enforcing it at rule level
        // means a stale token price in the config cannot double-charge.
        //
        // Resolution-keyed token pricing also bypasses the tier catalog, but its
        // rates arrive as dimensions instead: the price depends on the rendered
        // resolution and on whether a reference video was supplied, neither of
        // which the tier catalog can key on.
        let (base_input_price, base_output_price) = if video_pricing.per_second_enabled {
            (0.0, 0.0)
        } else {
            (
                tier_value(&first_tier, "input_price_per_1m", 0.0),
                tier_value(&first_tier, "output_price_per_1m", 0.0),
            )
        };
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
            (
                "video_cost",
                "video_seconds_unmetered * video_price_per_second",
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
            // Per-second billing zeroes the token rates above. The tier tables
            // have to be dropped alongside them, or a tier lookup would resolve
            // the stale configured price and re-introduce double charging.
            let token_tier_entries = |key: &str| -> Vec<Value> {
                if video_pricing.per_second_enabled {
                    Vec::new()
                } else {
                    build_tier_entries(&tiers, key, None, false)
                }
            };
            dimension_mappings.insert(
                "input_price_per_1m".to_string(),
                json!({
                    "source": "tiered",
                    "tier_key": "total_input_context",
                    "allow_zero": true,
                    "tiers": token_tier_entries("input_price_per_1m"),
                    "default": base_input_price,
                }),
            );
            dimension_mappings.insert(
                "output_price_per_1m".to_string(),
                json!({
                    "source": "tiered",
                    "tier_key": "total_input_context",
                    "allow_zero": true,
                    "tiers": token_tier_entries("output_price_per_1m"),
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

        // Resolution-keyed token rates replace whatever the tier catalog would
        // have resolved. They are written last so the override holds regardless
        // of whether a tier catalog was configured, and read from dimensions
        // because only settlement knows the rendered resolution.
        if video_pricing.per_token_by_resolution_enabled {
            for (name, key) in [
                ("input_price_per_1m", "video_token_input_price_per_1m"),
                ("output_price_per_1m", "video_token_output_price_per_1m"),
            ] {
                dimension_mappings.insert(
                    name.to_string(),
                    json!({
                        "source": "dimension",
                        "key": key,
                        "required": false,
                        "allow_zero": true,
                        "default": 0.0,
                    }),
                );
            }
        }

        Some(VirtualBillingRule {
            id: "__default__".to_string(),
            name: format!("Default rule for {global_model_name}"),
            task_type: normalize_task_type(task_type).to_string(),
            expression: "input_cost + output_cost + cache_creation_uncategorized_cost + cache_creation_ephemeral_5m_cost + cache_creation_ephemeral_1h_cost + cache_read_cost + image_output_cost + request_cost + video_cost".to_string(),
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

/// How a video model charges: by rendered seconds, or by tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum VideoBillingMode {
    /// Resolution-keyed price multiplied by the rendered duration.
    #[default]
    PerSecond,
    /// Ordinary token pricing, as reported by the provider.
    PerToken,
}

impl VideoBillingMode {
    fn from_config(video_pricing: Option<&Value>) -> Self {
        match video_pricing
            .and_then(|value| value.get("mode"))
            .and_then(Value::as_str)
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("per_token") | Some("token") => Self::PerToken,
            // Absent mode keeps existing per-second configurations working.
            _ => Self::PerSecond,
        }
    }
}

/// Config key holding the per-second price table for one input kind.
const VIDEO_PER_SECOND_TABLE_KEY: &str = "price_per_second_by_resolution";
/// Config key holding the per-second price used when no resolution row matches.
const VIDEO_PER_SECOND_DEFAULT_KEY: &str = "price_per_second_default";
/// Config key holding the resolution-keyed token price table for one input kind.
const VIDEO_TOKEN_TABLE_KEY: &str = "token_prices_by_resolution";
/// Config key holding the token rates used when no resolution row matches.
const VIDEO_TOKEN_DEFAULT_KEY: &str = "token_price_default";
/// Nested section overriding prices for requests that carry a reference video.
const VIDEO_WITH_INPUT_KEY: &str = "with_video_input";

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct VideoPricingState {
    pub(crate) mode: VideoBillingMode,
    /// Whether per-second pricing is both selected and actually configured.
    pub(crate) per_second_enabled: bool,
    /// Whether resolution-keyed token pricing is selected and configured.
    ///
    /// Token rates for video also vary by rendered resolution and by whether a
    /// reference video was supplied, so `per_token` gets its own price tables
    /// rather than falling back to the model-wide token tier catalog.
    pub(crate) per_token_by_resolution_enabled: bool,
}

/// Whether `video_pricing` configures a price under `table_key` or `default_key`,
/// for either input kind.
fn has_video_pricing_configured(
    video_pricing: Option<&Value>,
    table_key: &str,
    default_key: &str,
) -> bool {
    let Some(video_pricing) = video_pricing else {
        return false;
    };
    let section_is_priced = |section: Option<&Value>| {
        let Some(section) = section else {
            return false;
        };
        let table_is_populated = section
            .get(table_key)
            .and_then(Value::as_object)
            .is_some_and(|table| !table.is_empty());
        table_is_populated || section.get(default_key).is_some()
    };
    section_is_priced(Some(video_pricing))
        || section_is_priced(video_pricing.get(VIDEO_WITH_INPUT_KEY))
}

pub(crate) fn video_pricing_state(video_pricing: Option<&Value>) -> VideoPricingState {
    let mode = VideoBillingMode::from_config(video_pricing);
    VideoPricingState {
        mode,
        per_second_enabled: mode == VideoBillingMode::PerSecond
            && has_video_pricing_configured(
                video_pricing,
                VIDEO_PER_SECOND_TABLE_KEY,
                VIDEO_PER_SECOND_DEFAULT_KEY,
            ),
        per_token_by_resolution_enabled: mode == VideoBillingMode::PerToken
            && has_video_pricing_configured(
                video_pricing,
                VIDEO_TOKEN_TABLE_KEY,
                VIDEO_TOKEN_DEFAULT_KEY,
            ),
    }
}

/// Resolves one price, preferring the `with_video_input` override section and
/// falling back to the base section.
///
/// Each section is searched in full — the resolution row first, then that
/// section's default price — before moving on. So a `with_video_input` block
/// that configures only a default still covers every resolution, while a model
/// with no override at all keeps using its base prices. A resolution the
/// operator never listed is charged the default rather than going unpriced,
/// matching how image output pricing behaves; with no default configured it
/// still resolves to nothing rather than inventing a price.
fn resolve_video_price<T, F>(
    video_pricing: Option<&Value>,
    table_key: &str,
    default_key: &str,
    resolution: Option<&str>,
    has_video_input: bool,
    read_entry: F,
) -> Option<T>
where
    F: Fn(&Value) -> Option<T>,
{
    let video_pricing = video_pricing?;
    let key = normalize_video_resolution_key(resolution.unwrap_or_default());

    let resolve_section = |section: Option<&Value>| -> Option<T> {
        let section = section?;
        let row = (!key.is_empty())
            .then(|| section.get(table_key))
            .flatten()
            .and_then(Value::as_object)
            .and_then(|table| {
                table
                    .iter()
                    .find(|(candidate, _)| normalize_video_resolution_key(candidate) == key)
            })
            .and_then(|(_, entry)| read_entry(entry));
        row.or_else(|| section.get(default_key).and_then(&read_entry))
    };

    if has_video_input {
        if let Some(resolved) = resolve_section(video_pricing.get(VIDEO_WITH_INPUT_KEY)) {
            return Some(resolved);
        }
    }
    resolve_section(Some(video_pricing))
}

/// Resolves the per-second price for one request.
///
/// Requests carrying a reference video read `with_video_input` first, falling
/// back to the base section so a partially configured model still prices.
pub(crate) fn resolve_video_price_per_second(
    video_pricing: Option<&Value>,
    resolution: Option<&str>,
    has_video_input: bool,
) -> f64 {
    resolve_video_price(
        video_pricing,
        VIDEO_PER_SECOND_TABLE_KEY,
        VIDEO_PER_SECOND_DEFAULT_KEY,
        resolution,
        has_video_input,
        |entry| entry.as_f64().filter(|price| *price > 0.0),
    )
    .unwrap_or(0.0)
}

/// Token rates for one video request, keyed by resolution and input kind.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct VideoTokenPrices {
    pub(crate) input_price_per_1m: f64,
    pub(crate) output_price_per_1m: f64,
}

impl VideoTokenPrices {
    fn from_entry(entry: &Value) -> Option<Self> {
        // A bare number prices output tokens only: video models bill almost
        // entirely on generated tokens, and that is the shorthand operators
        // reach for.
        if let Some(output_price_per_1m) = entry.as_f64() {
            return Some(Self {
                input_price_per_1m: 0.0,
                output_price_per_1m,
            });
        }
        let entry = entry.as_object()?;
        let read = |keys: &[&str]| -> f64 {
            keys.iter()
                .find_map(|key| entry.get(*key).and_then(Value::as_f64))
                .unwrap_or(0.0)
        };
        let prices = Self {
            input_price_per_1m: read(&["input_price_per_1m", "input", "prompt"]),
            output_price_per_1m: read(&["output_price_per_1m", "output", "completion"]),
        };
        (prices.input_price_per_1m > 0.0 || prices.output_price_per_1m > 0.0).then_some(prices)
    }
}

/// Resolves resolution-keyed token rates for one request.
///
/// Mirrors [`resolve_video_price_per_second`]: same table selection, same
/// resolution normalization, so one config shape serves both billing modes.
pub(crate) fn resolve_video_token_prices(
    video_pricing: Option<&Value>,
    resolution: Option<&str>,
    has_video_input: bool,
) -> Option<VideoTokenPrices> {
    resolve_video_price(
        video_pricing,
        VIDEO_TOKEN_TABLE_KEY,
        VIDEO_TOKEN_DEFAULT_KEY,
        resolution,
        has_video_input,
        VideoTokenPrices::from_entry,
    )
}

/// Normalizes a resolution key so config and provider spellings match.
///
/// Must stay aligned with the admin UI's `normalizeResolutionKey`: providers
/// report either a tier (`720p`) or pixels (`1280x720`), and a pixel pair is
/// ordered smallest-first so portrait and landscape share one price entry.
pub(crate) fn normalize_video_resolution_key(value: &str) -> String {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .replace('×', "x")
        .replace(' ', "");
    let Some((width, height)) = normalized.split_once('x') else {
        return normalized;
    };
    match (width.parse::<u64>(), height.parse::<u64>()) {
        (Ok(width), Ok(height)) if width <= height => format!("{width}x{height}"),
        (Ok(width), Ok(height)) => format!("{height}x{width}"),
        _ => normalized,
    }
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

#[cfg(test)]
mod video_pricing_tests {
    use super::*;

    fn video_config() -> Value {
        json!({
            "price_per_second_by_resolution": {
                "480p": 1.0,
                "720p": 2.0,
                "1080p": 3.0,
                "1280x720": 2.5
            },
            "with_video_input": {
                "price_per_second_by_resolution": { "720p": 4.0 }
            }
        })
    }

    #[test]
    fn resolution_keys_normalize_across_provider_spellings() {
        for value in ["720p", "720P", " 720p ", "  720P"] {
            assert_eq!(normalize_video_resolution_key(value), "720p", "{value}");
        }
        // A pixel pair is ordered smallest-first so portrait and landscape
        // share one configured price.
        for value in ["1280x720", "720x1280", "1280X720", "1280×720", "1280 x 720"] {
            assert_eq!(normalize_video_resolution_key(value), "720x1280", "{value}");
        }
    }

    #[test]
    fn tier_and_pixel_spellings_both_resolve_a_price() {
        let config = video_config();
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("720p"), false),
            2.0
        );
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("720P"), false),
            2.0
        );
        // `1280x720` is stored landscape but normalizes to `720x1280`, so both
        // orientations hit the same entry.
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("1280x720"), false),
            2.5
        );
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("720x1280"), false),
            2.5
        );
    }

    #[test]
    fn video_input_selects_the_override_table_and_falls_back() {
        let config = video_config();
        // Configured in `with_video_input`: the override wins.
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("720p"), true),
            4.0
        );
        // Absent from `with_video_input`: falls back to the default table so a
        // partially configured model still prices.
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("1080p"), true),
            3.0
        );
        // Text-to-video never reads the override.
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("720p"), false),
            2.0
        );
    }

    #[test]
    fn unknown_or_missing_resolution_prices_at_zero() {
        let config = video_config();
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("8k"), false),
            0.0
        );
        assert_eq!(
            resolve_video_price_per_second(Some(&config), None, false),
            0.0
        );
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("   "), false),
            0.0
        );
        assert_eq!(
            resolve_video_price_per_second(None, Some("720p"), false),
            0.0
        );
    }

    #[test]
    fn a_configured_default_covers_resolutions_the_table_omits() {
        let mut config = video_config();
        config["price_per_second_default"] = json!(0.5);

        // Listed resolutions keep their own price.
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("720p"), false),
            2.0
        );
        // Unlisted, blank and absent resolutions fall back to the default.
        for resolution in [Some("8k"), Some("   "), None] {
            assert_eq!(
                resolve_video_price_per_second(Some(&config), resolution, false),
                0.5,
                "{resolution:?} should fall back to the default price"
            );
        }
    }

    #[test]
    fn the_video_input_default_covers_the_whole_override_section() {
        let mut config = video_config();
        config["price_per_second_default"] = json!(0.5);
        config["with_video_input"]["price_per_second_default"] = json!(9.0);

        // The override's own row still wins over its default.
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("720p"), true),
            4.0
        );
        // A resolution the override omits takes the override's default rather
        // than dropping back to the base section.
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("1080p"), true),
            9.0
        );
        // Text-to-video is unaffected by the override section.
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("1080p"), false),
            3.0
        );
    }

    #[test]
    fn a_base_default_still_backs_video_input_requests() {
        let mut config = video_config();
        config["price_per_second_default"] = json!(0.5);

        // No default inside `with_video_input`, so the base section answers.
        assert_eq!(
            resolve_video_price_per_second(Some(&config), Some("8k"), true),
            0.5
        );
    }

    #[test]
    fn a_default_alone_is_enough_to_enable_per_second_pricing() {
        let config = json!({ "price_per_second_default": 0.5 });
        let state = video_pricing_state(Some(&config));
        assert_eq!(state.mode, VideoBillingMode::PerSecond);
        assert!(state.per_second_enabled);

        let override_only = json!({
            "with_video_input": { "price_per_second_default": 0.5 }
        });
        assert!(video_pricing_state(Some(&override_only)).per_second_enabled);
    }

    #[test]
    fn a_token_default_alone_is_enough_to_enable_per_token_pricing() {
        let config = json!({
            "mode": "per_token",
            "token_price_default": { "output_price_per_1m": 15.0 }
        });
        let state = video_pricing_state(Some(&config));
        assert_eq!(state.mode, VideoBillingMode::PerToken);
        assert!(state.per_token_by_resolution_enabled);
    }

    #[test]
    fn absent_mode_keeps_existing_per_second_configs_working() {
        let config = video_config();
        let state = video_pricing_state(Some(&config));
        assert_eq!(state.mode, VideoBillingMode::PerSecond);
        assert!(state.per_second_enabled);
    }

    #[test]
    fn per_token_mode_disables_per_second_pricing() {
        let mut config = video_config();
        config["mode"] = json!("per_token");
        let state = video_pricing_state(Some(&config));
        assert_eq!(state.mode, VideoBillingMode::PerToken);
        assert!(
            !state.per_second_enabled,
            "per_token must not charge by rendered seconds"
        );
    }

    #[test]
    fn mode_without_any_price_table_is_not_enabled() {
        let state = video_pricing_state(Some(&json!({ "mode": "per_second" })));
        assert_eq!(state.mode, VideoBillingMode::PerSecond);
        assert!(
            !state.per_second_enabled,
            "an empty price table must not enable per-second billing"
        );
        assert!(!video_pricing_state(None).per_second_enabled);
    }

    #[test]
    fn per_second_mode_zeroes_token_prices_in_the_generated_rule() {
        let pricing = BillingPricingResolution {
            tiered_pricing: Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 3.0,
                    "output_price_per_1m": 15.0
                }]
            })),
            video_pricing: Some(video_config()),
            ..Default::default()
        };
        let rule = DefaultBillingRuleGenerator::generate_for_pricing("seedance", &pricing, "video")
            .expect("per-second video pricing must generate a rule");

        // Enforced at rule level so a stale token price in the config cannot
        // double-charge alongside the per-second price.
        assert_eq!(rule.variables.get("input_price_per_1m"), Some(&json!(0.0)));
        assert_eq!(rule.variables.get("output_price_per_1m"), Some(&json!(0.0)));
        assert!(rule.expression.contains("video_cost"));
        assert_eq!(
            rule.dimension_mappings
                .get("video_cost")
                .and_then(|value| value.get("expression"))
                .and_then(Value::as_str),
            Some("video_seconds_unmetered * video_price_per_second")
        );
    }

    #[test]
    fn per_token_mode_keeps_token_prices_in_the_generated_rule() {
        let mut video = video_config();
        video["mode"] = json!("per_token");
        let pricing = BillingPricingResolution {
            tiered_pricing: Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 3.0,
                    "output_price_per_1m": 15.0
                }]
            })),
            video_pricing: Some(video),
            ..Default::default()
        };
        let rule = DefaultBillingRuleGenerator::generate_for_pricing("seedance", &pricing, "video")
            .expect("token pricing must still generate a rule");

        assert_eq!(rule.variables.get("input_price_per_1m"), Some(&json!(3.0)));
        assert_eq!(
            rule.variables.get("output_price_per_1m"),
            Some(&json!(15.0))
        );
    }

    #[test]
    fn per_second_pricing_alone_is_enough_to_generate_a_rule() {
        let pricing = BillingPricingResolution {
            video_pricing: Some(video_config()),
            ..Default::default()
        };
        assert!(
            DefaultBillingRuleGenerator::generate_for_pricing("seedance", &pricing, "video")
                .is_some(),
            "a model priced only by rendered seconds must still bill"
        );
    }

    #[test]
    fn no_pricing_at_all_generates_no_rule() {
        let pricing = BillingPricingResolution::default();
        assert!(
            DefaultBillingRuleGenerator::generate_for_pricing("seedance", &pricing, "video")
                .is_none()
        );
    }
}
