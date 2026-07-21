use aether_data_contracts::repository::{
    billing::StoredBillingModelContext,
    global_models::{explicit_pricing_catalog_state, ExplicitPricingCatalogState},
    usage::normalize_provider_service_tier,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingPricingSource {
    ProviderOverride,
    GlobalDefault,
    Mixed,
}

impl BillingPricingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProviderOverride => "provider_override",
            Self::GlobalDefault => "global_default",
            Self::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid billing pricing configuration: {message}")]
pub struct BillingPricingConfigurationError {
    message: String,
}

impl BillingPricingConfigurationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingPricingResolution {
    pub requested_processing_tier: Option<String>,
    pub actual_processing_tier: Option<String>,
    pub billing_processing_tier: Option<String>,
    pub tiered_pricing: Option<Value>,
    pub tiered_pricing_source: Option<BillingPricingSource>,
    pub processing_tier_price_multiplier: Option<f64>,
    pub price_per_request: Option<f64>,
    pub price_per_request_source: Option<BillingPricingSource>,
}

impl BillingPricingResolution {
    pub fn requires_actual_processing_tier(&self) -> bool {
        self.billing_processing_tier.is_none()
    }

    pub fn pricing_source(&self) -> &'static str {
        match (self.tiered_pricing_source, self.price_per_request_source) {
            (Some(tiered), Some(request)) if tiered != request => "mixed",
            (Some(source), _) | (_, Some(source)) => source.as_str(),
            (None, None) => "unpriced",
        }
    }

    pub fn bills_standard_processing_tier(&self) -> bool {
        self.billing_processing_tier
            .as_deref()
            .is_some_and(processing_tier_is_standard)
    }

    pub fn bills_requested_processing_tier(&self) -> bool {
        let requested = self
            .requested_processing_tier
            .as_deref()
            .map(canonical_processing_tier)
            .unwrap_or_else(|| "standard".to_string());
        self.billing_processing_tier.as_deref() == Some(requested.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingModelPricingSnapshot {
    pub provider_id: String,
    pub provider_billing_type: Option<String>,
    pub provider_api_key_id: Option<String>,
    pub provider_api_key_rate_multipliers: Option<Value>,
    pub provider_api_key_cache_ttl_minutes: Option<i64>,
    pub global_model_id: String,
    pub global_model_name: String,
    pub global_model_config: Option<Value>,
    pub default_price_per_request: Option<f64>,
    pub default_tiered_pricing: Option<Value>,
    pub model_id: Option<String>,
    pub model_provider_model_name: Option<String>,
    pub model_config: Option<Value>,
    pub model_price_per_request: Option<f64>,
    pub model_tiered_pricing: Option<Value>,
}

impl BillingModelPricingSnapshot {
    pub fn resolve_pricing(
        &self,
        requested_processing_tier: Option<&str>,
        actual_processing_tier: Option<&str>,
    ) -> BillingPricingResolution {
        let requested_processing_tier = normalize_processing_tier(requested_processing_tier);
        let actual_processing_tier = normalize_processing_tier(actual_processing_tier);
        // The provider-reported actual tier is retained in the resolution for audit only. The
        // final upstream request is the sole authority for selecting a billing catalog.
        let billing_processing_tier = requested_processing_tier
            .as_deref()
            .map(canonical_processing_tier)
            .or_else(|| Some("standard".to_string()));

        let (tiered_pricing, tiered_pricing_source, processing_tier_price_multiplier) =
            billing_processing_tier
                .as_deref()
                .and_then(|tier| self.resolve_tiered_pricing(tier))
                .map_or((None, None, None), |(pricing, source, multiplier)| {
                    (Some(pricing), Some(source), multiplier)
                });
        let (price_per_request, price_per_request_source) = self
            .resolve_price_per_request()
            .map_or((None, None), |(price, source)| (Some(price), Some(source)));

        BillingPricingResolution {
            requested_processing_tier,
            actual_processing_tier,
            billing_processing_tier,
            tiered_pricing,
            tiered_pricing_source,
            processing_tier_price_multiplier,
            price_per_request,
            price_per_request_source,
        }
    }

    pub fn resolve_pricing_checked(
        &self,
        requested_processing_tier: Option<&str>,
        actual_processing_tier: Option<&str>,
    ) -> Result<BillingPricingResolution, BillingPricingConfigurationError> {
        self.validate_processing_tier_containers()?;
        let requested_processing_tier = normalize_processing_tier(requested_processing_tier);
        let actual_processing_tier = normalize_processing_tier(actual_processing_tier);
        // Keep this checked path aligned with `resolve_pricing`: response facts must never choose
        // the catalog used for settlement.
        let billing_processing_tier = requested_processing_tier
            .as_deref()
            .map(canonical_processing_tier)
            .or_else(|| Some("standard".to_string()));

        let (tiered_pricing, tiered_pricing_source, processing_tier_price_multiplier) =
            match billing_processing_tier.as_deref() {
                Some(tier) => self
                    .resolve_tiered_pricing_checked(tier)?
                    .map_or((None, None, None), |(pricing, source, multiplier)| {
                        (Some(pricing), Some(source), multiplier)
                    }),
                None => (None, None, None),
            };
        let (price_per_request, price_per_request_source) = self
            .resolve_price_per_request()
            .map_or((None, None), |(price, source)| (Some(price), Some(source)));

        Ok(BillingPricingResolution {
            requested_processing_tier,
            actual_processing_tier,
            billing_processing_tier,
            tiered_pricing,
            tiered_pricing_source,
            processing_tier_price_multiplier,
            price_per_request,
            price_per_request_source,
        })
    }

    pub fn resolve_authorization_pricing_candidates(
        &self,
        requested_processing_tier: Option<&str>,
    ) -> Result<Option<Vec<BillingPricingResolution>>, BillingPricingConfigurationError> {
        self.validate_processing_tier_containers()?;
        let requested_processing_tier = normalize_processing_tier(requested_processing_tier);
        let requested_billing_tier = requested_processing_tier
            .as_deref()
            .map(canonical_processing_tier)
            .unwrap_or_else(|| "standard".to_string());
        let requested_resolution = self.authorization_pricing_for_tier(
            requested_processing_tier.clone(),
            Some(requested_billing_tier.clone()),
        )?;
        if !processing_tier_is_standard(&requested_billing_tier)
            && requested_resolution.tiered_pricing.is_none()
            && requested_resolution.price_per_request.is_none()
        {
            return Ok(None);
        }

        Ok(Some(vec![requested_resolution]))
    }

    pub fn validate_authorization_pricing_configuration(
        &self,
        requested_processing_tier: Option<&str>,
    ) -> Result<(), BillingPricingConfigurationError> {
        self.resolve_authorization_pricing_candidates(requested_processing_tier)
            .map(|_| ())
    }

    fn authorization_pricing_for_tier(
        &self,
        requested_processing_tier: Option<String>,
        billing_processing_tier: Option<String>,
    ) -> Result<BillingPricingResolution, BillingPricingConfigurationError> {
        let (tiered_pricing, tiered_pricing_source, processing_tier_price_multiplier) =
            match billing_processing_tier.as_deref() {
                Some(tier) => self
                    .resolve_tiered_pricing_checked(tier)?
                    .map_or((None, None, None), |(pricing, source, multiplier)| {
                        (Some(pricing), Some(source), multiplier)
                    }),
                None => (None, None, None),
            };
        let (price_per_request, price_per_request_source) = self
            .resolve_price_per_request()
            .map_or((None, None), |(price, source)| (Some(price), Some(source)));

        Ok(BillingPricingResolution {
            requested_processing_tier,
            actual_processing_tier: None,
            billing_processing_tier,
            tiered_pricing,
            tiered_pricing_source,
            processing_tier_price_multiplier,
            price_per_request,
            price_per_request_source,
        })
    }

    fn resolve_tiered_pricing(
        &self,
        processing_tier: &str,
    ) -> Option<(Value, BillingPricingSource, Option<f64>)> {
        self.resolve_tiered_pricing_checked(processing_tier)
            .ok()
            .flatten()
    }

    fn resolve_tiered_pricing_checked(
        &self,
        processing_tier: &str,
    ) -> Result<Option<(Value, BillingPricingSource, Option<f64>)>, BillingPricingConfigurationError>
    {
        if processing_tier_is_standard(processing_tier) {
            return Ok(self
                .resolve_standard_tiered_pricing_checked()?
                .map(|(pricing, source)| (pricing.clone(), source, None)));
        }

        match processing_tier_overlay(self.model_tiered_pricing.as_ref(), processing_tier) {
            ProcessingTierOverlay::Explicit(pricing) => Ok(Some((
                pricing.clone(),
                BillingPricingSource::ProviderOverride,
                None,
            ))),
            ProcessingTierOverlay::Multiplier(multiplier) => self
                .resolve_multiplied_standard_tiered_pricing_checked(
                    multiplier,
                    BillingPricingSource::ProviderOverride,
                    processing_tier,
                )
                .map(|(pricing, source)| Some((pricing, source, Some(multiplier)))),
            ProcessingTierOverlay::Invalid(reason) => Err(invalid_processing_tier_error(
                BillingPricingSource::ProviderOverride,
                processing_tier,
                reason,
            )),
            ProcessingTierOverlay::Missing => {
                match processing_tier_overlay(self.default_tiered_pricing.as_ref(), processing_tier)
                {
                    ProcessingTierOverlay::Explicit(pricing) => Ok(Some((
                        pricing.clone(),
                        BillingPricingSource::GlobalDefault,
                        None,
                    ))),
                    ProcessingTierOverlay::Multiplier(multiplier) => self
                        .resolve_multiplied_standard_tiered_pricing_checked(
                            multiplier,
                            BillingPricingSource::GlobalDefault,
                            processing_tier,
                        )
                        .map(|(pricing, source)| Some((pricing, source, Some(multiplier)))),
                    ProcessingTierOverlay::Invalid(reason) => Err(invalid_processing_tier_error(
                        BillingPricingSource::GlobalDefault,
                        processing_tier,
                        reason,
                    )),
                    ProcessingTierOverlay::Missing => Ok(self
                        .resolve_standard_tiered_pricing_checked()?
                        .map(|(pricing, source)| (pricing.clone(), source, None))),
                }
            }
        }
    }

    fn resolve_standard_tiered_pricing_checked(
        &self,
    ) -> Result<Option<(&Value, BillingPricingSource)>, BillingPricingConfigurationError> {
        match self.model_tiered_pricing.as_ref() {
            Some(value)
                if explicit_pricing_catalog_state(value) == ExplicitPricingCatalogState::Valid =>
            {
                return Ok(Some((value, BillingPricingSource::ProviderOverride)));
            }
            Some(value)
                if explicit_pricing_catalog_state(value)
                    == ExplicitPricingCatalogState::Invalid =>
            {
                return Err(BillingPricingConfigurationError::new(
                    "provider Standard catalog contains malformed or unrecognized prices",
                ));
            }
            _ => {}
        }
        match self.default_tiered_pricing.as_ref() {
            Some(value)
                if explicit_pricing_catalog_state(value) == ExplicitPricingCatalogState::Valid =>
            {
                Ok(Some((value, BillingPricingSource::GlobalDefault)))
            }
            Some(value)
                if explicit_pricing_catalog_state(value)
                    == ExplicitPricingCatalogState::Invalid =>
            {
                Err(BillingPricingConfigurationError::new(
                    "global Standard catalog contains malformed or unrecognized prices",
                ))
            }
            _ => Ok(None),
        }
    }

    fn resolve_multiplied_standard_tiered_pricing_checked(
        &self,
        multiplier: f64,
        multiplier_source: BillingPricingSource,
        processing_tier: &str,
    ) -> Result<(Value, BillingPricingSource), BillingPricingConfigurationError> {
        let Some((standard, standard_source)) = self.resolve_standard_tiered_pricing_checked()?
        else {
            return Err(invalid_processing_tier_error(
                multiplier_source,
                processing_tier,
                "price_multiplier cannot be materialized without a valid Standard catalog",
            ));
        };
        let source = if standard_source == multiplier_source {
            standard_source
        } else {
            BillingPricingSource::Mixed
        };
        let pricing = multiply_pricing_catalog(standard, multiplier).ok_or_else(|| {
            invalid_processing_tier_error(
                multiplier_source,
                processing_tier,
                "price_multiplier produced an invalid pricing catalog",
            )
        })?;
        Ok((pricing, source))
    }

    fn validate_processing_tier_containers(&self) -> Result<(), BillingPricingConfigurationError> {
        for (pricing, source) in [
            (
                self.model_tiered_pricing.as_ref(),
                BillingPricingSource::ProviderOverride,
            ),
            (
                self.default_tiered_pricing.as_ref(),
                BillingPricingSource::GlobalDefault,
            ),
        ] {
            let Some(pricing) = pricing else {
                continue;
            };
            let Some(pricing) = pricing.as_object() else {
                return Err(BillingPricingConfigurationError::new(format!(
                    "{} tiered pricing must be an object",
                    source.as_str()
                )));
            };
            let Some(processing_tiers) = pricing.get("processing_tiers") else {
                continue;
            };
            if processing_tiers.is_null() {
                continue;
            }
            let Some(processing_tiers) = processing_tiers.as_object() else {
                return Err(BillingPricingConfigurationError::new(format!(
                    "{} processing_tiers must be an object",
                    source.as_str()
                )));
            };
            for (tier, overlay) in processing_tiers {
                let normalized_tier = normalize_processing_tier(Some(tier));
                if normalized_tier.as_deref() != Some(tier.as_str()) {
                    return Err(BillingPricingConfigurationError::new(format!(
                        "{} processing tier name `{tier}` must be canonical lowercase without surrounding whitespace",
                        source.as_str(),
                    )));
                }
                if !overlay.is_object() {
                    return Err(invalid_processing_tier_error(
                        source,
                        tier,
                        "overlay must be an object",
                    ));
                }
            }
        }
        Ok(())
    }

    fn resolve_price_per_request(&self) -> Option<(f64, BillingPricingSource)> {
        self.model_price_per_request
            .map(|price| (price, BillingPricingSource::ProviderOverride))
            .or_else(|| {
                self.default_price_per_request
                    .map(|price| (price, BillingPricingSource::GlobalDefault))
            })
    }

    pub fn is_free_tier(&self) -> bool {
        self.provider_billing_type
            .as_deref()
            .map(|value| value.eq_ignore_ascii_case("free_tier"))
            .unwrap_or(false)
    }

    pub fn rate_multiplier_for_api_format(&self, api_format: Option<&str>) -> f64 {
        let Some(api_format) = api_format.map(str::trim).filter(|value| !value.is_empty()) else {
            return 1.0;
        };
        let normalized = api_format.to_ascii_lowercase();
        let Some(mapping) = self
            .provider_api_key_rate_multipliers
            .as_ref()
            .and_then(Value::as_object)
        else {
            return 1.0;
        };
        mapping
            .get(&normalized)
            .and_then(|value| value.as_f64())
            .unwrap_or(1.0)
    }
}

impl From<&StoredBillingModelContext> for BillingModelPricingSnapshot {
    fn from(context: &StoredBillingModelContext) -> Self {
        Self {
            provider_id: context.provider_id.clone(),
            provider_billing_type: context.provider_billing_type.clone(),
            provider_api_key_id: context.provider_api_key_id.clone(),
            provider_api_key_rate_multipliers: context.provider_api_key_rate_multipliers.clone(),
            provider_api_key_cache_ttl_minutes: context.provider_api_key_cache_ttl_minutes,
            global_model_id: context.global_model_id.clone(),
            global_model_name: context.global_model_name.clone(),
            global_model_config: context.global_model_config.clone(),
            default_price_per_request: context.default_price_per_request,
            default_tiered_pricing: context.default_tiered_pricing.clone(),
            model_id: context.model_id.clone(),
            model_provider_model_name: context.model_provider_model_name.clone(),
            model_config: context.model_config.clone(),
            model_price_per_request: context.model_price_per_request,
            model_tiered_pricing: context.model_tiered_pricing.clone(),
        }
    }
}

impl From<StoredBillingModelContext> for BillingModelPricingSnapshot {
    fn from(context: StoredBillingModelContext) -> Self {
        Self {
            provider_id: context.provider_id,
            provider_billing_type: context.provider_billing_type,
            provider_api_key_id: context.provider_api_key_id,
            provider_api_key_rate_multipliers: context.provider_api_key_rate_multipliers,
            provider_api_key_cache_ttl_minutes: context.provider_api_key_cache_ttl_minutes,
            global_model_id: context.global_model_id,
            global_model_name: context.global_model_name,
            global_model_config: context.global_model_config,
            default_price_per_request: context.default_price_per_request,
            default_tiered_pricing: context.default_tiered_pricing,
            model_id: context.model_id,
            model_provider_model_name: context.model_provider_model_name,
            model_config: context.model_config,
            model_price_per_request: context.model_price_per_request,
            model_tiered_pricing: context.model_tiered_pricing,
        }
    }
}

fn normalize_processing_tier(value: Option<&str>) -> Option<String> {
    value.and_then(normalize_provider_service_tier)
}

fn canonical_processing_tier(value: &str) -> String {
    if processing_tier_is_standard(value) {
        "standard".to_string()
    } else {
        value.to_string()
    }
}

fn processing_tier_is_standard(value: &str) -> bool {
    matches!(value, "auto" | "default" | "standard")
}

enum ProcessingTierOverlay<'a> {
    Explicit(&'a Value),
    Multiplier(f64),
    Invalid(&'static str),
    Missing,
}

fn processing_tier_overlay<'a>(
    pricing: Option<&'a Value>,
    tier: &str,
) -> ProcessingTierOverlay<'a> {
    let Some(pricing) = pricing else {
        return ProcessingTierOverlay::Missing;
    };
    let Some(processing_tiers) = pricing.get("processing_tiers") else {
        return ProcessingTierOverlay::Missing;
    };
    if processing_tiers.is_null() {
        return ProcessingTierOverlay::Missing;
    }
    let Some(processing_tiers) = processing_tiers.as_object() else {
        return ProcessingTierOverlay::Invalid("processing_tiers must be an object");
    };
    let Some(overlay) = processing_tiers.get(tier) else {
        return ProcessingTierOverlay::Missing;
    };
    if !overlay.is_object() {
        return ProcessingTierOverlay::Invalid("overlay must be an object");
    }

    // An explicit catalog is authoritative even when a stale or future multiplier field is
    // present beside it. This preserves the legacy processing-tier contract.
    let explicit_catalog_state = explicit_pricing_catalog_state(overlay);
    match explicit_catalog_state {
        ExplicitPricingCatalogState::Valid => return ProcessingTierOverlay::Explicit(overlay),
        ExplicitPricingCatalogState::Invalid => {
            return ProcessingTierOverlay::Invalid(
                "explicit catalog contains malformed or unrecognized prices",
            );
        }
        ExplicitPricingCatalogState::Absent => {}
    }

    let Some(multiplier) = overlay.get("price_multiplier") else {
        return ProcessingTierOverlay::Invalid(
            "overlay must contain an explicit catalog or price_multiplier",
        );
    };
    match multiplier
        .as_f64()
        .filter(|multiplier| multiplier.is_finite() && *multiplier >= 0.0)
    {
        Some(multiplier) => ProcessingTierOverlay::Multiplier(multiplier),
        None => {
            ProcessingTierOverlay::Invalid("price_multiplier must be a non-negative finite number")
        }
    }
}

fn invalid_processing_tier_error(
    source: BillingPricingSource,
    processing_tier: &str,
    reason: &str,
) -> BillingPricingConfigurationError {
    BillingPricingConfigurationError::new(format!(
        "{} processing tier `{processing_tier}` {reason}",
        source.as_str()
    ))
}

fn multiply_pricing_catalog(pricing: &Value, multiplier: f64) -> Option<Value> {
    if !multiplier.is_finite() || multiplier < 0.0 {
        return None;
    }
    let mut multiplied = pricing.clone();
    let object = multiplied.as_object_mut()?;
    // A resolved catalog is self-contained. Keeping the source overlays here would make every
    // settlement snapshot recursively carry unrelated processing-tier configuration.
    object.remove("processing_tiers");

    if let Some(tiers) = object.get_mut("tiers").and_then(Value::as_array_mut) {
        for tier in tiers.iter_mut().filter_map(Value::as_object_mut) {
            multiply_price_fields(
                tier,
                &[
                    "input_price_per_1m",
                    "output_price_per_1m",
                    "cache_creation_price_per_1m",
                    "cache_read_price_per_1m",
                ],
                multiplier,
            )?;
            if let Some(ttl_prices) = tier
                .get_mut("cache_ttl_pricing")
                .and_then(Value::as_array_mut)
            {
                for ttl_price in ttl_prices.iter_mut().filter_map(Value::as_object_mut) {
                    multiply_price_fields(
                        ttl_price,
                        &["cache_creation_price_per_1m", "cache_read_price_per_1m"],
                        multiplier,
                    )?;
                }
            }
        }
    }

    multiply_price_fields(
        object,
        &["image_output_price_default", "image_price_default"],
        multiplier,
    )?;
    for key in [
        "image_output_prices",
        "image_output_price_per_image",
        "image_output_price_matrix",
        "image_prices",
    ] {
        if let Some(value) = object.get_mut(key) {
            multiply_image_price_entries(value, multiplier)?;
        }
    }
    if let Some(value) = object.get_mut("image_output_price_ranges") {
        multiply_image_price_ranges(value, multiplier)?;
    }

    Some(multiplied)
}

fn multiply_price_fields(
    object: &mut serde_json::Map<String, Value>,
    fields: &[&str],
    multiplier: f64,
) -> Option<()> {
    for field in fields {
        let Some(value) = object.get_mut(*field) else {
            continue;
        };
        multiply_numeric_price(value, multiplier)?;
    }
    Some(())
}

fn multiply_numeric_price(value: &mut Value, multiplier: f64) -> Option<()> {
    let Some(price) = value.as_f64() else {
        // Existing nulls and extension values keep their legacy meaning. Only numeric prices are
        // materialized, so bounds and unknown metadata are never interpreted as money.
        return Some(());
    };
    let multiplied = price * multiplier;
    if !multiplied.is_finite() {
        return None;
    }
    *value = Value::from(multiplied);
    Some(())
}

fn multiply_image_price_entries(value: &mut Value, multiplier: f64) -> Option<()> {
    match value {
        Value::Number(_) => multiply_numeric_price(value, multiplier)?,
        Value::Object(entries) => {
            for entry in entries.values_mut() {
                if entry.is_number() {
                    multiply_numeric_price(entry, multiplier)?;
                    continue;
                }
                if let Some(prices) = entry.as_object_mut() {
                    for price in prices.values_mut().filter(|price| price.is_number()) {
                        multiply_numeric_price(price, multiplier)?;
                    }
                }
            }
        }
        Value::Array(entries) => {
            for entry in entries.iter_mut().filter_map(Value::as_object_mut) {
                multiply_price_fields(entry, &["price_per_image", "price", "cost"], multiplier)?;
            }
        }
        _ => {}
    }
    Some(())
}

fn multiply_image_price_ranges(value: &mut Value, multiplier: f64) -> Option<()> {
    match value {
        Value::Array(ranges) => {
            for range in ranges.iter_mut().filter_map(Value::as_object_mut) {
                multiply_image_price_range(range, multiplier)?;
            }
        }
        Value::Object(ranges) => {
            for range in ranges.values_mut().filter_map(Value::as_object_mut) {
                multiply_image_price_range(range, multiplier)?;
            }
        }
        _ => {}
    }
    Some(())
}

fn multiply_image_price_range(
    range: &mut serde_json::Map<String, Value>,
    multiplier: f64,
) -> Option<()> {
    if let Some(prices) = range.get_mut("prices").and_then(Value::as_object_mut) {
        for price in prices.values_mut().filter(|price| price.is_number()) {
            multiply_numeric_price(price, multiplier)?;
        }
    }
    multiply_price_fields(
        range,
        &["low", "medium", "high", "price_per_image", "price", "value"],
        multiplier,
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{BillingModelPricingSnapshot, BillingPricingSource};

    fn snapshot(
        model_tiered_pricing: Option<serde_json::Value>,
        default_tiered_pricing: Option<serde_json::Value>,
    ) -> BillingModelPricingSnapshot {
        BillingModelPricingSnapshot {
            provider_id: "provider-1".to_string(),
            provider_billing_type: None,
            provider_api_key_id: None,
            provider_api_key_rate_multipliers: None,
            provider_api_key_cache_ttl_minutes: None,
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_config: None,
            default_price_per_request: None,
            default_tiered_pricing,
            model_id: Some("model-1".to_string()),
            model_provider_model_name: Some("gpt-5-upstream".to_string()),
            model_config: None,
            model_price_per_request: None,
            model_tiered_pricing,
        }
    }

    #[test]
    fn empty_provider_tiered_pricing_inherits_global_default() {
        let default_pricing =
            json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0}]});
        let pricing = snapshot(Some(json!({})), Some(default_pricing.clone()));

        let resolution = pricing.resolve_pricing(None, None);
        assert_eq!(resolution.tiered_pricing, Some(default_pricing.clone()));
        assert_eq!(
            resolution.tiered_pricing_source,
            Some(BillingPricingSource::GlobalDefault)
        );

        let pricing = snapshot(Some(json!({"tiers": []})), Some(default_pricing.clone()));

        let resolution = pricing.resolve_pricing(None, None);
        assert_eq!(resolution.tiered_pricing, Some(default_pricing));
        assert_eq!(
            resolution.tiered_pricing_source,
            Some(BillingPricingSource::GlobalDefault)
        );
    }

    #[test]
    fn populated_provider_tiered_pricing_overrides_global_default() {
        let provider_pricing =
            json!({"tiers":[{"up_to":null,"input_price_per_1m":1.0,"output_price_per_1m":2.0}]});
        let default_pricing =
            json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0}]});
        let pricing = snapshot(Some(provider_pricing.clone()), Some(default_pricing));

        let resolution = pricing.resolve_pricing(None, None);
        assert_eq!(resolution.tiered_pricing, Some(provider_pricing));
        assert_eq!(
            resolution.tiered_pricing_source,
            Some(BillingPricingSource::ProviderOverride)
        );
        assert_eq!(resolution.processing_tier_price_multiplier, None);
    }

    #[test]
    fn missing_processing_overlay_inherits_effective_standard_catalog() {
        let provider_pricing = json!({"tiers":[{"up_to":null,"input_price_per_1m":2.0}]});
        let default_pricing = json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0}]});

        let provider = snapshot(
            Some(provider_pricing.clone()),
            Some(default_pricing.clone()),
        )
        .resolve_pricing(Some("fast"), None);
        assert_eq!(provider.billing_processing_tier.as_deref(), Some("fast"));
        assert_eq!(provider.tiered_pricing, Some(provider_pricing));
        assert_eq!(
            provider.tiered_pricing_source,
            Some(BillingPricingSource::ProviderOverride)
        );
        assert_eq!(provider.processing_tier_price_multiplier, None);

        let global =
            snapshot(None, Some(default_pricing.clone())).resolve_pricing(Some("priority"), None);
        assert_eq!(global.billing_processing_tier.as_deref(), Some("priority"));
        assert_eq!(global.tiered_pricing, Some(default_pricing));
        assert_eq!(
            global.tiered_pricing_source,
            Some(BillingPricingSource::GlobalDefault)
        );
        assert_eq!(global.processing_tier_price_multiplier, None);
    }

    #[test]
    fn nonstandard_tier_keeps_fixed_request_price_without_token_catalog() {
        let mut pricing = snapshot(None, None);
        pricing.default_price_per_request = Some(0.02);

        let resolution = pricing.resolve_pricing(Some("fast"), None);
        assert_eq!(resolution.billing_processing_tier.as_deref(), Some("fast"));
        assert_eq!(resolution.tiered_pricing, None);
        assert_eq!(resolution.price_per_request, Some(0.02));
        assert_eq!(
            resolution.price_per_request_source,
            Some(BillingPricingSource::GlobalDefault)
        );
        assert!(pricing
            .resolve_authorization_pricing_candidates(Some("fast"))
            .expect("fixed request pricing should be valid")
            .is_some());
    }

    #[test]
    fn explicit_nonstandard_request_selects_requested_catalog_without_actual_tier() {
        let pricing = snapshot(
            None,
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 3.0}],
                "processing_tiers": {
                    "priority": {"tiers": [{"up_to": 272000, "input_price_per_1m": 6.0}]}
                }
            })),
        );

        let resolution = pricing.resolve_pricing(Some("Priority"), None);

        assert!(!resolution.requires_actual_processing_tier());
        assert_eq!(
            resolution.requested_processing_tier.as_deref(),
            Some("priority")
        );
        assert_eq!(
            resolution.billing_processing_tier.as_deref(),
            Some("priority")
        );
        assert_eq!(
            resolution
                .tiered_pricing
                .as_ref()
                .and_then(|value| value.pointer("/tiers/0/input_price_per_1m"))
                .and_then(serde_json::Value::as_f64),
            Some(6.0)
        );
    }

    #[test]
    fn response_actual_tier_is_audited_but_does_not_select_pricing_catalog() {
        let pricing = snapshot(
            Some(json!({
                "processing_tiers": {
                    "priority": {"tiers": [{"up_to": 272000, "input_price_per_1m": 9.0}]}
                }
            })),
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 3.0}],
                "processing_tiers": {
                    "flex": {"tiers": [{"up_to": null, "input_price_per_1m": 1.5}]}
                }
            })),
        );

        let flex = pricing.resolve_pricing(Some("priority"), Some("flex"));
        assert_eq!(flex.actual_processing_tier.as_deref(), Some("flex"));
        assert_eq!(flex.billing_processing_tier.as_deref(), Some("priority"));
        assert_eq!(
            flex.tiered_pricing_source,
            Some(BillingPricingSource::ProviderOverride)
        );
        assert_eq!(
            flex.tiered_pricing
                .as_ref()
                .and_then(|value| value.pointer("/tiers/0/input_price_per_1m"))
                .and_then(serde_json::Value::as_f64),
            Some(9.0)
        );

        let standard = pricing.resolve_pricing(Some("priority"), Some("Default"));
        assert_eq!(standard.actual_processing_tier.as_deref(), Some("default"));
        assert_eq!(
            standard.billing_processing_tier.as_deref(),
            Some("priority")
        );
        assert_eq!(
            standard.tiered_pricing_source,
            Some(BillingPricingSource::ProviderOverride)
        );
    }

    #[test]
    fn tiered_and_fixed_price_sources_are_recorded_independently() {
        let mut pricing = snapshot(
            None,
            Some(json!({"tiers": [{"up_to": null, "input_price_per_1m": 3.0}]})),
        );
        pricing.model_price_per_request = Some(0.02);

        let resolution = pricing.resolve_pricing(None, None);

        assert_eq!(
            resolution.tiered_pricing_source,
            Some(BillingPricingSource::GlobalDefault)
        );
        assert_eq!(
            resolution.price_per_request_source,
            Some(BillingPricingSource::ProviderOverride)
        );
        assert_eq!(resolution.pricing_source(), "mixed");
    }

    #[test]
    fn authorization_candidates_include_requested_catalog_without_inventing_actual_tier() {
        let pricing = snapshot(
            None,
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 3.0}],
                "processing_tiers": {
                    "priority": {"tiers": [{"up_to": 272000, "input_price_per_1m": 6.0}]}
                }
            })),
        );

        let candidates = pricing
            .resolve_authorization_pricing_candidates(Some("Priority"))
            .expect("authorization catalogs should resolve")
            .expect("authorization catalogs should be configured");
        let resolution = candidates
            .iter()
            .find(|resolution| resolution.billing_processing_tier.as_deref() == Some("priority"))
            .expect("priority catalog should be included");

        assert_eq!(
            resolution.requested_processing_tier.as_deref(),
            Some("priority")
        );
        assert_eq!(resolution.actual_processing_tier, None);
        assert_eq!(
            resolution.billing_processing_tier.as_deref(),
            Some("priority")
        );
        assert_eq!(
            resolution
                .tiered_pricing
                .as_ref()
                .and_then(|value| value.pointer("/tiers/0/input_price_per_1m"))
                .and_then(serde_json::Value::as_f64),
            Some(6.0)
        );
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn processing_multiplier_materializes_known_prices_without_touching_bounds_or_extensions() {
        let mut pricing = snapshot(
            Some(json!({
                "tiers": [{
                    "up_to": 272000,
                    "input_price_per_1m": 2.0,
                    "output_price_per_1m": 4.0,
                    "cache_creation_price_per_1m": 2.5,
                    "cache_read_price_per_1m": 0.2,
                    "cache_ttl_pricing": [{
                        "ttl_minutes": 60,
                        "cache_creation_price_per_1m": 5.0,
                        "cache_read_price_per_1m": null,
                        "future_ttl_option": 17
                    }],
                    "future_tier_option": 23
                }],
                "image_output_price_default": 0.01,
                "image_output_price_per_image": 0.05,
                "image_output_prices": {
                    "default": 0.02,
                    "1024x1024": {"high": 0.03}
                },
                "image_output_price_ranges": [{
                    "up_to_pixels": 1048576,
                    "prices": {"medium": 0.04},
                    "future_range_option": 29
                }],
                "future_catalog_option": 31,
                "processing_tiers": {
                    "priority": {"price_multiplier": 2.5}
                }
            })),
            None,
        );
        pricing.model_price_per_request = Some(0.02);

        let resolution = pricing.resolve_pricing(Some("priority"), Some("priority"));
        let catalog = resolution
            .tiered_pricing
            .as_ref()
            .expect("multiplier should materialize the Standard catalog");

        assert_eq!(resolution.price_per_request, Some(0.02));
        assert_eq!(
            resolution.tiered_pricing_source,
            Some(BillingPricingSource::ProviderOverride)
        );
        assert_eq!(resolution.processing_tier_price_multiplier, Some(2.5));
        assert_eq!(catalog.pointer("/tiers/0/up_to"), Some(&json!(272000)));
        assert_eq!(
            catalog.pointer("/tiers/0/input_price_per_1m"),
            Some(&json!(5.0))
        );
        assert_eq!(
            catalog.pointer("/tiers/0/output_price_per_1m"),
            Some(&json!(10.0))
        );
        assert_eq!(
            catalog.pointer("/tiers/0/cache_creation_price_per_1m"),
            Some(&json!(6.25))
        );
        assert_eq!(
            catalog.pointer("/tiers/0/cache_read_price_per_1m"),
            Some(&json!(0.5))
        );
        assert_eq!(
            catalog.pointer("/tiers/0/cache_ttl_pricing/0/ttl_minutes"),
            Some(&json!(60))
        );
        assert_eq!(
            catalog.pointer("/tiers/0/cache_ttl_pricing/0/cache_creation_price_per_1m"),
            Some(&json!(12.5))
        );
        assert_eq!(
            catalog.pointer("/tiers/0/cache_ttl_pricing/0/cache_read_price_per_1m"),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(
            catalog.pointer("/image_output_price_default"),
            Some(&json!(0.025))
        );
        assert_eq!(
            catalog.pointer("/image_output_price_per_image"),
            Some(&json!(0.125))
        );
        assert_eq!(
            catalog.pointer("/image_output_prices/default"),
            Some(&json!(0.05))
        );
        assert_eq!(
            catalog.pointer("/image_output_prices/1024x1024/high"),
            Some(&json!(0.075))
        );
        assert_eq!(
            catalog.pointer("/image_output_price_ranges/0/up_to_pixels"),
            Some(&json!(1048576))
        );
        assert_eq!(
            catalog.pointer("/image_output_price_ranges/0/prices/medium"),
            Some(&json!(0.1))
        );
        assert!(catalog.get("processing_tiers").is_none());
        for pointer in [
            "/tiers/0/future_tier_option",
            "/tiers/0/cache_ttl_pricing/0/future_ttl_option",
            "/image_output_price_ranges/0/future_range_option",
            "/future_catalog_option",
        ] {
            assert_eq!(
                catalog.pointer(pointer),
                pricing
                    .model_tiered_pricing
                    .as_ref()
                    .and_then(|value| value.pointer(pointer)),
                "unknown field changed at {pointer}"
            );
        }
    }

    #[test]
    fn processing_overlay_precedence_is_provider_explicit_multiplier_then_global() {
        let provider_explicit = snapshot(
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 2.0}],
                "processing_tiers": {
                    "priority": {
                        "price_multiplier": "invalid-but-shadowed",
                        "tiers": [{"up_to": null, "input_price_per_1m": 9.0}]
                    }
                }
            })),
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 3.0}],
                "processing_tiers": {
                    "priority": {"tiers": [{"up_to": null, "input_price_per_1m": 7.0}]}
                }
            })),
        );
        let resolved = provider_explicit.resolve_pricing(Some("priority"), Some("priority"));
        assert_eq!(
            resolved
                .tiered_pricing
                .as_ref()
                .and_then(|value| value.pointer("/tiers/0/input_price_per_1m")),
            Some(&json!(9.0))
        );
        assert_eq!(
            resolved.tiered_pricing_source,
            Some(BillingPricingSource::ProviderOverride)
        );
        assert_eq!(resolved.processing_tier_price_multiplier, None);

        let provider_multiplier = snapshot(
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 2.0}],
                "processing_tiers": {"priority": {"price_multiplier": 3.0}}
            })),
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 3.0}],
                "processing_tiers": {
                    "priority": {"tiers": [{"up_to": null, "input_price_per_1m": 7.0}]}
                }
            })),
        );
        let resolved = provider_multiplier.resolve_pricing(Some("priority"), Some("priority"));
        assert_eq!(
            resolved
                .tiered_pricing
                .as_ref()
                .and_then(|value| value.pointer("/tiers/0/input_price_per_1m")),
            Some(&json!(6.0))
        );
        assert_eq!(
            resolved.tiered_pricing_source,
            Some(BillingPricingSource::ProviderOverride)
        );
        assert_eq!(resolved.processing_tier_price_multiplier, Some(3.0));

        let global_explicit = snapshot(
            Some(json!({"tiers": [{"up_to": null, "input_price_per_1m": 2.0}]})),
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 3.0}],
                "processing_tiers": {
                    "priority": {
                        "price_multiplier": 4.0,
                        "tiers": [{"up_to": null, "input_price_per_1m": 7.0}]
                    }
                }
            })),
        );
        let resolved = global_explicit.resolve_pricing(Some("priority"), Some("priority"));
        assert_eq!(
            resolved
                .tiered_pricing
                .as_ref()
                .and_then(|value| value.pointer("/tiers/0/input_price_per_1m")),
            Some(&json!(7.0))
        );
        assert_eq!(
            resolved.tiered_pricing_source,
            Some(BillingPricingSource::GlobalDefault)
        );
        assert_eq!(resolved.processing_tier_price_multiplier, None);

        let global_multiplier = snapshot(
            Some(json!({"tiers": [{"up_to": null, "input_price_per_1m": 2.0}]})),
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 3.0}],
                "processing_tiers": {"priority": {"price_multiplier": 4.0}}
            })),
        );
        let resolved = global_multiplier.resolve_pricing(Some("priority"), Some("priority"));
        assert_eq!(
            resolved
                .tiered_pricing
                .as_ref()
                .and_then(|value| value.pointer("/tiers/0/input_price_per_1m")),
            Some(&json!(8.0)),
            "global multiplier must apply to the effective provider Standard catalog"
        );
        assert_eq!(
            resolved.tiered_pricing_source,
            Some(BillingPricingSource::Mixed)
        );
        assert_eq!(resolved.processing_tier_price_multiplier, Some(4.0));
        assert_eq!(resolved.pricing_source(), "mixed");

        let provider_multiplier_with_global_standard = snapshot(
            Some(json!({
                "processing_tiers": {"priority": {"price_multiplier": 3.0}}
            })),
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 3.0}]
            })),
        );
        let resolved = provider_multiplier_with_global_standard
            .resolve_pricing(Some("priority"), Some("priority"));
        assert_eq!(
            resolved
                .tiered_pricing
                .as_ref()
                .and_then(|value| value.pointer("/tiers/0/input_price_per_1m")),
            Some(&json!(9.0))
        );
        assert_eq!(
            resolved.tiered_pricing_source,
            Some(BillingPricingSource::Mixed)
        );
        assert_eq!(resolved.processing_tier_price_multiplier, Some(3.0));
        assert_eq!(resolved.pricing_source(), "mixed");
    }

    #[test]
    fn requested_claude_fast_uses_the_exact_provider_multiplier_overlay() {
        let pricing = snapshot(
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 2.0}],
                "processing_tiers": {"fast": {"price_multiplier": 2.0}}
            })),
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 3.0}],
                "processing_tiers": {"fast": {"price_multiplier": 9.0}}
            })),
        );

        let resolved = pricing.resolve_pricing(Some("fast"), Some("priority"));

        assert_eq!(resolved.actual_processing_tier.as_deref(), Some("priority"));
        assert_eq!(resolved.billing_processing_tier.as_deref(), Some("fast"));
        assert_eq!(
            resolved
                .tiered_pricing
                .as_ref()
                .and_then(|value| value.pointer("/tiers/0/input_price_per_1m")),
            Some(&json!(4.0))
        );
        assert_eq!(
            resolved.tiered_pricing_source,
            Some(BillingPricingSource::ProviderOverride)
        );
        assert_eq!(resolved.processing_tier_price_multiplier, Some(2.0));
    }

    #[test]
    fn invalid_processing_multiplier_blocks_lower_precedence_fallback() {
        let pricing = snapshot(
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 2.0}],
                "processing_tiers": {"priority": {"price_multiplier": "2.5"}}
            })),
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 3.0}],
                "processing_tiers": {
                    "priority": {"tiers": [{"up_to": null, "input_price_per_1m": 7.0}]}
                }
            })),
        );

        let resolution = pricing.resolve_pricing(Some("priority"), Some("priority"));
        assert_eq!(
            resolution.billing_processing_tier.as_deref(),
            Some("priority")
        );
        assert_eq!(resolution.tiered_pricing, None);
        assert_eq!(resolution.tiered_pricing_source, None);
        assert_eq!(resolution.processing_tier_price_multiplier, None);
    }

    #[test]
    fn authorization_candidates_include_multiplier_only_processing_catalog() {
        let pricing = snapshot(
            None,
            Some(json!({
                "tiers": [{"up_to": null, "input_price_per_1m": 3.0}],
                "processing_tiers": {"priority": {"price_multiplier": 2.5}}
            })),
        );

        let candidates = pricing
            .resolve_authorization_pricing_candidates(Some("priority"))
            .expect("multiplier-only processing catalog should authorize")
            .expect("multiplier-only processing catalog should be configured");
        let priority = candidates
            .iter()
            .find(|resolution| resolution.billing_processing_tier.as_deref() == Some("priority"))
            .expect("priority candidate should exist");

        assert_eq!(
            priority
                .tiered_pricing
                .as_ref()
                .and_then(|value| value.pointer("/tiers/0/input_price_per_1m")),
            Some(&json!(7.5))
        );
        assert_eq!(
            priority.tiered_pricing_source,
            Some(BillingPricingSource::GlobalDefault)
        );
        assert_eq!(priority.processing_tier_price_multiplier, Some(2.5));
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingUsageInput {
    pub task_type: String,
    pub api_format: Option<String>,
    #[serde(default)]
    pub requested_processing_tier: Option<String>,
    #[serde(default)]
    pub actual_processing_tier: Option<String>,
    pub request_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_creation_ephemeral_5m_tokens: i64,
    pub cache_creation_ephemeral_1h_tokens: i64,
    pub cache_read_tokens: i64,
    pub image_count: i64,
    pub image_size: Option<String>,
    pub image_quality: Option<String>,
    pub image_output_format: Option<String>,
    pub cache_ttl_minutes: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingAuthorizationEstimateInput {
    pub task_type: String,
    pub api_format: Option<String>,
    pub requested_processing_tier: Option<String>,
    #[serde(default)]
    pub cache_ttl_minutes: Option<i64>,
    pub input_tokens: i64,
    pub max_output_tokens: Option<i64>,
}

impl BillingAuthorizationEstimateInput {
    pub fn new(task_type: impl Into<String>, input_tokens: i64) -> Self {
        Self {
            task_type: task_type.into(),
            api_format: None,
            requested_processing_tier: None,
            cache_ttl_minutes: None,
            input_tokens: input_tokens.max(0),
            max_output_tokens: None,
        }
    }
}

impl BillingUsageInput {
    pub fn new(task_type: impl Into<String>) -> Self {
        Self {
            task_type: task_type.into(),
            api_format: None,
            requested_processing_tier: None,
            actual_processing_tier: None,
            request_count: 1,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_creation_ephemeral_5m_tokens: 0,
            cache_creation_ephemeral_1h_tokens: 0,
            cache_read_tokens: 0,
            image_count: 0,
            image_size: None,
            image_quality: None,
            image_output_format: None,
            cache_ttl_minutes: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingComputation {
    pub cost_result: crate::CostResult,
    pub actual_total_cost: f64,
    pub rate_multiplier: f64,
    pub is_free_tier: bool,
    pub pricing_resolution: BillingPricingResolution,
}
