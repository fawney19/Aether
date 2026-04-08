use std::collections::{BTreeMap, BTreeSet};

use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use aether_provider_transport::provider_types::provider_type_supports_model_fetch;
use regex::Regex;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFetchRunSummary {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelsFetchSuccess {
    pub fetched_model_ids: Vec<String>,
    pub cached_models: Vec<Value>,
}

pub fn extract_error_message(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(Value::as_object)
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

pub fn build_models_fetch_url(
    provider_type: &str,
    endpoint_api_format: &str,
    base_url: &str,
) -> Option<(String, String)> {
    let api_format = normalize_api_format(endpoint_api_format);
    if !provider_type_supports_model_fetch(provider_type) {
        return None;
    }

    let url = if api_format.starts_with("openai:") || api_format.starts_with("claude:") {
        build_v1_models_url(base_url)
    } else if api_format.starts_with("gemini:") {
        build_gemini_models_url(base_url)
    } else {
        return None;
    }?;
    Some((url, api_format))
}

pub fn parse_models_response(
    endpoint_api_format: &str,
    body: &Value,
) -> Result<ModelsFetchSuccess, String> {
    let api_format = normalize_api_format(endpoint_api_format);
    let mut cached_models = Vec::new();
    let mut fetched_model_ids = Vec::new();
    let mut seen = BTreeSet::new();

    if api_format.starts_with("openai:") || api_format.starts_with("claude:") {
        let items = if let Some(items) = body.get("data").and_then(Value::as_array) {
            items
        } else if let Some(items) = body.as_array() {
            items
        } else {
            return Err("models response is missing data array".to_string());
        };
        for item in items {
            let Some(model_id) = item
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            if !seen.insert(model_id.to_string()) {
                continue;
            }
            fetched_model_ids.push(model_id.to_string());
            cached_models.push(normalize_cached_model(item, model_id, &api_format));
        }
    } else if api_format.starts_with("gemini:") {
        let items = body
            .get("models")
            .and_then(Value::as_array)
            .ok_or_else(|| "gemini models response is missing models array".to_string())?;
        for item in items {
            let Some(name) = item
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let model_id = name.strip_prefix("models/").unwrap_or(name).trim();
            if model_id.is_empty() || !seen.insert(model_id.to_string()) {
                continue;
            }
            fetched_model_ids.push(model_id.to_string());
            cached_models.push(normalize_cached_model(item, model_id, &api_format));
        }
    } else {
        return Err("models response parser does not support this provider format".to_string());
    }

    Ok(ModelsFetchSuccess {
        fetched_model_ids,
        cached_models,
    })
}

pub fn select_models_fetch_endpoint(
    endpoints: &[StoredProviderCatalogEndpoint],
    key: &StoredProviderCatalogKey,
) -> Option<StoredProviderCatalogEndpoint> {
    let key_formats = json_string_list(key.api_formats.as_ref())
        .into_iter()
        .map(|value| normalize_api_format(&value))
        .collect::<BTreeSet<_>>();
    endpoints
        .iter()
        .filter(|endpoint| endpoint.is_active)
        .find(|endpoint| {
            let api_format = normalize_api_format(&endpoint.api_format);
            (key_formats.is_empty() || key_formats.contains(&api_format))
                && endpoint_supports_rust_models_fetch(&endpoint.api_format)
        })
        .cloned()
}

pub fn endpoint_supports_rust_models_fetch(api_format: &str) -> bool {
    let api_format = normalize_api_format(api_format);
    matches!(
        api_format.as_str(),
        "openai:chat"
            | "openai:cli"
            | "openai:responses"
            | "openai:compact"
            | "claude:chat"
            | "claude:cli"
            | "gemini:chat"
            | "gemini:cli"
    )
}

pub fn apply_model_filters(
    fetched_model_ids: &[String],
    locked_models: Vec<String>,
    include_patterns: Vec<String>,
    exclude_patterns: Vec<String>,
) -> Vec<String> {
    let mut filtered = BTreeSet::new();
    for model_id in fetched_model_ids {
        if model_id.trim().is_empty() {
            continue;
        }
        let included = if include_patterns.is_empty() {
            true
        } else {
            include_patterns
                .iter()
                .any(|pattern| wildcard_matches(pattern, model_id))
        };
        if !included {
            continue;
        }
        let excluded = exclude_patterns
            .iter()
            .any(|pattern| wildcard_matches(pattern, model_id));
        if !excluded {
            filtered.insert(model_id.trim().to_string());
        }
    }
    for model in locked_models {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            filtered.insert(trimmed.to_string());
        }
    }
    filtered.into_iter().collect()
}

pub fn json_string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub fn aggregate_models_for_cache(models: &[Value]) -> Vec<Value> {
    let mut aggregated = BTreeMap::<String, serde_json::Map<String, Value>>::new();
    let mut order = Vec::<String>::new();

    for model in models {
        let Some(object) = model.as_object() else {
            continue;
        };
        let Some(model_id) = object
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };

        let entry = aggregated.entry(model_id.to_string()).or_insert_with(|| {
            order.push(model_id.to_string());
            let mut cloned = object.clone();
            cloned.remove("api_format");
            cloned
        });

        let api_formats = object
            .get("api_formats")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let legacy_api_format = object
            .get("api_format")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let existing_formats = entry
            .get("api_formats")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let mut merged_formats = existing_formats
            .union(&api_formats)
            .cloned()
            .collect::<BTreeSet<_>>();
        if let Some(api_format) = legacy_api_format {
            merged_formats.insert(api_format);
        }
        let merged_formats = merged_formats
            .into_iter()
            .map(Value::String)
            .collect::<Vec<_>>();
        entry.insert("api_formats".to_string(), Value::Array(merged_formats));

        for (key, value) in object {
            if key == "api_format" || entry.contains_key(key) {
                continue;
            }
            entry.insert(key.clone(), value.clone());
        }
    }

    order
        .into_iter()
        .filter_map(|model_id| aggregated.remove(&model_id))
        .map(Value::Object)
        .collect()
}

