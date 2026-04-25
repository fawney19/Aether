use std::collections::{BTreeMap, BTreeSet};

use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use regex::Regex;
use serde_json::{json, Value};

const MODEL_FETCH_FORMAT_PRIORITY: &[&[&str]] = &[
    &["openai:chat", "openai:cli", "openai:compact"],
    &["claude:chat", "claude:cli"],
    &["gemini:chat", "gemini:cli"],
];

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

#[derive(Debug, Clone, PartialEq)]
pub struct ModelsFetchPage {
    pub fetched_model_ids: Vec<String>,
    pub cached_models: Vec<Value>,
    pub has_more: bool,
    pub next_after_id: Option<String>,
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
    _provider_type: &str,
    endpoint_api_format: &str,
    base_url: &str,
) -> Option<(String, String)> {
    let api_format = normalize_api_format(endpoint_api_format);
    if !endpoint_supports_rust_models_fetch(&api_format) {
        return None;
    }
    let url = if api_format.starts_with("openai:") || api_format.starts_with("claude:") {
        build_v1_models_url(base_url)
    } else if api_format.starts_with("gemini:") {
        build_gemini_models_url(base_url)
    } else {
        None
    }?;
    Some((url, api_format))
}

pub fn parse_models_response(
    endpoint_api_format: &str,
    body: &Value,
) -> Result<ModelsFetchSuccess, String> {
    let parsed = parse_models_response_page(endpoint_api_format, body)?;
    Ok(ModelsFetchSuccess {
        fetched_model_ids: parsed.fetched_model_ids,
        cached_models: parsed.cached_models,
    })
}

