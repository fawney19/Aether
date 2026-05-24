use std::collections::{BTreeMap, BTreeSet};

use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use regex::Regex;
use serde_json::{json, Value};

const MODEL_FETCH_FORMAT_PRIORITY: &[&[&str]] = &[
    &[
        "openai:chat",
        "openai:responses",
        "openai:responses:compact",
    ],
    &["claude:messages"],
    &["gemini:generate_content"],
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
    provider_type: &str,
    endpoint_api_format: &str,
    base_url: &str,
) -> Option<(String, String)> {
    let api_format = normalize_api_format(endpoint_api_format);
    if !endpoint_supports_rust_models_fetch(&api_format) {
        return None;
    }
    let provider_type = provider_type.trim().to_ascii_lowercase();
    let url = if provider_type == "codex" && api_format.starts_with("openai:") {
        build_codex_models_url(base_url)
    } else if api_format.starts_with("openai:") || api_format.starts_with("claude:") {
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
        } else if let Some(items) = body.get("models").and_then(Value::as_array) {
            items
        } else {
            return Err("models response is missing data array".to_string());
        };
        for item in items {
            let Some(model_id) = model_id_from_openai_like_item(item) else {
                continue;
            };
            if !seen.insert(model_id.clone()) {
                continue;
            }
            fetched_model_ids.push(model_id.clone());
            cached_models.push(normalize_cached_model(item, &model_id, &api_format));
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

pub fn parse_windsurf_model_configs_response(
    body: &Value,
    updated_at_unix_secs: u64,
) -> Result<(ModelsFetchSuccess, Value), String> {
    let configs = body
        .get("clientModelConfigs")
        .or_else(|| body.get("client_model_configs"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "windsurf model configs response is missing clientModelConfigs".to_string()
        })?;

    let mut cached_models = Vec::new();
    let mut metadata_models = Vec::new();
    let mut seen = BTreeSet::new();
    for config in configs {
        let Some(model_id) =
            windsurf_model_config_string(config, &["modelUid", "model_uid", "id", "name"])
        else {
            continue;
        };
        if !seen.insert(model_id.clone()) {
            continue;
        }

        let label = windsurf_model_config_string(config, &["label", "displayName", "display_name"]);
        let provider = windsurf_model_config_string(config, &["provider"]);
        let supports_images = config
            .get("supportsImages")
            .or_else(|| config.get("supports_images"))
            .and_then(windsurf_json_bool);
        let credit_multiplier = config
            .get("creditMultiplier")
            .or_else(|| config.get("credit_multiplier"))
            .and_then(windsurf_json_f64);

        let mut model = serde_json::Map::new();
        model.insert("id".to_string(), json!(model_id.clone()));
        model.insert("object".to_string(), json!("model"));
        model.insert("model_uid".to_string(), json!(model_id.clone()));
        model.insert(
            "display_name".to_string(),
            json!(label.as_deref().unwrap_or(model_id.as_str())),
        );
        model.insert(
            "owned_by".to_string(),
            json!(provider.as_deref().unwrap_or("windsurf")),
        );
        model.insert(
            "api_formats".to_string(),
            json!(["openai:chat", "openai:responses", "claude:messages"]),
        );
        if let Some(supports_images) = supports_images {
            model.insert("supports_images".to_string(), json!(supports_images));
        }
        if let Some(credit_multiplier) = credit_multiplier {
            model.insert("credit_multiplier".to_string(), json!(credit_multiplier));
        }
        cached_models.push(Value::Object(model));

        let mut metadata_model = serde_json::Map::new();
        metadata_model.insert("model_uid".to_string(), json!(model_id));
        if let Some(label) = label {
            metadata_model.insert("label".to_string(), json!(label));
        }
        if let Some(provider) = provider {
            metadata_model.insert("provider".to_string(), json!(provider));
        }
        if let Some(supports_images) = supports_images {
            metadata_model.insert("supports_images".to_string(), json!(supports_images));
        }
        if let Some(credit_multiplier) = credit_multiplier {
            metadata_model.insert("credit_multiplier".to_string(), json!(credit_multiplier));
        }
        metadata_models.push(Value::Object(metadata_model));
    }

    let mut windsurf_metadata = serde_json::Map::new();
    windsurf_metadata.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    windsurf_metadata.insert(
        "allowed_models_count".to_string(),
        json!(metadata_models.len() as u64),
    );
    windsurf_metadata.insert("models".to_string(), Value::Array(metadata_models));
    if let Some(default_model_uid) = body
        .get("defaultOverrideModelConfig")
        .or_else(|| body.get("default_override_model_config"))
        .and_then(|config| windsurf_model_config_string(config, &["modelUid", "model_uid"]))
    {
        windsurf_metadata.insert("default_model_uid".to_string(), json!(default_model_uid));
    }

    Ok((
        ModelsFetchSuccess {
            fetched_model_ids: collect_cached_model_ids(&cached_models),
            cached_models,
        },
        json!({ "windsurf": windsurf_metadata }),
    ))
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
        if let Some(existing) = by_format.get_mut(&api_format) {
            if endpoint.api_format.trim().eq_ignore_ascii_case(&api_format)
                && !existing.api_format.trim().eq_ignore_ascii_case(&api_format)
            {
                *existing = endpoint.clone();
            }
        } else {
            by_format.insert(api_format, endpoint.clone());
        }
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
            | "openai:responses"
            | "openai:responses:compact"
            | "claude:messages"
            | "gemini:generate_content"
    )
}

pub fn provider_type_uses_preset_models(provider_type: &str) -> bool {
    matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "claude_code" | "gemini_cli" | "grok"
    )
}

