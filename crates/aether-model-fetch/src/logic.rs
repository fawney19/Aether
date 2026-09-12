use std::collections::{BTreeMap, BTreeSet};

use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use aether_provider_transport::provider_types::is_codex_cli_backend_url;
use aether_provider_transport::url::{
    build_bigmodel_coding_models_url, build_openai_compatible_models_url,
    openai_compatible_base_includes_unversioned_api_root,
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

pub(crate) const CODEX_MODELS_MAX_ITEMS: usize = 512;
pub(crate) const CODEX_MODELS_MAX_JSON_BYTES: usize = 8 * 1024 * 1024;

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
    build_models_fetch_url_for_client_version(provider_type, endpoint_api_format, base_url, None)
}

pub fn build_models_fetch_url_for_client_version(
    provider_type: &str,
    endpoint_api_format: &str,
    base_url: &str,
    codex_client_version: Option<&str>,
) -> Option<(String, String)> {
    let api_format = normalize_api_format(endpoint_api_format);
    if !endpoint_supports_rust_models_fetch(&api_format) {
        return None;
    }
    let provider_type = provider_type.trim().to_ascii_lowercase();
    let url = if provider_type == "codex" && api_format.starts_with("openai:") {
        build_codex_models_url(base_url, codex_client_version)
    } else if api_format.starts_with("openai:") {
        build_v1_models_url(base_url)
    } else if api_format.starts_with("claude:") {
        build_claude_models_url(base_url)
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

/// Parses the Codex `/models` response without applying the generic cache projection.
///
/// Codex model cards are versioned protocol data. They must remain opaque so future fields and
/// instruction representations survive catalog caching and downstream projection. Invalid entries
/// reject the whole response instead of being skipped and accidentally replacing a complete LKG
/// with a partial directory.
pub(crate) fn parse_codex_models_response_page(body: &Value) -> Result<ModelsFetchPage, String> {
    let serialized = serde_json::to_vec(body)
        .map_err(|_| "Codex models response could not be serialized".to_string())?;
    if serialized.len() > CODEX_MODELS_MAX_JSON_BYTES {
        return Err(format!(
            "Codex models response exceeds {CODEX_MODELS_MAX_JSON_BYTES} bytes"
        ));
    }

    let items = body
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| "Codex models response is missing models array".to_string())?;
    if items.is_empty() {
        return Err("Codex models response contains no models".to_string());
    }
    if items.len() > CODEX_MODELS_MAX_ITEMS {
        return Err(format!(
            "Codex models response exceeds {CODEX_MODELS_MAX_ITEMS} models"
        ));
    }

    let cached_models = merge_codex_models_preserving_cards(items)?;
    let fetched_model_ids = cached_models
        .iter()
        .filter_map(codex_model_identity)
        .map(ToOwned::to_owned)
        .collect();

    Ok(ModelsFetchPage {
        fetched_model_ids,
        cached_models,
        has_more: false,
        next_after_id: None,
    })
}

/// Merges opaque Codex model cards without silently selecting one of two conflicting cards.
///
/// Both `id` and `slug` are mapping identities. Exact duplicate JSON cards can occur when the
/// same catalog is fetched through multiple endpoint transports and are collapsed. If any valid
/// identity is reused by a different card, the response is ambiguous and must not replace a
/// last-known-good catalog.
pub(crate) fn merge_codex_models_preserving_cards(models: &[Value]) -> Result<Vec<Value>, String> {
    let mut merged = Vec::<Value>::with_capacity(models.len());
    let mut index_by_identity = BTreeMap::<String, usize>::new();

    for model in models {
        let identities = codex_model_identities(model)?;
        let mut duplicate_index = None;
        for identity in &identities {
            let Some(existing_index) = index_by_identity.get(*identity).copied() else {
                continue;
            };
            if merged.get(existing_index) != Some(model) {
                return Err(format!(
                    "Codex models response contains conflicting cards for identity '{identity}'"
                ));
            }
            if duplicate_index.is_some_and(|index| index != existing_index) {
                return Err(format!(
                    "Codex models response contains conflicting cards for identity '{identity}'"
                ));
            }
            duplicate_index = Some(existing_index);
        }

        if let Some(existing_index) = duplicate_index {
            for identity in identities {
                index_by_identity
                    .entry(identity.to_string())
                    .or_insert(existing_index);
            }
            continue;
        }

        let model_index = merged.len();
        merged.push(model.clone());
        for identity in identities {
            index_by_identity.insert(identity.to_string(), model_index);
        }
    }

    Ok(merged)
}

fn codex_model_identities(model: &Value) -> Result<Vec<&str>, String> {
    let object = model
        .as_object()
        .ok_or_else(|| "Codex models response contains a non-object model card".to_string())?;
    let mut identities = Vec::with_capacity(2);
    for field in ["id", "slug"] {
        let Some(identity) = object
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| *value == value.trim())
            .filter(|value| valid_codex_model_identity(value))
        else {
            continue;
        };
        if !identities.contains(&identity) {
            identities.push(identity);
        }
    }
    if identities.is_empty() {
        return Err("Codex models response contains a card without a valid id or slug".to_string());
    }
    Ok(identities)
}

