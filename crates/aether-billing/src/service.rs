use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::default_rule::{
    explicit_image_output_price_default, explicit_image_output_price_entries,
    explicit_image_output_price_ranges, normalize_task_type, DefaultBillingRuleGenerator,
};
use crate::precision::quantize_cost;
use crate::pricing::{
    BillingAuthorizationEstimateInput, BillingComputation, BillingModelPricingSnapshot,
    BillingPricingResolution, BillingUsageInput,
};
use crate::schema::{
    BillingSnapshot, BillingSnapshotStatus, CostResult, BILLING_SNAPSHOT_SCHEMA_VERSION,
};
use crate::{
    normalize_input_tokens_for_billing, normalize_total_input_context_for_cache_hit_rate,
    ExpressionEvaluationError, FormulaEngine, FormulaEvaluationStatus,
};

pub struct BillingService {
    engine: FormulaEngine,
}

impl BillingService {
    pub fn new() -> Self {
        Self {
            engine: FormulaEngine::new(),
        }
    }

    pub fn calculate(
        &self,
        pricing: &BillingModelPricingSnapshot,
        input: &BillingUsageInput,
    ) -> Result<BillingComputation, ExpressionEvaluationError> {
        let pricing_resolution = pricing
            .resolve_pricing_checked(
                input.requested_processing_tier.as_deref(),
                input.actual_processing_tier.as_deref(),
            )
            .map_err(|err| ExpressionEvaluationError::Failed(err.to_string()))?;
        self.calculate_with_resolution(pricing, input, pricing_resolution)
    }

    pub fn estimate_authorization_cost_upper_bound(
        &self,
        pricing: &BillingModelPricingSnapshot,
        estimate: &BillingAuthorizationEstimateInput,
    ) -> Result<Option<f64>, ExpressionEvaluationError> {
        let Some(pricing_resolutions) = pricing
            .resolve_authorization_pricing_candidates(estimate.requested_processing_tier.as_deref())
            .map_err(|err| ExpressionEvaluationError::Failed(err.to_string()))?
        else {
            return Ok(None);
        };
        if normalize_task_type(&estimate.task_type) == "image" {
            return Ok(None);
        }
        if pricing.is_free_tier() {
            return Ok(Some(0.0));
        }
        if estimate.max_output_tokens.is_none()
            && pricing_resolutions.iter().any(|resolution| {
                resolution
                    .tiered_pricing
                    .as_ref()
                    .is_some_and(pricing_has_positive_output_rate)
            })
        {
            return Ok(None);
        }

        let input_tokens = estimate.input_tokens.max(0);
        let output_tokens = estimate.max_output_tokens.unwrap_or(0).max(0);
        let base_input = BillingUsageInput {
            task_type: estimate.task_type.clone(),
            api_format: estimate.api_format.clone(),
            requested_processing_tier: estimate.requested_processing_tier.clone(),
            actual_processing_tier: None,
            input_tokens,
            output_tokens,
            cache_ttl_minutes: estimate
                .cache_ttl_minutes
                .or(pricing.provider_api_key_cache_ttl_minutes),
            ..BillingUsageInput::new(estimate.task_type.clone())
        };
        let mut scenarios = vec![base_input.clone()];
        if input_tokens > 0
            && pricing_resolutions
                .iter()
                .any(|resolution| resolution.tiered_pricing.is_some())
        {
            let mut cache_creation = base_input.clone();
            cache_creation.cache_creation_tokens = input_tokens;
            scenarios.push(cache_creation);

            if estimate.cache_ttl_minutes.is_none() {
                let mut cache_creation_5m = base_input.clone();
                cache_creation_5m.cache_creation_tokens = input_tokens;
                cache_creation_5m.cache_creation_ephemeral_5m_tokens = input_tokens;
                cache_creation_5m.cache_ttl_minutes = Some(5);
                scenarios.push(cache_creation_5m);

                let mut cache_creation_1h = base_input.clone();
                cache_creation_1h.cache_creation_tokens = input_tokens;
                cache_creation_1h.cache_creation_ephemeral_1h_tokens = input_tokens;
                cache_creation_1h.cache_ttl_minutes = Some(60);
                scenarios.push(cache_creation_1h);
            }

            let mut cache_read = base_input;
            cache_read.cache_read_tokens = input_tokens;
            scenarios.push(cache_read);
        }

        let mut upper_bound = 0.0_f64;
        'pricing_catalogs: for pricing_resolution in pricing_resolutions {
            let is_requested_catalog = pricing_resolution.bills_requested_processing_tier();
            for scenario in &scenarios {
                let total_input_context = normalize_total_input_context_for_cache_hit_rate(
                    scenario.api_format.as_deref(),
                    scenario.input_tokens,
                    scenario.cache_creation_tokens,
                    scenario.cache_read_tokens,
                );
                let Some(pricing_candidates) =
                    authorization_pricing_candidates(&pricing_resolution, total_input_context)
                else {
                    return Ok(None);
                };

                // Validate the selected catalog and its finite coverage using the same path as
                // settlement before evaluating every reachable tier as an upper-bound candidate.
                let selected =
                    self.calculate_with_resolution(pricing, scenario, pricing_resolution.clone())?;
                if !billing_computation_is_bounded(&selected) {
                    if !is_requested_catalog
                        && billing_computation_is_outside_catalog_context(&selected)
                    {
                        continue 'pricing_catalogs;
                    }
                    return Ok(None);
                }

                for candidate in pricing_candidates {
                    let computation =
                        self.calculate_with_resolution(pricing, scenario, candidate)?;
                    if !billing_computation_is_bounded(&computation) {
                        return Ok(None);
                    }
                    upper_bound = upper_bound.max(computation.actual_total_cost);
                }
            }
        }
        Ok(Some(upper_bound))
    }

    fn calculate_with_resolution(
        &self,
        pricing: &BillingModelPricingSnapshot,
        input: &BillingUsageInput,
        pricing_resolution: BillingPricingResolution,
    ) -> Result<BillingComputation, ExpressionEvaluationError> {
        if pricing_resolution.requires_actual_processing_tier() {
            return Ok(no_rule_computation(
                pricing,
                input,
                pricing_resolution,
                "actual_processing_tier",
            ));
        }
        if !pricing_resolution.bills_standard_processing_tier()
            && pricing_resolution.tiered_pricing.is_none()
            && pricing_resolution.price_per_request.is_none()
        {
            return Ok(no_rule_computation(
                pricing,
                input,
                pricing_resolution,
                "processing_tier_catalog",
            ));
        }

        let total_input_context = normalize_total_input_context_for_cache_hit_rate(
            input.api_format.as_deref(),
            input.input_tokens,
            input.cache_creation_tokens,
            input.cache_read_tokens,
        );
        let has_token_usage = input.input_tokens > 0
            || input.output_tokens > 0
            || input.cache_creation_tokens > 0
            || input.cache_read_tokens > 0;
        if has_token_usage {
            if let Some(pricing_config) = pricing_resolution.tiered_pricing.as_ref() {
                let tiers = pricing_config
                    .get("tiers")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                if tiers.is_empty() && pricing_resolution.price_per_request.is_none() {
                    return Ok(no_rule_computation(
                        pricing,
                        input,
                        pricing_resolution,
                        "token_pricing",
                    ));
                }
                if !tiers.is_empty()
                    && !pricing_covers_input_context(pricing_config, total_input_context)
                {
                    return Ok(no_rule_computation(
                        pricing,
                        input,
                        pricing_resolution,
                        "input_context_tier",
                    ));
                }
            } else if pricing_resolution.price_per_request.is_none() {
                return Ok(no_rule_computation(
                    pricing,
                    input,
                    pricing_resolution,
                    "token_pricing",
                ));
            }
        }

        let Some(rule) = DefaultBillingRuleGenerator::generate_for_pricing(
            &pricing.global_model_name,
            &pricing_resolution,
            &input.task_type,
        ) else {
            return Ok(no_rule_computation(
                pricing,
                input,
                pricing_resolution,
                "pricing_rule",
            ));
        };

        let dims = build_dimensions(input, &pricing_resolution);
        let result = self.engine.evaluate(
            &rule.expression,
            Some(&rule.variables),
            Some(&dims),
            Some(&rule.dimension_mappings),
            false,
        )?;

        let status = match result.status {
            FormulaEvaluationStatus::Complete => BillingSnapshotStatus::Complete,
            FormulaEvaluationStatus::Incomplete => BillingSnapshotStatus::Incomplete,
        };
        let total_cost = if matches!(status, BillingSnapshotStatus::Complete) {
            result.cost
        } else {
            0.0
        };
        let rate_multiplier = pricing.rate_multiplier_for_api_format(input.api_format.as_deref());
        let is_free_tier = pricing.is_free_tier();
        let actual_total_cost = if is_free_tier {
            0.0
        } else {
            quantize_cost(total_cost * rate_multiplier)
        };

        Ok(BillingComputation {
            cost_result: CostResult {
                cost: total_cost,
                status,
                snapshot: BillingSnapshot {
                    schema_version: BILLING_SNAPSHOT_SCHEMA_VERSION.to_string(),
                    rule_id: Some(rule.id),
                    rule_name: Some(rule.name),
                    scope: Some(rule.scope),
                    expression: Some(rule.expression),
                    resolved_dimensions: result.resolved_dimensions,
                    resolved_variables: result.resolved_variables,
                    cost_breakdown: result.cost_breakdown,
                    total_cost,
                    tier_index: result.tier_index,
                    tier_info: result.tier_info,
                    missing_required: result.missing_required,
                    status,
                    calculated_at: now_marker(),
                    engine_version: "2.0".to_string(),
                },
            },
            actual_total_cost,
            rate_multiplier,
            is_free_tier,
            pricing_resolution,
        })
    }
}