#[rustfmt::skip]
pub fn preset_models_for_provider(provider_type: &str) -> Option<Vec<Value>> {
    let models = match provider_type.trim().to_ascii_lowercase().as_str() {
        "gemini_cli" => vec![
            preset_model("gemini-2.5-pro", "google", "Gemini 2.5 Pro", "gemini:generate_content"),
            preset_model("gemini-2.5-flash", "google", "Gemini 2.5 Flash", "gemini:generate_content"),
            preset_model("gemini-3-pro-preview", "google", "Gemini 3 Pro Preview", "gemini:generate_content"),
            preset_model("gemini-3-flash-preview", "google", "Gemini 3 Flash Preview", "gemini:generate_content"),
            preset_model("gemini-3.1-pro-preview", "google", "Gemini 3.1 Pro Preview", "gemini:generate_content"),
        ],
        "kiro" => vec![
            preset_model("auto", "kiro", "Auto", "claude:messages"),
            preset_model("claude-opus-4.7", "anthropic", "Claude Opus 4.7", "claude:messages"),
            preset_model("claude-opus-4.6", "anthropic", "Claude Opus 4.6", "claude:messages"),
            preset_model("claude-sonnet-4.6", "anthropic", "Claude Sonnet 4.6", "claude:messages"),
            preset_model("claude-opus-4.5", "anthropic", "Claude Opus 4.5", "claude:messages"),
            preset_model("claude-sonnet-4.5", "anthropic", "Claude Sonnet 4.5", "claude:messages"),
            preset_model("claude-sonnet-4", "anthropic", "Claude Sonnet 4", "claude:messages"),
            preset_model("claude-haiku-4.5", "anthropic", "Claude Haiku 4.5", "claude:messages"),
            preset_model("deepseek-3.2", "deepseek", "Deepseek v3.2", "claude:messages"),
            preset_model("minimax-m2.5", "minimax", "MiniMax M2.5", "claude:messages"),
            preset_model("minimax-m2.1", "minimax", "MiniMax M2.1", "claude:messages"),
            preset_model("glm-5", "zhipu", "GLM 5", "claude:messages"),
            preset_model("qwen3-coder-next", "alibaba", "Qwen3 Coder Next", "claude:messages"),
        ],
        "claude_code" => vec![
            preset_model("claude-opus-4-5-20251101", "anthropic", "Claude Opus 4.5", "claude:messages"),
            preset_model("claude-opus-4-6", "anthropic", "Claude Opus 4.6", "claude:messages"),
            preset_model("claude-sonnet-4-6", "anthropic", "Claude Sonnet 4.6", "claude:messages"),
            preset_model("claude-sonnet-4-5-20250929", "anthropic", "Claude Sonnet 4.5", "claude:messages"),
            preset_model("claude-haiku-4-5-20251001", "anthropic", "Claude Haiku 4.5", "claude:messages"),
        ],
        "codex" => vec![
            preset_model("gpt-5.5", "openai", "GPT-5.5", "openai:responses"),
            preset_model("gpt-5.4", "openai", "GPT-5.4", "openai:responses"),
            preset_model("gpt-5.4-mini", "openai", "GPT-5.4 Mini", "openai:responses"),
            preset_model("gpt-5.3-codex", "openai", "GPT-5.3 Codex", "openai:responses"),
            preset_model("gpt-5.3-codex-spark", "openai", "GPT-5.3 Codex Spark", "openai:responses"),
        ],
        "grok" => vec![
            preset_model("grok-4.20-0309-non-reasoning", "xai", "Grok 4.20 0309 Non-Reasoning", "openai:chat"),
            preset_model("grok-4.20-0309", "xai", "Grok 4.20 0309", "openai:chat"),
            preset_model("grok-4.20-0309-reasoning", "xai", "Grok 4.20 0309 Reasoning", "openai:chat"),
            preset_model("grok-4.20-0309-non-reasoning-super", "xai", "Grok 4.20 0309 Non-Reasoning Super", "openai:chat"),
            preset_model("grok-4.20-0309-super", "xai", "Grok 4.20 0309 Super", "openai:chat"),
            preset_model("grok-4.20-0309-reasoning-super", "xai", "Grok 4.20 0309 Reasoning Super", "openai:chat"),
            preset_model("grok-4.20-0309-non-reasoning-heavy", "xai", "Grok 4.20 0309 Non-Reasoning Heavy", "openai:chat"),
            preset_model("grok-4.20-0309-heavy", "xai", "Grok 4.20 0309 Heavy", "openai:chat"),
            preset_model("grok-4.20-0309-reasoning-heavy", "xai", "Grok 4.20 0309 Reasoning Heavy", "openai:chat"),
            preset_model("grok-4.20-multi-agent-0309", "xai", "Grok 4.20 Multi-Agent 0309", "openai:chat"),
            preset_model("grok-4.20-auto", "xai", "Grok 4.20 Auto", "openai:chat"),
            preset_model("grok-4.20-fast", "xai", "Grok 4.20 Fast", "openai:chat"),
            preset_model("grok-4.20-expert", "xai", "Grok 4.20 Expert", "openai:chat"),
            preset_model("grok-4.20-heavy", "xai", "Grok 4.20 Heavy", "openai:chat"),
            preset_model("grok-4.3-beta", "xai", "Grok 4.3 Beta", "openai:chat"),
            preset_model("grok-imagine-image-lite", "xai", "Grok Imagine Image Lite", "openai:image"),
            preset_model("grok-imagine-image", "xai", "Grok Imagine Image", "openai:image"),
            preset_model("grok-imagine-image-pro", "xai", "Grok Imagine Image Pro", "openai:image"),
            preset_model("grok-imagine-image-edit", "xai", "Grok Imagine Image Edit", "openai:image"),
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

fn api_format_priority(api_format: &str) -> Option<(usize, usize)> {
    MODEL_FETCH_FORMAT_PRIORITY
        .iter()
        .enumerate()
        .find_map(|(group_index, group)| {
            group
                .iter()
                .position(|candidate| candidate.eq_ignore_ascii_case(api_format))
                .map(|format_index| (group_index, format_index))
        })
}

fn sorted_api_formats(formats: BTreeSet<String>) -> Vec<String> {
    let mut formats = formats.into_iter().collect::<Vec<_>>();
    formats.sort_by(
        |left, right| match (api_format_priority(left), api_format_priority(right)) {
            (Some(left_priority), Some(right_priority)) => left_priority.cmp(&right_priority),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.cmp(right),
        },
    );
    formats
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
        let merged_formats = sorted_api_formats(merged_formats)
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

fn build_codex_models_url(base_url: &str) -> Option<String> {
    let (trimmed_base_url, query) = split_url_query(base_url);
    let trimmed_base_url = trimmed_base_url.trim_end_matches('/');
    if trimmed_base_url.is_empty() {
        return None;
    }
    let mut url = if trimmed_base_url.ends_with("/models") {
        trimmed_base_url.to_string()
    } else {
        format!("{trimmed_base_url}/models")
    };
    let mut has_client_version = false;
    if let Some(query) = query.filter(|value| !value.trim().is_empty()) {
        has_client_version = query.split('&').any(|part| {
            part.split_once('=')
                .map(|(key, _)| key)
                .unwrap_or(part)
                .trim()
                .eq_ignore_ascii_case("client_version")
        });
        url.push('?');
        url.push_str(query);
    }
    if !has_client_version {
        let separator = if url.contains('?') { '&' } else { '?' };
        url.push(separator);
        url.push_str("client_version=0.128.0-alpha.1");
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

fn model_id_from_openai_like_item(item: &Value) -> Option<String> {
    if let Some(value) = item
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(value.trim_start_matches("models/").to_string());
    }

    ["id", "model", "slug", "name"].iter().find_map(|field| {
        item.get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.trim_start_matches("models/").to_string())
    })
}

fn windsurf_model_config_string(value: &Value, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        value
            .get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn windsurf_json_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn windsurf_json_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn collect_cached_model_ids(models: &[Value]) -> Vec<String> {
    let mut ids = Vec::new();
    for model in models {
        let Some(model_id) = model
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        ids.push(model_id.to_string());
    }
    ids
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
    aether_ai_formats::normalize_api_format_alias(value)
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
            json!({"id":"alpha","api_formats":["openai:responses"]}),
            json!({"id":"alpha","api_formats":["openai:chat"]}),
        ]);
        assert_eq!(aggregated.len(), 2);
        assert_eq!(aggregated[0]["id"], "alpha");
        assert_eq!(aggregated[1]["id"], "zeta");
        assert_eq!(
            aggregated[0]["api_formats"],
            json!(["openai:chat", "openai:responses"])
        );
    }

    #[test]
    fn aggregate_models_for_cache_orders_api_formats_by_canonical_priority() {
        let aggregated = aggregate_models_for_cache(&[
            json!({"id":"claude-sonnet-4-6","api_formats":["claude:messages"]}),
            json!({"id":"claude-sonnet-4-6","api_formats":["openai:responses"]}),
            json!({"id":"claude-sonnet-4-6","api_formats":["openai:chat"]}),
        ]);
        assert_eq!(aggregated.len(), 1);
        assert_eq!(
            aggregated[0]["api_formats"],
            json!(["openai:chat", "openai:responses", "claude:messages"])
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
    fn build_models_fetch_url_supports_openai_responses() {
        assert_eq!(
            build_models_fetch_url("openai", "openai:responses", "https://example.com"),
            Some((
                "https://example.com/v1/models".to_string(),
                "openai:responses".to_string()
            ))
        );
    }

    #[test]
    fn build_models_fetch_url_uses_codex_backend_models_endpoint() {
        assert_eq!(
            build_models_fetch_url(
                "codex",
                "openai:responses",
                "https://chatgpt.com/backend-api/codex"
            ),
            Some((
                "https://chatgpt.com/backend-api/codex/models?client_version=0.128.0-alpha.1"
                    .to_string(),
                "openai:responses".to_string()
            ))
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
    fn parse_models_response_accepts_codex_models_array_payload() {
        let parsed = parse_models_response(
            "openai:responses",
            &json!({"models": [{"id": "gpt-5-codex"}, {"slug": "gpt-5.4"}]}),
        )
        .expect("response should parse");
        assert_eq!(
            parsed.fetched_model_ids,
            vec!["gpt-5-codex".to_string(), "gpt-5.4".to_string()]
        );
        assert_eq!(
            parsed.cached_models[0]["api_formats"],
            json!(["openai:responses"])
        );
    }

    #[test]
    fn parse_models_response_page_reads_claude_pagination_state() {
        let parsed = parse_models_response_page(
            "claude:messages",
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
    fn selected_models_fetch_endpoints_prefers_chat_then_responses() {
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
                "endpoint-compact",
                "openai:responses:compact",
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

        let key = sample_key("provider-1", "key-1", &["openai:responses"]);
        let endpoints = vec![
            sample_endpoint(
                "provider-1",
                "endpoint-compact",
                "openai:responses:compact",
                "https://example.com",
            ),
            sample_endpoint(
                "provider-1",
                "endpoint-responses",
                "openai:responses",
                "https://example.com",
            ),
        ];
        let selected = selected_models_fetch_endpoints(&endpoints, &key);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "endpoint-responses");
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
        let model_ids = models
            .iter()
            .map(|model| model["id"].as_str().expect("model id"))
            .collect::<Vec<_>>();
        assert_eq!(
            model_ids,
            vec![
                "gpt-5.5",
                "gpt-5.4",
                "gpt-5.4-mini",
                "gpt-5.3-codex",
                "gpt-5.3-codex-spark",
            ]
        );
    }

    #[test]
    fn preset_models_cover_kiro_catalog() {
        let models = preset_models_for_provider("kiro").expect("preset models should exist");
        let model_ids = models
            .iter()
            .map(|model| model["id"].as_str().expect("model id"))
            .collect::<Vec<_>>();
        assert_eq!(
            model_ids,
            vec![
                "auto",
                "claude-opus-4.7",
                "claude-opus-4.6",
                "claude-sonnet-4.6",
                "claude-opus-4.5",
                "claude-sonnet-4.5",
                "claude-sonnet-4",
                "claude-haiku-4.5",
                "deepseek-3.2",
                "minimax-m2.5",
                "minimax-m2.1",
                "glm-5",
                "qwen3-coder-next",
            ]
        );
        assert!(models
            .iter()
            .all(|model| model["api_formats"] == json!(["claude:messages"])));
    }

    #[test]
    fn preset_models_cover_grok_non_video_catalog() {
        let models = preset_models_for_provider("grok").expect("preset models should exist");
        let model_ids = models
            .iter()
            .map(|model| model["id"].as_str().expect("model id"))
            .collect::<Vec<_>>();
        assert_eq!(
            model_ids,
            vec![
                "grok-4.20-0309-non-reasoning",
                "grok-4.20-0309",
                "grok-4.20-0309-reasoning",
                "grok-4.20-0309-non-reasoning-super",
                "grok-4.20-0309-super",
                "grok-4.20-0309-reasoning-super",
                "grok-4.20-0309-non-reasoning-heavy",
                "grok-4.20-0309-heavy",
                "grok-4.20-0309-reasoning-heavy",
                "grok-4.20-multi-agent-0309",
                "grok-4.20-auto",
                "grok-4.20-fast",
                "grok-4.20-expert",
                "grok-4.20-heavy",
                "grok-4.3-beta",
                "grok-imagine-image-lite",
                "grok-imagine-image",
                "grok-imagine-image-pro",
                "grok-imagine-image-edit",
            ]
        );
        assert!(!model_ids.contains(&"grok-imagine-video"));
        assert_eq!(models[0]["api_formats"], json!(["openai:chat"]));
        assert_eq!(models[10]["api_formats"], json!(["openai:chat"]));
        assert_eq!(models[15]["api_formats"], json!(["openai:image"]));
        assert_eq!(models[18]["api_formats"], json!(["openai:image"]));
    }
}
