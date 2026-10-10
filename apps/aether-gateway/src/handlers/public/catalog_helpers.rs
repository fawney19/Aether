use crate::api::ai::public_api_format_local_path;
use crate::handlers::shared::{query_param_value, unix_ms_to_rfc3339, unix_secs_to_rfc3339};
use crate::provider_key_auth::{
    provider_key_configured_api_formats, provider_key_effective_api_formats,
};
use crate::AppState;
use aether_data_contracts::repository::candidates::{
    sanitize_request_candidate_error_type, PublicHealthTimelineBucket, RequestCandidateStatus,
    StoredRequestCandidate,
};
use aether_data_contracts::repository::global_models::{
    PublicCatalogModelListQuery, PublicCatalogModelSearchQuery, StoredPublicCatalogModel,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::usage::{
    StoredRequestUsageAudit, StoredUsageBreakdownSummaryRow, StoredUsageHealthTimelineRow,
    UsageAuditListQuery, UsageBreakdownGroupBy, UsageBreakdownSummaryQuery,
    UsageHealthTimelineGroupBy, UsageHealthTimelineQuery,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

const USER_CANCELLED_STATUS_CODE: u16 = 499;

#[derive(Clone, Copy, Debug, Default)]
struct HealthTimelineMetricBucket {
    total_count: u64,
    success_count: u64,
    failed_count: u64,
    latency_sum_ms: u64,
    latency_samples: u64,
    first_byte_sum_ms: u64,
    first_byte_samples: u64,
    output_tokens: u64,
    response_time_sum_ms: u64,
}

#[derive(Clone, Copy, Debug)]
struct HealthTimelineDetailCounts {
    status: &'static str,
    total_attempts: u64,
    success_count: u64,
    failed_count: u64,
}

#[derive(Clone, Copy, Debug)]
struct HealthTimelineWindow {
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
}

impl HealthTimelineMetricBucket {
    fn add_usage_event(&mut self, event: &StoredRequestUsageAudit) {
        self.total_count = self.total_count.saturating_add(1);
        if model_health_event_success(event) {
            self.success_count = self.success_count.saturating_add(1);
        } else {
            self.failed_count = self.failed_count.saturating_add(1);
        }
        if let Some(response_time_ms) = event.response_time_ms {
            self.latency_sum_ms = self.latency_sum_ms.saturating_add(response_time_ms);
            self.latency_samples = self.latency_samples.saturating_add(1);
            self.response_time_sum_ms = self.response_time_sum_ms.saturating_add(response_time_ms);
        }
        if let Some(first_byte_time_ms) = event.first_byte_time_ms {
            self.first_byte_sum_ms = self.first_byte_sum_ms.saturating_add(first_byte_time_ms);
            self.first_byte_samples = self.first_byte_samples.saturating_add(1);
        }
        self.output_tokens = self.output_tokens.saturating_add(event.output_tokens);
    }

    fn avg_latency_ms(self) -> Option<f64> {
        if self.latency_samples == 0 {
            None
        } else {
            Some(self.latency_sum_ms as f64 / self.latency_samples as f64)
        }
    }

    fn avg_first_byte_ms(self) -> Option<f64> {
        if self.first_byte_samples == 0 {
            None
        } else {
            Some(self.first_byte_sum_ms as f64 / self.first_byte_samples as f64)
        }
    }

    fn avg_tps(self) -> Option<f64> {
        if self.output_tokens == 0 || self.response_time_sum_ms == 0 {
            None
        } else {
            Some(self.output_tokens as f64 / (self.response_time_sum_ms as f64 / 1000.0))
        }
    }
}

pub(crate) fn request_candidate_status_label(status: RequestCandidateStatus) -> &'static str {
    match status {
        RequestCandidateStatus::Available => "available",
        RequestCandidateStatus::Unused => "unused",
        RequestCandidateStatus::Pending => "pending",
        RequestCandidateStatus::Streaming => "streaming",
        RequestCandidateStatus::Success => "success",
        RequestCandidateStatus::Failed => "failed",
        RequestCandidateStatus::Cancelled => "cancelled",
        RequestCandidateStatus::Skipped => "skipped",
    }
}

pub(crate) fn request_candidate_event_unix_ms(candidate: &StoredRequestCandidate) -> u64 {
    candidate
        .finished_at_unix_ms
        .or(candidate.started_at_unix_ms)
        .unwrap_or(candidate.created_at_unix_ms)
}

pub(crate) fn normalize_admin_base_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err("base_url 不能为空".to_string());
    }
    let mut parsed = url::Url::parse(trimmed).map_err(|_| "base_url 不是有效 URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("URL 必须以 http:// 或 https:// 开头".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("base_url 必须包含有效主机".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("base_url 不允许包含用户名或密码".to_string());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("base_url 不允许包含查询参数或片段".to_string());
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    let normalized = parsed.to_string();
    Ok(normalized.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod normalize_admin_base_url_tests {
    use super::normalize_admin_base_url;

    #[test]
    fn endpoint_base_url_rejects_embedded_credentials_and_hidden_suffixes() {
        for value in [
            "https://user:password@api.example.test/v1",
            "https://api.example.test/v1?key=secret",
            "https://api.example.test/v1#secret",
            "http://user:password@api.example.test/v1",
            "http://api.example.test/v1?key=secret",
            "http://api.example.test/v1#secret",
            "ftp://api.example.test/v1",
            "file:///v1",
            "api.example.test/v1",
            "",
            "https://",
            "http://",
            "https://api.example.test:invalid/v1",
        ] {
            assert!(normalize_admin_base_url(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn endpoint_base_url_accepts_remote_http_hosts() {
        for (raw_url, expected) in [
            (
                " HTTP://API.EXAMPLE.TEST:8080/v1/ ",
                "http://api.example.test:8080/v1",
            ),
            ("http://8.8.8.8:8080/v1/", "http://8.8.8.8:8080/v1"),
            ("http://10.0.0.1:8080/v1/", "http://10.0.0.1:8080/v1"),
            (
                "http://[2606:4700:4700::1111]:8080/v1/",
                "http://[2606:4700:4700::1111]:8080/v1",
            ),
        ] {
            assert_eq!(
                normalize_admin_base_url(raw_url).expect("HTTP base URL should be accepted"),
                expected,
            );
        }
    }

    #[test]
    fn endpoint_base_url_is_parsed_and_normalized() {
        assert_eq!(
            normalize_admin_base_url(" HTTPS://API.EXAMPLE.TEST/v1/ ").expect("valid base URL"),
            "https://api.example.test/v1"
        );
        assert_eq!(
            normalize_admin_base_url("http://127.0.0.1:18181/v1/")
                .expect("literal loopback HTTP should remain available"),
            "http://127.0.0.1:18181/v1"
        );
        assert_eq!(
            normalize_admin_base_url("http://[::1]:18181/v1/")
                .expect("IPv6 loopback HTTP should remain available"),
            "http://[::1]:18181/v1"
        );
    }
}

pub(crate) fn sanitize_public_model_config_for_user(
    config: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let Some(config) = config else {
        return None;
    };
    let object = config.as_object()?;
    let mut public = serde_json::Map::new();

    if let Some(description) = object
        .get("description")
        .and_then(serde_json::Value::as_str)
    {
        public.insert(
            "description".to_string(),
            serde_json::Value::String(description.to_string()),
        );
    }
    for key in [
        "streaming",
        "image_generation",
        "vision",
        "function_calling",
        "extended_thinking",
        "embedding",
        "rerank",
    ] {
        if let Some(value) = object.get(key).and_then(serde_json::Value::as_bool) {
            public.insert(key.to_string(), serde_json::Value::Bool(value));
        }
    }
    for key in ["model_type", "type"] {
        if let Some(value) = object.get(key).and_then(serde_json::Value::as_str) {
            public.insert(
                key.to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
    }
    for key in ["api_formats", "capabilities", "supported_capabilities"] {
        if let Some(values) = public_string_array(object.get(key)) {
            public.insert(key.to_string(), serde_json::Value::Array(values));
        }
    }
    if let Some(video_billing) = public_video_billing(object.get("billing")) {
        public.insert("billing".to_string(), video_billing);
    }

    (!public.is_empty()).then(|| serde_json::Value::Object(public))
}

pub(crate) fn sanitize_public_model_capabilities(
    capabilities: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    public_string_array(capabilities.as_ref()).map(serde_json::Value::Array)
}

pub(crate) fn sanitize_public_tiered_pricing(
    pricing: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let pricing = pricing?.as_object()?.clone();
    let mut public = serde_json::Map::new();

    if let Some(tiers) = pricing.get("tiers").and_then(serde_json::Value::as_array) {
        let tiers = tiers
            .iter()
            .filter_map(public_pricing_tier)
            .collect::<Vec<_>>();
        if !tiers.is_empty() {
            public.insert("tiers".to_string(), serde_json::Value::Array(tiers));
        }
    }
    copy_public_price(&pricing, &mut public, "image_output_price_default", false);
    copy_public_price_matrix(&pricing, &mut public, "image_output_prices");
    copy_public_price_ranges(&pricing, &mut public, "image_output_price_ranges");

    if let Some(processing_tiers) = pricing
        .get("processing_tiers")
        .and_then(serde_json::Value::as_object)
    {
        let processing_tiers = processing_tiers
            .iter()
            .filter_map(|(name, pricing)| {
                let mut pricing_without_nested_tiers = pricing.as_object()?.clone();
                pricing_without_nested_tiers.remove("processing_tiers");
                let mut sanitized = sanitize_public_tiered_pricing(Some(
                    serde_json::Value::Object(pricing_without_nested_tiers),
                ))
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
                let sanitized = sanitized.as_object_mut()?;
                if let Some(multiplier) = pricing
                    .get("price_multiplier")
                    .and_then(nonnegative_finite_number)
                {
                    sanitized.insert("price_multiplier".to_string(), multiplier);
                }
                (!sanitized.is_empty())
                    .then(|| (name.clone(), serde_json::Value::Object(sanitized.clone())))
            })
            .collect::<serde_json::Map<_, _>>();
        if !processing_tiers.is_empty() {
            public.insert(
                "processing_tiers".to_string(),
                serde_json::Value::Object(processing_tiers),
            );
        }
    }

    (!public.is_empty()).then(|| serde_json::Value::Object(public))
}

fn public_string_array(value: Option<&serde_json::Value>) -> Option<Vec<serde_json::Value>> {
    let values = value?.as_array()?;
    let values = values
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(|value| serde_json::Value::String(value.to_string()))
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

fn public_pricing_tier(value: &serde_json::Value) -> Option<serde_json::Value> {
    let value = value.as_object()?;
    let mut public = serde_json::Map::new();
    match value.get("up_to") {
        Some(serde_json::Value::Null) => {
            public.insert("up_to".to_string(), serde_json::Value::Null);
        }
        Some(value) => {
            public.insert("up_to".to_string(), nonnegative_integer_number(value)?);
        }
        None => return None,
    }
    for key in [
        "input_price_per_1m",
        "output_price_per_1m",
        "cache_creation_price_per_1m",
        "cache_read_price_per_1m",
    ] {
        copy_public_price(value, &mut public, key, false);
    }
    if let Some(cache_ttl_pricing) = value
        .get("cache_ttl_pricing")
        .and_then(serde_json::Value::as_array)
    {
        let cache_ttl_pricing = cache_ttl_pricing
            .iter()
            .filter_map(public_cache_ttl_price)
            .collect::<Vec<_>>();
        if !cache_ttl_pricing.is_empty() {
            public.insert(
                "cache_ttl_pricing".to_string(),
                serde_json::Value::Array(cache_ttl_pricing),
            );
        }
    }
    Some(serde_json::Value::Object(public))
}

fn public_cache_ttl_price(value: &serde_json::Value) -> Option<serde_json::Value> {
    let value = value.as_object()?;
    let mut public = serde_json::Map::new();
    public.insert(
        "ttl_minutes".to_string(),
        nonnegative_integer_number(value.get("ttl_minutes")?)?,
    );
    for key in ["cache_creation_price_per_1m", "cache_read_price_per_1m"] {
        copy_public_price(value, &mut public, key, false);
    }
    Some(serde_json::Value::Object(public))
}

fn copy_public_price(
    source: &serde_json::Map<String, serde_json::Value>,
    target: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    allow_null: bool,
) {
    match source.get(key) {
        Some(serde_json::Value::Null) if allow_null => {
            target.insert(key.to_string(), serde_json::Value::Null);
        }
        Some(value) => {
            if let Some(value) = nonnegative_finite_number(value) {
                target.insert(key.to_string(), value);
            }
        }
        None => {}
    }
}

fn copy_public_price_matrix(
    source: &serde_json::Map<String, serde_json::Value>,
    target: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
) {
    let Some(matrix) = source.get(key).and_then(serde_json::Value::as_object) else {
        return;
    };
    let matrix = matrix
        .iter()
        .filter_map(|(size, prices)| {
            let prices = prices.as_object()?;
            let prices = prices
                .iter()
                .filter_map(|(quality, price)| {
                    nonnegative_finite_number(price).map(|price| (quality.clone(), price))
                })
                .collect::<serde_json::Map<_, _>>();
            (!prices.is_empty()).then(|| (size.clone(), serde_json::Value::Object(prices)))
        })
        .collect::<serde_json::Map<_, _>>();
    if !matrix.is_empty() {
        target.insert(key.to_string(), serde_json::Value::Object(matrix));
    }
}

fn copy_public_price_ranges(
    source: &serde_json::Map<String, serde_json::Value>,
    target: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
) {
    let Some(ranges) = source.get(key).and_then(serde_json::Value::as_array) else {
        return;
    };
    let ranges = ranges
        .iter()
        .filter_map(|range| {
            let range = range.as_object()?;
            let mut public = serde_json::Map::new();
            match range.get("up_to_pixels") {
                Some(serde_json::Value::Null) => {
                    public.insert("up_to_pixels".to_string(), serde_json::Value::Null);
                }
                Some(value) => {
                    public.insert(
                        "up_to_pixels".to_string(),
                        nonnegative_integer_number(value)?,
                    );
                }
                None => return None,
            }
            if let Some(label) = range.get("label").and_then(serde_json::Value::as_str) {
                public.insert(
                    "label".to_string(),
                    serde_json::Value::String(label.to_string()),
                );
            }
            let prices = range.get("prices")?.as_object()?;
            let prices = prices
                .iter()
                .filter_map(|(quality, price)| {
                    nonnegative_finite_number(price).map(|price| (quality.clone(), price))
                })
                .collect::<serde_json::Map<_, _>>();
            if prices.is_empty() {
                return None;
            }
            public.insert("prices".to_string(), serde_json::Value::Object(prices));
            Some(serde_json::Value::Object(public))
        })
        .collect::<Vec<_>>();
    if !ranges.is_empty() {
        target.insert(key.to_string(), serde_json::Value::Array(ranges));
    }
}

fn nonnegative_integer_number(value: &serde_json::Value) -> Option<serde_json::Value> {
    value
        .as_u64()
        .map(serde_json::Number::from)
        .map(serde_json::Value::Number)
}

fn nonnegative_finite_number(value: &serde_json::Value) -> Option<serde_json::Value> {
    value
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .and_then(serde_json::Number::from_f64)
        .map(serde_json::Value::Number)
}

fn public_video_billing(value: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    let prices = value?
        .get("video")?
        .get("price_per_second_by_resolution")?
        .as_object()?;
    let prices = prices
        .iter()
        .filter_map(|(resolution, price)| {
            price
                .as_f64()
                .filter(|price| price.is_finite() && *price >= 0.0)
                .map(|price| {
                    (
                        resolution.clone(),
                        serde_json::Number::from_f64(price)
                            .map(serde_json::Value::Number)
                            .expect("finite price should serialize"),
                    )
                })
        })
        .collect::<serde_json::Map<_, _>>();
    if prices.is_empty() {
        return None;
    }
    Some(json!({
        "video": {
            "price_per_second_by_resolution": prices,
        }
    }))
}

pub(crate) fn admin_requested_force_stream(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::String(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "force_stream" | "stream" | "sse" | "true" | "1" | "yes"
        ),
        serde_json::Value::Number(value) => value.as_i64() == Some(1),
        _ => false,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ApiFormatHealthMonitorOptions {
    pub(crate) include_api_path: bool,
    pub(crate) include_provider_count: bool,
    pub(crate) include_key_count: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct ModelHealthMonitorOptions {
    pub(crate) include_provider_count: bool,
}

const API_FORMAT_HEALTH_TIMELINE_SEGMENTS: u32 = 60;
const MODEL_HEALTH_TIMELINE_SEGMENTS: u32 = 60;

pub(crate) fn provider_key_api_formats(key: &StoredProviderCatalogKey) -> Vec<String> {
    provider_key_configured_api_formats(key)
}

pub(crate) async fn build_public_providers_payload(
    state: &AppState,
    query: Option<&str>,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() {
        return None;
    }

    let skip = query_param_value(query, "skip")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = query_param_value(query, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0 && *value <= 1000)
        .unwrap_or(100);

    let mut providers = state
        .list_provider_catalog_providers(true)
        .await
        .ok()
        .unwrap_or_default();
    providers.sort_by(|left, right| {
        left.provider_priority
            .cmp(&right.provider_priority)
            .then_with(|| left.name.cmp(&right.name))
    });
    let providers = providers
        .into_iter()
        .skip(skip)
        .take(limit)
        .collect::<Vec<_>>();
    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let provider_ids_set = provider_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let endpoints = if provider_ids.is_empty() {
        Vec::new()
    } else {
        state
            .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
            .await
            .ok()
            .unwrap_or_default()
    };

    let mut endpoints_count_by_provider = BTreeMap::<String, usize>::new();
    let mut active_endpoints_count_by_provider = BTreeMap::<String, usize>::new();
    let mut api_formats = BTreeSet::<String>::new();
    for endpoint in endpoints.iter().filter(|endpoint| endpoint.is_active) {
        *endpoints_count_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default() += 1;
        *active_endpoints_count_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default() += 1;
        api_formats.insert(endpoint.api_format.clone());
    }

    let mut models_by_provider = BTreeMap::<String, BTreeSet<String>>::new();
    if state.has_minimal_candidate_selection_reader() {
        for api_format in api_formats {
            let rows = state
                .list_minimal_candidate_selection_rows_for_api_format(&api_format)
                .await
                .ok()
                .unwrap_or_default();
            for row in rows {
                if provider_ids_set.contains(row.provider_id.as_str()) {
                    models_by_provider
                        .entry(row.provider_id.clone())
                        .or_default()
                        .insert(row.global_model_id.clone());
                }
            }
        }
    }

    let providers = providers
        .into_iter()
        .map(|provider| {
            let provider_id = provider.id.clone();
            let model_count = models_by_provider
                .get(&provider_id)
                .map(BTreeSet::len)
                .unwrap_or(0);
            json!({
                "id": provider_id.clone(),
                "is_active": provider.is_active,
                "provider_priority": provider.provider_priority,
                "models_count": model_count,
                "active_models_count": model_count,
                "endpoints_count": endpoints_count_by_provider.get(&provider_id).copied().unwrap_or(0),
                "active_endpoints_count": active_endpoints_count_by_provider.get(&provider_id).copied().unwrap_or(0),
            })
        })
        .collect::<Vec<_>>();

    Some(serde_json::Value::Array(providers))
}

fn serialize_public_catalog_model(model: StoredPublicCatalogModel) -> serde_json::Value {
    json!({
        "id": model.id,
        "name": model.name,
        "display_name": model.display_name,
        "description": model.description,
        "tags": serde_json::Value::Null,
        "icon_url": model.icon_url,
        "input_price_per_1m": model.input_price_per_1m,
        "output_price_per_1m": model.output_price_per_1m,
        "cache_creation_price_per_1m": model.cache_creation_price_per_1m,
        "cache_read_price_per_1m": model.cache_read_price_per_1m,
        "supports_vision": model.supports_vision,
        "supports_function_calling": model.supports_function_calling,
        "supports_streaming": model.supports_streaming,
        "supports_embedding": model.supports_embedding,
        "is_active": model.is_active,
    })
}

pub(crate) async fn build_public_catalog_models_payload(
    state: &AppState,
    query: Option<&str>,
) -> Option<serde_json::Value> {
    if !state.has_global_model_data_reader() {
        return None;
    }

    let provider_id = query_param_value(query, "provider_id")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let skip = query_param_value(query, "skip")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = query_param_value(query, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0 && *value <= 1000)
        .unwrap_or(100);

    let items = state
        .list_public_catalog_models(&PublicCatalogModelListQuery {
            provider_id,
            offset: skip,
            limit,
        })
        .await
        .ok()?;

    Some(serde_json::Value::Array(
        items
            .into_iter()
            .map(serialize_public_catalog_model)
            .collect(),
    ))
}

pub(crate) async fn build_public_catalog_search_models_payload(
    state: &AppState,
    query: Option<&str>,
) -> Option<serde_json::Value> {
    if !state.has_global_model_data_reader() {
        return None;
    }

    let search = query_param_value(query, "q")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let provider_id = query_param_value(query, "provider_id")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let limit = query_param_value(query, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0 && *value <= 1000)
        .unwrap_or(20);

    let items = state
        .search_public_catalog_models(&PublicCatalogModelSearchQuery {
            search,
            provider_id,
            limit,
        })
        .await
        .ok()?;

    Some(serde_json::Value::Array(
        items
            .into_iter()
            .map(serialize_public_catalog_model)
            .collect(),
    ))
}

pub(crate) async fn build_api_format_health_monitor_payload(
    state: &AppState,
    lookback_hours: u64,
    per_format_limit: usize,
    options: ApiFormatHealthMonitorOptions,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() || !state.has_request_candidate_data_reader() {
        return None;
    }

    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let since_unix_secs = now_unix_secs.saturating_sub(lookback_hours * 3600);
    let usage_data_available = state.has_usage_data_reader();
    let usage_breakdown_by_format = if usage_data_available {
        state
            .summarize_usage_breakdown(&UsageBreakdownSummaryQuery {
                created_from_unix_secs: since_unix_secs,
                created_until_unix_secs: now_unix_secs,
                user_id: None,
                provider_name: None,
                model: None,
                api_format: None,
                exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
                group_by: UsageBreakdownGroupBy::ApiFormat,
            })
            .await
            .ok()
            .unwrap_or_default()
            .into_iter()
            .filter(|row| !row.group_key.trim().is_empty())
            .map(|row| (row.group_key.clone(), row))
            .collect::<BTreeMap<_, _>>()
    } else {
        BTreeMap::new()
    };

    let providers = state
        .list_provider_catalog_providers(true)
        .await
        .ok()
        .unwrap_or_default();
    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let active_endpoints = if provider_ids.is_empty() {
        Vec::new()
    } else {
        state
            .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
            .await
            .ok()
            .unwrap_or_default()
            .into_iter()
            .filter(|endpoint| endpoint.is_active)
            .collect::<Vec<_>>()
    };

    let mut endpoint_ids_by_format = BTreeMap::<String, Vec<String>>::new();
    let mut endpoint_to_format = BTreeMap::<String, String>::new();
    let mut provider_ids_by_format = BTreeMap::<String, BTreeSet<String>>::new();
    let mut active_endpoints_by_provider = BTreeMap::<String, Vec<_>>::new();
    let provider_type_by_id = providers
        .iter()
        .map(|provider| (provider.id.clone(), provider.provider_type.clone()))
        .collect::<BTreeMap<_, _>>();
    for endpoint in active_endpoints {
        endpoint_to_format.insert(endpoint.id.clone(), endpoint.api_format.clone());
        endpoint_ids_by_format
            .entry(endpoint.api_format.clone())
            .or_default()
            .push(endpoint.id.clone());
        provider_ids_by_format
            .entry(endpoint.api_format.clone())
            .or_default()
            .insert(endpoint.provider_id.clone());
        active_endpoints_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default()
            .push(endpoint);
    }
    let all_endpoint_ids = endpoint_to_format.keys().cloned().collect::<Vec<_>>();

    let mut key_counts_by_format = BTreeMap::<String, usize>::new();
    if options.include_key_count && !provider_ids.is_empty() {
        let keys = state
            .list_provider_catalog_key_summaries_by_provider_ids(&provider_ids)
            .await
            .ok()
            .unwrap_or_default();
        for key in keys.into_iter().filter(|key| key.is_active) {
            let provider_type = provider_type_by_id
                .get(&key.provider_id)
                .map(String::as_str)
                .unwrap_or("");
            let endpoints = active_endpoints_by_provider
                .get(&key.provider_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            for api_format in provider_key_effective_api_formats(&key, provider_type, endpoints) {
                if provider_ids_by_format
                    .get(&api_format)
                    .is_some_and(|provider_ids| provider_ids.contains(key.provider_id.as_str()))
                {
                    *key_counts_by_format.entry(api_format).or_default() += 1;
                }
            }
        }
    }

    let status_counts = state
        .count_finalized_request_candidate_statuses_by_endpoint_ids_since(
            &all_endpoint_ids,
            since_unix_secs,
        )
        .await
        .ok()
        .unwrap_or_default();
    let mut status_totals = BTreeMap::<String, (u64, u64, u64)>::new();
    for row in status_counts {
        let Some(api_format) = endpoint_to_format.get(&row.endpoint_id) else {
            continue;
        };
        let entry = status_totals.entry(api_format.clone()).or_insert((0, 0, 0));
        match row.status {
            RequestCandidateStatus::Success => entry.0 += row.count,
            RequestCandidateStatus::Failed => entry.1 += row.count,
            RequestCandidateStatus::Skipped => entry.2 += row.count,
            _ => {}
        }
    }

    let timeline_rows = state
        .aggregate_finalized_request_candidate_timeline_by_endpoint_ids_since(
            &all_endpoint_ids,
            since_unix_secs,
            now_unix_secs,
            API_FORMAT_HEALTH_TIMELINE_SEGMENTS,
        )
        .await
        .ok()
        .unwrap_or_default();
    let mut timeline_by_format =
        BTreeMap::<String, BTreeMap<u32, PublicHealthTimelineBucket>>::new();
    for row in timeline_rows {
        let Some(api_format) = endpoint_to_format.get(&row.endpoint_id) else {
            continue;
        };
        let bucket = timeline_by_format
            .entry(api_format.clone())
            .or_default()
            .entry(row.segment_idx)
            .or_insert_with(|| PublicHealthTimelineBucket {
                endpoint_id: api_format.clone(),
                segment_idx: row.segment_idx,
                total_count: 0,
                success_count: 0,
                failed_count: 0,
                min_created_at_unix_ms: None,
                max_created_at_unix_ms: None,
            });
        bucket.total_count += row.total_count;
        bucket.success_count += row.success_count;
        bucket.failed_count += row.failed_count;
        bucket.min_created_at_unix_ms =
            match (bucket.min_created_at_unix_ms, row.min_created_at_unix_ms) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (None, Some(right)) => Some(right),
                (left, None) => left,
            };
        bucket.max_created_at_unix_ms =
            match (bucket.max_created_at_unix_ms, row.max_created_at_unix_ms) {
                (Some(left), Some(right)) => Some(left.max(right)),
                (None, Some(right)) => Some(right),
                (left, None) => left,
            };
    }

    let mut formats = Vec::new();
    for (api_format, endpoint_ids) in endpoint_ids_by_format {
        let attempts = state
            .list_finalized_request_candidates_by_endpoint_ids_since(
                &endpoint_ids,
                since_unix_secs,
                per_format_limit,
            )
            .await
            .ok()
            .unwrap_or_default();
        let (success_count, failed_count, skipped_count) =
            status_totals.get(&api_format).copied().unwrap_or((0, 0, 0));
        let total_attempts = success_count + failed_count + skipped_count;
        let actual_completed = success_count + failed_count;
        let success_rate = if actual_completed > 0 {
            success_count as f64 / actual_completed as f64
        } else {
            1.0
        };
        let last_event_at = attempts.first().and_then(|candidate| {
            candidate
                .finished_at_unix_ms
                .or(candidate.started_at_unix_ms)
                .or(Some(candidate.created_at_unix_ms))
        });
        let avg_latency_ms = usage_breakdown_by_format
            .get(&api_format)
            .and_then(model_health_average_latency_ms)
            .or_else(|| request_candidate_average_latency_ms(&attempts));
        let avg_tps = usage_breakdown_by_format
            .get(&api_format)
            .and_then(model_health_average_tps);
        let usage_events = if usage_data_available {
            state
                .list_usage_audits(&UsageAuditListQuery {
                    created_from_unix_secs: Some(since_unix_secs),
                    created_until_unix_secs: Some(now_unix_secs),
                    api_format: Some(api_format.clone()),
                    exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
                    limit: Some(per_format_limit),
                    newest_first: true,
                    ..UsageAuditListQuery::default()
                })
                .await
                .ok()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let avg_first_byte_ms = model_health_average_first_byte_ms(&usage_events);
        let events = attempts
            .into_iter()
            .filter_map(public_request_candidate_health_event)
            .collect::<Vec<_>>();
        let empty_timeline = BTreeMap::new();
        let timeline_source = timeline_by_format
            .get(&api_format)
            .unwrap_or(&empty_timeline);
        let (timeline, time_range_start, time_range_end) =
            build_public_health_timeline(timeline_source, API_FORMAT_HEALTH_TIMELINE_SEGMENTS);
        let timeline_details = build_public_health_timeline_details(
            timeline_source,
            since_unix_secs,
            now_unix_secs,
            API_FORMAT_HEALTH_TIMELINE_SEGMENTS,
            &usage_events,
        );

        let mut format_payload = json!({
            "api_format": api_format.clone(),
            "total_attempts": total_attempts,
            "success_count": success_count,
            "failed_count": failed_count,
            "skipped_count": skipped_count,
            "success_rate": success_rate,
            "avg_latency_ms": avg_latency_ms,
            "avg_first_byte_ms": avg_first_byte_ms,
            "avg_tps": avg_tps,
            "last_event_at": last_event_at.and_then(unix_ms_to_rfc3339),
            "events": events,
            "timeline": timeline,
            "timeline_details": timeline_details,
            "time_range_start": time_range_start.and_then(unix_ms_to_rfc3339),
            "time_range_end": time_range_end.map(|ms| unix_ms_to_rfc3339(ms)).unwrap_or_else(|| unix_secs_to_rfc3339(now_unix_secs)),
        });
        if options.include_api_path {
            format_payload["api_path"] = json!(public_api_format_local_path(&api_format));
        }
        if options.include_provider_count {
            format_payload["provider_count"] = json!(provider_ids_by_format
                .get(&api_format)
                .map(BTreeSet::len)
                .unwrap_or(0));
        }
        if options.include_key_count {
            format_payload["key_count"] =
                json!(*key_counts_by_format.get(&api_format).unwrap_or(&0));
        }
        formats.push(format_payload);
    }

    Some(json!({
        "generated_at": unix_secs_to_rfc3339(now_unix_secs),
        "formats": formats,
    }))
}

fn public_request_candidate_health_event(
    candidate: StoredRequestCandidate,
) -> Option<serde_json::Value> {
    let timestamp = candidate
        .finished_at_unix_ms
        .or(candidate.started_at_unix_ms)
        .unwrap_or(candidate.created_at_unix_ms);
    Some(json!({
        "timestamp": unix_ms_to_rfc3339(timestamp)?,
        "status": request_candidate_status_label(candidate.status),
        "status_code": candidate.status_code,
        "latency_ms": candidate.latency_ms,
        "error_type": sanitize_request_candidate_error_type(candidate.error_type),
    }))
}

pub(crate) async fn build_model_health_monitor_payload(
    state: &AppState,
    lookback_hours: u64,
    model_limit: usize,
    per_model_limit: usize,
    options: ModelHealthMonitorOptions,
) -> Option<serde_json::Value> {
    if !state.has_usage_data_reader() {
        return None;
    }

    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let since_unix_secs = now_unix_secs.saturating_sub(lookback_hours * 3600);

    let breakdown = state
        .summarize_usage_breakdown(&UsageBreakdownSummaryQuery {
            created_from_unix_secs: since_unix_secs,
            created_until_unix_secs: now_unix_secs,
            user_id: None,
            provider_name: None,
            model: None,
            api_format: None,
            exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
            group_by: UsageBreakdownGroupBy::Model,
        })
        .await
        .ok()
        .unwrap_or_default();

    let selected_models = breakdown
        .into_iter()
        .filter(|row| !row.group_key.trim().is_empty())
        .take(model_limit)
        .collect::<Vec<_>>();

    // 时间线条必须来自数据库侧的全窗口分段聚合：
    // 之前用 "最新 per_model_limit 条事件" 分桶，导致高流量模型的早期时段被判成"无请求"，
    // 与同一张卡片上的"265 次请求 / 可用率"自相矛盾。
    // Note: 取舍与残留问题见 .agents/notes/implemented/bug-fix/2026-10-09-model-health-history-timeline-truncated-sample.md
    let timeline_buckets = load_usage_health_timeline_buckets(
        state,
        UsageHealthTimelineQuery {
            created_from_unix_secs: since_unix_secs,
            created_until_unix_secs: now_unix_secs,
            group_by: UsageHealthTimelineGroupBy::Model,
            group_values: selected_models
                .iter()
                .map(|row| row.group_key.clone())
                .collect::<Vec<_>>(),
            provider_name: None,
            model: None,
            api_format: None,
            segments: MODEL_HEALTH_TIMELINE_SEGMENTS,
            exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
        },
        "model",
    )
    .await;

    let mut models = Vec::with_capacity(selected_models.len());
    for row in selected_models {
        let events = state
            .list_usage_audits(&UsageAuditListQuery {
                created_from_unix_secs: Some(since_unix_secs),
                created_until_unix_secs: Some(now_unix_secs),
                model: Some(row.group_key.clone()),
                exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
                limit: Some(per_model_limit),
                newest_first: true,
                ..UsageAuditListQuery::default()
            })
            .await
            .ok()
            .unwrap_or_default();

        // 该分组拿不到聚合结果（查询失败或缺少 usage reader）时退回事件样本，
        // 保证卡片不整条变灰；内存仓储已实现聚合，正常不会走到这里。
        let buckets = timeline_buckets.get(&row.group_key);
        let fallback_buckets = buckets.is_none().then(|| {
            usage_health_buckets_from_events(
                &events,
                since_unix_secs,
                now_unix_secs,
                MODEL_HEALTH_TIMELINE_SEGMENTS,
            )
        });
        let empty_buckets = BTreeMap::new();
        let buckets = buckets
            .or(fallback_buckets.as_ref())
            .unwrap_or(&empty_buckets);
        let (timeline, timeline_details) = build_usage_health_timeline_from_buckets(
            buckets,
            since_unix_secs,
            now_unix_secs,
            MODEL_HEALTH_TIMELINE_SEGMENTS,
        );
        let provider_count = model_health_provider_count(&events);
        let first_byte_average = usage_health_average_first_byte_ms(buckets);
        let last_event_at = events
            .first()
            .and_then(|item| unix_secs_to_rfc3339(item.created_at_unix_ms));
        let event_payload = events
            .iter()
            .rev()
            .map(model_health_event_payload)
            .collect::<Vec<_>>();

        let total_attempts = row.request_count;
        let success_count = row.success_count.min(total_attempts);
        let failed_count = total_attempts.saturating_sub(success_count);
        let success_rate = if total_attempts > 0 {
            success_count as f64 / total_attempts as f64
        } else {
            1.0
        };
        let avg_latency_ms = model_health_average_latency_ms(&row);

        let model_name = row.group_key.clone();
        let mut model_payload = json!({
            "model": model_name,
            "display_name": model_health_display_name(&row.group_key),
            "total_attempts": total_attempts,
            "success_count": success_count,
            "failed_count": failed_count,
            "success_rate": success_rate,
            "avg_latency_ms": avg_latency_ms,
            "avg_first_byte_ms": first_byte_average,
            "avg_tps": model_health_average_tps(&row),
            "last_event_at": last_event_at,
            "events": event_payload,
            "timeline": timeline,
            "timeline_details": timeline_details,
            "time_range_start": unix_secs_to_rfc3339(since_unix_secs),
            "time_range_end": unix_secs_to_rfc3339(now_unix_secs),
        });
        if options.include_provider_count {
            model_payload["provider_count"] = json!(provider_count);
        }
        models.push(model_payload);
    }

    Some(json!({
        "generated_at": unix_secs_to_rfc3339(now_unix_secs),
        "models": models,
    }))
}

pub(crate) async fn build_provider_health_monitor_payload(
    state: &AppState,
    lookback_hours: u64,
    provider_limit: usize,
    per_provider_model_limit: usize,
    per_model_event_limit: usize,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() || !state.has_usage_data_reader() {
        return None;
    }

    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let since_unix_secs = now_unix_secs.saturating_sub(lookback_hours * 3600);

    let providers = state
        .list_provider_catalog_providers(true)
        .await
        .ok()
        .unwrap_or_default()
        .into_iter()
        .filter(|provider| provider.is_active)
        .take(provider_limit)
        .collect::<Vec<_>>();

    let provider_breakdown = state
        .summarize_usage_breakdown(&UsageBreakdownSummaryQuery {
            created_from_unix_secs: since_unix_secs,
            created_until_unix_secs: now_unix_secs,
            user_id: None,
            provider_name: None,
            model: None,
            api_format: None,
            exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
            group_by: UsageBreakdownGroupBy::Provider,
        })
        .await
        .ok()
        .unwrap_or_default()
        .into_iter()
        .map(|row| (row.group_key.clone(), row))
        .collect::<BTreeMap<_, _>>();

    // 提供商主条一次性批量聚合：避免为每个提供商各发一次查询（最多 50 个）。
    let provider_timeline_buckets = load_usage_health_timeline_buckets(
        state,
        UsageHealthTimelineQuery {
            created_from_unix_secs: since_unix_secs,
            created_until_unix_secs: now_unix_secs,
            group_by: UsageHealthTimelineGroupBy::Provider,
            group_values: providers
                .iter()
                .map(|provider| provider.name.clone())
                .collect::<Vec<_>>(),
            provider_name: None,
            model: None,
            api_format: None,
            segments: MODEL_HEALTH_TIMELINE_SEGMENTS,
            exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
        },
        "provider",
    )
    .await;

    let mut payload = Vec::with_capacity(providers.len());
    for provider in providers {
        let provider_stats = provider_breakdown.get(&provider.name);
        // 先按名字取出时间条（provider 随后被移动进构造函数，不能同时借用它的字段）。
        let provider_name = provider.name.clone();
        let timeline_buckets = provider_timeline_buckets.get(&provider_name);
        payload.push(
            build_provider_health_payload(
                state,
                provider,
                provider_stats,
                timeline_buckets,
                since_unix_secs,
                now_unix_secs,
                per_provider_model_limit,
                per_model_event_limit,
            )
            .await,
        );
    }

    payload.sort_by(|left, right| {
        let left_rank = provider_health_sort_rank(left);
        let right_rank = provider_health_sort_rank(right);
        left_rank.cmp(&right_rank).then_with(|| {
            left.get("provider_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .cmp(
                    right
                        .get("provider_name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                )
        })
    });

    Some(json!({
        "generated_at": unix_secs_to_rfc3339(now_unix_secs),
        "providers": payload,
    }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HealthMonitorRelationDimension {
    Endpoint,
    Model,
    Provider,
}

impl HealthMonitorRelationDimension {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "endpoint" | "api_format" | "api-format" => Some(Self::Endpoint),
            "model" => Some(Self::Model),
            "provider" => Some(Self::Provider),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Endpoint => "endpoint",
            Self::Model => "model",
            Self::Provider => "provider",
        }
    }
}

pub(crate) async fn build_related_health_monitor_payload(
    state: &AppState,
    lookback_hours: u64,
    dimension: HealthMonitorRelationDimension,
    value: &str,
    related_limit: usize,
    per_item_limit: usize,
    include_provider_info: bool,
) -> Option<serde_json::Value> {
    if !state.has_usage_data_reader() {
        return None;
    }

    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let since_unix_secs = now_unix_secs.saturating_sub(lookback_hours * 3600);
    let related_limit = related_limit.max(1);
    let per_item_limit = per_item_limit.max(1);

    let mut related_endpoints = Vec::new();
    let mut related_models = Vec::new();
    let mut related_providers = Vec::new();

    match dimension {
        HealthMonitorRelationDimension::Endpoint => {
            let api_format = value.to_string();
            related_models = build_related_health_items(
                state,
                breakdown_summary_query(
                    since_unix_secs,
                    now_unix_secs,
                    None,
                    None,
                    Some(api_format.clone()),
                    UsageBreakdownGroupBy::Model,
                ),
                since_unix_secs,
                now_unix_secs,
                related_limit,
                per_item_limit,
                "model",
                {
                    let api_format = api_format.clone();
                    move |row| {
                        usage_audit_query(
                            since_unix_secs,
                            now_unix_secs,
                            None,
                            Some(row.group_key.clone()),
                            Some(api_format.clone()),
                            per_item_limit,
                        )
                    }
                },
                |row, events| related_model_display_meta(row, events),
            )
            .await;

            if include_provider_info {
                let api_format = value.to_string();
                related_providers = build_related_health_items(
                    state,
                    breakdown_summary_query(
                        since_unix_secs,
                        now_unix_secs,
                        None,
                        None,
                        Some(api_format.clone()),
                        UsageBreakdownGroupBy::Provider,
                    ),
                    since_unix_secs,
                    now_unix_secs,
                    related_limit,
                    per_item_limit,
                    "provider",
                    {
                        let api_format = api_format.clone();
                        move |row| {
                            usage_audit_query(
                                since_unix_secs,
                                now_unix_secs,
                                Some(row.group_key.clone()),
                                None,
                                Some(api_format.clone()),
                                per_item_limit,
                            )
                        }
                    },
                    related_provider_display_meta,
                )
                .await;
            }
        }
        HealthMonitorRelationDimension::Model => {
            let model = value.to_string();
            related_endpoints = build_related_health_items(
                state,
                breakdown_summary_query(
                    since_unix_secs,
                    now_unix_secs,
                    None,
                    Some(model.clone()),
                    None,
                    UsageBreakdownGroupBy::ApiFormat,
                ),
                since_unix_secs,
                now_unix_secs,
                related_limit,
                per_item_limit,
                "endpoint",
                {
                    let model = model.clone();
                    move |row| {
                        usage_audit_query(
                            since_unix_secs,
                            now_unix_secs,
                            None,
                            Some(model.clone()),
                            Some(row.group_key.clone()),
                            per_item_limit,
                        )
                    }
                },
                related_endpoint_display_meta,
            )
            .await;

            if include_provider_info {
                let model = value.to_string();
                related_providers = build_related_health_items(
                    state,
                    breakdown_summary_query(
                        since_unix_secs,
                        now_unix_secs,
                        None,
                        Some(model.clone()),
                        None,
                        UsageBreakdownGroupBy::Provider,
                    ),
                    since_unix_secs,
                    now_unix_secs,
                    related_limit,
                    per_item_limit,
                    "provider",
                    {
                        let model = model.clone();
                        move |row| {
                            usage_audit_query(
                                since_unix_secs,
                                now_unix_secs,
                                Some(row.group_key.clone()),
                                Some(model.clone()),
                                None,
                                per_item_limit,
                            )
                        }
                    },
                    related_provider_display_meta,
                )
                .await;
            }
        }
        HealthMonitorRelationDimension::Provider => {
            let provider_name = value.to_string();
            related_endpoints = build_related_health_items(
                state,
                breakdown_summary_query(
                    since_unix_secs,
                    now_unix_secs,
                    Some(provider_name.clone()),
                    None,
                    None,
                    UsageBreakdownGroupBy::ApiFormat,
                ),
                since_unix_secs,
                now_unix_secs,
                related_limit,
                per_item_limit,
                "endpoint",
                {
                    let provider_name = provider_name.clone();
                    move |row| {
                        usage_audit_query(
                            since_unix_secs,
                            now_unix_secs,
                            Some(provider_name.clone()),
                            None,
                            Some(row.group_key.clone()),
                            per_item_limit,
                        )
                    }
                },
                related_endpoint_display_meta,
            )
            .await;

            let provider_name = value.to_string();
            related_models = build_related_health_items(
                state,
                breakdown_summary_query(
                    since_unix_secs,
                    now_unix_secs,
                    Some(provider_name.clone()),
                    None,
                    None,
                    UsageBreakdownGroupBy::Model,
                ),
                since_unix_secs,
                now_unix_secs,
                related_limit,
                per_item_limit,
                "model",
                {
                    let provider_name = provider_name.clone();
                    move |row| {
                        usage_audit_query(
                            since_unix_secs,
                            now_unix_secs,
                            Some(provider_name.clone()),
                            Some(row.group_key.clone()),
                            None,
                            per_item_limit,
                        )
                    }
                },
                |row, events| related_model_display_meta(row, events),
            )
            .await;
        }
    }

    Some(json!({
        "generated_at": unix_secs_to_rfc3339(now_unix_secs),
        "dimension": dimension.as_str(),
        "value": value,
        "related_endpoints": related_endpoints,
        "related_models": related_models,
        "related_providers": related_providers,
    }))
}

fn breakdown_summary_query(
    created_from_unix_secs: u64,
    created_until_unix_secs: u64,
    provider_name: Option<String>,
    model: Option<String>,
    api_format: Option<String>,
    group_by: UsageBreakdownGroupBy,
) -> UsageBreakdownSummaryQuery {
    UsageBreakdownSummaryQuery {
        created_from_unix_secs,
        created_until_unix_secs,
        user_id: None,
        provider_name,
        model,
        api_format,
        exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
        group_by,
    }
}

fn usage_audit_query(
    created_from_unix_secs: u64,
    created_until_unix_secs: u64,
    provider_name: Option<String>,
    model: Option<String>,
    api_format: Option<String>,
    limit: usize,
) -> UsageAuditListQuery {
    UsageAuditListQuery {
        created_from_unix_secs: Some(created_from_unix_secs),
        created_until_unix_secs: Some(created_until_unix_secs),
        provider_name,
        model,
        api_format,
        exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
        limit: Some(limit),
        newest_first: true,
        ..UsageAuditListQuery::default()
    }
}

async fn build_related_health_items<F, G>(
    state: &AppState,
    query: UsageBreakdownSummaryQuery,
    since_unix_secs: u64,
    now_unix_secs: u64,
    related_limit: usize,
    per_item_limit: usize,
    kind: &'static str,
    build_events_query: F,
    build_display_meta: G,
) -> Vec<serde_json::Value>
where
    F: Fn(&StoredUsageBreakdownSummaryRow) -> UsageAuditListQuery,
    G: Fn(&StoredUsageBreakdownSummaryRow, &[StoredRequestUsageAudit]) -> (String, Option<String>),
{
    let mut rows = state
        .summarize_usage_breakdown(&query)
        .await
        .ok()
        .unwrap_or_default()
        .into_iter()
        .filter(|row| !row.group_key.trim().is_empty())
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| {
        related_health_sort_rank(left)
            .cmp(&related_health_sort_rank(right))
            .then_with(|| right.request_count.cmp(&left.request_count))
            .then_with(|| left.group_key.cmp(&right.group_key))
    });
    let selected_rows = rows.into_iter().take(related_limit).collect::<Vec<_>>();

    // 关联项的时间线条同样改用全窗口分段聚合，口径与其头部 breakdown 保持一致。
    let timeline_group_by = match query.group_by {
        UsageBreakdownGroupBy::Model => UsageHealthTimelineGroupBy::Model,
        UsageBreakdownGroupBy::Provider => UsageHealthTimelineGroupBy::Provider,
        UsageBreakdownGroupBy::ApiFormat => UsageHealthTimelineGroupBy::ApiFormat,
    };
    let timeline_buckets = load_usage_health_timeline_buckets(
        state,
        UsageHealthTimelineQuery {
            created_from_unix_secs: since_unix_secs,
            created_until_unix_secs: now_unix_secs,
            group_by: timeline_group_by,
            group_values: selected_rows
                .iter()
                .map(|row| row.group_key.clone())
                .collect::<Vec<_>>(),
            provider_name: query.provider_name.clone(),
            model: query.model.clone(),
            api_format: query.api_format.clone(),
            segments: MODEL_HEALTH_TIMELINE_SEGMENTS,
            exclude_status_codes: query.exclude_status_codes.clone(),
        },
        "related",
    )
    .await;

    let mut items = Vec::new();
    for row in selected_rows {
        let events = state
            .list_usage_audits(&build_events_query(&row))
            .await
            .ok()
            .unwrap_or_default();
        let (display_name, meta_text) = build_display_meta(&row, &events);
        items.push(related_health_item_payload(
            kind,
            &row,
            &events,
            timeline_buckets.get(&row.group_key),
            display_name,
            meta_text,
            since_unix_secs,
            now_unix_secs,
        ));
    }

    items
}

fn related_health_item_payload(
    kind: &'static str,
    row: &StoredUsageBreakdownSummaryRow,
    events: &[StoredRequestUsageAudit],
    buckets: Option<&BTreeMap<u32, StoredUsageHealthTimelineRow>>,
    display_name: String,
    meta_text: Option<String>,
    since_unix_secs: u64,
    now_unix_secs: u64,
) -> serde_json::Value {
    // 优先使用全窗口分段聚合；聚合不可用时退回事件样本。
    let fallback_buckets = buckets.is_none().then(|| {
        usage_health_buckets_from_events(
            events,
            since_unix_secs,
            now_unix_secs,
            MODEL_HEALTH_TIMELINE_SEGMENTS,
        )
    });
    let empty_buckets = BTreeMap::new();
    let buckets = buckets
        .or(fallback_buckets.as_ref())
        .unwrap_or(&empty_buckets);
    let (timeline, timeline_details) = build_usage_health_timeline_from_buckets(
        buckets,
        since_unix_secs,
        now_unix_secs,
        MODEL_HEALTH_TIMELINE_SEGMENTS,
    );
    let total_attempts = row.request_count;
    let success_count = row.success_count.min(total_attempts);
    let failed_count = total_attempts.saturating_sub(success_count);
    let success_rate = if total_attempts > 0 {
        success_count as f64 / total_attempts as f64
    } else {
        1.0
    };
    let last_event_at = events
        .first()
        .and_then(|item| unix_secs_to_rfc3339(item.created_at_unix_ms));

    json!({
        "kind": kind,
        "key": row.group_key.clone(),
        "display_name": display_name,
        "meta_text": meta_text,
        "total_attempts": total_attempts,
        "success_count": success_count,
        "failed_count": failed_count,
        "success_rate": success_rate,
        "avg_latency_ms": model_health_average_latency_ms(row),
        "avg_first_byte_ms": usage_health_average_first_byte_ms(buckets),
        "avg_tps": model_health_average_tps(row),
        "last_event_at": last_event_at,
        "timeline": timeline,
        "timeline_details": timeline_details,
        "time_range_start": unix_secs_to_rfc3339(since_unix_secs),
        "time_range_end": unix_secs_to_rfc3339(now_unix_secs),
    })
}

fn related_model_display_meta(
    row: &StoredUsageBreakdownSummaryRow,
    events: &[StoredRequestUsageAudit],
) -> (String, Option<String>) {
    let provider_count = model_health_provider_count(events);
    let meta_text = if provider_count > 0 {
        Some(format!("{provider_count} 个提供商"))
    } else {
        None
    };
    (model_health_display_name(&row.group_key), meta_text)
}

fn related_provider_display_meta(
    row: &StoredUsageBreakdownSummaryRow,
    _events: &[StoredRequestUsageAudit],
) -> (String, Option<String>) {
    (row.group_key.clone(), None)
}

fn related_endpoint_display_meta(
    row: &StoredUsageBreakdownSummaryRow,
    _events: &[StoredRequestUsageAudit],
) -> (String, Option<String>) {
    (
        api_format_display_name(&row.group_key),
        Some(public_api_format_local_path(&row.group_key).to_string()),
    )
}

fn related_health_sort_rank(row: &StoredUsageBreakdownSummaryRow) -> u8 {
    if row.request_count == 0 {
        return 3;
    }
    let success_count = row.success_count.min(row.request_count);
    let success_rate = success_count as f64 / row.request_count as f64;
    if success_rate < 0.8 {
        0
    } else if success_rate < 0.95 {
        1
    } else {
        2
    }
}

async fn build_provider_health_payload(
    state: &AppState,
    provider: StoredProviderCatalogProvider,
    provider_stats: Option<&StoredUsageBreakdownSummaryRow>,
    provider_timeline_buckets: Option<&BTreeMap<u32, StoredUsageHealthTimelineRow>>,
    since_unix_secs: u64,
    now_unix_secs: u64,
    per_provider_model_limit: usize,
    per_model_event_limit: usize,
) -> serde_json::Value {
    let model_breakdown = state
        .summarize_usage_breakdown(&UsageBreakdownSummaryQuery {
            created_from_unix_secs: since_unix_secs,
            created_until_unix_secs: now_unix_secs,
            user_id: None,
            provider_name: Some(provider.name.clone()),
            model: None,
            api_format: None,
            exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
            group_by: UsageBreakdownGroupBy::Model,
        })
        .await
        .ok()
        .unwrap_or_default();

    let provider_event_limit = per_model_event_limit
        .saturating_mul(per_provider_model_limit.max(1))
        .max(per_model_event_limit);
    let provider_events = state
        .list_usage_audits(&UsageAuditListQuery {
            created_from_unix_secs: Some(since_unix_secs),
            created_until_unix_secs: Some(now_unix_secs),
            provider_name: Some(provider.name.clone()),
            exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
            limit: Some(provider_event_limit),
            newest_first: true,
            ..UsageAuditListQuery::default()
        })
        .await
        .ok()
        .unwrap_or_default();
    let selected_model_rows = model_breakdown
        .iter()
        .filter(|row| !row.group_key.trim().is_empty())
        .take(per_provider_model_limit)
        .collect::<Vec<_>>();

    // 提供商主条与内嵌模型条都改用全窗口分段聚合，避免用被 LIMIT 截断的事件样本画历史。
    // 主条由调用方一次性批量聚合后传入，内嵌模型条按"该提供商范围内的模型"聚合。
    let model_timeline_buckets = load_usage_health_timeline_buckets(
        state,
        UsageHealthTimelineQuery {
            created_from_unix_secs: since_unix_secs,
            created_until_unix_secs: now_unix_secs,
            group_by: UsageHealthTimelineGroupBy::Model,
            group_values: selected_model_rows
                .iter()
                .map(|row| row.group_key.clone())
                .collect::<Vec<_>>(),
            provider_name: Some(provider.name.clone()),
            model: None,
            api_format: None,
            segments: MODEL_HEALTH_TIMELINE_SEGMENTS,
            exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
        },
        "provider_model",
    )
    .await;

    let mut events_by_model = selected_model_rows
        .iter()
        .map(|row| (row.group_key.clone(), Vec::new()))
        .collect::<BTreeMap<_, _>>();
    for event in &provider_events {
        if let Some(events) = events_by_model.get_mut(&event.model) {
            if events.len() < per_model_event_limit {
                events.push(event.clone());
            }
        }
    }

    let mut models = Vec::new();
    for row in selected_model_rows {
        let events = events_by_model.remove(&row.group_key).unwrap_or_default();
        models.push(model_health_payload_from_row(
            row,
            &events,
            model_timeline_buckets.get(&row.group_key),
            since_unix_secs,
            now_unix_secs,
            None,
        ));
    }

    let (total_attempts, success_count, failed_count, success_rate, avg_latency_ms, avg_tps) =
        if let Some(row) = provider_stats {
            let total_attempts = row.request_count;
            let success_count = row.success_count.min(total_attempts);
            let failed_count = total_attempts.saturating_sub(success_count);
            let success_rate = if total_attempts > 0 {
                success_count as f64 / total_attempts as f64
            } else {
                1.0
            };
            (
                total_attempts,
                success_count,
                failed_count,
                success_rate,
                model_health_average_latency_ms(row),
                model_health_average_tps(row),
            )
        } else {
            (0, 0, 0, 1.0, None, None)
        };

    // 提供商主条的时间线条同样取自全窗口聚合；聚合不可用时退回事件样本。
    let provider_fallback_buckets = provider_timeline_buckets.is_none().then(|| {
        usage_health_buckets_from_events(
            &provider_events,
            since_unix_secs,
            now_unix_secs,
            MODEL_HEALTH_TIMELINE_SEGMENTS,
        )
    });
    let empty_buckets = BTreeMap::new();
    let provider_buckets = provider_timeline_buckets
        .or(provider_fallback_buckets.as_ref())
        .unwrap_or(&empty_buckets);
    let (timeline, timeline_details) = build_usage_health_timeline_from_buckets(
        provider_buckets,
        since_unix_secs,
        now_unix_secs,
        MODEL_HEALTH_TIMELINE_SEGMENTS,
    );
    let last_event_at = provider_events
        .iter()
        .max_by_key(|event| event.created_at_unix_ms)
        .and_then(|event| unix_secs_to_rfc3339(event.created_at_unix_ms));

    json!({
        "provider_id": provider.id,
        "provider_name": provider.name,
        "provider_type": provider.provider_type,
        "is_active": provider.is_active,
        "total_attempts": total_attempts,
        "success_count": success_count,
        "failed_count": failed_count,
        "success_rate": success_rate,
        "avg_latency_ms": avg_latency_ms,
        "avg_first_byte_ms": usage_health_average_first_byte_ms(provider_buckets),
        "avg_tps": avg_tps,
        "model_count": model_breakdown.len(),
        "last_event_at": last_event_at,
        "timeline": timeline,
        "timeline_details": timeline_details,
        "time_range_start": unix_secs_to_rfc3339(since_unix_secs),
        "time_range_end": unix_secs_to_rfc3339(now_unix_secs),
        "models": models,
    })
}

fn provider_health_sort_rank(provider: &serde_json::Value) -> u8 {
    let total_attempts = provider
        .get("total_attempts")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if total_attempts == 0 {
        return 3;
    }
    let success_rate = provider
        .get("success_rate")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0);
    if success_rate < 0.8 {
        0
    } else if success_rate < 0.95 {
        1
    } else {
        2
    }
}

fn model_health_payload_from_row(
    row: &StoredUsageBreakdownSummaryRow,
    events: &[StoredRequestUsageAudit],
    buckets: Option<&BTreeMap<u32, StoredUsageHealthTimelineRow>>,
    since_unix_secs: u64,
    now_unix_secs: u64,
    provider_count: Option<usize>,
) -> serde_json::Value {
    // 优先使用全窗口分段聚合；聚合不可用时退回事件样本，避免整条历史变灰。
    let fallback_buckets = buckets.is_none().then(|| {
        usage_health_buckets_from_events(
            events,
            since_unix_secs,
            now_unix_secs,
            MODEL_HEALTH_TIMELINE_SEGMENTS,
        )
    });
    let empty_buckets = BTreeMap::new();
    let buckets = buckets
        .or(fallback_buckets.as_ref())
        .unwrap_or(&empty_buckets);
    let (timeline, timeline_details) = build_usage_health_timeline_from_buckets(
        buckets,
        since_unix_secs,
        now_unix_secs,
        MODEL_HEALTH_TIMELINE_SEGMENTS,
    );
    let last_event_at = events
        .first()
        .and_then(|item| unix_secs_to_rfc3339(item.created_at_unix_ms));
    let event_payload = events
        .iter()
        .rev()
        .map(model_health_event_payload)
        .collect::<Vec<_>>();

    let total_attempts = row.request_count;
    let success_count = row.success_count.min(total_attempts);
    let failed_count = total_attempts.saturating_sub(success_count);
    let success_rate = if total_attempts > 0 {
        success_count as f64 / total_attempts as f64
    } else {
        1.0
    };

    let mut model_payload = json!({
        "model": row.group_key.clone(),
        "display_name": model_health_display_name(&row.group_key),
        "total_attempts": total_attempts,
        "success_count": success_count,
        "failed_count": failed_count,
        "success_rate": success_rate,
        "avg_latency_ms": model_health_average_latency_ms(row),
        "avg_first_byte_ms": usage_health_average_first_byte_ms(buckets),
        "avg_tps": model_health_average_tps(row),
        "last_event_at": last_event_at,
        "events": event_payload,
        "timeline": timeline,
        "timeline_details": timeline_details,
        "time_range_start": unix_secs_to_rfc3339(since_unix_secs),
        "time_range_end": unix_secs_to_rfc3339(now_unix_secs),
    });
    if let Some(provider_count) = provider_count {
        model_payload["provider_count"] = json!(provider_count);
    }
    model_payload
}

fn request_candidate_average_latency_ms(candidates: &[StoredRequestCandidate]) -> Option<f64> {
    let mut sum = 0u64;
    let mut count = 0u64;
    for candidate in candidates {
        if let Some(latency_ms) = candidate.latency_ms {
            sum = sum.saturating_add(latency_ms);
            count = count.saturating_add(1);
        }
    }
    if count == 0 {
        None
    } else {
        Some(sum as f64 / count as f64)
    }
}

fn model_health_average_latency_ms(row: &StoredUsageBreakdownSummaryRow) -> Option<f64> {
    if row.overall_response_time_samples > 0 {
        return Some(row.overall_response_time_sum_ms / row.overall_response_time_samples as f64);
    }
    if row.response_time_samples > 0 {
        return Some(row.response_time_sum_ms / row.response_time_samples as f64);
    }
    None
}

fn model_health_average_tps(row: &StoredUsageBreakdownSummaryRow) -> Option<f64> {
    if row.output_tokens == 0 || row.response_time_sum_ms <= 0.0 {
        return None;
    }
    Some(row.output_tokens as f64 / (row.response_time_sum_ms / 1000.0))
}

fn model_health_average_first_byte_ms(events: &[StoredRequestUsageAudit]) -> Option<f64> {
    let mut sum = 0u64;
    let mut count = 0u64;
    for event in events {
        if let Some(first_byte_time_ms) = event.first_byte_time_ms {
            sum = sum.saturating_add(first_byte_time_ms);
            count = count.saturating_add(1);
        }
    }
    if count == 0 {
        None
    } else {
        Some(sum as f64 / count as f64)
    }
}

fn model_health_provider_count(events: &[StoredRequestUsageAudit]) -> usize {
    let mut providers = BTreeSet::new();
    for event in events {
        if let Some(provider_id) = event.provider_id.as_deref() {
            providers.insert(provider_id.to_string());
        } else if !event.provider_name.trim().is_empty() {
            providers.insert(event.provider_name.clone());
        }
    }
    providers.len()
}

fn model_health_event_payload(event: &StoredRequestUsageAudit) -> serde_json::Value {
    json!({
        "timestamp": unix_secs_to_rfc3339(event.created_at_unix_ms),
        "status": model_health_event_status(event),
        "status_code": event.status_code,
        "latency_ms": event.response_time_ms,
        "first_byte_time_ms": event.first_byte_time_ms,
        "error_type": event.error_category,
    })
}

fn model_health_event_status(event: &StoredRequestUsageAudit) -> &'static str {
    if model_health_event_success(event) {
        "success"
    } else {
        "failed"
    }
}

fn model_health_event_success(event: &StoredRequestUsageAudit) -> bool {
    !event.status.eq_ignore_ascii_case("failed")
        && event.status_code.is_none_or(|status| status < 400)
        && event
            .error_message
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
}

fn health_timeline_status(success_count: u64, failed_count: u64) -> &'static str {
    let actual_completed = success_count.saturating_add(failed_count);
    if actual_completed == 0 {
        return "unknown";
    }
    let success_rate = success_count as f64 / actual_completed as f64;
    if success_rate >= 0.95 {
        "healthy"
    } else if success_rate >= 0.7 {
        "warning"
    } else {
        "unhealthy"
    }
}

fn health_timeline_success_rate(success_count: u64, failed_count: u64) -> Option<f64> {
    let actual_completed = success_count.saturating_add(failed_count);
    if actual_completed == 0 {
        None
    } else {
        Some(success_count as f64 / actual_completed as f64)
    }
}

fn health_timeline_segment_index(
    timestamp_unix_secs: u64,
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
) -> Option<usize> {
    if segments == 0
        || timestamp_unix_secs < since_unix_secs
        || timestamp_unix_secs > until_unix_secs
    {
        return None;
    }
    let safe_range = until_unix_secs.saturating_sub(since_unix_secs).max(1);
    let offset = timestamp_unix_secs.saturating_sub(since_unix_secs);
    let mut segment_idx = ((offset as u128 * segments as u128) / safe_range as u128) as usize;
    if segment_idx >= segments as usize {
        segment_idx = segments.saturating_sub(1) as usize;
    }
    Some(segment_idx)
}

fn health_timeline_segment_bounds(
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
    segment_idx: u32,
) -> (u64, u64) {
    let segment_count = segments.max(1);
    let safe_range = until_unix_secs.saturating_sub(since_unix_secs).max(1);
    let start_offset =
        (safe_range as u128 * u128::from(segment_idx) / u128::from(segment_count)) as u64;
    let end_offset = (safe_range as u128 * u128::from(segment_idx.saturating_add(1))
        / u128::from(segment_count)) as u64;
    let start = since_unix_secs.saturating_add(start_offset);
    let end = if segment_idx.saturating_add(1) >= segment_count {
        until_unix_secs
    } else {
        since_unix_secs.saturating_add(end_offset)
    };
    (start, end.max(start))
}

fn aggregate_usage_timeline_metrics(
    events: &[StoredRequestUsageAudit],
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
) -> Vec<HealthTimelineMetricBucket> {
    let mut buckets = (0..segments)
        .map(|_| HealthTimelineMetricBucket::default())
        .collect::<Vec<_>>();
    for event in events {
        let Some(segment_idx) = health_timeline_segment_index(
            event.created_at_unix_ms,
            since_unix_secs,
            until_unix_secs,
            segments,
        ) else {
            continue;
        };
        if let Some(bucket) = buckets.get_mut(segment_idx) {
            bucket.add_usage_event(event);
        }
    }
    buckets
}

fn health_timeline_detail_payload(
    segment_idx: u32,
    counts: HealthTimelineDetailCounts,
    metrics: HealthTimelineMetricBucket,
    window: HealthTimelineWindow,
) -> serde_json::Value {
    let (range_start, range_end) = health_timeline_segment_bounds(
        window.since_unix_secs,
        window.until_unix_secs,
        window.segments,
        segment_idx,
    );
    json!({
        "segment_index": segment_idx,
        "status": counts.status,
        "time_range_start": unix_secs_to_rfc3339(range_start),
        "time_range_end": unix_secs_to_rfc3339(range_end),
        "total_attempts": counts.total_attempts,
        "success_count": counts.success_count,
        "failed_count": counts.failed_count,
        "success_rate": health_timeline_success_rate(counts.success_count, counts.failed_count),
        "avg_latency_ms": metrics.avg_latency_ms(),
        "avg_first_byte_ms": metrics.avg_first_byte_ms(),
        "avg_tps": metrics.avg_tps(),
    })
}

pub(crate) fn build_public_health_timeline_details(
    buckets_by_segment: &BTreeMap<u32, PublicHealthTimelineBucket>,
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
    usage_events: &[StoredRequestUsageAudit],
) -> Vec<serde_json::Value> {
    let usage_metrics =
        aggregate_usage_timeline_metrics(usage_events, since_unix_secs, until_unix_secs, segments);
    let window = HealthTimelineWindow {
        since_unix_secs,
        until_unix_secs,
        segments,
    };
    (0..segments)
        .map(|segment_idx| {
            let count_bucket = buckets_by_segment.get(&segment_idx);
            let metrics = usage_metrics
                .get(segment_idx as usize)
                .copied()
                .unwrap_or_default();
            let total_attempts = count_bucket
                .map(|bucket| bucket.total_count)
                .unwrap_or(metrics.total_count);
            let success_count = count_bucket
                .map(|bucket| bucket.success_count)
                .unwrap_or(metrics.success_count);
            let failed_count = count_bucket
                .map(|bucket| bucket.failed_count)
                .unwrap_or(metrics.failed_count);
            let status = health_timeline_status(success_count, failed_count);
            health_timeline_detail_payload(
                segment_idx,
                HealthTimelineDetailCounts {
                    status,
                    total_attempts,
                    success_count,
                    failed_count,
                },
                metrics,
                window,
            )
        })
        .collect()
}

/// 健康时间线条的按段索引：分组键 -> 段序号 -> 聚合桶。
type UsageHealthTimelineBuckets = BTreeMap<String, BTreeMap<u32, StoredUsageHealthTimelineRow>>;

/// 把数据库返回的分段聚合行整理成按分组键索引的桶。
fn index_usage_health_timeline_rows(
    rows: Vec<StoredUsageHealthTimelineRow>,
) -> UsageHealthTimelineBuckets {
    let mut indexed = UsageHealthTimelineBuckets::new();
    for row in rows {
        indexed
            .entry(row.group_key.clone())
            .or_default()
            .insert(row.segment_idx, row);
    }
    indexed
}

/// 拉取全窗口分段聚合。
///
/// 失败（仓储不可用或查询报错）时返回空结果，调用方会退回被截断的事件样本；
/// 这条降级路径会静默丢掉历史时段，因此必须留下告警日志，否则线上只会表现为
/// "时间条又变灰了"，无从定位。
async fn load_usage_health_timeline_buckets(
    state: &AppState,
    query: UsageHealthTimelineQuery,
    scope: &'static str,
) -> UsageHealthTimelineBuckets {
    match state.aggregate_usage_health_timeline(&query).await {
        Ok(rows) => index_usage_health_timeline_rows(rows),
        Err(error) => {
            tracing::warn!(
                scope,
                error = ?error,
                "usage health timeline aggregation unavailable; falling back to truncated event sample"
            );
            UsageHealthTimelineBuckets::new()
        }
    }
}

/// 把分段聚合桶转换成前端 Tooltip 使用的指标结构。
fn usage_health_metric_bucket(row: &StoredUsageHealthTimelineRow) -> HealthTimelineMetricBucket {
    HealthTimelineMetricBucket {
        total_count: row.request_count,
        success_count: row.success_count,
        failed_count: row.request_count.saturating_sub(row.success_count),
        latency_sum_ms: row.response_time_sum_ms as u64,
        latency_samples: row.response_time_samples,
        first_byte_sum_ms: row.first_byte_sum_ms as u64,
        first_byte_samples: row.first_byte_samples,
        output_tokens: row.output_tokens,
        response_time_sum_ms: row.response_time_sum_ms as u64,
    }
}

/// 用分段聚合桶生成时间线状态数组与每段 Tooltip 明细。
///
/// 桶必须来自数据库侧的全窗口聚合：若改用"取最近 N 条事件再分桶"，
/// 高流量模型/提供商的早期时段会因为没有样本而被判成"无请求"灰条，
/// 与卡片头部的全窗口请求数、可用率对不上。
fn build_usage_health_timeline_from_buckets(
    buckets: &BTreeMap<u32, StoredUsageHealthTimelineRow>,
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
) -> (Vec<&'static str>, Vec<serde_json::Value>) {
    let window = HealthTimelineWindow {
        since_unix_secs,
        until_unix_secs,
        segments,
    };
    let mut timeline = Vec::with_capacity(segments as usize);
    let mut details = Vec::with_capacity(segments as usize);
    for segment_idx in 0..segments {
        let bucket = buckets.get(&segment_idx);
        let total_attempts = bucket.map(|row| row.request_count).unwrap_or(0);
        let success_count = bucket.map(|row| row.success_count).unwrap_or(0);
        let failed_count = total_attempts.saturating_sub(success_count);
        let status = health_timeline_status(success_count, failed_count);
        let metrics = bucket.map(usage_health_metric_bucket).unwrap_or_default();
        details.push(health_timeline_detail_payload(
            segment_idx,
            HealthTimelineDetailCounts {
                status,
                total_attempts,
                success_count,
                failed_count,
            },
            metrics,
            window,
        ));
        timeline.push(status);
    }
    (timeline, details)
}

/// 降级路径：数据库聚合不可用时退回用事件样本分桶，避免整条历史变灰。
fn usage_health_buckets_from_events(
    events: &[StoredRequestUsageAudit],
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
) -> BTreeMap<u32, StoredUsageHealthTimelineRow> {
    let mut buckets = BTreeMap::new();
    for event in events {
        let Some(segment_idx) = health_timeline_segment_index(
            event.created_at_unix_ms,
            since_unix_secs,
            until_unix_secs,
            segments,
        ) else {
            continue;
        };
        let bucket =
            buckets
                .entry(segment_idx as u32)
                .or_insert_with(|| StoredUsageHealthTimelineRow {
                    segment_idx: segment_idx as u32,
                    ..StoredUsageHealthTimelineRow::default()
                });
        bucket.request_count = bucket.request_count.saturating_add(1);
        if model_health_event_success(event) {
            bucket.success_count = bucket.success_count.saturating_add(1);
        }
        if let Some(response_time_ms) = event.response_time_ms {
            bucket.response_time_sum_ms += response_time_ms as f64;
            bucket.response_time_samples = bucket.response_time_samples.saturating_add(1);
        }
        if let Some(first_byte_time_ms) = event.first_byte_time_ms {
            bucket.first_byte_sum_ms += first_byte_time_ms as f64;
            bucket.first_byte_samples = bucket.first_byte_samples.saturating_add(1);
        }
        bucket.output_tokens = bucket.output_tokens.saturating_add(event.output_tokens);
    }
    buckets
}

/// 从全窗口分段聚合中取平均 TTFB，口径与卡片头部保持一致。
fn usage_health_average_first_byte_ms(
    buckets: &BTreeMap<u32, StoredUsageHealthTimelineRow>,
) -> Option<f64> {
    let mut sum = 0.0f64;
    let mut count = 0u64;
    for row in buckets.values() {
        sum += row.first_byte_sum_ms;
        count = count.saturating_add(row.first_byte_samples);
    }
    if count == 0 {
        None
    } else {
        Some(sum / count as f64)
    }
}

fn model_health_display_name(model: &str) -> String {
    model.trim().to_string()
}

pub(crate) fn build_public_health_timeline(
    buckets_by_segment: &BTreeMap<u32, PublicHealthTimelineBucket>,
    segments: u32,
) -> (Vec<&'static str>, Option<u64>, Option<u64>) {
    let mut timeline = Vec::with_capacity(segments as usize);
    let mut earliest_time: Option<u64> = None;
    let mut latest_time: Option<u64> = None;

    for segment_idx in 0..segments {
        let Some(bucket) = buckets_by_segment.get(&segment_idx) else {
            timeline.push("unknown");
            continue;
        };
        if bucket.total_count == 0 {
            timeline.push("unknown");
            continue;
        }

        earliest_time = match (earliest_time, bucket.min_created_at_unix_ms) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (None, Some(right)) => Some(right),
            (left, None) => left,
        };
        latest_time = match (latest_time, bucket.max_created_at_unix_ms) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (None, Some(right)) => Some(right),
            (left, None) => left,
        };

        let actual_completed = bucket.success_count + bucket.failed_count;
        let success_rate = if actual_completed > 0 {
            bucket.success_count as f64 / actual_completed as f64
        } else {
            1.0
        };
        if success_rate >= 0.95 {
            timeline.push("healthy");
        } else if success_rate >= 0.7 {
            timeline.push("warning");
        } else {
            timeline.push("unhealthy");
        }
    }

    (timeline, earliest_time, latest_time)
}

pub(crate) fn api_format_display_name(api_format: &str) -> String {
    let raw = api_format.trim();
    let normalized = raw.to_ascii_lowercase();
    let Some((family, kind)) = normalized.split_once(':') else {
        return if raw.is_empty() {
            api_format.to_string()
        } else {
            raw.to_string()
        };
    };

    let family_label = match family {
        "claude" => "Claude",
        "openai" => "OpenAI",
        "gemini" => "Gemini",
        other => other,
    };
    let kind_label = match kind {
        "chat" => "Chat",
        "messages" => "Messages",
        "generate_content" => "Generate Content",
        "responses" => "Responses",
        "responses:compact" => "Responses Compact",
        "compact" => "Compact",
        "video" => "Video",
        "image" => "Image",
        "files" => "Files",
        other => other,
    };
    format!("{family_label} {kind_label}")
}

#[cfg(test)]
mod tests {
    use super::{
        build_usage_health_timeline_from_buckets, index_usage_health_timeline_rows,
        public_request_candidate_health_event, request_candidate_event_unix_ms,
        sanitize_public_model_capabilities, sanitize_public_model_config_for_user,
        sanitize_public_tiered_pricing, usage_health_average_first_byte_ms,
        usage_health_buckets_from_events,
    };
    use crate::handlers::shared::{unix_ms_to_rfc3339, unix_secs_to_rfc3339};
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, StoredRequestCandidate,
    };
    use aether_data_contracts::repository::usage::StoredUsageHealthTimelineRow;

    /// 全窗口聚合出 60 段数据时，时间线条必须段段有状态，且计数可对账。
    #[test]
    fn usage_health_timeline_paints_every_segment_covered_by_aggregation() {
        let segments = 60u32;
        let since = 1_800_000_000u64;
        let until = since + 6 * 3600;
        // 模拟 6 小时内 265 次请求均匀分布：全窗口聚合会覆盖绝大部分分段。
        let width = (until - since) as f64 / f64::from(segments);
        let rows = (0..265u64)
            .map(|index| {
                let at = since as f64 + index as f64 * (until - since) as f64 / 265.0;
                StoredUsageHealthTimelineRow {
                    group_key: "deepseek-v4.1-flash".to_string(),
                    segment_idx: (((at - since as f64) / width).floor() as u32).min(segments - 1),
                    request_count: 1,
                    success_count: 1,
                    response_time_sum_ms: 100.0,
                    response_time_samples: 1,
                    first_byte_sum_ms: 50.0,
                    first_byte_samples: 1,
                    output_tokens: 10,
                }
            })
            .collect::<Vec<_>>();
        let indexed = index_usage_health_timeline_rows(rows);
        let buckets = indexed.get("deepseek-v4.1-flash").expect("bucket exists");

        let (timeline, details) =
            build_usage_health_timeline_from_buckets(buckets, since, until, segments);

        assert_eq!(timeline.len(), segments as usize);
        assert_eq!(details.len(), segments as usize);
        // 用户报告的缺陷正是"265 次请求却大片灰条"，这里要求绝大多数分段不是"无请求"。
        let painted = timeline
            .iter()
            .filter(|status| **status != "unknown")
            .count();
        assert!(
            painted >= segments as usize - 1,
            "265 次请求应铺满整个窗口，实际只有 {painted} 段有状态"
        );
        assert!(timeline.iter().all(|status| *status == "healthy"));
        assert!(!details.is_empty());
        assert_eq!(
            details[0]["time_range_start"].as_str(),
            unix_secs_to_rfc3339(since).as_deref()
        );
    }

    /// 聚合桶为空（仓储不可用）时，降级路径仍应能画出采样到的分段，而不是整条变灰。
    #[test]
    fn usage_health_timeline_falls_back_to_event_sample_when_aggregation_missing() {
        let segments = 60u32;
        let since = 1_800_000_000u64;
        let until = since + 6 * 3600;
        let buckets = usage_health_buckets_from_events(&[], since, until, segments);
        let (timeline, _) =
            build_usage_health_timeline_from_buckets(&buckets, since, until, segments);
        assert_eq!(timeline.len(), segments as usize);
        assert!(timeline.iter().all(|status| *status == "unknown"));
        assert_eq!(usage_health_average_first_byte_ms(&buckets), None);
    }

    #[test]
    fn request_candidate_event_timestamp_uses_millisecond_precision() {
        let candidate = StoredRequestCandidate::new(
            "cand-1".to_string(),
            "req-1".to_string(),
            None,
            None,
            None,
            None,
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("key-1".to_string()),
            RequestCandidateStatus::Success,
            None,
            false,
            Some(200),
            None,
            None,
            Some(42),
            Some(1),
            None,
            None,
            1_700_000_000_000,
            Some(1_700_000_000_111),
            Some(1_700_000_000_123),
        )
        .expect("candidate should build");

        let event_unix_ms = request_candidate_event_unix_ms(&candidate);
        assert_eq!(event_unix_ms, 1_700_000_000_123);
        assert_eq!(
            unix_ms_to_rfc3339(event_unix_ms).as_deref(),
            Some("2023-11-14T22:13:20.123Z")
        );
    }

    #[test]
    fn public_candidate_health_event_classifies_untrusted_error_text() {
        let mut candidate = StoredRequestCandidate::new(
            "cand-unsafe".to_string(),
            "req-1".to_string(),
            None,
            None,
            None,
            None,
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("key-1".to_string()),
            RequestCandidateStatus::Failed,
            None,
            false,
            Some(500),
            None,
            None,
            Some(42),
            Some(1),
            None,
            None,
            1_700_000_000_000,
            Some(1_700_000_000_111),
            Some(1_700_000_000_123),
        )
        .expect("candidate should build");
        candidate.error_type = Some("Bearer public-health-secret".to_string());

        let event = public_request_candidate_health_event(candidate)
            .expect("candidate health event should build");

        assert_eq!(event["error_type"], "unclassified_error");
        assert!(!event.to_string().contains("public-health-secret"));
    }

    #[test]
    fn public_model_metadata_uses_typed_allowlists() {
        let config = sanitize_public_model_config_for_user(Some(serde_json::json!({
            "description": "Public model",
            "streaming": true,
            "api_formats": ["openai:responses", {"secret": "nested"}],
            "client_secret": "hidden",
            "billing": {
                "video": {
                    "price_per_second_by_resolution": {
                        "720p": 0.12,
                        "internal": "hidden"
                    },
                    "private_key": "hidden"
                }
            }
        })))
        .expect("public config should remain");
        assert_eq!(config["description"], "Public model");
        assert_eq!(config["streaming"], true);
        assert_eq!(
            config["api_formats"],
            serde_json::json!(["openai:responses"])
        );
        assert_eq!(
            config["billing"]["video"]["price_per_second_by_resolution"],
            serde_json::json!({"720p": 0.12})
        );
        assert!(config.get("client_secret").is_none());
        assert!(config["billing"]["video"].get("private_key").is_none());

        assert_eq!(
            sanitize_public_model_capabilities(Some(serde_json::json!([
                "vision",
                {"secret": "hidden"}
            ]))),
            Some(serde_json::json!(["vision"]))
        );

        let pricing = sanitize_public_tiered_pricing(Some(serde_json::json!({
            "tiers": [{
                "up_to": null,
                "input_price_per_1m": 3.0,
                "output_price_per_1m": 15.0,
                "internal_note": "hidden",
                "cache_ttl_pricing": [{
                    "ttl_minutes": 60,
                    "cache_creation_price_per_1m": 4.0,
                    "secret": "hidden"
                }]
            }],
            "processing_tiers": {
                "priority": {
                    "price_multiplier": 1.5,
                    "private_note": "hidden"
                }
            },
            "internal_pricing": {"secret": "hidden"}
        })))
        .expect("public pricing should remain");
        assert_eq!(pricing["tiers"][0]["input_price_per_1m"], 3.0);
        assert!(pricing.get("internal_pricing").is_none());
        assert!(pricing["tiers"][0].get("internal_note").is_none());
        assert!(pricing["tiers"][0]["cache_ttl_pricing"][0]
            .get("secret")
            .is_none());
        assert_eq!(
            pricing["processing_tiers"]["priority"],
            serde_json::json!({"price_multiplier": 1.5})
        );
    }
}