pub fn parse_models_response_page(
    endpoint_api_format: &str,
    body: &Value,
) -> Result<ModelsFetchPage, String> {
    let api_format = normalize_api_format(endpoint_api_format);
    let mut cached_models = Vec::new();
    let mut fetched_model_ids = Vec::new();
    let mut seen = BTreeSet::new();
    let mut has_more = false;
    let mut next_after_id = None;

    if api_format.starts_with("openai:") || api_format.starts_with("claude:") {
        let items = if let Some(items) = body.get("data").and_then(Value::as_array) {
            has_more = body
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if api_format.starts_with("claude:") && has_more {
                next_after_id = body
                    .get("last_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
            }
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

    Ok(ModelsFetchPage {
        fetched_model_ids,
        cached_models,
        has_more,
        next_after_id,
    })
}

pub fn selected_models_fetch_endpoints(
    endpoints: &[StoredProviderCatalogEndpoint],
    key: &StoredProviderCatalogKey,
) -> Vec<StoredProviderCatalogEndpoint> {
    let key_formats = json_string_list(key.api_formats.as_ref())
        .into_iter()
        .map(|value| normalize_api_format(&value))
        .collect::<BTreeSet<_>>();
    let mut by_format = BTreeMap::<String, StoredProviderCatalogEndpoint>::new();

    for endpoint in endpoints.iter().filter(|endpoint| endpoint.is_active) {
        let api_format = normalize_api_format(&endpoint.api_format);
        if api_format.is_empty() || !endpoint_supports_rust_models_fetch(&api_format) {
            continue;
        }
        if !key_formats.is_empty() && !key_formats.contains(&api_format) {
            continue;
        }
        by_format
            .entry(api_format)
            .or_insert_with(|| endpoint.clone());
    }

    MODEL_FETCH_FORMAT_PRIORITY
        .iter()
        .filter_map(|candidates| {
            candidates
                .iter()
                .find_map(|api_format| by_format.remove(*api_format))
        })
        .collect()
}

pub fn select_models_fetch_endpoint(
    endpoints: &[StoredProviderCatalogEndpoint],
    key: &StoredProviderCatalogKey,
) -> Option<StoredProviderCatalogEndpoint> {
    selected_models_fetch_endpoints(endpoints, key)
        .into_iter()
        .next()
}

pub fn endpoint_supports_rust_models_fetch(api_format: &str) -> bool {
    let api_format = normalize_api_format(api_format);
    matches!(
        api_format.as_str(),
        "openai:chat"
            | "openai:cli"
            | "openai:compact"
            | "claude:chat"
            | "claude:cli"
            | "gemini:chat"
            | "gemini:cli"
    )
}

pub fn provider_type_uses_preset_models(provider_type: &str) -> bool {
    matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "codex" | "kiro" | "claude_code" | "gemini_cli"
    )
}

#[rustfmt::skip]
pub fn preset_models_for_provider(provider_type: &str) -> Option<Vec<Value>> {
    let models = match provider_type.trim().to_ascii_lowercase().as_str() {
        "gemini_cli" => vec![
            preset_model("gemini-2.5-pro", "google", "Gemini 2.5 Pro", "gemini:cli"),
            preset_model("gemini-2.5-flash", "google", "Gemini 2.5 Flash", "gemini:cli"),
            preset_model("gemini-3-pro-preview", "google", "Gemini 3 Pro Preview", "gemini:cli"),
            preset_model("gemini-3-flash-preview", "google", "Gemini 3 Flash Preview", "gemini:cli"),
            preset_model("gemini-3.1-pro-preview", "google", "Gemini 3.1 Pro Preview", "gemini:cli"),
        ],
        "kiro" => vec![
            preset_model("claude-sonnet-4.5", "anthropic", "Claude Sonnet 4.5", "claude:cli"),
            preset_model("claude-sonnet-4.6", "anthropic", "Claude Sonnet 4.6", "claude:cli"),
            preset_model("claude-opus-4.5", "anthropic", "Claude Opus 4.5", "claude:cli"),
            preset_model("claude-opus-4.6", "anthropic", "Claude Opus 4.6", "claude:cli"),
            preset_model("claude-haiku-4.5", "anthropic", "Claude Haiku 4.5", "claude:cli"),
        ],
        "claude_code" => vec![
            preset_model("claude-opus-4-5-20251101", "anthropic", "Claude Opus 4.5", "claude:cli"),
            preset_model("claude-opus-4-6", "anthropic", "Claude Opus 4.6", "claude:cli"),
            preset_model("claude-sonnet-4-6", "anthropic", "Claude Sonnet 4.6", "claude:cli"),
            preset_model("claude-sonnet-4-5-20250929", "anthropic", "Claude Sonnet 4.5", "claude:cli"),
            preset_model("claude-haiku-4-5-20251001", "anthropic", "Claude Haiku 4.5", "claude:cli"),
        ],
        "codex" => vec![
            preset_model("gpt-5", "openai", "GPT-5", "openai:cli"),
            preset_model("gpt-image-1", "openai", "GPT Image 1", "openai:image"),
            preset_model("gpt-image-1.5", "openai", "GPT Image 1.5", "openai:image"),
            preset_model("gpt-image-1-mini", "openai", "GPT Image 1 Mini", "openai:image"),
            preset_model("gpt-image-2", "openai", "GPT Image 2", "openai:image"),
            preset_model("chatgpt-image-latest", "openai", "ChatGPT Image Latest", "openai:image"),
            preset_model("dall-e-2", "openai", "DALL-E 2", "openai:image"),
            preset_model("dall-e-3", "openai", "DALL-E 3", "openai:image"),
            preset_model("gpt-5-codex", "openai", "GPT-5 Codex", "openai:cli"),
            preset_model("gpt-5-codex-mini", "openai", "GPT-5 Codex Mini", "openai:cli"),
            preset_model("gpt-5.1", "openai", "GPT-5.1", "openai:cli"),
            preset_model("gpt-5.1-codex", "openai", "GPT-5.1 Codex", "openai:cli"),
            preset_model("gpt-5.1-codex-mini", "openai", "GPT-5.1 Codex Mini", "openai:cli"),
            preset_model("gpt-5.1-codex-max", "openai", "GPT-5.1 Codex Max", "openai:cli"),
            preset_model("gpt-5.2", "openai", "GPT-5.2", "openai:cli"),
            preset_model("gpt-5.2-codex", "openai", "GPT-5.2 Codex", "openai:cli"),
            preset_model("gpt-5.3-codex", "openai", "GPT-5.3 Codex", "openai:cli"),
            preset_model("gpt-5.4", "openai", "GPT-5.4", "openai:cli"),
        ],
        _ => return None,
    };
    Some(models)
}

pub fn merge_upstream_metadata(current: Option<&Value>, incoming: &Value) -> Value {
    let mut merged = current
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let Some(incoming_object) = incoming.as_object() else {
        return Value::Object(merged);
    };

    for (namespace, value) in incoming_object {
        let mut next_value = value.clone();
        if let (Some(next_namespace), Some(old_namespace)) = (
            next_value.as_object_mut(),
            merged.get(namespace).and_then(Value::as_object),
        ) {
            if let (Some(new_quota), Some(old_quota)) = (
                next_namespace
                    .get_mut("quota_by_model")
                    .and_then(Value::as_object_mut),
                old_namespace
                    .get("quota_by_model")
                    .and_then(Value::as_object),
            ) {
                for (model_id, new_info) in new_quota.iter_mut() {
                    let Some(new_info_object) = new_info.as_object_mut() else {
                        continue;
                    };
                    let Some(old_info_object) = old_quota.get(model_id).and_then(Value::as_object)
                    else {
                        continue;
                    };
                    if !new_info_object.contains_key("reset_time") {
                        if let Some(reset_time) = old_info_object.get("reset_time") {
                            new_info_object.insert("reset_time".to_string(), reset_time.clone());
                        }
                    }
                }
            }
        }
        merged.insert(namespace.clone(), next_value);
    }

    Value::Object(merged)
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

    aggregated.into_values().map(Value::Object).collect()
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
    if api_format.starts_with("gemini:") {
        object
            .entry("owned_by".to_string())
            .or_insert_with(|| Value::String("google".to_string()));
        if !object.contains_key("display_name") {
            let display_name = item
                .get("displayName")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(model_id);
            object.insert(
                "display_name".to_string(),
                Value::String(display_name.to_string()),
            );
        }
    }
    object.remove("api_format");
    Value::Object(object)
}

fn preset_model(model_id: &str, owned_by: &str, display_name: &str, api_format: &str) -> Value {
    json!({
        "id": model_id,
        "object": "model",
        "owned_by": owned_by,
        "display_name": display_name,
        "api_formats": [api_format],
    })
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
        build_models_fetch_url, merge_upstream_metadata, parse_models_response,
        parse_models_response_page, preset_models_for_provider, selected_models_fetch_endpoints,
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

    fn sample_key(
        provider_id: &str,
        key_id: &str,
        api_formats: &[&str],
    ) -> StoredProviderCatalogKey {
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
            Some(json!(api_formats)),
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
    fn aggregate_models_for_cache_merges_api_formats_and_sorts_by_model_id() {
        let aggregated = aggregate_models_for_cache(&[
            json!({"id":"zeta","api_formats":["openai:chat"]}),
            json!({"id":"alpha","api_formats":["openai:cli"]}),
            json!({"id":"alpha","api_formats":["openai:chat"]}),
        ]);
        assert_eq!(aggregated.len(), 2);
        assert_eq!(aggregated[0]["id"], "alpha");
        assert_eq!(aggregated[1]["id"], "zeta");
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
    fn build_models_fetch_url_excludes_openai_responses() {
        assert_eq!(
            build_models_fetch_url("openai", "openai:responses", "https://example.com"),
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
    fn parse_models_response_page_reads_claude_pagination_state() {
        let parsed = parse_models_response_page(
            "claude:chat",
            &json!({
                "data": [{"id": "claude-sonnet-4"}],
                "has_more": true,
                "last_id": "cursor-2"
            }),
        )
        .expect("response should parse");
        assert!(parsed.has_more);
        assert_eq!(parsed.next_after_id.as_deref(), Some("cursor-2"));
    }

    #[test]
    fn selected_models_fetch_endpoints_prefers_chat_and_excludes_responses() {
        let key = sample_key("provider-1", "key-1", &["openai:chat", "openai:responses"]);
        let endpoints = vec![
            sample_endpoint(
                "provider-1",
                "endpoint-responses",
                "openai:responses",
                "https://example.com",
            ),
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
        let selected = selected_models_fetch_endpoints(&endpoints, &key);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "endpoint-chat");
    }

    #[test]
    fn merge_upstream_metadata_keeps_existing_reset_time_for_returned_models() {
        let merged = merge_upstream_metadata(
            Some(&json!({
                "antigravity": {
                    "quota_by_model": {
                        "gemini-2.5-pro": {
                            "remaining_fraction": 0.3,
                            "reset_time": "2026-04-12T00:00:00Z"
                        },
                        "stale-model": {
                            "remaining_fraction": 0.1,
                            "reset_time": "old"
                        }
                    }
                }
            })),
            &json!({
                "antigravity": {
                    "quota_by_model": {
                        "gemini-2.5-pro": {
                            "remaining_fraction": 0.6
                        }
                    }
                }
            }),
        );
        assert_eq!(
            merged["antigravity"]["quota_by_model"]["gemini-2.5-pro"]["reset_time"],
            "2026-04-12T00:00:00Z"
        );
        assert!(merged["antigravity"]["quota_by_model"]
            .get("stale-model")
            .is_none());
    }

    #[test]
    fn preset_models_cover_codex_catalog() {
        let models = preset_models_for_provider("codex").expect("preset models should exist");
        assert!(models.iter().any(|model| model["id"] == "gpt-5.4"));
    }
}