fn build_v1_models_url(base_url: &str) -> Option<String> {
    let (trimmed_base_url, query) = split_url_query(base_url);
    let trimmed_base_url = trimmed_base_url.trim_end_matches('/');
    if trimmed_base_url.is_empty() {
        return None;
    }
    let mut url = if trimmed_base_url.ends_with("/v1") {
        format!("{trimmed_base_url}/models")
    } else {
        format!("{trimmed_base_url}/v1/models")
    };
    if let Some(query) = query.filter(|value| !value.trim().is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    Some(url)
}

fn build_gemini_models_url(base_url: &str) -> Option<String> {
    let (trimmed_base_url, base_query) = split_url_query(base_url);
    let trimmed_base_url = trimmed_base_url.trim_end_matches('/');
    if trimmed_base_url.is_empty() {
        return None;
    }

    let mut url = if trimmed_base_url.ends_with("/v1beta") {
        format!("{trimmed_base_url}/models")
    } else if trimmed_base_url.contains("/v1beta/models") {
        trimmed_base_url.to_string()
    } else {
        format!("{trimmed_base_url}/v1beta/models")
    };
    if let Some(query) = base_query.filter(|value| !value.trim().is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    Some(url)
}

fn split_url_query(base_url: &str) -> (&str, Option<&str>) {
    let trimmed = base_url.trim();
    trimmed
        .split_once('?')
        .map(|(base, query)| (base, Some(query)))
        .unwrap_or((trimmed, None))
}

fn normalize_cached_model(item: &Value, model_id: &str, api_format: &str) -> Value {
    let mut object = item.as_object().cloned().unwrap_or_default();
    object.insert("id".to_string(), Value::String(model_id.to_string()));
    object.insert(
        "api_formats".to_string(),
        Value::Array(vec![Value::String(api_format.to_string())]),
    );
    object.remove("api_format");
    Value::Object(object)
}

fn wildcard_matches(pattern: &str, model_id: &str) -> bool {
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            other => regex.push_str(&regex::escape(&other.to_string())),
        }
    }
    regex.push('$');
    Regex::new(&regex)
        .ok()
        .is_some_and(|compiled| compiled.is_match(model_id))
}

