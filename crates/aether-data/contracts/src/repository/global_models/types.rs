use async_trait::async_trait;
use serde_json::Value;

const EMBEDDING_CAPABILITY: &str = "embedding";
const EMBEDDING_API_FORMATS: &[&str] = &[
    "openai:embedding",
    "gemini:embedding",
    "jina:embedding",
    "doubao:embedding",
    "aliyun:multimodal_embedding",
    "/v1/embeddings",
    "/jina/v1/embeddings",
];

fn validate_optional_price(
    field_name: &str,
    value: Option<f64>,
) -> Result<(), crate::DataLayerError> {
    if value.is_some_and(|price| !price.is_finite() || price < 0.0) {
        return Err(crate::DataLayerError::UnexpectedValue(format!(
            "{field_name} must be a non-negative finite number"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplicitPricingCatalogState {
    Absent,
    Valid,
    Invalid,
}

const TOKEN_PRICE_FIELDS: &[&str] = &[
    "input_price_per_1m",
    "output_price_per_1m",
    "cache_creation_price_per_1m",
    "cache_read_price_per_1m",
];

const IMAGE_MATRIX_PRICE_FIELDS: &[&str] = &[
    "image_output_prices",
    "image_output_price_per_image",
    "image_output_price_matrix",
    "image_prices",
];

/// Classifies whether a JSON object contains a complete pricing catalog understood by the
/// default billing runtime.
///
/// `Absent` deliberately differs from `Invalid`: a provider override containing only
/// `processing_tiers` may inherit its Standard catalog, while a present-but-malformed catalog
/// must never shadow a valid lower-precedence catalog or silently bill as zero.
pub fn explicit_pricing_catalog_state(value: &Value) -> ExplicitPricingCatalogState {
    let Some(object) = value.as_object() else {
        return ExplicitPricingCatalogState::Invalid;
    };

    let mut has_catalog_field = false;
    let mut has_valid_price_data = false;

    if let Some(tiers) = object.get("tiers") {
        if tiers.is_null() {
            // Null and [] both represent an inherited/absent Standard catalog in legacy rows.
        } else {
            let Some(tiers) = tiers.as_array() else {
                return ExplicitPricingCatalogState::Invalid;
            };
            // An empty provider Standard list is the legacy representation for "inherit the global
            // catalog". It is not an explicit catalog and therefore cannot shadow a multiplier or a
            // lower-precedence Standard catalog.
            if !tiers.is_empty() {
                has_catalog_field = true;
                if !token_pricing_tiers_are_valid(tiers) {
                    return ExplicitPricingCatalogState::Invalid;
                }
                has_valid_price_data = true;
            }
        }
    }

    for key in ["image_output_price_default", "image_price_default"] {
        let Some(price) = object.get(key) else {
            continue;
        };
        if price.is_null() {
            continue;
        }
        has_catalog_field = true;
        if !value_is_valid_price(price) {
            return ExplicitPricingCatalogState::Invalid;
        }
        has_valid_price_data = true;
    }

    for key in IMAGE_MATRIX_PRICE_FIELDS {
        let Some(prices) = object.get(*key) else {
            continue;
        };
        let Some(has_prices) = validate_image_matrix_prices(key, prices) else {
            return ExplicitPricingCatalogState::Invalid;
        };
        if has_prices {
            has_catalog_field = true;
            has_valid_price_data = true;
        }
    }

    if let Some(ranges) = object.get("image_output_price_ranges") {
        let Some(has_prices) = validate_image_price_ranges(ranges) else {
            return ExplicitPricingCatalogState::Invalid;
        };
        if has_prices {
            has_catalog_field = true;
            has_valid_price_data = true;
        }
    }

    match (has_catalog_field, has_valid_price_data) {
        (false, _) => ExplicitPricingCatalogState::Absent,
        (true, true) => ExplicitPricingCatalogState::Valid,
        (true, false) => ExplicitPricingCatalogState::Invalid,
    }
}

fn token_pricing_tiers_are_valid(tiers: &[Value]) -> bool {
    let mut previous_up_to = None;
    for (index, tier) in tiers.iter().enumerate() {
        let Some(tier) = tier.as_object() else {
            return false;
        };
        let up_to = match tier.get("up_to") {
            None | Some(Value::Null) => None,
            Some(value) => match nonnegative_i64(value) {
                Some(value) => Some(value),
                None => return false,
            },
        };
        if index + 1 < tiers.len() && up_to.is_none() {
            return false;
        }
        if let (Some(previous), Some(current)) = (previous_up_to, up_to) {
            if current <= previous {
                return false;
            }
        }
        previous_up_to = up_to;

        let mut has_tier_price = false;
        for field in TOKEN_PRICE_FIELDS {
            let Some(price) = tier.get(*field) else {
                continue;
            };
            let Some(is_declared) = validate_optional_price_value(price) else {
                return false;
            };
            has_tier_price |= is_declared;
        }
        if !has_tier_price {
            return false;
        }

        if let Some(ttl_pricing) = tier.get("cache_ttl_pricing") {
            let Some(entries) = ttl_pricing.as_array() else {
                return false;
            };
            for entry in entries {
                let Some(entry) = entry.as_object() else {
                    return false;
                };
                if entry
                    .get("ttl_minutes")
                    .is_some_and(|value| nonnegative_i64(value).is_none())
                {
                    return false;
                }
                for field in ["cache_creation_price_per_1m", "cache_read_price_per_1m"] {
                    if let Some(price) = entry.get(field) {
                        if validate_optional_price_value(price).is_none() {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}

fn validate_image_matrix_prices(field: &str, value: &Value) -> Option<bool> {
    match value {
        Value::Object(entries) => {
            let mut has_price = false;
            for (key, entry) in entries {
                if key.eq_ignore_ascii_case("default") {
                    if field == "image_output_prices" {
                        has_price |= validate_optional_price_value(entry)?;
                    } else if entry.is_number() {
                        return None;
                    }
                    continue;
                }
                match entry {
                    Value::Number(_) => {
                        if !value_is_valid_price(entry) {
                            return None;
                        }
                        let has_reachable_flat_key = key
                            .split_once(':')
                            .or_else(|| key.split_once('|'))
                            .is_some_and(|(size, quality)| {
                                !size.trim().is_empty() && !quality.trim().is_empty()
                            });
                        if !has_reachable_flat_key {
                            return None;
                        }
                        has_price = true;
                    }
                    Value::Object(nested) => {
                        for (nested_key, price) in nested {
                            if key.trim().is_empty() || nested_key.trim().is_empty() {
                                return None;
                            }
                            match price {
                                Value::Number(_) if value_is_valid_price(price) => {
                                    has_price = true;
                                }
                                Value::Number(_) => return None,
                                Value::Null => {}
                                _ => {}
                            }
                        }
                    }
                    Value::Null => {}
                    _ => {}
                }
            }
            Some(has_price)
        }
        Value::Array(entries) => {
            let mut has_price = false;
            for entry in entries {
                let Some(entry) = entry.as_object() else {
                    continue;
                };
                if entry
                    .get("size")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .is_none_or(str::is_empty)
                {
                    continue;
                }
                let Some(price) = entry
                    .get("price_per_image")
                    .or_else(|| entry.get("price"))
                    .or_else(|| entry.get("cost"))
                else {
                    continue;
                };
                let is_declared = validate_optional_price_value(price)?;
                has_price |= is_declared;
            }
            Some(has_price)
        }
        // The runtime only treats object/array matrix shapes as price catalogs. Preserve scalar
        // extension values without letting them make an otherwise empty catalog authoritative.
        _ => Some(false),
    }
}

fn validate_image_price_ranges(value: &Value) -> Option<bool> {
    let (ranges, allow_direct_prices) = match value {
        Value::Array(ranges) => (ranges.iter().collect::<Vec<_>>(), true),
        Value::Object(ranges) => (ranges.values().collect::<Vec<_>>(), false),
        Value::Null => return Some(false),
        _ => return None,
    };
    let mut has_price = false;
    for range in ranges {
        let range = range.as_object()?;
        for key in ["up_to_pixels", "up_to", "max_pixels"] {
            if let Some(value) = range.get(key) {
                if !value.is_null() && nonnegative_i64(value).is_none() {
                    return None;
                }
            }
        }

        let mut range_has_price = false;
        let mut has_price_field = false;
        if let Some(prices) = range.get("prices") {
            let prices = prices.as_object().filter(|prices| !prices.is_empty())?;
            for price in prices.values() {
                has_price_field = true;
                let is_declared = match price {
                    Value::Number(_) | Value::Null => validate_optional_price_value(price)?,
                    _ => false,
                };
                range_has_price |= is_declared;
            }
        } else if allow_direct_prices {
            for key in ["low", "medium", "high", "price_per_image", "price", "value"] {
                let Some(price) = range.get(key) else {
                    continue;
                };
                has_price_field = true;
                let is_declared = validate_optional_price_value(price)?;
                range_has_price |= is_declared;
            }
        }
        if !range_has_price {
            if !has_price_field {
                return None;
            }
            continue;
        }
        has_price = true;
    }
    Some(has_price)
}

fn value_is_valid_price(value: &Value) -> bool {
    value
        .as_f64()
        .is_some_and(|price| price.is_finite() && price >= 0.0)
}

fn validate_optional_price_value(value: &Value) -> Option<bool> {
    if value.is_null() {
        return Some(false);
    }
    value_is_valid_price(value).then_some(true)
}

fn nonnegative_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .filter(|value| *value >= 0)
}

fn validate_processing_tier_price_multipliers(
    field_name: &str,
    tiered_pricing: Option<&Value>,
) -> Result<(), crate::DataLayerError> {
    let Some(tiered_pricing) = tiered_pricing else {
        return Ok(());
    };
    let Some(processing_tiers_value) = tiered_pricing.get("processing_tiers") else {
        return Ok(());
    };
    if processing_tiers_value.is_null() {
        return Ok(());
    }
    let Some(processing_tiers) = processing_tiers_value.as_object() else {
        return Err(crate::DataLayerError::UnexpectedValue(format!(
            "{field_name}.processing_tiers must be an object"
        )));
    };

    let standard_catalog_state = explicit_pricing_catalog_state(tiered_pricing);

    for (tier_name, overlay) in processing_tiers {
        let canonical_tier_name =
            crate::repository::usage::normalize_provider_service_tier(tier_name);
        if canonical_tier_name.as_deref() != Some(tier_name.as_str()) {
            return Err(crate::DataLayerError::UnexpectedValue(format!(
                "{field_name}.processing_tiers tier name `{tier_name}` must be canonical lowercase without surrounding whitespace"
            )));
        }
        let Some(overlay) = overlay.as_object().map(|_| overlay) else {
            return Err(crate::DataLayerError::UnexpectedValue(format!(
                "{field_name}.processing_tiers.{tier_name} must be an object"
            )));
        };

        // Settlement gives an explicit catalog precedence over a multiplier. Keep accepting a
        // stale or future multiplier beside an authoritative explicit catalog for compatibility.
        match explicit_pricing_catalog_state(overlay) {
            ExplicitPricingCatalogState::Valid => continue,
            ExplicitPricingCatalogState::Invalid => {
                return Err(crate::DataLayerError::UnexpectedValue(format!(
                    "{field_name}.processing_tiers.{tier_name} explicit catalog must contain valid non-negative finite token or image prices"
                )));
            }
            ExplicitPricingCatalogState::Absent => {}
        }

        let Some(multiplier) = overlay.get("price_multiplier") else {
            return Err(crate::DataLayerError::UnexpectedValue(format!(
                "{field_name}.processing_tiers.{tier_name} must contain an explicit catalog or price_multiplier"
            )));
        };
        if multiplier
            .as_f64()
            .is_none_or(|multiplier| !multiplier.is_finite() || multiplier < 0.0)
        {
            return Err(crate::DataLayerError::UnexpectedValue(format!(
                "{field_name}.processing_tiers.{tier_name}.price_multiplier must be a non-negative finite number"
            )));
        }
        if field_name == "global_models.default_tiered_pricing"
            && standard_catalog_state != ExplicitPricingCatalogState::Valid
        {
            return Err(crate::DataLayerError::UnexpectedValue(format!(
                "{field_name}.processing_tiers.{tier_name}.price_multiplier requires a valid Standard pricing catalog"
            )));
        }
    }

    Ok(())
}

fn validate_embedding_global_billing(
    default_price_per_request: Option<f64>,
    default_tiered_pricing: Option<&Value>,
    supported_capabilities: Option<&Value>,
    config: Option<&Value>,
) -> Result<(), crate::DataLayerError> {
    validate_optional_price(
        "global_models.default_price_per_request",
        default_price_per_request,
    )?;
    if !has_embedding_metadata(supported_capabilities, config) {
        return Ok(());
    }
    if has_request_or_input_token_pricing(default_price_per_request, default_tiered_pricing) {
        return Ok(());
    }
    Err(crate::DataLayerError::UnexpectedValue(
        "embedding global model requires default_price_per_request or default_tiered_pricing.tiers[].input_price_per_1m".to_string(),
    ))
}

fn validate_provider_model_pricing(
    price_per_request: Option<f64>,
) -> Result<(), crate::DataLayerError> {
    validate_optional_price("models.price_per_request", price_per_request)
}

fn has_request_or_input_token_pricing(
    price_per_request: Option<f64>,
    tiered_pricing: Option<&Value>,
) -> bool {
    price_per_request.is_some_and(|price| price.is_finite() && price >= 0.0)
        || tiered_pricing
            .and_then(|value| value.get("tiers"))
            .and_then(Value::as_array)
            .is_some_and(|tiers| {
                tiers.iter().any(|tier| {
                    tier.get("input_price_per_1m")
                        .and_then(Value::as_f64)
                        .is_some_and(|price| price.is_finite() && price >= 0.0)
                })
            })
}

fn has_embedding_metadata(supported_capabilities: Option<&Value>, config: Option<&Value>) -> bool {
    metadata_supports_embedding(supported_capabilities, config, None).unwrap_or(false)
}

/// Derives embedding support from global capabilities and both global/model metadata.
///
/// `Some(false)` is intentional: callers use this value to distinguish a completed
/// metadata decision from an absent database column.
pub fn metadata_supports_embedding(
    supported_capabilities: Option<&Value>,
    global_config: Option<&Value>,
    model_config: Option<&Value>,
) -> Option<bool> {
    Some(
        supported_capabilities.is_some_and(value_contains_embedding_capability)
            || global_config.is_some_and(value_contains_embedding_metadata)
            || model_config.is_some_and(value_contains_embedding_metadata),
    )
}

fn value_contains_embedding_capability(value: &Value) -> bool {
    match value {
        Value::String(value) => value.trim().eq_ignore_ascii_case(EMBEDDING_CAPABILITY),
        Value::Array(values) => values.iter().any(value_contains_embedding_capability),
        Value::Object(object) => {
            object
                .get(EMBEDDING_CAPABILITY)
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || [
                    "capability",
                    "model_type",
                    "type",
                    "task_type",
                    "request_type",
                ]
                .iter()
                .any(|key| {
                    object
                        .get(*key)
                        .is_some_and(value_contains_embedding_capability)
                })
                || ["capabilities", "supported_capabilities"]
                    .iter()
                    .any(|key| {
                        object
                            .get(*key)
                            .is_some_and(value_contains_embedding_capability)
                    })
        }
        _ => false,
    }
}

fn value_contains_embedding_metadata(value: &Value) -> bool {
    match value {
        Value::String(value) => {
            value.trim().eq_ignore_ascii_case(EMBEDDING_CAPABILITY)
                || is_known_embedding_api_format(value)
        }
        Value::Array(values) => values.iter().any(value_contains_embedding_metadata),
        Value::Object(object) => {
            value_contains_embedding_capability(value)
                || ["api_format", "client_api_format", "provider_api_format"]
                    .iter()
                    .any(|key| {
                        object
                            .get(*key)
                            .and_then(Value::as_str)
                            .is_some_and(is_known_embedding_api_format)
                    })
                || ["api_formats", "client_api_formats", "provider_api_formats"]
                    .iter()
                    .any(|key| {
                        object
                            .get(*key)
                            .is_some_and(value_contains_embedding_metadata)
                    })
        }
        _ => false,
    }
}

fn is_known_embedding_api_format(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    EMBEDDING_API_FORMATS
        .iter()
        .any(|api_format| normalized == *api_format || normalized.ends_with(*api_format))
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredPublicGlobalModel {
    pub id: String,
    pub name: String,
    pub display_name: Option<String>,
    pub is_active: bool,
    pub default_price_per_request: Option<f64>,
    pub default_tiered_pricing: Option<Value>,
    pub supported_capabilities: Option<Value>,
    pub config: Option<Value>,
    pub usage_count: u64,
}

impl StoredPublicGlobalModel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        name: String,
        display_name: Option<String>,
        is_active: bool,
        default_price_per_request: Option<f64>,
        default_tiered_pricing: Option<Value>,
        supported_capabilities: Option<Value>,
        config: Option<Value>,
        usage_count: u64,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.id is empty".to_string(),
            ));
        }
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.name is empty".to_string(),
            ));
        }
        validate_embedding_global_billing(
            default_price_per_request,
            default_tiered_pricing.as_ref(),
            supported_capabilities.as_ref(),
            config.as_ref(),
        )?;

        Ok(Self {
            id,
            name,
            display_name,
            is_active,
            default_price_per_request,
            default_tiered_pricing,
            supported_capabilities,
            config,
            usage_count,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PublicGlobalModelQuery {
    pub offset: usize,
    pub limit: usize,
    pub is_active: Option<bool>,
    pub search: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredPublicCatalogModel {
    pub id: String,
    pub provider_id: String,
    pub provider_name: String,
    pub provider_model_name: String,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub icon_url: Option<String>,
    pub input_price_per_1m: Option<f64>,
    pub output_price_per_1m: Option<f64>,
    pub cache_creation_price_per_1m: Option<f64>,
    pub cache_read_price_per_1m: Option<f64>,
    pub supports_vision: Option<bool>,
    pub supports_function_calling: Option<bool>,
    pub supports_streaming: Option<bool>,
    pub supports_embedding: Option<bool>,
    pub is_active: bool,
}

impl StoredPublicCatalogModel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        provider_id: String,
        provider_name: String,
        provider_model_name: String,
        name: String,
        display_name: String,
        description: Option<String>,
        icon_url: Option<String>,
        input_price_per_1m: Option<f64>,
        output_price_per_1m: Option<f64>,
        cache_creation_price_per_1m: Option<f64>,
        cache_read_price_per_1m: Option<f64>,
        supports_vision: Option<bool>,
        supports_function_calling: Option<bool>,
        supports_streaming: Option<bool>,
        supports_embedding: Option<bool>,
        is_active: bool,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.id is empty".to_string(),
            ));
        }
        if provider_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.provider_id is empty".to_string(),
            ));
        }
        if provider_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "providers.name is empty".to_string(),
            ));
        }
        if provider_model_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.provider_model_name is empty".to_string(),
            ));
        }
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "public model name is empty".to_string(),
            ));
        }
        if display_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "public model display_name is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            provider_id,
            provider_name,
            provider_model_name,
            name,
            display_name,
            description,
            icon_url,
            input_price_per_1m,
            output_price_per_1m,
            cache_creation_price_per_1m,
            cache_read_price_per_1m,
            supports_vision,
            supports_function_calling,
            supports_streaming,
            supports_embedding,
            is_active,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PublicCatalogModelListQuery {
    pub provider_id: Option<String>,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicCatalogModelSearchQuery {
    pub search: String,
    pub provider_id: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminProviderModelListQuery {
    pub provider_id: String,
    pub is_active: Option<bool>,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminGlobalModel {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub is_active: bool,
    pub default_price_per_request: Option<f64>,
    pub default_tiered_pricing: Option<Value>,
    pub supported_capabilities: Option<Value>,
    pub config: Option<Value>,
    pub provider_count: u64,
    pub active_provider_count: u64,
    pub usage_count: u64,
    pub created_at_unix_ms: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
}

impl StoredAdminGlobalModel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        name: String,
        display_name: String,
        is_active: bool,
        default_price_per_request: Option<f64>,
        default_tiered_pricing: Option<Value>,
        supported_capabilities: Option<Value>,
        config: Option<Value>,
        provider_count: u64,
        active_provider_count: u64,
        usage_count: u64,
        created_at_unix_ms: Option<u64>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.id is empty".to_string(),
            ));
        }
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.name is empty".to_string(),
            ));
        }
        if display_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.display_name is empty".to_string(),
            ));
        }
        validate_embedding_global_billing(
            default_price_per_request,
            default_tiered_pricing.as_ref(),
            supported_capabilities.as_ref(),
            config.as_ref(),
        )?;

        Ok(Self {
            id,
            name,
            display_name,
            is_active,
            default_price_per_request,
            default_tiered_pricing,
            supported_capabilities,
            config,
            provider_count,
            active_provider_count,
            usage_count,
            created_at_unix_ms,
            updated_at_unix_secs,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminGlobalModelListQuery {
    pub offset: usize,
    pub limit: usize,
    pub is_active: Option<bool>,
    pub search: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminProviderModel {
    pub id: String,
    pub provider_id: String,
    pub global_model_id: String,
    pub provider_model_name: String,
    pub provider_model_mappings: Option<Value>,
    pub price_per_request: Option<f64>,
    pub tiered_pricing: Option<Value>,
    pub supports_vision: Option<bool>,
    pub supports_function_calling: Option<bool>,
    pub supports_streaming: Option<bool>,
    pub supports_extended_thinking: Option<bool>,
    pub supports_image_generation: Option<bool>,
    pub is_active: bool,
    pub is_available: bool,
    pub config: Option<Value>,
    pub created_at_unix_ms: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
    pub global_model_name: Option<String>,
    pub global_model_display_name: Option<String>,
    pub global_model_default_price_per_request: Option<f64>,
    pub global_model_default_tiered_pricing: Option<Value>,
    pub global_model_supported_capabilities: Option<Value>,
    pub global_model_config: Option<Value>,
}

impl StoredAdminProviderModel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        provider_id: String,
        global_model_id: String,
        provider_model_name: String,
        provider_model_mappings: Option<Value>,
        price_per_request: Option<f64>,
        tiered_pricing: Option<Value>,
        supports_vision: Option<bool>,
        supports_function_calling: Option<bool>,
        supports_streaming: Option<bool>,
        supports_extended_thinking: Option<bool>,
        supports_image_generation: Option<bool>,
        is_active: bool,
        is_available: bool,
        config: Option<Value>,
        created_at_unix_ms: Option<u64>,
        updated_at_unix_secs: Option<u64>,
        global_model_name: Option<String>,
        global_model_display_name: Option<String>,
        global_model_default_price_per_request: Option<f64>,
        global_model_default_tiered_pricing: Option<Value>,
        global_model_supported_capabilities: Option<Value>,
        global_model_config: Option<Value>,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.id is empty".to_string(),
            ));
        }
        if provider_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.provider_id is empty".to_string(),
            ));
        }
        if global_model_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.global_model_id is empty".to_string(),
            ));
        }
        if provider_model_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.provider_model_name is empty".to_string(),
            ));
        }
        validate_provider_model_pricing(price_per_request)?;

        Ok(Self {
            id,
            provider_id,
            global_model_id,
            provider_model_name,
            provider_model_mappings,
            price_per_request,
            tiered_pricing,
            supports_vision,
            supports_function_calling,
            supports_streaming,
            supports_extended_thinking,
            supports_image_generation,
            is_active,
            is_available,
            config,
            created_at_unix_ms,
            updated_at_unix_secs,
            global_model_name,
            global_model_display_name,
            global_model_default_price_per_request,
            global_model_default_tiered_pricing,
            global_model_supported_capabilities,
            global_model_config,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpsertAdminProviderModelRecord {
    pub id: String,
    pub provider_id: String,
    pub global_model_id: String,
    pub provider_model_name: String,
    pub provider_model_mappings: Option<Value>,
    pub price_per_request: Option<f64>,
    pub tiered_pricing: Option<Value>,
    pub supports_vision: Option<bool>,
    pub supports_function_calling: Option<bool>,
    pub supports_streaming: Option<bool>,
    pub supports_extended_thinking: Option<bool>,
    pub supports_image_generation: Option<bool>,
    pub is_active: bool,
    pub is_available: bool,
    pub config: Option<Value>,
}

impl UpsertAdminProviderModelRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        provider_id: String,
        global_model_id: String,
        provider_model_name: String,
        provider_model_mappings: Option<Value>,
        price_per_request: Option<f64>,
        tiered_pricing: Option<Value>,
        supports_vision: Option<bool>,
        supports_function_calling: Option<bool>,
        supports_streaming: Option<bool>,
        supports_extended_thinking: Option<bool>,
        supports_image_generation: Option<bool>,
        is_active: bool,
        is_available: bool,
        config: Option<Value>,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.id is empty".to_string(),
            ));
        }
        if provider_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.provider_id is empty".to_string(),
            ));
        }
        if global_model_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.global_model_id is empty".to_string(),
            ));
        }
        if provider_model_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.provider_model_name is empty".to_string(),
            ));
        }
        validate_provider_model_pricing(price_per_request)?;
        validate_processing_tier_price_multipliers(
            "models.tiered_pricing",
            tiered_pricing.as_ref(),
        )?;

        Ok(Self {
            id,
            provider_id,
            global_model_id,
            provider_model_name,
            provider_model_mappings,
            price_per_request,
            tiered_pricing,
            supports_vision,
            supports_function_calling,
            supports_streaming,
            supports_extended_thinking,
            supports_image_generation,
            is_active,
            is_available,
            config,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateAdminGlobalModelRecord {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub is_active: bool,
    pub default_price_per_request: Option<f64>,
    pub default_tiered_pricing: Option<Value>,
    pub supported_capabilities: Option<Value>,
    pub config: Option<Value>,
    #[serde(default)]
    pub usage_count: Option<u64>,
}

impl CreateAdminGlobalModelRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        name: String,
        display_name: String,
        is_active: bool,
        default_price_per_request: Option<f64>,
        default_tiered_pricing: Option<Value>,
        supported_capabilities: Option<Value>,
        config: Option<Value>,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.id is empty".to_string(),
            ));
        }
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.name is empty".to_string(),
            ));
        }
        if display_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.display_name is empty".to_string(),
            ));
        }
        validate_embedding_global_billing(
            default_price_per_request,
            default_tiered_pricing.as_ref(),
            supported_capabilities.as_ref(),
            config.as_ref(),
        )?;
        validate_processing_tier_price_multipliers(
            "global_models.default_tiered_pricing",
            default_tiered_pricing.as_ref(),
        )?;

        Ok(Self {
            id,
            name,
            display_name,
            is_active,
            default_price_per_request,
            default_tiered_pricing,
            supported_capabilities,
            config,
            usage_count: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpdateAdminGlobalModelRecord {
    pub id: String,
    pub display_name: String,
    pub is_active: bool,
    pub default_price_per_request: Option<f64>,
    pub default_tiered_pricing: Option<Value>,
    pub supported_capabilities: Option<Value>,
    pub config: Option<Value>,
    #[serde(default)]
    pub usage_count: Option<u64>,
}

impl UpdateAdminGlobalModelRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        display_name: String,
        is_active: bool,
        default_price_per_request: Option<f64>,
        default_tiered_pricing: Option<Value>,
        supported_capabilities: Option<Value>,
        config: Option<Value>,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.id is empty".to_string(),
            ));
        }
        if display_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.display_name is empty".to_string(),
            ));
        }
        validate_embedding_global_billing(
            default_price_per_request,
            default_tiered_pricing.as_ref(),
            supported_capabilities.as_ref(),
            config.as_ref(),
        )?;
        validate_processing_tier_price_multipliers(
            "global_models.default_tiered_pricing",
            default_tiered_pricing.as_ref(),
        )?;

        Ok(Self {
            id,
            display_name,
            is_active,
            default_price_per_request,
            default_tiered_pricing,
            supported_capabilities,
            config,
            usage_count: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredPublicGlobalModelPage {
    pub items: Vec<StoredPublicGlobalModel>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminGlobalModelPage {
    pub items: Vec<StoredAdminGlobalModel>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderModelStats {
    pub provider_id: String,
    pub total_models: u64,
    pub active_models: u64,
}

impl StoredProviderModelStats {
    pub fn new(
        provider_id: String,
        total_models: i64,
        active_models: i64,
    ) -> Result<Self, crate::DataLayerError> {
        if provider_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider model stats provider_id is empty".to_string(),
            ));
        }
        if total_models < 0 || active_models < 0 {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider model stats count is negative".to_string(),
            ));
        }
        Ok(Self {
            provider_id,
            total_models: total_models as u64,
            active_models: active_models as u64,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderActiveGlobalModel {
    pub provider_id: String,
    pub global_model_id: String,
}

impl StoredProviderActiveGlobalModel {
    pub fn new(
        provider_id: String,
        global_model_id: String,
    ) -> Result<Self, crate::DataLayerError> {
        if provider_id.trim().is_empty() || global_model_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider active global model identity is empty".to_string(),
            ));
        }
        Ok(Self {
            provider_id,
            global_model_id,
        })
    }
}