fn pricing_has_positive_output_rate(pricing: &Value) -> bool {
    pricing
        .get("tiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tier| tier.get("output_price_per_1m").and_then(Value::as_f64))
        .any(|price| price.is_finite() && price > 0.0)
}

fn billing_computation_is_bounded(computation: &BillingComputation) -> bool {
    computation.cost_result.status == BillingSnapshotStatus::Complete
        && computation.actual_total_cost.is_finite()
        && computation.actual_total_cost >= 0.0
}

fn billing_computation_is_outside_catalog_context(computation: &BillingComputation) -> bool {
    computation.cost_result.status == BillingSnapshotStatus::NoRule
        && computation.cost_result.snapshot.missing_required == ["input_context_tier"]
}

fn authorization_pricing_candidates(
    pricing: &BillingPricingResolution,
    max_input_context: i64,
) -> Option<Vec<BillingPricingResolution>> {
    let Some(config) = pricing.tiered_pricing.as_ref() else {
        return Some(vec![pricing.clone()]);
    };
    let Some(tiers) = config.get("tiers").and_then(Value::as_array) else {
        return Some(vec![pricing.clone()]);
    };
    if tiers.is_empty() {
        return Some(vec![pricing.clone()]);
    }

    let max_input_context = max_input_context.max(0);
    let mut previous_up_to: Option<i64> = None;
    let mut candidates = Vec::new();
    for tier in tiers {
        let tier_object = tier.as_object()?;
        let up_to = match tier_object.get("up_to") {
            None | Some(Value::Null) => None,
            Some(value) => Some(nonnegative_i64(value)?),
        };
        if let (Some(previous), Some(current)) = (previous_up_to, up_to) {
            if current < previous {
                return None;
            }
        }

        let lower_bound = previous_up_to.map_or(0, |value| value.saturating_add(1));
        if lower_bound <= max_input_context {
            let mut candidate_config = config.clone();
            let candidate_object = candidate_config.as_object_mut()?;
            let mut candidate_tier = tier.clone();
            candidate_tier
                .as_object_mut()?
                .insert("up_to".to_string(), Value::Null);
            candidate_object.insert("tiers".to_string(), Value::Array(vec![candidate_tier]));

            let mut candidate = pricing.clone();
            candidate.tiered_pricing = Some(candidate_config);
            candidates.push(candidate);
        }

        match up_to {
            Some(up_to) if max_input_context > up_to => previous_up_to = Some(up_to),
            _ => break,
        }
    }

    (!candidates.is_empty()).then_some(candidates)
}

fn nonnegative_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .filter(|value| *value >= 0)
}

fn no_rule_computation(
    pricing: &BillingModelPricingSnapshot,
    input: &BillingUsageInput,
    pricing_resolution: BillingPricingResolution,
    missing_required: &str,
) -> BillingComputation {
    let resolved_dimensions = build_dimensions(input, &pricing_resolution);
    BillingComputation {
        cost_result: CostResult {
            cost: 0.0,
            status: BillingSnapshotStatus::NoRule,
            snapshot: BillingSnapshot {
                schema_version: BILLING_SNAPSHOT_SCHEMA_VERSION.to_string(),
                rule_id: None,
                rule_name: None,
                scope: None,
                expression: None,
                resolved_dimensions,
                resolved_variables: BTreeMap::new(),
                cost_breakdown: BTreeMap::new(),
                total_cost: 0.0,
                tier_index: None,
                tier_info: None,
                missing_required: vec![missing_required.to_string()],
                status: BillingSnapshotStatus::NoRule,
                calculated_at: now_marker(),
                engine_version: "2.0".to_string(),
            },
        },
        actual_total_cost: 0.0,
        rate_multiplier: pricing.rate_multiplier_for_api_format(input.api_format.as_deref()),
        is_free_tier: pricing.is_free_tier(),
        pricing_resolution,
    }
}

fn pricing_covers_input_context(pricing: &Value, total_input_context: i64) -> bool {
    let Some(tiers) = pricing.get("tiers").and_then(Value::as_array) else {
        return true;
    };
    let Some(last_tier) = tiers.last() else {
        return true;
    };
    match last_tier.get("up_to") {
        None | Some(Value::Null) => true,
        Some(value) => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .is_some_and(|up_to| total_input_context <= up_to),
    }
}

impl Default for BillingService {
    fn default() -> Self {
        Self::new()
    }
}

fn build_dimensions(
    input: &BillingUsageInput,
    pricing: &BillingPricingResolution,
) -> BTreeMap<String, Value> {
    let normalized_input_tokens = normalize_input_tokens_for_billing(
        input.api_format.as_deref(),
        input.input_tokens,
        input.cache_creation_tokens,
        input.cache_read_tokens,
    );
    let classified_cache_creation_tokens = input
        .cache_creation_ephemeral_5m_tokens
        .saturating_add(input.cache_creation_ephemeral_1h_tokens);
    let cache_creation_uncategorized_tokens = input
        .cache_creation_tokens
        .saturating_sub(classified_cache_creation_tokens)
        .max(0);
    let total_input_context = normalize_total_input_context_for_cache_hit_rate(
        input.api_format.as_deref(),
        input.input_tokens,
        input.cache_creation_tokens,
        input.cache_read_tokens,
    );
    let pricing_config = pricing.tiered_pricing.as_ref();
    let image_output_pricing = image_output_pricing_state(pricing_config);
    let image_output_resolution = resolve_image_output_price_resolution(pricing_config, input);

    let mut out = BTreeMap::from([
        ("input_tokens".to_string(), json!(normalized_input_tokens)),
        ("output_tokens".to_string(), json!(input.output_tokens)),
        (
            "cache_creation_tokens".to_string(),
            json!(input.cache_creation_tokens),
        ),
        (
            "cache_creation_ephemeral_5m_tokens".to_string(),
            json!(input.cache_creation_ephemeral_5m_tokens),
        ),
        (
            "cache_creation_ephemeral_1h_tokens".to_string(),
            json!(input.cache_creation_ephemeral_1h_tokens),
        ),
        (
            "cache_creation_uncategorized_tokens".to_string(),
            json!(cache_creation_uncategorized_tokens),
        ),
        (
            "cache_read_tokens".to_string(),
            json!(input.cache_read_tokens),
        ),
        (
            "request_count".to_string(),
            json!(input.request_count.max(0)),
        ),
        ("image_count".to_string(), json!(input.image_count.max(0))),
        (
            "image_count_unmetered".to_string(),
            json!(if image_output_pricing.enabled {
                input.image_count.max(0)
            } else {
                0
            }),
        ),
        (
            "image_output_pricing_enabled".to_string(),
            json!(image_output_pricing.enabled),
        ),
        (
            "image_output_matrix_enabled".to_string(),
            json!(image_output_pricing.matrix_enabled),
        ),
        (
            "image_output_range_enabled".to_string(),
            json!(image_output_pricing.range_enabled),
        ),
        (
            "image_output_pricing_mode".to_string(),
            json!(image_output_resolution.pricing_mode),
        ),
        (
            "image_output_price_per_image".to_string(),
            json!(image_output_resolution.price_per_image),
        ),
        (
            "total_input_context".to_string(),
            json!(total_input_context),
        ),
        (
            "effective_task_type".to_string(),
            json!(normalize_task_type(&input.task_type)),
        ),
        (
            "requested_processing_tier".to_string(),
            json!(pricing.requested_processing_tier),
        ),
        (
            "actual_processing_tier".to_string(),
            json!(pricing.actual_processing_tier),
        ),
        (
            "billing_processing_tier".to_string(),
            json!(pricing.billing_processing_tier),
        ),
    ]);

    out.insert(
        "cache_creation_ephemeral_5m_ttl_minutes".to_string(),
        json!(5),
    );
    out.insert(
        "cache_creation_ephemeral_1h_ttl_minutes".to_string(),
        json!(60),
    );

    if let Some(cache_ttl_minutes) = input.cache_ttl_minutes {
        out.insert(
            "cache_ttl_minutes".to_string(),
            json!(cache_ttl_minutes.max(0)),
        );
    }
    if let Some(image_pixels) = image_output_resolution.image_pixels {
        out.insert("image_pixels".to_string(), json!(image_pixels));
    }
    if let Some(price_bucket) = image_output_resolution.price_bucket.as_ref() {
        out.insert("image_output_price_bucket".to_string(), json!(price_bucket));
    }
    if input.image_count > 0 {
        let image_size = input
            .image_size
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let image_quality = input
            .image_quality
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        if let Some(image_size) = image_size.as_ref() {
            out.insert("image_size".to_string(), json!(image_size));
        }
        if let Some(image_quality) = image_quality.as_ref() {
            out.insert("image_quality".to_string(), json!(image_quality));
        }
        if let (Some(image_size), Some(image_quality)) =
            (image_size.as_ref(), image_quality.as_ref())
        {
            out.insert(
                "image_price_key".to_string(),
                json!(format!(
                    "{}:{}",
                    normalize_image_output_size(image_size),
                    normalize_image_output_quality(image_quality)
                )),
            );
        }
    }
    if let Some(output_format) = input
        .image_output_format
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        out.insert("image_output_format".to_string(), json!(output_format));
    }
    out
}