fn normalize_api_format(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    };
    use serde_json::json;

    use super::{
        aggregate_models_for_cache, apply_model_filters, build_gemini_models_url,
        build_models_fetch_url, parse_models_response, select_models_fetch_endpoint,
    };

    fn sample_endpoint(
        provider_id: &str,
        endpoint_id: &str,
        api_format: &str,
        base_url: &str,
    ) -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            endpoint_id.to_string(),
            provider_id.to_string(),
            api_format.to_string(),
            None,
            None,
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            base_url.to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_key(provider_id: &str, key_id: &str) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            key_id.to_string(),
            provider_id.to_string(),
            "primary".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["openai:chat"])),
            "encrypted".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    #[test]
    fn apply_model_filters_respects_include_exclude_and_locked_models() {
        let filtered = apply_model_filters(
            &[
                "gpt-5".to_string(),
                "gpt-beta".to_string(),
                "claude-4".to_string(),
            ],
            vec!["locked-model".to_string()],
            vec!["gpt-*".to_string()],
            vec!["gpt-beta".to_string()],
        );
        assert_eq!(
            filtered,
            vec!["gpt-5".to_string(), "locked-model".to_string()]
        );
    }

    #[test]
    fn aggregate_models_for_cache_merges_api_formats_by_model_id() {
        let aggregated = aggregate_models_for_cache(&[
            json!({"id":"gpt-5","api_formats":["openai:chat"]}),
            json!({"id":"gpt-5","api_formats":["openai:cli"]}),
        ]);
        assert_eq!(aggregated.len(), 1);
        assert_eq!(
            aggregated[0]["api_formats"],
            json!(["openai:chat", "openai:cli"])
        );
    }

    #[test]
    fn aggregate_models_for_cache_preserves_legacy_api_format_field() {
        let aggregated = aggregate_models_for_cache(&[json!({
            "id":"gpt-5",
            "api_format":"openai:chat"
        })]);
        assert_eq!(aggregated.len(), 1);
        assert_eq!(aggregated[0]["api_formats"], json!(["openai:chat"]));
        assert!(aggregated[0].get("api_format").is_none());
    }

    #[test]
    fn build_gemini_models_url_preserves_base_query() {
        let url =
            build_gemini_models_url("https://generativelanguage.googleapis.com/v1beta?key=abc")
                .expect("gemini models url should build");
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models?key=abc"
        );
    }

    #[test]
    fn build_models_fetch_url_rejects_provider_types_without_fetch_support() {
        assert_eq!(
            build_models_fetch_url("vertex_ai", "gemini:chat", "https://example.com"),
            None
        );
    }

    #[test]
    fn parse_models_response_normalizes_openai_payload() {
        let parsed = parse_models_response(
            "openai:chat",
            &json!({"data": [{"id": "gpt-5"}, {"id": "gpt-5"}]}),
        )
        .expect("response should parse");
        assert_eq!(parsed.fetched_model_ids, vec!["gpt-5".to_string()]);
        assert_eq!(
            parsed.cached_models[0]["api_formats"],
            json!(["openai:chat"])
        );
    }

    #[test]
    fn select_models_fetch_endpoint_respects_key_api_formats() {
        let key = sample_key("provider-1", "key-1");
        let endpoints = vec![
            sample_endpoint(
                "provider-1",
                "endpoint-cli",
                "openai:cli",
                "https://example.com",
            ),
            sample_endpoint(
                "provider-1",
                "endpoint-chat",
                "openai:chat",
                "https://example.com",
            ),
        ];
        let selected =
            select_models_fetch_endpoint(&endpoints, &key).expect("endpoint should be selected");
        assert_eq!(selected.id, "endpoint-chat");
    }
}