#[async_trait]
pub trait GlobalModelReadRepository: Send + Sync {
    async fn list_public_models(
        &self,
        query: &PublicGlobalModelQuery,
    ) -> Result<StoredPublicGlobalModelPage, crate::DataLayerError>;

    async fn get_public_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredPublicGlobalModel>, crate::DataLayerError>;

    async fn list_public_catalog_models(
        &self,
        query: &PublicCatalogModelListQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, crate::DataLayerError>;

    async fn search_public_catalog_models(
        &self,
        query: &PublicCatalogModelSearchQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, crate::DataLayerError>;

    async fn list_admin_global_models(
        &self,
        query: &AdminGlobalModelListQuery,
    ) -> Result<StoredAdminGlobalModelPage, crate::DataLayerError>;

    async fn list_admin_provider_models(
        &self,
        query: &AdminProviderModelListQuery,
    ) -> Result<Vec<StoredAdminProviderModel>, crate::DataLayerError>;

    async fn list_admin_provider_available_source_models(
        &self,
        provider_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, crate::DataLayerError>;

    async fn get_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Option<StoredAdminProviderModel>, crate::DataLayerError>;

    async fn get_admin_global_model_by_id(
        &self,
        global_model_id: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, crate::DataLayerError>;

    async fn get_admin_global_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, crate::DataLayerError>;

    async fn list_admin_provider_models_by_global_model_id(
        &self,
        global_model_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, crate::DataLayerError>;

    async fn list_provider_model_stats(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderModelStats>, crate::DataLayerError>;

    async fn list_active_global_model_ids_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderActiveGlobalModel>, crate::DataLayerError>;
}

#[async_trait]
pub trait GlobalModelWriteRepository: Send + Sync {
    async fn create_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, crate::DataLayerError>;

    async fn update_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, crate::DataLayerError>;

    async fn delete_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn create_admin_global_model(
        &self,
        record: &CreateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, crate::DataLayerError>;

    async fn update_admin_global_model(
        &self,
        record: &UpdateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, crate::DataLayerError>;

    async fn delete_admin_global_model(
        &self,
        global_model_id: &str,
    ) -> Result<bool, crate::DataLayerError>;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        explicit_pricing_catalog_state, CreateAdminGlobalModelRecord, ExplicitPricingCatalogState,
        UpdateAdminGlobalModelRecord, UpsertAdminProviderModelRecord,
    };

    #[test]
    fn embedding_missing_billing_config_rejected() {
        let err = CreateAdminGlobalModelRecord::new(
            "gm-embedding".to_string(),
            "text-embedding-3-small".to_string(),
            "Text Embedding 3 Small".to_string(),
            true,
            None,
            None,
            Some(json!(["embedding"])),
            None,
        )
        .expect_err("embedding model without explicit billing should be rejected");

        assert!(err.to_string().contains(
            "embedding global model requires default_price_per_request or default_tiered_pricing.tiers[].input_price_per_1m"
        ));
    }

    #[test]
    fn embedding_api_format_config_requires_billing_config() {
        let err = CreateAdminGlobalModelRecord::new(
            "gm-embedding".to_string(),
            "jina-embeddings-v3".to_string(),
            "Jina Embeddings v3".to_string(),
            true,
            None,
            Some(json!({"tiers": []})),
            None,
            Some(json!({"api_formats": ["jina:embedding"]})),
        )
        .expect_err("embedding API format without price should be rejected");

        assert!(err.to_string().contains("embedding global model requires"));
    }

    #[test]
    fn embedding_input_token_or_request_pricing_is_accepted() {
        CreateAdminGlobalModelRecord::new(
            "gm-embedding-input".to_string(),
            "text-embedding-3-small".to_string(),
            "Text Embedding 3 Small".to_string(),
            true,
            None,
            Some(json!({"tiers":[{"up_to":null,"input_price_per_1m":0.02}]})),
            Some(json!(["embedding"])),
            Some(json!({"dimensions": 1536})),
        )
        .expect("input-token pricing should satisfy embedding billing");

        CreateAdminGlobalModelRecord::new(
            "gm-embedding-request".to_string(),
            "custom-embedding".to_string(),
            "Custom Embedding".to_string(),
            true,
            Some(0.0),
            None,
            Some(json!(["embedding"])),
            None,
        )
        .expect("explicit request pricing should satisfy embedding billing");
    }

    #[test]
    fn provider_model_negative_request_price_rejected() {
        let err = UpsertAdminProviderModelRecord::new(
            "model-1".to_string(),
            "provider-1".to_string(),
            "global-model-1".to_string(),
            "text-embedding-3-small".to_string(),
            None,
            Some(-0.01),
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            true,
            None,
        )
        .expect_err("negative provider model request price should be rejected");

        assert!(err
            .to_string()
            .contains("models.price_per_request must be a non-negative finite number"));
    }

    #[test]
    fn global_model_invalid_processing_tier_multiplier_rejected_on_create_and_update() {
        let invalid_pricing = json!({
            "tiers": [{"input_price_per_1m": 1.0, "output_price_per_1m": 2.0}],
            "processing_tiers": {
                "fast": {"price_multiplier": -1.0}
            }
        });

        let create_err = CreateAdminGlobalModelRecord::new(
            "global-model-1".to_string(),
            "model-1".to_string(),
            "Model 1".to_string(),
            true,
            None,
            Some(invalid_pricing.clone()),
            None,
            None,
        )
        .expect_err("create should reject a negative processing-tier multiplier");
        assert!(create_err.to_string().contains(
            "global_models.default_tiered_pricing.processing_tiers.fast.price_multiplier must be a non-negative finite number"
        ));

        let update_err = UpdateAdminGlobalModelRecord::new(
            "global-model-1".to_string(),
            "Model 1".to_string(),
            true,
            None,
            Some(invalid_pricing),
            None,
            None,
        )
        .expect_err("update should reject a negative processing-tier multiplier");
        assert!(update_err.to_string().contains(
            "global_models.default_tiered_pricing.processing_tiers.fast.price_multiplier must be a non-negative finite number"
        ));
    }

    #[test]
    fn provider_model_invalid_processing_tier_multiplier_rejected() {
        let err = UpsertAdminProviderModelRecord::new(
            "model-1".to_string(),
            "provider-1".to_string(),
            "global-model-1".to_string(),
            "provider-model-1".to_string(),
            None,
            None,
            Some(json!({
                "processing_tiers": {
                    "flex": {"price_multiplier": "0.5"}
                }
            })),
            None,
            None,
            None,
            None,
            None,
            true,
            true,
            None,
        )
        .expect_err("provider model should reject a non-numeric processing-tier multiplier");

        assert!(err.to_string().contains(
            "models.tiered_pricing.processing_tiers.flex.price_multiplier must be a non-negative finite number"
        ));
    }

    #[test]
    fn explicit_processing_tier_catalog_takes_precedence_over_stale_multiplier() {
        let pricing = json!({
            "tiers": [{"input_price_per_1m": 1.0, "output_price_per_1m": 2.0}],
            "processing_tiers": {
                "fast": {
                    "tiers": [{"input_price_per_1m": 2.0, "output_price_per_1m": 4.0}],
                    "price_multiplier": "stale"
                }
            }
        });

        CreateAdminGlobalModelRecord::new(
            "global-model-1".to_string(),
            "model-1".to_string(),
            "Model 1".to_string(),
            true,
            None,
            Some(pricing.clone()),
            None,
            None,
        )
        .expect("an explicit global processing-tier catalog should shadow a stale multiplier");

        UpsertAdminProviderModelRecord::new(
            "model-1".to_string(),
            "provider-1".to_string(),
            "global-model-1".to_string(),
            "provider-model-1".to_string(),
            None,
            None,
            Some(pricing),
            None,
            None,
            None,
            None,
            None,
            true,
            true,
            None,
        )
        .expect("an explicit provider processing-tier catalog should shadow a stale multiplier");
    }

    #[test]
    fn non_negative_processing_tier_multipliers_are_accepted() {
        let pricing = json!({
            "tiers": [{"input_price_per_1m": 1.0, "output_price_per_1m": 2.0}],
            "processing_tiers": {
                "flex": {"price_multiplier": 0.0},
                "fast": {"price_multiplier": 2.5}
            }
        });

        CreateAdminGlobalModelRecord::new(
            "global-model-1".to_string(),
            "model-1".to_string(),
            "Model 1".to_string(),
            true,
            None,
            Some(pricing),
            None,
            None,
        )
        .expect("finite non-negative processing-tier multipliers should be accepted");
    }

    #[test]
    fn empty_explicit_processing_catalog_cannot_hide_an_invalid_multiplier() {
        let err = CreateAdminGlobalModelRecord::new(
            "global-model-1".to_string(),
            "model-1".to_string(),
            "Model 1".to_string(),
            true,
            None,
            Some(json!({
                "tiers": [{"input_price_per_1m": 1.0}],
                "processing_tiers": {
                    "priority": {
                        "tiers": [{}],
                        "price_multiplier": -1.0
                    }
                }
            })),
            None,
            None,
        )
        .expect_err("an empty explicit catalog must not bypass processing-tier validation");

        assert!(err
            .to_string()
            .contains("processing_tiers.priority explicit catalog must contain valid"));
    }

    #[test]
    fn processing_catalog_accepts_input_only_and_image_only_prices() {
        CreateAdminGlobalModelRecord::new(
            "global-model-1".to_string(),
            "model-1".to_string(),
            "Model 1".to_string(),
            true,
            None,
            Some(json!({
                "tiers": [{"input_price_per_1m": 1.0}],
                "processing_tiers": {
                    "embedding": {
                        "tiers": [{
                            "up_to": null,
                            "input_price_per_1m": 0.1,
                            "output_price_per_1m": null
                        }]
                    },
                    "image": {"image_output_price_default": 0.04}
                }
            })),
            None,
            None,
        )
        .expect("input-only and image-only processing catalogs are billable");
    }

    #[test]
    fn global_multiplier_requires_a_valid_standard_catalog() {
        let err = CreateAdminGlobalModelRecord::new(
            "global-model-1".to_string(),
            "model-1".to_string(),
            "Model 1".to_string(),
            true,
            None,
            Some(json!({
                "processing_tiers": {
                    "priority": {"price_multiplier": 2.0}
                }
            })),
            None,
            None,
        )
        .expect_err("a global multiplier without Standard prices cannot be materialized");

        assert!(err
            .to_string()
            .contains("price_multiplier requires a valid Standard pricing catalog"));
    }

    #[test]
    fn nullable_processing_tiers_remains_an_empty_compatible_value() {
        CreateAdminGlobalModelRecord::new(
            "global-model-1".to_string(),
            "model-1".to_string(),
            "Model 1".to_string(),
            true,
            None,
            Some(json!({
                "tiers": [{"input_price_per_1m": 1.0}],
                "processing_tiers": null
            })),
            None,
            None,
        )
        .expect("null processing_tiers should keep its legacy empty meaning");
    }

    #[test]
    fn noncanonical_processing_tier_name_is_rejected() {
        let err = CreateAdminGlobalModelRecord::new(
            "global-model-1".to_string(),
            "model-1".to_string(),
            "Model 1".to_string(),
            true,
            None,
            Some(json!({
                "tiers": [{"input_price_per_1m": 1.0}],
                "processing_tiers": {
                    " Priority ": {"price_multiplier": 2.0}
                }
            })),
            None,
            None,
        )
        .expect_err("noncanonical processing-tier keys are not resolvable at runtime");

        assert!(err
            .to_string()
            .contains("must be canonical lowercase without surrounding whitespace"));
    }

    #[test]
    fn image_catalog_requires_a_runtime_reachable_price_key() {
        for unreachable in [
            json!({"image_prices": {"default": 0.1}}),
            json!({"image_output_prices": {"1024x1024": 0.1}}),
        ] {
            assert_ne!(
                explicit_pricing_catalog_state(&unreachable),
                ExplicitPricingCatalogState::Valid
            );
        }

        for reachable in [
            json!({"image_output_prices": {"default": 0.1}}),
            json!({"image_output_prices": {"1024x1024:high": 0.1}}),
            json!({"image_prices": {"1024x1024": {"high": 0.1}}}),
        ] {
            assert_eq!(
                explicit_pricing_catalog_state(&reachable),
                ExplicitPricingCatalogState::Valid
            );
        }
    }
}