pub(crate) fn codex_model_identity(model: &Value) -> Option<&str> {
    let object = model.as_object()?;
    ["slug", "id"].iter().find_map(|field| {
        object
            .get(*field)
            .and_then(Value::as_str)
            .filter(|value| *value == value.trim())
            .filter(|value| valid_codex_model_identity(value))
    })
}

/// Projects opaque Codex cards into the legacy model-cache shape used by permission sync.
///
/// The source cards remain untouched. Only formats from transports that actually returned the
/// card are admitted into `api_formats`; an upstream `api_format` field is protocol data and is
/// preserved as-is rather than interpreted as an Aether endpoint format.
pub fn project_codex_models_for_legacy_cache<'a>(
    successful_transports: impl IntoIterator<Item = (&'a str, &'a [Value])>,
) -> Vec<Value> {
    let mut projected = BTreeMap::<String, serde_json::Map<String, Value>>::new();

    for (endpoint_api_format, models) in successful_transports {
        let api_format = normalize_api_format(endpoint_api_format);
        if api_format.is_empty() {
            continue;
        }

        for model in models {
            let Some(model_id) = codex_model_identity(model).map(ToOwned::to_owned) else {
                continue;
            };
            let Some(source) = model.as_object() else {
                continue;
            };

            let entry = projected.entry(model_id.clone()).or_insert_with(|| {
                let mut card = source.clone();
                card.insert("id".to_string(), Value::String(model_id));
                // `api_formats` is Aether's routing projection. Never inherit a similarly named
                // opaque upstream field when constructing this legacy view.
                card.insert("api_formats".to_string(), Value::Array(Vec::new()));
                card
            });
            let mut formats = entry
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
            formats.insert(api_format.clone());
            entry.insert(
                "api_formats".to_string(),
                Value::Array(
                    sorted_api_formats(formats)
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }
    }

    projected.into_values().map(Value::Object).collect()
}

fn valid_codex_model_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value == value.trim()
        && value
            .chars()
            .all(|character| !character.is_whitespace() && !character.is_control())
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
        "codex" => aether_ai_formats::bundled_codex_model_cards().to_vec(),
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
            if namespace.eq_ignore_ascii_case("antigravity") {
                for field in ["quota_groups", "quota_groups_updated_at"] {
                    if !next_namespace.contains_key(field) {
                        if let Some(value) = old_namespace.get(field) {
                            next_namespace.insert(field.to_string(), value.clone());
                        }
                    }
                }
            }
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

pub fn model_catalog_upstream_metadata(
    provider_type: &str,
    cached_models: &[Value],
) -> Option<Value> {
    provider_type.trim().eq_ignore_ascii_case("codex").then(|| {
        let cards = aether_ai_formats::effective_codex_model_cards(cached_models);
        aether_ai_formats::build_codex_model_catalog_metadata(&cards)
    })
}

pub fn upstream_metadata_namespace_updates(
    current: Option<&Value>,
    incoming: &Value,
) -> Vec<(String, Value)> {
    let Some(incoming) = incoming.as_object() else {
        return Vec::new();
    };
    let merged = merge_upstream_metadata(current, &Value::Object(incoming.clone()));
    incoming
        .keys()
        .filter_map(|namespace| {
            merged
                .get(namespace)
                .cloned()
                .map(|value| (namespace.clone(), value))
        })
        .collect()
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

        let has_api_formats_array = object
            .get("api_formats")
            .and_then(Value::as_array)
            .is_some();
        let entry = aggregated.entry(model_id.to_string()).or_insert_with(|| {
            let mut cloned = object.clone();
            if !has_api_formats_array {
                cloned.remove("api_format");
            }
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
        let legacy_api_format = (!has_api_formats_array)
            .then(|| {
                object
                    .get("api_format")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
            })
            .flatten();
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
            if key == "api_format" {
                if has_api_formats_array && !entry.contains_key(key) {
                    entry.insert(key.clone(), value.clone());
                }
                continue;
            }
            if entry.contains_key(key) {
                continue;
            }
            entry.insert(key.clone(), value.clone());
        }
    }

    aggregated.into_values().map(Value::Object).collect()
}

fn build_v1_models_url(base_url: &str) -> Option<String> {
    build_openai_compatible_models_url(base_url)
}

fn build_claude_models_url(base_url: &str) -> Option<String> {
    if let Some(url) = build_deepseek_anthropic_models_url(base_url) {
        return Some(url);
    }

    let (trimmed_base_url, base_query) = split_url_query(base_url);
    let trimmed_base_url = trimmed_base_url.trim_end_matches('/');
    if trimmed_base_url.is_empty() {
        return None;
    }

    let mut url = if trimmed_base_url.ends_with("/models") {
        trimmed_base_url.to_string()
    } else {
        format!("{trimmed_base_url}/models")
    };
    if let Some(query) = base_query.filter(|value| !value.trim().is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    Some(url)
}

pub fn deepseek_anthropic_models_fetch_uses_openai_auth(base_url: &str) -> bool {
    build_deepseek_anthropic_models_url(base_url).is_some()
}

fn build_deepseek_anthropic_models_url(base_url: &str) -> Option<String> {
    let (trimmed_base_url, base_query) = split_url_query(base_url);
    let trimmed_base_url = trimmed_base_url.trim_end_matches('/');
    let normalized = trimmed_base_url.to_ascii_lowercase();
    if normalized != "https://api.deepseek.com/anthropic"
        && normalized != "https://api.deepseek.com/anthropic/v1"
    {
        return None;
    }

    let mut url = "https://api.deepseek.com/models".to_string();
    if let Some(query) = base_query.filter(|value| !value.trim().is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    Some(url)
}

fn build_codex_models_url(base_url: &str, client_version: Option<&str>) -> Option<String> {
    if let Some(url) = build_bigmodel_coding_models_url(base_url) {
        return Some(
            client_version
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|client_version| {
                    replace_or_append_query_param(&url, "client_version", client_version)
                })
                .unwrap_or(url),
        );
    }

    let (trimmed_base_url, query) = split_url_query(base_url);
    let trimmed_base_url = trimmed_base_url.trim_end_matches('/');
    if trimmed_base_url.is_empty() {
        return None;
    }
    let is_codex_backend = is_codex_cli_backend_url(trimmed_base_url)
        || trimmed_base_url.ends_with("/codex")
        || trimmed_base_url.ends_with("/models");
    if !is_codex_backend && openai_compatible_base_includes_unversioned_api_root(base_url) {
        let url = build_openai_compatible_models_url(base_url)?;
        return Some(
            client_version
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|client_version| {
                    replace_or_append_query_param(&url, "client_version", client_version)
                })
                .unwrap_or(url),
        );
    }
    let mut url = if trimmed_base_url.ends_with("/models") {
        trimmed_base_url.to_string()
    } else {
        format!("{trimmed_base_url}/models")
    };
    let explicit_client_version = client_version
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut query_parts = query
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.split('&').map(ToOwned::to_owned).collect::<Vec<_>>())
        .unwrap_or_default();
    let has_client_version = query_parts.iter().any(|part| {
        part.split_once('=')
            .map(|(key, _)| key)
            .unwrap_or(part)
            .trim()
            .eq_ignore_ascii_case("client_version")
    });
    if let Some(client_version) = explicit_client_version {
        query_parts.retain(|part| {
            !part
                .split_once('=')
                .map(|(key, _)| key)
                .unwrap_or(part)
                .trim()
                .eq_ignore_ascii_case("client_version")
        });
        query_parts.push(encoded_query_pair("client_version", client_version));
    } else if !has_client_version {
        query_parts.push(format!(
            "client_version={}",
            aether_ai_formats::CODEX_CLIENT_VERSION
        ));
    }
    if !query_parts.is_empty() {
        url.push('?');
        url.push_str(&query_parts.join("&"));
    }
    Some(url)
}

fn replace_or_append_query_param(url: &str, name: &str, value: &str) -> String {
    let (base, query) = split_url_query(url);
    let mut query_parts = query
        .filter(|query| !query.trim().is_empty())
        .map(|query| query.split('&').map(ToOwned::to_owned).collect::<Vec<_>>())
        .unwrap_or_default();
    query_parts.retain(|part| {
        !part
            .split_once('=')
            .map(|(key, _)| key)
            .unwrap_or(part)
            .trim()
            .eq_ignore_ascii_case(name)
    });
    query_parts.push(encoded_query_pair(name, value));
    format!("{base}?{}", query_parts.join("&"))
}

fn encoded_query_pair(name: &str, value: &str) -> String {
    let name = url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>();
    let value = url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>();
    format!("{name}={value}")
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
        let Some(model_id) = codex_model_identity(model).or_else(|| {
            model
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        }) else {
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
        build_models_fetch_url, build_models_fetch_url_for_client_version, merge_upstream_metadata,
        parse_codex_models_response_page, parse_models_response, parse_models_response_page,
        preset_models_for_provider, project_codex_models_for_legacy_cache,
        selected_models_fetch_endpoints,
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
    fn aggregate_models_for_cache_preserves_opaque_api_format_on_projected_cards() {
        let card = json!({
            "slug": "gpt-slug-only-future",
            "api_format": "opaque-upstream-protocol",
            "model_messages": {"instructions_template": "Future instructions"},
            "future_capability": {"opaque": true}
        });
        let cards = vec![card];
        let projected =
            project_codex_models_for_legacy_cache([("openai:responses", cards.as_slice())]);

        let aggregated = aggregate_models_for_cache(&projected);

        assert_eq!(aggregated.len(), 1);
        assert_eq!(aggregated[0]["id"], "gpt-slug-only-future");
        assert_eq!(aggregated[0]["api_format"], "opaque-upstream-protocol");
        assert_eq!(aggregated[0]["api_formats"], json!(["openai:responses"]));
        assert_eq!(aggregated[0]["future_capability"]["opaque"], true);
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
                "https://example.com/models".to_string(),
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
                format!(
                    "https://chatgpt.com/backend-api/codex/models?client_version={}",
                    aether_ai_formats::CODEX_CLIENT_VERSION
                ),
                "openai:responses".to_string()
            ))
        );
    }

    #[test]
    fn build_models_fetch_url_uses_explicit_codex_client_version() {
        assert_eq!(
            build_models_fetch_url_for_client_version(
                "codex",
                "openai:responses",
                "https://chatgpt.com/backend-api/codex",
                Some("0.145.2"),
            ),
            Some((
                "https://chatgpt.com/backend-api/codex/models?client_version=0.145.2".to_string(),
                "openai:responses".to_string()
            ))
        );
    }

    #[test]
    fn explicit_codex_client_version_cannot_inject_query_parameters() {
        let (url, _) = build_models_fetch_url_for_client_version(
            "codex",
            "openai:responses",
            "https://chatgpt.com/backend-api/codex",
            Some("0.145.2&admin=true#fragment"),
        )
        .expect("models URL should build");

        assert_eq!(
            url,
            "https://chatgpt.com/backend-api/codex/models?client_version=0.145.2%26admin%3Dtrue%23fragment"
        );
    }

    #[test]
    fn explicit_codex_client_version_replaces_stale_base_query_value() {
        assert_eq!(
            build_models_fetch_url_for_client_version(
                "codex",
                "openai:responses",
                "https://chatgpt.com/backend-api/codex?feature=on&client_version=0.144.1",
                Some("0.145.2"),
            ),
            Some((
                "https://chatgpt.com/backend-api/codex/models?feature=on&client_version=0.145.2"
                    .to_string(),
                "openai:responses".to_string()
            ))
        );
    }

    #[test]
    fn explicit_codex_client_version_preserves_preencoded_base_query_values() {
        assert_eq!(
            build_models_fetch_url_for_client_version(
                "codex",
                "openai:responses",
                "https://chatgpt.com/backend-api/codex?feature=beta%2Bdesktop",
                Some("0.145.2"),
            ),
            Some((
                "https://chatgpt.com/backend-api/codex/models?feature=beta%2Bdesktop&client_version=0.145.2"
                    .to_string(),
                "openai:responses".to_string()
            ))
        );
    }

    #[test]
    fn explicit_codex_client_version_is_forwarded_through_compatible_proxy_roots() {
        assert_eq!(
            build_models_fetch_url_for_client_version(
                "codex",
                "openai:responses",
                "https://proxy.example.com/api?feature=on&client_version=0.144.1",
                Some("0.145.2"),
            ),
            Some((
                "https://proxy.example.com/api/models?feature=on&client_version=0.145.2"
                    .to_string(),
                "openai:responses".to_string()
            ))
        );
    }

    #[test]
    fn build_models_fetch_url_supports_bigmodel_coding_paas_root() {
        assert_eq!(
            build_models_fetch_url(
                "openai",
                "openai:chat",
                "https://open.bigmodel.cn/api/coding/paas/v4"
            ),
            Some((
                "https://open.bigmodel.cn/api/coding/paas/v4/models".to_string(),
                "openai:chat".to_string()
            ))
        );
        assert_eq!(
            build_models_fetch_url(
                "codex",
                "openai:responses",
                "https://open.bigmodel.cn/api/coding/paas/v4"
            ),
            Some((
                "https://open.bigmodel.cn/api/coding/paas/v4/models".to_string(),
                "openai:responses".to_string()
            ))
        );
        assert_eq!(
            build_models_fetch_url_for_client_version(
                "codex",
                "openai:responses",
                "https://open.bigmodel.cn/api/coding/paas/v4?tenant=demo&client_version=0.144.1",
                Some("0.145.2"),
            ),
            Some((
                "https://open.bigmodel.cn/api/coding/paas/v4/models?tenant=demo&client_version=0.145.2"
                    .to_string(),
                "openai:responses".to_string()
            ))
        );
    }

    #[test]
    fn build_models_fetch_url_preserves_unversioned_api_root() {
        assert_eq!(
            build_models_fetch_url("openai", "openai:chat", "https://proxy.example.com/api"),
            Some((
                "https://proxy.example.com/api/models".to_string(),
                "openai:chat".to_string()
            ))
        );
        assert_eq!(
            build_models_fetch_url("openai", "openai:chat", "https://proxy.example.com/openai"),
            Some((
                "https://proxy.example.com/openai/models".to_string(),
                "openai:chat".to_string()
            ))
        );
        assert_eq!(
            build_models_fetch_url("openai", "openai:chat", "https://proxy.example.com"),
            Some((
                "https://proxy.example.com/models".to_string(),
                "openai:chat".to_string()
            ))
        );
        assert_eq!(
            build_models_fetch_url("openai", "openai:chat", "https://api.deepseek.com"),
            Some((
                "https://api.deepseek.com/models".to_string(),
                "openai:chat".to_string()
            ))
        );
        assert_eq!(
            build_models_fetch_url("codex", "openai:responses", "https://proxy.example.com/api"),
            Some((
                "https://proxy.example.com/api/models".to_string(),
                "openai:responses".to_string()
            ))
        );
        assert_eq!(
            build_models_fetch_url(
                "anthropic",
                "claude:messages",
                "https://proxy.example.com/api"
            ),
            Some((
                "https://proxy.example.com/api/models".to_string(),
                "claude:messages".to_string()
            ))
        );
    }

    #[test]
    fn build_models_fetch_url_uses_deepseek_openai_models_for_anthropic_base() {
        assert_eq!(
            build_models_fetch_url(
                "custom",
                "claude:messages",
                "https://api.deepseek.com/anthropic"
            ),
            Some((
                "https://api.deepseek.com/models".to_string(),
                "claude:messages".to_string()
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
    fn parse_models_response_preserves_gpt_5_6_model_card_capabilities() {
        let card = json!({
            "slug": "gpt-5.6-sol",
            "default_reasoning_level": "low",
            "supported_reasoning_levels": [
                {"effort": "low"},
                {"effort": "max"},
                {"effort": "ultra"}
            ],
            "multi_agent_version": "v2",
            "supports_image_detail_original": true,
            "future_capability": {"mode": "preserve-me"}
        });
        let parsed = parse_models_response("openai:responses", &json!({"models": [card]}))
            .expect("Codex model card should parse");

        let cached = &parsed.cached_models[0];
        assert_eq!(cached["id"], "gpt-5.6-sol");
        assert_eq!(cached["default_reasoning_level"], "low");
        assert_eq!(cached["supported_reasoning_levels"][2]["effort"], "ultra");
        assert_eq!(cached["multi_agent_version"], "v2");
        assert_eq!(cached["supports_image_detail_original"], true);
        assert_eq!(cached["future_capability"]["mode"], "preserve-me");
        assert_eq!(cached["api_formats"], json!(["openai:responses"]));
    }

    #[test]
    fn strict_codex_parser_preserves_opaque_cards_without_cache_projection() {
        let card = json!({
            "id": "gpt-future-dynamic",
            "slug": "gpt-future-dynamic",
            "api_format": "future-protocol-field",
            "model_messages": {"instructions_template": "Future instructions"},
            "available_in_plans": ["plus"],
            "future_capability": {"opaque": true}
        });
        let parsed = parse_codex_models_response_page(&json!({"models": [card.clone()]}))
            .expect("opaque Codex card should parse");

        assert_eq!(parsed.fetched_model_ids, vec!["gpt-future-dynamic"]);
        assert_eq!(parsed.cached_models, vec![card]);
    }

    #[test]
    fn codex_legacy_projector_adds_internal_identity_and_only_successful_endpoint_formats() {
        let card = json!({
            "id": "opaque-upstream-id",
            "slug": "gpt-slug-only-future",
            "api_format": "opaque-upstream-protocol",
            "api_formats": ["opaque-upstream-format-list"],
            "model_messages": {"instructions_template": "Future instructions"},
            "future_capability": {"opaque": true}
        });
        let cards = vec![card.clone()];

        let projected = project_codex_models_for_legacy_cache([
            ("openai:responses", cards.as_slice()),
            ("openai:chat", cards.as_slice()),
        ]);

        assert_eq!(cards, vec![card]);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0]["id"], "gpt-slug-only-future");
        assert_eq!(
            projected[0]["api_formats"],
            json!(["openai:chat", "openai:responses"])
        );
        assert_eq!(projected[0]["api_format"], "opaque-upstream-protocol");
        assert_eq!(
            projected[0]["model_messages"]["instructions_template"],
            "Future instructions"
        );
        assert_eq!(projected[0]["future_capability"]["opaque"], true);
    }

    #[test]
    fn strict_codex_parser_rejects_empty_models_array() {
        let error = parse_codex_models_response_page(&json!({"models": []}))
            .expect_err("empty Codex catalog must fail");

        assert!(error.contains("no models"));
    }

    #[test]
    fn strict_codex_parser_merges_only_exact_duplicate_cards() {
        let card = json!({
            "id": "gpt-future-duplicate",
            "slug": "gpt-future-duplicate",
            "model_messages": {"instructions_template": "Opaque instructions"},
            "future_capability": {"opaque": true}
        });
        let parsed = parse_codex_models_response_page(&json!({
            "models": [card.clone(), card.clone()]
        }))
        .expect("exact duplicate Codex cards should merge");

        assert_eq!(parsed.fetched_model_ids, vec!["gpt-future-duplicate"]);
        assert_eq!(parsed.cached_models, vec![card]);
    }

    #[test]
    fn strict_codex_parser_rejects_same_or_cross_identity_conflicts() {
        let conflicts = [
            json!({
                "models": [
                    {"id": "gpt-conflict", "slug": "gpt-conflict", "future": 1},
                    {"id": "gpt-conflict", "slug": "gpt-conflict", "future": 2}
                ]
            }),
            json!({
                "models": [
                    {"id": "gpt-id-one", "slug": "gpt-cross-identity", "future": 1},
                    {"id": "gpt-cross-identity", "slug": "gpt-slug-two", "future": 2}
                ]
            }),
        ];

        for body in conflicts {
            let error = parse_codex_models_response_page(&body)
                .expect_err("ambiguous Codex identities must fail");
            assert!(error.contains("conflicting cards"));
        }
    }

    #[test]
    fn strict_codex_parser_rejects_non_object_or_synthetic_identity_cards() {
        for body in [
            json!({"models": ["gpt-future-dynamic"]}),
            json!({"models": [{"model": "gpt-future-dynamic"}]}),
            json!({"models": [{"name": "gpt-future-dynamic"}]}),
            json!({"models": [{"slug": " gpt-future-dynamic "}]}),
        ] {
            assert!(parse_codex_models_response_page(&body).is_err());
        }
    }

    #[test]
    fn strict_codex_parser_does_not_impose_an_ascii_symbol_allowlist_on_identities() {
        let card = json!({
            "slug": "gpt+future@dynamic",
            "model_messages": {"instructions_template": "Future instructions"}
        });
        let parsed = parse_codex_models_response_page(&json!({"models": [card.clone()]}))
            .expect("future identity punctuation should remain opaque");

        assert_eq!(parsed.fetched_model_ids, vec!["gpt+future@dynamic"]);
        assert_eq!(parsed.cached_models, vec![card]);
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
                    "quota_groups": [{
                        "display_name": "Claude and GPT models",
                        "buckets": [{"bucket_id": "3p-5h", "window": "5h"}]
                    }],
                    "quota_groups_updated_at": 1_777_000_000u64,
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
        assert_eq!(
            merged["antigravity"]["quota_groups"][0]["buckets"][0]["bucket_id"],
            "3p-5h"
        );
        assert_eq!(
            merged["antigravity"]["quota_groups_updated_at"],
            json!(1_777_000_000u64)
        );
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
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.5",
                "gpt-5.4",
                "gpt-5.4-mini",
                "gpt-5.2",
                "codex-auto-review",
            ]
        );
        let sol = models
            .iter()
            .find(|model| model["id"] == "gpt-5.6-sol")
            .expect("Sol preset");
        assert_eq!(sol["default_reasoning_level"], "low");
        assert_eq!(
            sol["supported_reasoning_levels"]
                .as_array()
                .expect("reasoning levels")
                .iter()
                .filter_map(|level| level["effort"].as_str())
                .collect::<Vec<_>>(),
            vec!["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(sol["multi_agent_version"], "v2");
        assert_eq!(sol["supports_image_detail_original"], true);
        assert_eq!(sol["context_window"], 372_000);

        for model_id in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            let model = models
                .iter()
                .find(|model| model["id"] == model_id)
                .expect("GPT-5.6 Codex preset");
            assert_eq!(model["shell_type"], "shell_command");
            assert_eq!(model["comp_hash"], "3000");
            assert_eq!(model["experimental_supported_tools"], json!([]));
            assert_eq!(model["tool_mode"], "code_mode_only");
            assert_eq!(model["prefer_websockets"], true);
            assert_eq!(model["reasoning_summary_format"], "experimental");
            assert_eq!(model["truncation_policy"]["limit"], 10_000);
            assert_eq!(model["minimal_client_version"], "0.144.0");
            assert!(model.get("effective_context_window_percent").is_none());
        }

        let luna = models
            .iter()
            .find(|model| model["id"] == "gpt-5.6-luna")
            .expect("Luna preset");
        assert_eq!(luna["default_reasoning_level"], "medium");
        assert_eq!(luna["multi_agent_version"], "v1");
        assert!(!luna["supported_reasoning_levels"]
            .as_array()
            .expect("reasoning levels")
            .iter()
            .any(|level| level["effort"] == "ultra"));

        let auto_review = models
            .iter()
            .find(|model| model["id"] == "codex-auto-review")
            .expect("Codex auto review preset");
        assert_eq!(auto_review["visibility"], "hide");
        assert_eq!(auto_review["supported_in_api"], true);
        assert_eq!(auto_review["default_reasoning_level"], "medium");
        assert_eq!(auto_review["default_reasoning_summary"], "none");
        assert_eq!(auto_review["use_responses_lite"], false);
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