#[derive(Debug, Clone, Copy)]
struct ImageOutputPricingState {
    enabled: bool,
    matrix_enabled: bool,
    range_enabled: bool,
}

#[derive(Debug, Clone)]
struct ImageOutputPriceResolution {
    price_per_image: f64,
    pricing_mode: &'static str,
    price_bucket: Option<String>,
    image_pixels: Option<i64>,
}

#[derive(Debug, Clone)]
struct ParsedImageOutputPriceRange {
    up_to_pixels: Option<i64>,
    label: Option<String>,
    prices: BTreeMap<String, f64>,
}

fn image_output_pricing_state(pricing: Option<&Value>) -> ImageOutputPricingState {
    let matrix_enabled = pricing_has_image_output_matrix(pricing);
    let range_enabled = pricing_has_image_output_ranges(pricing);
    let default_enabled = pricing_has_image_output_default_price(pricing);
    ImageOutputPricingState {
        enabled: matrix_enabled || range_enabled || default_enabled,
        matrix_enabled,
        range_enabled,
    }
}

fn resolve_image_output_price_resolution(
    pricing: Option<&Value>,
    input: &BillingUsageInput,
) -> ImageOutputPriceResolution {
    let default_price = explicit_image_output_price_default(pricing);
    let image_size = input
        .image_size
        .as_deref()
        .map(normalize_image_output_size)
        .filter(|value| !value.is_empty());
    let image_quality = input
        .image_quality
        .as_deref()
        .map(normalize_image_output_quality)
        .filter(|value| !value.is_empty());
    let image_pixels = image_size.as_deref().and_then(parse_image_size_pixels);

    if let (Some(size), Some(entries)) = (
        image_size.as_deref(),
        explicit_image_output_price_entries(pricing),
    ) {
        for key in image_price_lookup_keys(size, image_quality.as_deref()) {
            if let Some(price) = entries.get(&key).and_then(Value::as_f64) {
                return ImageOutputPriceResolution {
                    price_per_image: price,
                    pricing_mode: "matrix",
                    price_bucket: None,
                    image_pixels,
                };
            }
        }
    }

    if let Some(pixels) = image_pixels {
        if let Some((price, bucket)) = resolve_image_output_range_price(
            explicit_image_output_price_ranges(pricing).unwrap_or_default(),
            pixels,
            image_quality.as_deref(),
            default_price,
        ) {
            return ImageOutputPriceResolution {
                price_per_image: price,
                pricing_mode: "pixel_tiers",
                price_bucket: Some(bucket),
                image_pixels,
            };
        }
    }

    if let Some(price) = default_price {
        return ImageOutputPriceResolution {
            price_per_image: price,
            pricing_mode: "per_image",
            price_bucket: Some("default".to_string()),
            image_pixels,
        };
    }

    ImageOutputPriceResolution {
        price_per_image: 0.0,
        pricing_mode: "none",
        price_bucket: None,
        image_pixels,
    }
}

fn pricing_has_image_output_matrix(pricing: Option<&Value>) -> bool {
    let Some(config) = pricing else {
        return false;
    };
    [
        "image_output_prices",
        "image_output_price_per_image",
        "image_output_price_matrix",
        "image_prices",
    ]
    .iter()
    .any(|key| {
        config
            .get(key)
            .is_some_and(image_price_entries_have_matrix_values)
    })
}

fn pricing_has_image_output_ranges(pricing: Option<&Value>) -> bool {
    explicit_image_output_price_ranges(pricing).is_some_and(|ranges| !ranges.is_empty())
}

fn pricing_has_image_output_default_price(pricing: Option<&Value>) -> bool {
    let Some(config) = pricing else {
        return false;
    };
    config
        .get("image_output_price_default")
        .or_else(|| config.get("image_price_default"))
        .or_else(|| {
            config
                .get("image_output_prices")
                .and_then(|value| value.get("default"))
        })
        .and_then(Value::as_f64)
        .is_some()
}

fn image_price_entries_have_matrix_values(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            !key.eq_ignore_ascii_case("default")
                && (value.as_f64().is_some() || image_price_entries_have_matrix_values(value))
        }),
        Value::Array(items) => items.iter().any(image_price_entries_have_matrix_values),
        _ => false,
    }
}

fn resolve_image_output_range_price(
    ranges: Vec<Value>,
    image_pixels: i64,
    image_quality: Option<&str>,
    default_price: Option<f64>,
) -> Option<(f64, String)> {
    let mut parsed_ranges = ranges
        .iter()
        .filter_map(parse_image_output_price_range)
        .collect::<Vec<_>>();
    parsed_ranges.sort_by(
        |left, right| match (left.up_to_pixels, right.up_to_pixels) {
            (Some(left), Some(right)) => left.cmp(&right),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        },
    );

    for range in parsed_ranges {
        if !range
            .up_to_pixels
            .map(|up_to| image_pixels <= up_to)
            .unwrap_or(true)
        {
            continue;
        }
        let Some(price) =
            image_output_price_for_quality(&range.prices, image_quality).or(default_price)
        else {
            continue;
        };
        return Some((price, image_output_range_bucket(&range)));
    }

    None
}

fn parse_image_output_price_range(value: &Value) -> Option<ParsedImageOutputPriceRange> {
    let object = value.as_object()?;
    let prices = object
        .get("prices")
        .and_then(Value::as_object)?
        .iter()
        .filter_map(|(key, value)| {
            value
                .as_f64()
                .map(|price| (key.to_ascii_lowercase(), price))
        })
        .collect::<BTreeMap<_, _>>();
    if prices.is_empty() {
        return None;
    }
    Some(ParsedImageOutputPriceRange {
        up_to_pixels: object.get("up_to_pixels").and_then(value_as_positive_i64),
        label: object
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        prices,
    })
}

fn image_output_price_for_quality(
    prices: &BTreeMap<String, f64>,
    image_quality: Option<&str>,
) -> Option<f64> {
    for key in image_quality_lookup_keys(image_quality) {
        if let Some(price) = prices.get(&key) {
            return Some(*price);
        }
    }
    None
}

fn image_price_lookup_keys(size: &str, image_quality: Option<&str>) -> Vec<String> {
    image_quality_lookup_keys(image_quality)
        .into_iter()
        .filter(|quality| quality != "default")
        .map(|quality| format!("{}:{}", size, quality))
        .collect()
}

fn image_quality_lookup_keys(image_quality: Option<&str>) -> Vec<String> {
    let quality = image_quality
        .map(normalize_image_output_quality)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "medium".to_string());
    let mut keys = vec![quality.clone()];
    if quality == "auto" {
        keys.push("medium".to_string());
    }
    keys.push("default".to_string());
    keys
}

fn image_output_range_bucket(range: &ParsedImageOutputPriceRange) -> String {
    range
        .label
        .clone()
        .unwrap_or_else(|| match range.up_to_pixels {
            Some(up_to_pixels) => format!("<={up_to_pixels}px"),
            None => "unbounded".to_string(),
        })
}

fn normalize_image_output_size(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace('×', "x")
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

fn normalize_image_output_quality(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn parse_image_size_pixels(size: &str) -> Option<i64> {
    let (width, height) = size.split_once('x')?;
    let width = width.parse::<i64>().ok()?;
    let height = height.parse::<i64>().ok()?;
    if width <= 0 || height <= 0 {
        return None;
    }
    width.checked_mul(height)
}

fn value_as_positive_i64(value: &Value) -> Option<i64> {
    let parsed = value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_f64().map(|value| value as i64))
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))?;
    (parsed > 0).then_some(parsed)
}

fn now_marker() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::BillingService;
    use crate::{
        BillingAuthorizationEstimateInput, BillingModelPricingSnapshot, BillingPricingSource,
        BillingSnapshotStatus, BillingUsageInput,
    };

    fn pricing() -> BillingModelPricingSnapshot {
        BillingModelPricingSnapshot {
            provider_id: "provider-1".to_string(),
            provider_billing_type: Some("pay_as_you_go".to_string()),
            provider_api_key_id: Some("key-1".to_string()),
            provider_api_key_rate_multipliers: Some(json!({"openai:chat": 0.5})),
            provider_api_key_cache_ttl_minutes: Some(60),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_config: None,
            default_price_per_request: Some(0.02),
            default_tiered_pricing: Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 3.0,
                    "output_price_per_1m": 15.0,
                    "cache_creation_price_per_1m": 3.75,
                    "cache_read_price_per_1m": 0.30
                }]
            })),
            model_id: Some("model-1".to_string()),
            model_provider_model_name: Some("gpt-5-upstream".to_string()),
            model_config: None,
            model_price_per_request: None,
            model_tiered_pricing: None,
        }
    }

    fn processing_pricing() -> BillingModelPricingSnapshot {
        BillingModelPricingSnapshot {
            provider_api_key_rate_multipliers: None,
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [
                    {"up_to": 272000, "input_price_per_1m": 5.0, "cache_creation_price_per_1m": 6.25, "cache_read_price_per_1m": 0.5, "output_price_per_1m": 30.0},
                    {"up_to": null, "input_price_per_1m": 10.0, "cache_creation_price_per_1m": 12.5, "cache_read_price_per_1m": 1.0, "output_price_per_1m": 45.0}
                ],
                "processing_tiers": {
                    "flex": {"tiers": [
                        {"up_to": 272000, "input_price_per_1m": 2.5, "cache_creation_price_per_1m": 3.125, "cache_read_price_per_1m": 0.25, "output_price_per_1m": 15.0},
                        {"up_to": null, "input_price_per_1m": 5.0, "cache_creation_price_per_1m": 6.25, "cache_read_price_per_1m": 0.5, "output_price_per_1m": 22.5}
                    ]}
                }
            })),
            model_tiered_pricing: Some(json!({
                "processing_tiers": {
                    "priority": {"tiers": [
                        {"up_to": 272000, "input_price_per_1m": 10.0, "cache_creation_price_per_1m": 12.5, "cache_read_price_per_1m": 1.0, "output_price_per_1m": 60.0}
                    ]}
                }
            })),
            ..pricing()
        }
    }

    fn processing_usage(
        requested: Option<&str>,
        actual: Option<&str>,
        input_tokens: i64,
    ) -> BillingUsageInput {
        BillingUsageInput {
            api_format: Some("openai:responses".to_string()),
            requested_processing_tier: requested.map(ToOwned::to_owned),
            actual_processing_tier: actual.map(ToOwned::to_owned),
            input_tokens,
            cache_creation_tokens: 10,
            cache_ttl_minutes: Some(30),
            ..BillingUsageInput::new("chat")
        }
    }

    #[test]
    fn calculates_complete_snapshot_for_usage() {
        let result = BillingService::new()
            .calculate(
                &pricing(),
                &BillingUsageInput {
                    task_type: "chat".to_string(),
                    api_format: Some("openai:chat".to_string()),
                    requested_processing_tier: None,
                    actual_processing_tier: None,
                    request_count: 1,
                    input_tokens: 1_000,
                    output_tokens: 500,
                    cache_creation_tokens: 0,
                    cache_creation_ephemeral_5m_tokens: 0,
                    cache_creation_ephemeral_1h_tokens: 0,
                    cache_read_tokens: 100,
                    image_count: 0,
                    image_size: None,
                    image_quality: None,
                    image_output_format: None,
                    cache_ttl_minutes: Some(60),
                },
            )
            .expect("billing should calculate");

        assert_eq!(result.cost_result.status, BillingSnapshotStatus::Complete);
        assert!(result.cost_result.cost > 0.0);
        assert!(result.actual_total_cost > 0.0);
        assert_eq!(result.rate_multiplier, 0.5);
    }

    #[test]
    fn openai_cache_hit_context_does_not_double_count_cache_read() {
        let result = BillingService::new()
            .calculate(
                &pricing(),
                &BillingUsageInput {
                    task_type: "chat".to_string(),
                    api_format: Some("openai:responses".to_string()),
                    requested_processing_tier: None,
                    actual_processing_tier: None,
                    request_count: 1,
                    input_tokens: 1_000,
                    output_tokens: 10,
                    cache_creation_tokens: 0,
                    cache_creation_ephemeral_5m_tokens: 0,
                    cache_creation_ephemeral_1h_tokens: 0,
                    cache_read_tokens: 800,
                    image_count: 0,
                    image_size: None,
                    image_quality: None,
                    image_output_format: None,
                    cache_ttl_minutes: Some(60),
                },
            )
            .expect("billing should calculate");

        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_dimensions
                .get("input_tokens"),
            Some(&json!(200))
        );
        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_dimensions
                .get("total_input_context"),
            Some(&json!(1_000))
        );
    }

    #[test]
    fn openai_cache_write_and_read_are_billed_separately() {
        let result = BillingService::new()
            .calculate(
                &pricing(),
                &BillingUsageInput {
                    task_type: "chat".to_string(),
                    api_format: Some("openai:responses".to_string()),
                    requested_processing_tier: None,
                    actual_processing_tier: None,
                    request_count: 1,
                    input_tokens: 1_000,
                    output_tokens: 10,
                    cache_creation_tokens: 100,
                    cache_creation_ephemeral_5m_tokens: 0,
                    cache_creation_ephemeral_1h_tokens: 0,
                    cache_read_tokens: 800,
                    image_count: 0,
                    image_size: None,
                    image_quality: None,
                    image_output_format: None,
                    cache_ttl_minutes: Some(60),
                },
            )
            .expect("billing should calculate");

        let dimensions = &result.cost_result.snapshot.resolved_dimensions;
        assert_eq!(dimensions.get("input_tokens"), Some(&json!(100)));
        assert_eq!(dimensions.get("cache_creation_tokens"), Some(&json!(100)));
        assert_eq!(dimensions.get("cache_read_tokens"), Some(&json!(800)));
        assert_eq!(dimensions.get("total_input_context"), Some(&json!(1_000)));

        let costs = &result.cost_result.snapshot.cost_breakdown;
        assert!(costs.get("input_cost").copied().unwrap_or_default() > 0.0);
        assert!(
            costs
                .get("cache_creation_uncategorized_cost")
                .copied()
                .unwrap_or_default()
                > 0.0
        );
        assert!(costs.get("cache_read_cost").copied().unwrap_or_default() > 0.0);
    }

    #[test]
    fn nonstandard_request_without_actual_tier_uses_requested_catalog() {
        let result = BillingService::new()
            .calculate(
                &processing_pricing(),
                &processing_usage(Some("priority"), None, 100),
            )
            .expect("billing should calculate");

        assert_eq!(result.cost_result.status, BillingSnapshotStatus::Complete);
        assert_eq!(
            result.cost_result.snapshot.resolved_dimensions["billing_processing_tier"],
            json!("priority")
        );
        assert_eq!(
            result.cost_result.snapshot.resolved_variables["input_price_per_1m"],
            json!(10.0)
        );
    }

    #[test]
    fn openai_fast_request_without_overlay_uses_global_standard_catalog() {
        let pricing = BillingModelPricingSnapshot {
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 3.0,
                    "output_price_per_1m": 15.0
                }]
            })),
            model_tiered_pricing: None,
            ..pricing()
        };

        let result = BillingService::new()
            .calculate(
                &pricing,
                &BillingUsageInput {
                    api_format: Some("openai:responses".to_string()),
                    requested_processing_tier: Some("priority".to_string()),
                    input_tokens: 1_000_000,
                    ..BillingUsageInput::new("chat")
                },
            )
            .expect("global Standard pricing should calculate OpenAI Fast usage");

        assert_eq!(result.cost_result.status, BillingSnapshotStatus::Complete);
        assert_eq!(result.cost_result.cost, 3.0);
        assert_eq!(
            result.pricing_resolution.tiered_pricing_source,
            Some(crate::BillingPricingSource::GlobalDefault)
        );
        assert_eq!(
            result.cost_result.snapshot.resolved_variables["input_price_per_1m"],
            json!(3.0)
        );
    }

    #[test]
    fn nonstandard_request_with_only_fixed_price_is_still_billable() {
        let pricing = BillingModelPricingSnapshot {
            default_price_per_request: Some(0.02),
            default_tiered_pricing: None,
            model_tiered_pricing: None,
            ..pricing()
        };

        let result = BillingService::new()
            .calculate(&pricing, &processing_usage(Some("fast"), None, 1_000))
            .expect("fixed request pricing should calculate Fast usage");

        assert_eq!(result.cost_result.status, BillingSnapshotStatus::Complete);
        assert_eq!(result.cost_result.cost, 0.02);
        assert_eq!(result.pricing_resolution.tiered_pricing, None);
        assert_eq!(result.pricing_resolution.price_per_request, Some(0.02));
    }

    #[test]
    fn response_actual_tier_does_not_override_requested_catalog() {
        let cases = ["default", "flex", "priority"];

        for actual in cases {
            let result = BillingService::new()
                .calculate(
                    &processing_pricing(),
                    &processing_usage(Some("priority"), Some(actual), 100),
                )
                .expect("processing tier should resolve");

            assert_eq!(result.cost_result.status, BillingSnapshotStatus::Complete);
            assert_eq!(
                result.pricing_resolution.actual_processing_tier.as_deref(),
                Some(actual)
            );
            assert_eq!(
                result.pricing_resolution.billing_processing_tier.as_deref(),
                Some("priority")
            );
            assert_eq!(
                result.cost_result.snapshot.resolved_variables["input_price_per_1m"],
                json!(10.0)
            );
            assert_eq!(
                result.cost_result.snapshot.resolved_variables["cache_creation_price_per_1m"],
                json!(12.5)
            );
            assert_eq!(
                result.pricing_resolution.tiered_pricing_source,
                Some(BillingPricingSource::ProviderOverride)
            );
        }
    }

    #[test]
    fn processing_multiplier_is_settled_and_authorized_from_standard_prices() {
        let pricing = BillingModelPricingSnapshot {
            provider_api_key_rate_multipliers: None,
            default_price_per_request: Some(0.02),
            default_tiered_pricing: Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 2.0,
                    "output_price_per_1m": 4.0,
                    "cache_creation_price_per_1m": 0.0,
                    "cache_read_price_per_1m": 0.0
                }],
                "processing_tiers": {
                    "priority": {"price_multiplier": 2.5}
                }
            })),
            model_price_per_request: None,
            model_tiered_pricing: None,
            ..pricing()
        };
        let usage = BillingUsageInput {
            api_format: Some("openai:responses".to_string()),
            requested_processing_tier: Some("priority".to_string()),
            actual_processing_tier: Some("priority".to_string()),
            input_tokens: 1_000_000,
            ..BillingUsageInput::new("chat")
        };

        let settled = BillingService::new()
            .calculate(&pricing, &usage)
            .expect("multiplier pricing should settle");
        assert_eq!(settled.cost_result.status, BillingSnapshotStatus::Complete);
        assert_eq!(settled.cost_result.cost, 5.02);
        assert_eq!(settled.actual_total_cost, 5.02);
        assert_eq!(
            settled.cost_result.snapshot.resolved_variables["input_price_per_1m"],
            json!(5.0)
        );
        assert_eq!(
            settled.cost_result.snapshot.resolved_variables["price_per_request"],
            json!(0.02),
            "processing multiplier must not affect price_per_request"
        );

        let mut estimate = BillingAuthorizationEstimateInput::new("chat", 1_000_000);
        estimate.api_format = Some("openai:responses".to_string());
        estimate.requested_processing_tier = Some("priority".to_string());
        estimate.max_output_tokens = Some(0);
        assert_eq!(
            BillingService::new()
                .estimate_authorization_cost_upper_bound(&pricing, &estimate)
                .expect("multiplier pricing should authorize"),
            Some(5.02)
        );
    }

    #[test]
    fn invalid_processing_multiplier_fails_closed_at_settlement_and_authorization() {
        let pricing = BillingModelPricingSnapshot {
            provider_api_key_rate_multipliers: None,
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 2.0}],
                "processing_tiers": {
                    "priority": {"price_multiplier": -1.0}
                }
            })),
            model_tiered_pricing: None,
            ..pricing()
        };
        let usage = processing_usage(Some("priority"), Some("priority"), 1_000);

        let settlement_error = BillingService::new()
            .calculate(&pricing, &usage)
            .expect_err("invalid multiplier settlement must return a configuration error");
        assert!(settlement_error
            .to_string()
            .contains("price_multiplier must be a non-negative finite number"));

        let mut estimate = BillingAuthorizationEstimateInput::new("chat", 1_000);
        estimate.api_format = Some("openai:responses".to_string());
        estimate.requested_processing_tier = Some("priority".to_string());
        estimate.max_output_tokens = Some(0);
        let authorization_error = BillingService::new()
            .estimate_authorization_cost_upper_bound(&pricing, &estimate)
            .expect_err("invalid multiplier authorization must return a configuration error");
        assert!(authorization_error
            .to_string()
            .contains("price_multiplier must be a non-negative finite number"));
    }

    #[test]
    fn empty_processing_catalog_cannot_turn_an_invalid_multiplier_into_zero_cost() {
        let pricing = BillingModelPricingSnapshot {
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 2.0}],
                "processing_tiers": {
                    "priority": {
                        "tiers": [{}],
                        "price_multiplier": 2.0
                    }
                }
            })),
            model_tiered_pricing: None,
            ..pricing()
        };
        let usage = processing_usage(Some("priority"), Some("priority"), 1_000);

        let settlement_error = BillingService::new()
            .calculate(&pricing, &usage)
            .expect_err("malformed processing pricing must not settle as zero");
        assert!(settlement_error
            .to_string()
            .contains("explicit catalog contains malformed or unrecognized prices"));

        let mut estimate = BillingAuthorizationEstimateInput::new("chat", 1_000);
        estimate.requested_processing_tier = Some("priority".to_string());
        estimate.max_output_tokens = Some(0);
        let authorization_error = BillingService::new()
            .estimate_authorization_cost_upper_bound(&pricing, &estimate)
            .expect_err("malformed processing pricing must stop authorization");
        assert!(authorization_error
            .to_string()
            .contains("explicit catalog contains malformed or unrecognized prices"));
    }

    #[test]
    fn authorization_accepts_input_only_processing_catalog() {
        let pricing = BillingModelPricingSnapshot {
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 0.1}],
                "processing_tiers": {
                    "embedding": {
                        "tiers": [{
                            "up_to": null,
                            "input_price_per_1m": 0.2,
                            "output_price_per_1m": null
                        }]
                    }
                }
            })),
            model_tiered_pricing: None,
            ..pricing()
        };
        let mut estimate = BillingAuthorizationEstimateInput::new("embedding", 1_000_000);
        estimate.requested_processing_tier = Some("embedding".to_string());

        assert_eq!(
            BillingService::new()
                .estimate_authorization_cost_upper_bound(&pricing, &estimate)
                .expect("input-only processing catalog should be valid"),
            Some(0.45),
            "the bound includes the runtime's conservative cache-write fallback"
        );
    }

    #[test]
    fn invalid_image_catalog_is_reported_before_the_unavailable_image_estimate() {
        let pricing = BillingModelPricingSnapshot {
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "image_output_price_ranges": {
                    "1048576": {"low": 0.04}
                }
            })),
            model_tiered_pricing: None,
            ..pricing()
        };
        let estimate = BillingAuthorizationEstimateInput::new("image", 0);

        let err = BillingService::new()
            .estimate_authorization_cost_upper_bound(&pricing, &estimate)
            .expect_err("an unparseable image range must be a configuration error");
        assert!(err
            .to_string()
            .contains("Standard catalog contains malformed or unrecognized prices"));
    }

    #[test]
    fn historical_noncanonical_processing_tier_key_is_a_configuration_error() {
        let pricing = BillingModelPricingSnapshot {
            default_tiered_pricing: Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 1.0}],
                "processing_tiers": {
                    "Priority": {"price_multiplier": 2.0}
                }
            })),
            model_tiered_pricing: None,
            ..pricing()
        };
        let mut estimate = BillingAuthorizationEstimateInput::new("chat", 1_000);
        estimate.requested_processing_tier = Some("priority".to_string());
        estimate.max_output_tokens = Some(0);

        let err = BillingService::new()
            .estimate_authorization_cost_upper_bound(&pricing, &estimate)
            .expect_err("a noncanonical historical key must not disappear during lookup");
        assert!(err
            .to_string()
            .contains("must be canonical lowercase without surrounding whitespace"));
    }

    #[test]
    fn finite_processing_catalog_fails_only_on_requested_catalog_bounds() {
        let priority = BillingService::new()
            .calculate(
                &processing_pricing(),
                &processing_usage(Some("priority"), Some("priority"), 300_000),
            )
            .expect("billing should calculate");
        assert_eq!(priority.cost_result.status, BillingSnapshotStatus::NoRule);
        assert_eq!(
            priority.cost_result.snapshot.missing_required,
            vec!["input_context_tier"]
        );

        let conflicting_actual = BillingService::new()
            .calculate(
                &processing_pricing(),
                &processing_usage(Some("priority"), Some("expedited"), 100),
            )
            .expect("billing should calculate");
        assert_eq!(
            conflicting_actual.cost_result.status,
            BillingSnapshotStatus::Complete
        );
        assert_eq!(
            conflicting_actual
                .pricing_resolution
                .billing_processing_tier
                .as_deref(),
            Some("priority")
        );
    }

    #[test]
    fn processing_catalog_boundaries_match_context_and_priority_contracts() {
        let cases = [
            (
                "default",
                272_000,
                BillingSnapshotStatus::Complete,
                Some(5.0),
            ),
            (
                "default",
                272_001,
                BillingSnapshotStatus::Complete,
                Some(10.0),
            ),
            ("flex", 272_000, BillingSnapshotStatus::Complete, Some(2.5)),
            ("flex", 272_001, BillingSnapshotStatus::Complete, Some(5.0)),
            (
                "priority",
                272_000,
                BillingSnapshotStatus::Complete,
                Some(10.0),
            ),
            ("priority", 272_001, BillingSnapshotStatus::NoRule, None),
        ];

        for (actual, input_tokens, status, input_price) in cases {
            let result = BillingService::new()
                .calculate(
                    &processing_pricing(),
                    &processing_usage(Some(actual), Some(actual), input_tokens),
                )
                .expect("processing boundary should resolve");
            assert_eq!(
                result.cost_result.status, status,
                "{actual} at {input_tokens}"
            );
            if let Some(input_price) = input_price {
                assert_eq!(
                    result.cost_result.snapshot.resolved_variables["input_price_per_1m"],
                    json!(input_price),
                    "{actual} at {input_tokens}"
                );
            } else {
                assert_eq!(
                    result.cost_result.snapshot.missing_required,
                    vec!["input_context_tier"]
                );
            }
        }
    }

    #[test]
    fn authorization_estimate_uses_known_request_cache_ttl() {
        let pricing = BillingModelPricingSnapshot {
            provider_api_key_rate_multipliers: None,
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 1.0,
                    "output_price_per_1m": 0.0,
                    "cache_creation_price_per_1m": 1.25,
                    "cache_read_price_per_1m": 0.1,
                    "cache_ttl_pricing": [{
                        "ttl_minutes": 60,
                        "cache_creation_price_per_1m": 100.0,
                        "cache_read_price_per_1m": 100.0
                    }]
                }]
            })),
            ..pricing()
        };
        let service = BillingService::new();
        let mut estimate = BillingAuthorizationEstimateInput::new("chat", 1_000_000);
        estimate.api_format = Some("openai:responses".to_string());
        estimate.max_output_tokens = Some(0);
        estimate.cache_ttl_minutes = Some(30);

        assert_eq!(
            service
                .estimate_authorization_cost_upper_bound(&pricing, &estimate)
                .expect("known TTL estimate should calculate"),
            Some(1.25)
        );

        estimate.cache_ttl_minutes = None;
        assert_eq!(
            service
                .estimate_authorization_cost_upper_bound(&pricing, &estimate)
                .expect("unknown TTL estimate should calculate"),
            Some(100.0)
        );
    }

    #[test]
    fn authorization_estimate_uses_only_processing_catalogs_eligible_for_context() {
        let service = BillingService::new();
        let mut estimate = BillingAuthorizationEstimateInput::new("chat", 300_000);
        estimate.api_format = Some("openai:responses".to_string());
        estimate.max_output_tokens = Some(0);
        estimate.cache_ttl_minutes = Some(30);

        for (requested_processing_tier, expected) in [
            (None, 3.75),
            (Some("standard"), 3.75),
            (Some("flex"), 1.875),
        ] {
            estimate.requested_processing_tier = requested_processing_tier.map(ToOwned::to_owned);
            assert_eq!(
                service
                    .estimate_authorization_cost_upper_bound(&processing_pricing(), &estimate)
                    .expect("eligible processing catalogs should calculate"),
                Some(expected),
                "requested tier: {requested_processing_tier:?}"
            );
        }

        estimate.requested_processing_tier = Some("priority".to_string());
        assert_eq!(
            service
                .estimate_authorization_cost_upper_bound(&processing_pricing(), &estimate)
                .expect("ineligible requested catalog should resolve"),
            None
        );
    }

    #[test]
    fn unknown_actual_tier_does_not_override_requested_tier_or_fixed_price() {
        let pricing = BillingModelPricingSnapshot {
            default_price_per_request: Some(0.02),
            ..processing_pricing()
        };
        let result = BillingService::new()
            .calculate(
                &pricing,
                &processing_usage(Some("priority"), Some("expedited"), 100),
            )
            .expect("billing should calculate");

        assert_eq!(result.cost_result.status, BillingSnapshotStatus::Complete);
        assert_eq!(
            result.pricing_resolution.billing_processing_tier.as_deref(),
            Some("priority")
        );
        assert_eq!(
            result.pricing_resolution.actual_processing_tier.as_deref(),
            Some("expedited")
        );
        assert_eq!(result.pricing_resolution.price_per_request, Some(0.02));
    }

    #[test]
    fn authorization_estimate_bounds_only_the_requested_catalog() {
        let service = BillingService::new();
        let mut estimate = BillingAuthorizationEstimateInput::new("chat", 100_000);
        estimate.api_format = Some("openai:responses".to_string());
        estimate.max_output_tokens = Some(1_000_000);

        estimate.requested_processing_tier = Some("priority".to_string());
        let priority = service
            .estimate_authorization_cost_upper_bound(&processing_pricing(), &estimate)
            .expect("priority estimate should calculate")
            .expect("priority estimate should be bounded");

        estimate.requested_processing_tier = Some("flex".to_string());
        let flex = service
            .estimate_authorization_cost_upper_bound(&processing_pricing(), &estimate)
            .expect("flex estimate should calculate")
            .expect("flex estimate should be bounded");

        assert_eq!(priority, 61.25);
        assert_eq!(flex, 15.3125);
        assert!(priority > flex);
    }

    #[test]
    fn authorization_estimate_returns_none_when_the_bound_cannot_be_proven() {
        let service = BillingService::new();
        let mut estimate = BillingAuthorizationEstimateInput::new("chat", 100);
        estimate.api_format = Some("openai:responses".to_string());
        estimate.requested_processing_tier = Some("priority".to_string());

        assert_eq!(
            service
                .estimate_authorization_cost_upper_bound(&processing_pricing(), &estimate)
                .expect("unbounded output estimate should resolve"),
            None
        );

        estimate.max_output_tokens = Some(10);
        estimate.requested_processing_tier = Some("expedited".to_string());
        assert_eq!(
            service
                .estimate_authorization_cost_upper_bound(&processing_pricing(), &estimate)
                .expect("unknown tier should inherit the effective Standard catalog"),
            Some(0.000925)
        );

        estimate.requested_processing_tier = Some("priority".to_string());
        estimate.input_tokens = 300_000;
        assert_eq!(
            service
                .estimate_authorization_cost_upper_bound(&processing_pricing(), &estimate)
                .expect("finite catalog estimate should resolve"),
            None
        );
    }

    #[test]
    fn authorization_estimate_supports_standard_fixed_price_and_free_tier() {
        let service = BillingService::new();
        let estimate = BillingAuthorizationEstimateInput::new("chat", 1_000);
        let fixed_pricing = BillingModelPricingSnapshot {
            default_tiered_pricing: None,
            default_price_per_request: Some(0.02),
            provider_api_key_rate_multipliers: None,
            ..pricing()
        };
        assert_eq!(
            service
                .estimate_authorization_cost_upper_bound(&fixed_pricing, &estimate)
                .expect("fixed estimate should calculate"),
            Some(0.02)
        );

        let free_pricing = BillingModelPricingSnapshot {
            provider_billing_type: Some("free_tier".to_string()),
            ..processing_pricing()
        };
        assert_eq!(
            service
                .estimate_authorization_cost_upper_bound(&free_pricing, &estimate)
                .expect("free estimate should calculate"),
            Some(0.0)
        );
    }

    #[test]
    fn authorization_estimate_checks_every_reachable_non_monotonic_price_tier() {
        let pricing = BillingModelPricingSnapshot {
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [
                    {
                        "up_to": 10_000,
                        "input_price_per_1m": 100.0,
                        "output_price_per_1m": 100.0,
                        "cache_creation_price_per_1m": 100.0,
                        "cache_read_price_per_1m": 100.0
                    },
                    {
                        "up_to": null,
                        "input_price_per_1m": 1.0,
                        "output_price_per_1m": 1.0,
                        "cache_creation_price_per_1m": 1.0,
                        "cache_read_price_per_1m": 1.0
                    }
                ]
            })),
            model_tiered_pricing: None,
            ..pricing()
        };
        let mut estimate = BillingAuthorizationEstimateInput::new("chat", 100_000);
        estimate.api_format = Some("openai:responses".to_string());
        estimate.max_output_tokens = Some(0);

        assert_eq!(
            BillingService::new()
                .estimate_authorization_cost_upper_bound(&pricing, &estimate)
                .expect("non-monotonic catalog should calculate"),
            Some(10.0)
        );
    }

    #[test]
    fn authorization_estimate_uses_api_key_cache_read_ttl_price() {
        let pricing = BillingModelPricingSnapshot {
            provider_api_key_cache_ttl_minutes: Some(60),
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 1.0,
                    "output_price_per_1m": 1.0,
                    "cache_creation_price_per_1m": 1.25,
                    "cache_read_price_per_1m": 0.1,
                    "cache_ttl_pricing": [{
                        "ttl_minutes": 60,
                        "cache_creation_price_per_1m": 1.25,
                        "cache_read_price_per_1m": 200.0
                    }]
                }]
            })),
            model_tiered_pricing: None,
            ..pricing()
        };
        let mut estimate = BillingAuthorizationEstimateInput::new("chat", 100_000);
        estimate.api_format = Some("openai:responses".to_string());
        estimate.max_output_tokens = Some(0);

        assert_eq!(
            BillingService::new()
                .estimate_authorization_cost_upper_bound(&pricing, &estimate)
                .expect("cache read TTL catalog should calculate"),
            Some(20.0)
        );
    }

    #[test]
    fn fixed_request_pricing_remains_independent_for_standard_usage() {
        let pricing = BillingModelPricingSnapshot {
            default_tiered_pricing: None,
            default_price_per_request: Some(0.02),
            ..pricing()
        };
        let result = BillingService::new()
            .calculate(&pricing, &processing_usage(None, None, 1_000))
            .expect("fixed request pricing should calculate");

        assert_eq!(result.cost_result.status, BillingSnapshotStatus::Complete);
        assert_eq!(result.cost_result.cost, 0.02);
        assert_eq!(result.pricing_resolution.tiered_pricing, None);
        assert_eq!(result.pricing_resolution.price_per_request, Some(0.02));
    }

    #[test]
    fn image_token_usage_without_image_output_price_bills_tokens_only() {
        let pricing = BillingModelPricingSnapshot {
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 1.0,
                    "output_price_per_1m": 2.0
                }]
            })),
            ..pricing()
        };

        let result = BillingService::new()
            .calculate(
                &pricing,
                &BillingUsageInput {
                    task_type: "image".to_string(),
                    api_format: Some("openai:image".to_string()),
                    requested_processing_tier: None,
                    actual_processing_tier: None,
                    request_count: 1,
                    input_tokens: 1_000,
                    output_tokens: 20_000,
                    cache_creation_tokens: 0,
                    cache_creation_ephemeral_5m_tokens: 0,
                    cache_creation_ephemeral_1h_tokens: 0,
                    cache_read_tokens: 0,
                    image_count: 1,
                    image_size: Some("1024x1024".to_string()),
                    image_quality: Some("medium".to_string()),
                    image_output_format: Some("png".to_string()),
                    cache_ttl_minutes: None,
                },
            )
            .expect("billing should calculate");

        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_dimensions
                .get("image_output_pricing_mode"),
            Some(&json!("none"))
        );
        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_dimensions
                .get("image_count_unmetered"),
            Some(&json!(0))
        );
        assert_eq!(
            result
                .cost_result
                .snapshot
                .cost_breakdown
                .get("image_output_cost"),
            Some(&0.0)
        );
        assert_eq!(result.cost_result.cost, 0.041);
    }

    #[test]
    fn image_default_output_price_adds_image_cost_even_with_token_usage() {
        let pricing = BillingModelPricingSnapshot {
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 1.0,
                    "output_price_per_1m": 2.0
                }],
                "image_output_price_default": 0.05
            })),
            ..pricing()
        };

        let result = BillingService::new()
            .calculate(
                &pricing,
                &BillingUsageInput {
                    task_type: "image".to_string(),
                    api_format: Some("openai:image".to_string()),
                    requested_processing_tier: None,
                    actual_processing_tier: None,
                    request_count: 1,
                    input_tokens: 1_000,
                    output_tokens: 20_000,
                    cache_creation_tokens: 0,
                    cache_creation_ephemeral_5m_tokens: 0,
                    cache_creation_ephemeral_1h_tokens: 0,
                    cache_read_tokens: 0,
                    image_count: 1,
                    image_size: Some("1024x1024".to_string()),
                    image_quality: Some("medium".to_string()),
                    image_output_format: Some("png".to_string()),
                    cache_ttl_minutes: None,
                },
            )
            .expect("billing should calculate");

        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_dimensions
                .get("image_output_pricing_mode"),
            Some(&json!("per_image"))
        );
        assert_eq!(
            result
                .cost_result
                .snapshot
                .cost_breakdown
                .get("image_output_cost"),
            Some(&0.05)
        );
        assert_eq!(result.cost_result.cost, 0.091);
    }

    #[test]
    fn image_default_output_price_generates_rule_without_token_tiers() {
        let pricing = BillingModelPricingSnapshot {
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "image_output_price_default": 0.05
            })),
            ..pricing()
        };

        let result = BillingService::new()
            .calculate(
                &pricing,
                &BillingUsageInput {
                    task_type: "image".to_string(),
                    api_format: Some("openai:image".to_string()),
                    requested_processing_tier: None,
                    actual_processing_tier: None,
                    request_count: 1,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_creation_ephemeral_5m_tokens: 0,
                    cache_creation_ephemeral_1h_tokens: 0,
                    cache_read_tokens: 0,
                    image_count: 2,
                    image_size: Some("1024x1024".to_string()),
                    image_quality: Some("medium".to_string()),
                    image_output_format: Some("png".to_string()),
                    cache_ttl_minutes: None,
                },
            )
            .expect("billing should calculate");

        assert_eq!(result.cost_result.status, BillingSnapshotStatus::Complete);
        assert_eq!(
            result
                .cost_result
                .snapshot
                .cost_breakdown
                .get("image_output_cost"),
            Some(&0.1)
        );
        assert_eq!(result.cost_result.cost, 0.1);
    }

    #[test]
    fn image_pixel_ranges_generate_rule_without_token_tiers() {
        let pricing = BillingModelPricingSnapshot {
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "image_output_price_ranges": [{
                    "up_to_pixels": null,
                    "prices": { "medium": 0.04 }
                }]
            })),
            ..pricing()
        };

        let result = BillingService::new()
            .calculate(
                &pricing,
                &BillingUsageInput {
                    task_type: "image".to_string(),
                    api_format: Some("openai:image".to_string()),
                    requested_processing_tier: None,
                    actual_processing_tier: None,
                    request_count: 1,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_creation_ephemeral_5m_tokens: 0,
                    cache_creation_ephemeral_1h_tokens: 0,
                    cache_read_tokens: 0,
                    image_count: 2,
                    image_size: Some("1024x1024".to_string()),
                    image_quality: Some("medium".to_string()),
                    image_output_format: Some("png".to_string()),
                    cache_ttl_minutes: None,
                },
            )
            .expect("billing should calculate");

        assert_eq!(result.cost_result.status, BillingSnapshotStatus::Complete);
        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_dimensions
                .get("image_output_pricing_mode"),
            Some(&json!("pixel_tiers"))
        );
        assert_eq!(
            result
                .cost_result
                .snapshot
                .cost_breakdown
                .get("image_output_cost"),
            Some(&0.08)
        );
        assert_eq!(result.cost_result.cost, 0.08);
    }

    #[test]
    fn image_token_usage_with_matrix_adds_matrix_image_cost() {
        let pricing = BillingModelPricingSnapshot {
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 1.0,
                    "output_price_per_1m": 2.0
                }],
                "image_output_price_default": 0.01,
                "image_output_prices": {
                    "1024x1024": { "medium": 0.05 }
                }
            })),
            ..pricing()
        };

        let result = BillingService::new()
            .calculate(
                &pricing,
                &BillingUsageInput {
                    task_type: "image".to_string(),
                    api_format: Some("openai:image".to_string()),
                    requested_processing_tier: None,
                    actual_processing_tier: None,
                    request_count: 1,
                    input_tokens: 1_000,
                    output_tokens: 20_000,
                    cache_creation_tokens: 0,
                    cache_creation_ephemeral_5m_tokens: 0,
                    cache_creation_ephemeral_1h_tokens: 0,
                    cache_read_tokens: 0,
                    image_count: 1,
                    image_size: Some("1024x1024".to_string()),
                    image_quality: Some("medium".to_string()),
                    image_output_format: Some("png".to_string()),
                    cache_ttl_minutes: None,
                },
            )
            .expect("billing should calculate");

        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_dimensions
                .get("image_output_pricing_mode"),
            Some(&json!("matrix"))
        );
        assert_eq!(
            result
                .cost_result
                .snapshot
                .cost_breakdown
                .get("image_output_cost"),
            Some(&0.05)
        );
    }

    #[test]
    fn image_token_usage_with_pixel_ranges_adds_range_image_cost() {
        let pricing = BillingModelPricingSnapshot {
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 1.0,
                    "output_price_per_1m": 2.0
                }],
                "image_output_price_default": 0.01,
                "image_output_price_ranges": [
                    {
                        "up_to_pixels": 1_048_576,
                        "prices": { "medium": 0.04 }
                    },
                    {
                        "up_to_pixels": 2_097_152,
                        "prices": { "medium": 0.08 }
                    }
                ]
            })),
            ..pricing()
        };

        let result = BillingService::new()
            .calculate(
                &pricing,
                &BillingUsageInput {
                    task_type: "image".to_string(),
                    api_format: Some("openai:image".to_string()),
                    requested_processing_tier: None,
                    actual_processing_tier: None,
                    request_count: 1,
                    input_tokens: 1_000,
                    output_tokens: 20_000,
                    cache_creation_tokens: 0,
                    cache_creation_ephemeral_5m_tokens: 0,
                    cache_creation_ephemeral_1h_tokens: 0,
                    cache_read_tokens: 0,
                    image_count: 1,
                    image_size: Some("1536 x 1024".to_string()),
                    image_quality: Some("medium".to_string()),
                    image_output_format: Some("png".to_string()),
                    cache_ttl_minutes: None,
                },
            )
            .expect("billing should calculate");

        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_dimensions
                .get("image_output_pricing_mode"),
            Some(&json!("pixel_tiers"))
        );
        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_dimensions
                .get("image_pixels"),
            Some(&json!(1_572_864))
        );
        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_dimensions
                .get("image_output_price_bucket"),
            Some(&json!("<=2097152px"))
        );
        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_variables
                .get("image_output_price_per_image"),
            Some(&json!(0.08))
        );
        assert_eq!(
            result
                .cost_result
                .snapshot
                .cost_breakdown
                .get("image_output_cost"),
            Some(&0.08)
        );
        assert_eq!(result.cost_result.cost, 0.121);
    }

    #[test]
    fn five_minute_cache_ttl_uses_base_cache_prices() {
        let pricing = BillingModelPricingSnapshot {
            provider_id: "provider-1".to_string(),
            provider_billing_type: Some("pay_as_you_go".to_string()),
            provider_api_key_id: Some("key-1".to_string()),
            provider_api_key_rate_multipliers: None,
            provider_api_key_cache_ttl_minutes: Some(5),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-5.4".to_string(),
            global_model_config: None,
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 2.5,
                    "output_price_per_1m": 15.0,
                    "cache_creation_price_per_1m": 3.125,
                    "cache_read_price_per_1m": 0.25,
                    "cache_ttl_pricing": [{
                        "ttl_minutes": 60,
                        "cache_creation_price_per_1m": 5.0,
                        "cache_read_price_per_1m": null
                    }]
                }]
            })),
            model_id: None,
            model_provider_model_name: None,
            model_config: None,
            model_price_per_request: None,
            model_tiered_pricing: None,
        };

        let result = BillingService::new()
            .calculate(
                &pricing,
                &BillingUsageInput {
                    task_type: "chat".to_string(),
                    api_format: None,
                    requested_processing_tier: None,
                    actual_processing_tier: None,
                    request_count: 1,
                    input_tokens: 1_000,
                    output_tokens: 10,
                    cache_creation_tokens: 0,
                    cache_creation_ephemeral_5m_tokens: 0,
                    cache_creation_ephemeral_1h_tokens: 0,
                    cache_read_tokens: 100,
                    image_count: 0,
                    image_size: None,
                    image_quality: None,
                    image_output_format: None,
                    cache_ttl_minutes: Some(5),
                },
            )
            .expect("billing should calculate");

        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_variables
                .get("cache_creation_price_per_1m"),
            Some(&json!(3.125))
        );
        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_variables
                .get("cache_read_price_per_1m"),
            Some(&json!(0.25))
        );
    }

    #[test]
    fn one_hour_cache_ttl_keeps_base_cache_read_when_ttl_entry_omits_it() {
        let pricing = BillingModelPricingSnapshot {
            provider_id: "provider-1".to_string(),
            provider_billing_type: Some("pay_as_you_go".to_string()),
            provider_api_key_id: Some("key-1".to_string()),
            provider_api_key_rate_multipliers: None,
            provider_api_key_cache_ttl_minutes: Some(60),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-5.4".to_string(),
            global_model_config: None,
            default_price_per_request: None,
            default_tiered_pricing: Some(json!({
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 2.5,
                    "output_price_per_1m": 15.0,
                    "cache_creation_price_per_1m": 3.125,
                    "cache_read_price_per_1m": 0.25,
                    "cache_ttl_pricing": [{
                        "ttl_minutes": 60,
                        "cache_creation_price_per_1m": 5.0,
                        "cache_read_price_per_1m": null
                    }]
                }]
            })),
            model_id: None,
            model_provider_model_name: None,
            model_config: None,
            model_price_per_request: None,
            model_tiered_pricing: None,
        };

        let result = BillingService::new()
            .calculate(
                &pricing,
                &BillingUsageInput {
                    task_type: "chat".to_string(),
                    api_format: None,
                    requested_processing_tier: None,
                    actual_processing_tier: None,
                    request_count: 1,
                    input_tokens: 1_000,
                    output_tokens: 10,
                    cache_creation_tokens: 0,
                    cache_creation_ephemeral_5m_tokens: 0,
                    cache_creation_ephemeral_1h_tokens: 0,
                    cache_read_tokens: 100,
                    image_count: 0,
                    image_size: None,
                    image_quality: None,
                    image_output_format: None,
                    cache_ttl_minutes: Some(60),
                },
            )
            .expect("billing should calculate");

        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_variables
                .get("cache_creation_price_per_1m"),
            Some(&json!(5.0))
        );
        assert_eq!(
            result
                .cost_result
                .snapshot
                .resolved_variables
                .get("cache_read_price_per_1m"),
            Some(&json!(0.25))
        );
    }
}
