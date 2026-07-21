use std::collections::BTreeMap;
use std::sync::OnceLock;

use aether_ai_formats::provider_compat::proxy::rules::body_rules_handle_path;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const CODEX_DEFAULT_REASONING_EFFORT: &str = "medium";
const CODEX_REASONING_ENCRYPTED_CONTENT_INCLUDE: &str = "reasoning.encrypted_content";
const CODEX_PROMPT_CACHE_IDENTITY_NAMESPACE: &str =
    "https://github.com/fawney19/Aether/codex/prompt-cache-identity/v1/";
pub const CODEX_RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";
pub const CODEX_MODEL_CATALOG_METADATA_FIELD: &str = "codex_models";
const CODEX_OPENAI_RESPONSES_UNSUPPORTED_BODY_FIELDS: &[&str] = &[
    "max_output_tokens",
    "max_completion_tokens",
    "temperature",
    "top_p",
    "frequency_penalty",
    "presence_penalty",
    "user",
    "metadata",
    "prompt_cache_options",
    "prompt_cache_retention",
    "safety_identifier",
    "previous_response_id",
];
const CODEX_OPENAI_RESPONSES_COMPACT_BODY_FIELDS: &[&str] = &[
    "model",
    "input",
    "instructions",
    "tools",
    "parallel_tool_calls",
    "reasoning",
    "service_tier",
    "prompt_cache_key",
    "text",
];
pub const CODEX_CLIENT_VERSION: &str = "0.144.1";
pub const CODEX_CLIENT_USER_AGENT: &str = "codex_cli_rs/0.144.1";
pub const CODEX_CLIENT_ORIGINATOR: &str = "codex_cli_rs";
pub const CODEX_OPENAI_IMAGE_INTERNAL_MODEL: &str = "gpt-5.4-mini";
pub const CODEX_OPENAI_IMAGE_DEFAULT_MODEL: &str = "gpt-image-2";
pub const CODEX_OPENAI_IMAGE_DEFAULT_VARIATION_MODEL: &str = "dall-e-2";
pub const CODEX_OPENAI_IMAGE_DEFAULT_OUTPUT_FORMAT: &str = "png";
pub const CODEX_OPENAI_IMAGE_DEFAULT_VARIATION_PROMPT: &str =
    "Create a faithful variation of the provided image.";
const CODEX_IMAGE_TOOL_DEFAULT_SIZE: &str = "1024x1024";
const CODEX_IMAGE_TOOL_DEFAULT_QUALITY: &str = "high";
const CODEX_IMAGE_TOOL_DEFAULT_BACKGROUND: &str = "auto";
fn is_codex_openai_responses_request(provider_type: &str, provider_api_format: &str) -> bool {
    provider_type.trim().eq_ignore_ascii_case("codex")
        && aether_ai_formats::is_openai_responses_family_format(provider_api_format)
}

fn is_openai_responses_compact_request(provider_api_format: &str) -> bool {
    aether_ai_formats::is_openai_responses_compact_format(provider_api_format)
}

fn is_openai_image_request(provider_api_format: &str) -> bool {
    provider_api_format
        .trim()
        .eq_ignore_ascii_case("openai:image")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CodexOpenAiEndpointKind {
    Responses,
    Compact,
    Search,
    Images,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexResponsesModelCapabilities {
    pub use_responses_lite: bool,
    pub supports_reasoning_summary_parameter: bool,
    pub default_reasoning_effort: Option<String>,
    pub default_reasoning_summary: Option<String>,
    pub supported_reasoning_efforts: Vec<String>,
    pub supports_parallel_tool_calls: bool,
    pub support_verbosity: bool,
    pub default_verbosity: Option<String>,
    pub supported_service_tiers: Vec<String>,
}

impl CodexResponsesModelCapabilities {
    pub fn supports_reasoning_effort(&self, effort: &str) -> bool {
        self.supported_reasoning_efforts
            .iter()
            .any(|candidate| candidate == effort.trim())
    }

    fn supports_service_tier(&self, service_tier: &str) -> bool {
        self.supported_service_tiers
            .iter()
            .any(|candidate| candidate == service_tier)
    }
}

fn codex_namespaced_model_suffix(model: &str) -> Option<&str> {
    let (namespace, suffix) = model.split_once('/')?;
    if suffix.contains('/')
        || namespace.is_empty()
        || !namespace
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return None;
    }
    Some(suffix)
}

fn conservative_codex_responses_model_capabilities() -> CodexResponsesModelCapabilities {
    CodexResponsesModelCapabilities {
        use_responses_lite: false,
        supports_reasoning_summary_parameter: true,
        default_reasoning_effort: None,
        default_reasoning_summary: None,
        supported_reasoning_efforts: Vec::new(),
        supports_parallel_tool_calls: false,
        support_verbosity: false,
        default_verbosity: None,
        supported_service_tiers: Vec::new(),
    }
}

pub fn codex_responses_model_capabilities_from_card(
    card: &Value,
) -> Option<(String, CodexResponsesModelCapabilities)> {
    let card = card.as_object()?;
    let model_id = card
        .get("slug")
        .or_else(|| card.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let mut capabilities = conservative_codex_responses_model_capabilities();
    if let Some(value) = card.get("use_responses_lite").and_then(Value::as_bool) {
        capabilities.use_responses_lite = value;
    }
    capabilities.supports_reasoning_summary_parameter = card
        .get("supports_reasoning_summary_parameter")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if let Some(value) = card
        .get("default_reasoning_level")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        capabilities.default_reasoning_effort = Some(value.to_string());
    }
    if let Some(value) = card
        .get("default_reasoning_summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        capabilities.default_reasoning_summary =
            (!value.eq_ignore_ascii_case("none")).then(|| value.to_ascii_lowercase());
    }
    if let Some(levels) = card
        .get("supported_reasoning_levels")
        .and_then(Value::as_array)
    {
        capabilities.supported_reasoning_efforts = levels
            .iter()
            .filter_map(|level| {
                level
                    .get("effort")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
            .collect();
    }
    if let Some(value) = card
        .get("supports_parallel_tool_calls")
        .and_then(Value::as_bool)
    {
        capabilities.supports_parallel_tool_calls = value;
    }
    if let Some(value) = card.get("support_verbosity").and_then(Value::as_bool) {
        capabilities.support_verbosity = value;
    }
    if let Some(value) = card
        .get("default_verbosity")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        capabilities.default_verbosity = Some(value.to_ascii_lowercase());
    }
    if let Some(service_tiers) = card.get("service_tiers").and_then(Value::as_array) {
        capabilities.supported_service_tiers = service_tiers
            .iter()
            .filter_map(|tier| {
                tier.get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
            .collect();
    }
    Some((model_id.to_string(), capabilities))
}

pub fn build_codex_model_catalog_metadata(cards: &[Value]) -> Value {
    let cards = cards
        .iter()
        .filter_map(|card| {
            let model_id = card
                .get("slug")
                .or_else(|| card.get("id"))
                .and_then(Value::as_str)?;
            Some((model_id.to_string(), codex_execution_model_card(card)))
        })
        .collect::<serde_json::Map<_, _>>();
    json!({
        CODEX_MODEL_CATALOG_METADATA_FIELD: {
            "cards": cards,
        },
    })
}

fn codex_execution_model_card(card: &Value) -> Value {
    const EXCLUDED_FIELDS: [&str; 3] =
        ["base_instructions", "model_messages", "available_in_plans"];
    let Some(card) = card.as_object() else {
        return card.clone();
    };
    Value::Object(
        card.iter()
            .filter(|(key, _)| !EXCLUDED_FIELDS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

pub fn effective_codex_model_cards(remote_cards: &[Value]) -> Vec<Value> {
    if remote_cards.iter().any(|card| {
        card.get("visibility")
            .and_then(Value::as_str)
            .is_some_and(|visibility| visibility == "list")
    }) {
        return remote_cards.to_vec();
    }

    let mut cards = bundled_codex_model_cards().to_vec();
    for remote_card in remote_cards {
        let Some(remote_id) = remote_card
            .get("slug")
            .or_else(|| remote_card.get("id"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if let Some(index) = cards.iter().position(|card| {
            card.get("slug")
                .or_else(|| card.get("id"))
                .and_then(Value::as_str)
                .is_some_and(|model_id| model_id == remote_id)
        }) {
            let Some(base) = cards[index].as_object() else {
                cards[index] = remote_card.clone();
                continue;
            };
            let Some(overlay) = remote_card.as_object() else {
                cards[index] = remote_card.clone();
                continue;
            };
            let mut merged = base.clone();
            merged.extend(overlay.clone());
            cards[index] = Value::Object(merged);
        } else {
            cards.push(remote_card.clone());
        }
    }
    cards
}

fn reasoning_level_cards(efforts: &[&str]) -> Vec<Value> {
    efforts
        .iter()
        .map(|effort| {
            let description = match *effort {
                "low" => "Fast responses with lighter reasoning",
                "medium" => "Balances speed and reasoning depth for everyday tasks",
                "high" => "Greater reasoning depth for complex problems",
                "xhigh" => "Extra high reasoning depth for complex problems",
                "max" => "Maximum reasoning depth for the hardest problems",
                "ultra" => "Maximum reasoning with automatic task delegation",
                _ => "",
            };
            json!({ "effort": effort, "description": description })
        })
        .collect()
}

struct BundledCodexModelCardSpec<'a> {
    model_id: &'a str,
    display_name: &'a str,
    description: &'a str,
    default_reasoning_level: &'a str,
    default_reasoning_summary: &'a str,
    use_responses_lite: bool,
    efforts: &'a [&'a str],
    default_verbosity: &'a str,
    supports_priority_tier: bool,
}

fn bundled_codex_model_card(spec: BundledCodexModelCardSpec<'_>) -> Value {
    let service_tiers = spec
        .supports_priority_tier
        .then(|| {
            json!({
                "id": "priority",
                "name": "Fast",
                "description": "1.5x speed, increased usage",
            })
        })
        .into_iter()
        .collect::<Vec<_>>();
    json!({
        "id": spec.model_id,
        "slug": spec.model_id,
        "object": "model",
        "owned_by": "openai",
        "display_name": spec.display_name,
        "description": spec.description,
        "api_formats": ["openai:responses"],
        "default_reasoning_level": spec.default_reasoning_level,
        "supported_reasoning_levels": reasoning_level_cards(spec.efforts),
        "default_reasoning_summary": spec.default_reasoning_summary,
        "support_verbosity": true,
        "default_verbosity": spec.default_verbosity,
        "supports_parallel_tool_calls": true,
        "service_tiers": service_tiers,
        "use_responses_lite": spec.use_responses_lite,
    })
}

fn bundled_gpt_5_6_codex_model_card(
    model_id: &str,
    display_name: &str,
    description: &str,
    default_reasoning_level: &str,
    priority: u64,
    multi_agent_version: &str,
    supports_ultra: bool,
) -> Value {
    let mut efforts = vec!["low", "medium", "high", "xhigh", "max"];
    if supports_ultra {
        efforts.push("ultra");
    }
    let mut card = bundled_codex_model_card(BundledCodexModelCardSpec {
        model_id,
        display_name,
        description,
        default_reasoning_level,
        default_reasoning_summary: "none",
        use_responses_lite: true,
        efforts: &efforts,
        default_verbosity: "low",
        supports_priority_tier: true,
    });
    let object = card
        .as_object_mut()
        .expect("bundled Codex model card must be an object");
    object.extend(
        json!({
            "shell_type": "shell_command",
            "supports_image_detail_original": true,
            "supports_search_tool": true,
            "input_modalities": ["text", "image"],
            "context_window": 372_000,
            "max_context_window": 372_000,
            "comp_hash": "3000",
            "experimental_supported_tools": [],
            "visibility": "list",
            "supported_in_api": true,
            "priority": priority,
            "additional_speed_tiers": ["fast"],
            "multi_agent_version": multi_agent_version,
            "tool_mode": "code_mode_only",
            "prefer_websockets": true,
            "reasoning_summary_format": "experimental",
            "include_skills_usage_instructions": false,
            "apply_patch_tool_type": "freeform",
            "web_search_tool_type": "text_and_image",
            "truncation_policy": { "mode": "tokens", "limit": 10_000 },
            "minimal_client_version": "0.144.0",
        })
        .as_object()
        .expect("bundled Codex model card extension must be an object")
        .clone(),
    );
    card
}

fn bundled_codex_auto_review_model_card() -> Value {
    let mut card = bundled_codex_model_card(BundledCodexModelCardSpec {
        model_id: "codex-auto-review",
        display_name: "Codex Auto Review",
        description: "Automatic approval review model for Codex.",
        default_reasoning_level: "medium",
        default_reasoning_summary: "none",
        use_responses_lite: false,
        efforts: &["low", "medium", "high", "xhigh"],
        default_verbosity: "low",
        supports_priority_tier: false,
    });
    let object = card
        .as_object_mut()
        .expect("bundled Codex model card must be an object");
    object.extend(
        json!({
            "shell_type": "shell_command",
            "supports_image_detail_original": true,
            "supports_search_tool": true,
            "input_modalities": ["text", "image"],
            "context_window": 272_000,
            "max_context_window": 1_000_000,
            "experimental_supported_tools": [],
            "visibility": "hide",
            "supported_in_api": true,
            "priority": 43,
            "additional_speed_tiers": [],
            "prefer_websockets": true,
            "reasoning_summary_format": "experimental",
            "include_skills_usage_instructions": false,
            "apply_patch_tool_type": "freeform",
            "web_search_tool_type": "text_and_image",
            "truncation_policy": { "mode": "tokens", "limit": 10_000 },
            "minimal_client_version": "0.98.0",
        })
        .as_object()
        .expect("bundled Codex model card extension must be an object")
        .clone(),
    );
    card
}

pub fn bundled_codex_model_cards() -> &'static [Value] {
    static CARDS: OnceLock<Vec<Value>> = OnceLock::new();
    CARDS.get_or_init(|| {
        vec![
            bundled_gpt_5_6_codex_model_card(
                "gpt-5.6-sol",
                "GPT-5.6-Sol",
                "Latest frontier agentic coding model.",
                "low",
                1,
                "v2",
                true,
            ),
            bundled_gpt_5_6_codex_model_card(
                "gpt-5.6-terra",
                "GPT-5.6-Terra",
                "Balanced agentic coding model for everyday work.",
                "medium",
                2,
                "v2",
                true,
            ),
            bundled_gpt_5_6_codex_model_card(
                "gpt-5.6-luna",
                "GPT-5.6-Luna",
                "Fast and affordable agentic coding model.",
                "medium",
                3,
                "v1",
                false,
            ),
            bundled_codex_model_card(BundledCodexModelCardSpec {
                model_id: "gpt-5.5",
                display_name: "GPT-5.5",
                description: "Frontier model for complex coding, research, and real-world work.",
                default_reasoning_level: "medium",
                default_reasoning_summary: "none",
                use_responses_lite: false,
                efforts: &["low", "medium", "high", "xhigh"],
                default_verbosity: "low",
                supports_priority_tier: true,
            }),
            bundled_codex_model_card(BundledCodexModelCardSpec {
                model_id: "gpt-5.4",
                display_name: "GPT-5.4",
                description: "Strong model for everyday coding.",
                default_reasoning_level: "medium",
                default_reasoning_summary: "none",
                use_responses_lite: false,
                efforts: &["low", "medium", "high", "xhigh"],
                default_verbosity: "low",
                supports_priority_tier: true,
            }),
            bundled_codex_model_card(BundledCodexModelCardSpec {
                model_id: "gpt-5.4-mini",
                display_name: "GPT-5.4 Mini",
                description: "Small, fast, and cost-efficient model for simpler coding tasks.",
                default_reasoning_level: "medium",
                default_reasoning_summary: "none",
                use_responses_lite: false,
                efforts: &["low", "medium", "high", "xhigh"],
                default_verbosity: "medium",
                supports_priority_tier: false,
            }),
            bundled_codex_model_card(BundledCodexModelCardSpec {
                model_id: "gpt-5.2",
                display_name: "GPT-5.2",
                description: "Optimized for professional work and long-running agents.",
                default_reasoning_level: "medium",
                default_reasoning_summary: "auto",
                use_responses_lite: false,
                efforts: &["low", "medium", "high", "xhigh"],
                default_verbosity: "low",
                supports_priority_tier: false,
            }),
            bundled_codex_auto_review_model_card(),
        ]
    })
}

fn bundled_codex_model_card_for_identity(model_id: &str) -> Option<&'static Value> {
    let find = |model: &str| {
        bundled_codex_model_cards()
            .iter()
            .filter_map(|card| {
                let slug = card.get("slug").and_then(Value::as_str)?;
                model.starts_with(slug).then_some((slug.len(), card))
            })
            .max_by_key(|(slug_len, _)| *slug_len)
            .map(|(_, card)| card)
    };
    find(model_id).or_else(|| codex_namespaced_model_suffix(model_id).and_then(find))
}

fn catalog_codex_model_card_for_identity<'a>(
    cards: &'a serde_json::Map<String, Value>,
    model_id: &str,
) -> Option<&'a Value> {
    let find = |model: &str| {
        cards
            .iter()
            .filter_map(|(catalog_id, card)| {
                model
                    .starts_with(catalog_id)
                    .then_some((catalog_id.len(), card))
            })
            .max_by_key(|(slug_len, _)| *slug_len)
            .map(|(_, card)| card)
    };
    find(model_id).or_else(|| codex_namespaced_model_suffix(model_id).and_then(find))
}

pub fn resolve_codex_responses_model_capabilities(
    provider_model: &str,
    source_model: &str,
    upstream_metadata: Option<&Value>,
) -> CodexResponsesModelCapabilities {
    let provider_model = provider_model.trim();
    let source_model = source_model.trim();
    let source_fallback = (provider_model.is_empty()
        || crate::formats::shared::model_directives::openai_model_capability_is_opaque(
            provider_model,
            provider_model,
        ))
    .then_some(source_model)
    .filter(|model| !model.is_empty() && *model != provider_model);
    if let Some(cards) = upstream_metadata
        .and_then(|metadata| metadata.get(CODEX_MODEL_CATALOG_METADATA_FIELD))
        .and_then(|catalog| catalog.get("cards"))
        .and_then(Value::as_object)
    {
        if !cards.is_empty() {
            return catalog_codex_model_card_for_identity(cards, provider_model)
                .or_else(|| {
                    source_fallback
                        .and_then(|model| catalog_codex_model_card_for_identity(cards, model))
                })
                .and_then(codex_responses_model_capabilities_from_card)
                .map(|(_, capabilities)| capabilities)
                .unwrap_or_else(conservative_codex_responses_model_capabilities);
        }
    }
    bundled_codex_model_card_for_identity(provider_model)
        .or_else(|| source_fallback.and_then(bundled_codex_model_card_for_identity))
        .and_then(codex_responses_model_capabilities_from_card)
        .map(|(_, capabilities)| capabilities)
        .unwrap_or_else(conservative_codex_responses_model_capabilities)
}

fn codex_openai_endpoint_kind(
    provider_type: &str,
    provider_api_format: &str,
) -> Option<CodexOpenAiEndpointKind> {
    if !provider_type.trim().eq_ignore_ascii_case("codex") {
        return None;
    }
    if is_openai_responses_compact_request(provider_api_format) {
        Some(CodexOpenAiEndpointKind::Compact)
    } else if aether_ai_formats::is_openai_responses_format(provider_api_format) {
        Some(CodexOpenAiEndpointKind::Responses)
    } else if aether_ai_formats::api_format_alias_matches(provider_api_format, "openai:search") {
        Some(CodexOpenAiEndpointKind::Search)
    } else if is_openai_image_request(provider_api_format) {
        Some(CodexOpenAiEndpointKind::Images)
    } else {
        None
    }
}

/// Matches an explicit image-generation selection in `tool_choice`.
/// The `tools` array describes availability and does not select a tool.
fn codex_openai_responses_tool_choice_references_image_generation(
    body_object: &serde_json::Map<String, Value>,
) -> bool {
    match body_object.get("tool_choice") {
        Some(Value::String(name)) => name.trim().eq_ignore_ascii_case("image_generation"),
        Some(Value::Object(choice)) => choice
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("image_generation")),
        _ => false,
    }
}

fn apply_codex_openai_image_tool_overrides(body_object: &mut serde_json::Map<String, Value>) {
    let mut tool = body_object
        .get("tools")
        .and_then(Value::as_array)
        .and_then(|tools| tools.first())
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    tool.insert("type".to_string(), json!("image_generation"));
    tool.entry("output_format".to_string())
        .or_insert_with(|| json!(CODEX_OPENAI_IMAGE_DEFAULT_OUTPUT_FORMAT));
    let action = tool
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("generate")
        .to_string();
    if !tool.contains_key("action") {
        tool.insert("action".to_string(), json!("generate"));
    }
    if action == "generate" {
        tool.entry("size".to_string())
            .or_insert_with(|| json!(CODEX_IMAGE_TOOL_DEFAULT_SIZE));
        tool.entry("quality".to_string())
            .or_insert_with(|| json!(CODEX_IMAGE_TOOL_DEFAULT_QUALITY));
        tool.entry("background".to_string())
            .or_insert_with(|| json!(CODEX_IMAGE_TOOL_DEFAULT_BACKGROUND));
    }

    body_object.insert("tools".to_string(), json!([tool]));
    body_object.insert(
        "tool_choice".to_string(),
        json!({
            "type": "image_generation"
        }),
    );
}

fn codex_openai_image_has_prompt(body_object: &serde_json::Map<String, Value>) -> bool {
    body_object
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|item| item.get("content"))
        .any(|content| match content {
            Value::String(text) => !text.trim().is_empty(),
            Value::Array(items) => items.iter().any(|item| {
                item.as_object()
                    .filter(|item| item.get("type").and_then(Value::as_str) == Some("input_text"))
                    .and_then(|item| item.get("text").and_then(Value::as_str))
                    .map(str::trim)
                    .is_some_and(|text| !text.is_empty())
            }),
            _ => false,
        })
}

fn inject_codex_default_variation_prompt(body_object: &mut serde_json::Map<String, Value>) {
    let Some(action) = body_object
        .get("tools")
        .and_then(Value::as_array)
        .and_then(|tools| tools.first())
        .and_then(Value::as_object)
        .and_then(|tool| tool.get("action"))
        .and_then(Value::as_str)
    else {
        return;
    };
    if action != "edit" || codex_openai_image_has_prompt(body_object) {
        return;
    }

    let Some(input) = body_object.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    let Some(first_message) = input.first_mut().and_then(Value::as_object_mut) else {
        return;
    };
    let Some(content) = first_message
        .get_mut("content")
        .and_then(Value::as_array_mut)
    else {
        return;
    };

    content.insert(
        0,
        json!({
            "type": "input_text",
            "text": CODEX_OPENAI_IMAGE_DEFAULT_VARIATION_PROMPT,
        }),
    );
}

fn strip_codex_content_cache_control_fields(content: &mut Value) {
    match content {
        Value::Array(parts) => {
            for part in parts {
                if let Some(part) = part.as_object_mut() {
                    part.remove("cache_control");
                }
            }
        }
        Value::Object(part) => {
            part.remove("cache_control");
        }
        _ => {}
    }
}

fn strip_codex_tool_cache_control_fields(tools: &mut Value) {
    let Some(tools) = tools.as_array_mut() else {
        return;
    };
    for tool in tools {
        let Some(tool) = tool.as_object_mut() else {
            continue;
        };
        tool.remove("cache_control");
        if let Some(function) = tool.get_mut("function").and_then(Value::as_object_mut) {
            function.remove("cache_control");
        }
    }
}

fn strip_codex_cache_control_fields(value: &mut Value) {
    let Some(body) = value.as_object_mut() else {
        return;
    };
    body.remove("cache_control");
    if let Some(tools) = body.get_mut("tools") {
        strip_codex_tool_cache_control_fields(tools);
    }
    if let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) {
        for item in input {
            let Some(item) = item.as_object_mut() else {
                continue;
            };
            item.remove("cache_control");
            if let Some(content) = item.get_mut("content") {
                strip_codex_content_cache_control_fields(content);
            }
            if item
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|item_type| item_type == "additional_tools")
            {
                if let Some(tools) = item.get_mut("tools") {
                    strip_codex_tool_cache_control_fields(tools);
                }
            }
        }
    }
}

fn remove_btree_header(headers: &mut BTreeMap<String, String>, header_name: &str) {
    headers.retain(|name, _| !name.trim().eq_ignore_ascii_case(header_name));
}

fn header_value_contains_media_type(value: &str, media_type: &str) -> bool {
    value.split(',').any(|media_range| {
        media_range
            .split(';')
            .next()
            .map(str::trim)
            .is_some_and(|value| value.eq_ignore_ascii_case(media_type))
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexAuthIdentity {
    pub account_id: Option<String>,
    pub is_fedramp: bool,
    pub uses_codex_backend: bool,
}

pub fn parse_codex_auth_identity(decrypted_auth_config_raw: Option<&str>) -> CodexAuthIdentity {
    let Some(raw) = decrypted_auth_config_raw
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return CodexAuthIdentity::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return CodexAuthIdentity::default();
    };
    let namespaced_auth = value
        .get("https://api.openai.com/auth")
        .and_then(Value::as_object);
    let account_id = value
        .get("account_id")
        .or_else(|| value.get("chatgpt_account_id"))
        .or_else(|| namespaced_auth.and_then(|auth| auth.get("chatgpt_account_id")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let is_fedramp = value
        .get("is_fedramp")
        .or_else(|| value.get("chatgpt_account_is_fedramp"))
        .or_else(|| namespaced_auth.and_then(|auth| auth.get("chatgpt_account_is_fedramp")))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let uses_codex_backend = account_id.is_some()
        || value
            .get("provider_type")
            .and_then(Value::as_str)
            .is_some_and(|provider_type| provider_type.trim().eq_ignore_ascii_case("codex"));

    CodexAuthIdentity {
        account_id,
        is_fedramp,
        uses_codex_backend,
    }
}

fn set_codex_client_header(
    provider_request_headers: &mut BTreeMap<String, String>,
    header_name: &str,
    header_value: &str,
) {
    remove_btree_header(provider_request_headers, header_name);
    provider_request_headers.insert(header_name.to_string(), header_value.to_string());
}

pub fn apply_openai_responses_compact_special_body_edits(
    provider_request_body: &mut Value,
    provider_api_format: &str,
) {
    if !is_openai_responses_compact_request(provider_api_format) {
        return;
    }

    let Some(body_object) = provider_request_body.as_object_mut() else {
        return;
    };
    super::request::apply_compact_request_projection(body_object);
}

pub fn apply_codex_openai_responses_compact_body_edits(
    provider_request_body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
) {
    if !is_codex_openai_responses_request(provider_type, provider_api_format)
        || !is_openai_responses_compact_request(provider_api_format)
    {
        return;
    }
    strip_codex_cache_control_fields(provider_request_body);
    let Some(body_object) = provider_request_body.as_object_mut() else {
        return;
    };
    body_object
        .retain(|field, _| CODEX_OPENAI_RESPONSES_COMPACT_BODY_FIELDS.contains(&field.as_str()));
    body_object
        .entry("parallel_tool_calls".to_string())
        .or_insert_with(|| json!(true));
    for field in [
        "tools",
        "reasoning",
        "service_tier",
        "prompt_cache_key",
        "text",
    ] {
        if body_object.get(field).is_some_and(Value::is_null) {
            body_object.remove(field);
        }
    }
    if body_object
        .get("instructions")
        .is_some_and(|value| value.is_null() || value.as_str().is_some_and(str::is_empty))
    {
        body_object.remove("instructions");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexOpenAiCompactRequestContractViolation {
    pub field: &'static str,
    pub reason: &'static str,
}

pub fn validate_codex_openai_responses_compact_request_contract(
    provider_request_body: &Value,
    provider_type: &str,
    provider_api_format: &str,
) -> Result<(), CodexOpenAiCompactRequestContractViolation> {
    if codex_openai_endpoint_kind(provider_type, provider_api_format)
        != Some(CodexOpenAiEndpointKind::Compact)
    {
        return Ok(());
    }
    let Some(body) = provider_request_body.as_object() else {
        return Err(CodexOpenAiCompactRequestContractViolation {
            field: "$",
            reason: "Codex Compact request body must be an object",
        });
    };
    if body
        .get("model")
        .and_then(Value::as_str)
        .is_none_or(|model| model.trim().is_empty())
    {
        return Err(CodexOpenAiCompactRequestContractViolation {
            field: "model",
            reason: "Codex Compact model must be a non-empty string",
        });
    }
    let Some(input) = body.get("input").and_then(Value::as_array) else {
        return Err(CodexOpenAiCompactRequestContractViolation {
            field: "input",
            reason: "Codex Compact input must be an array",
        });
    };
    if input.iter().any(|item| {
        !item.as_object().is_some_and(|item| {
            item.get("type")
                .and_then(Value::as_str)
                .is_some_and(|item_type| !item_type.trim().is_empty())
        })
    }) {
        return Err(CodexOpenAiCompactRequestContractViolation {
            field: "input",
            reason: "Codex Compact input items must be typed objects",
        });
    }
    for (field, valid) in [
        (
            "instructions",
            body.get("instructions").is_none_or(Value::is_string),
        ),
        ("tools", body.get("tools").is_none_or(Value::is_array)),
        (
            "reasoning",
            body.get("reasoning").is_none_or(Value::is_object),
        ),
        (
            "service_tier",
            body.get("service_tier").is_none_or(Value::is_string),
        ),
        (
            "prompt_cache_key",
            body.get("prompt_cache_key").is_none_or(Value::is_string),
        ),
        ("text", body.get("text").is_none_or(Value::is_object)),
    ] {
        if !valid {
            return Err(CodexOpenAiCompactRequestContractViolation {
                field,
                reason: "Codex Compact optional field has an invalid type",
            });
        }
    }
    if !body
        .get("parallel_tool_calls")
        .is_some_and(Value::is_boolean)
    {
        return Err(CodexOpenAiCompactRequestContractViolation {
            field: "parallel_tool_calls",
            reason: "Codex Compact parallel_tool_calls must be a boolean",
        });
    }
    Ok(())
}

pub fn apply_codex_openai_compact_terminal_headers(
    provider_request_headers: &mut BTreeMap<String, String>,
    provider_type: &str,
    provider_api_format: &str,
) {
    if codex_openai_endpoint_kind(provider_type, provider_api_format)
        != Some(CodexOpenAiEndpointKind::Compact)
    {
        return;
    }
    remove_btree_header(provider_request_headers, "x-client-request-id");
    remove_btree_header(provider_request_headers, "accept");
    remove_btree_header(provider_request_headers, "content-encoding");
}

fn apply_codex_model_request_capabilities(
    body_object: &mut serde_json::Map<String, Value>,
    provider_api_format: &str,
    capabilities: &CodexResponsesModelCapabilities,
    body_rules: Option<&Value>,
) {
    if is_openai_image_request(provider_api_format) {
        return;
    }
    if !body_rules_handle_path(body_rules, "parallel_tool_calls") {
        let requested = body_object
            .get("parallel_tool_calls")
            .and_then(Value::as_bool)
            .unwrap_or(capabilities.supports_parallel_tool_calls);
        body_object.insert(
            "parallel_tool_calls".to_string(),
            json!(
                requested
                    && capabilities.supports_parallel_tool_calls
                    && !capabilities.use_responses_lite
            ),
        );
    }

    if !is_openai_responses_compact_request(provider_api_format)
        && !body_rules_handle_path(body_rules, "include")
    {
        let include = if body_object.get("reasoning").is_some_and(Value::is_object) {
            json!([CODEX_REASONING_ENCRYPTED_CONTENT_INCLUDE])
        } else {
            json!([])
        };
        body_object.insert("include".to_string(), include);
    }

    if !body_rules_handle_path(body_rules, "text") {
        if capabilities.support_verbosity {
            if let Some(default_verbosity) = capabilities.default_verbosity.as_deref() {
                match body_object.get_mut("text") {
                    Some(Value::Object(text)) => {
                        text.entry("verbosity".to_string())
                            .or_insert_with(|| json!(default_verbosity));
                    }
                    None | Some(Value::Null) => {
                        body_object.insert(
                            "text".to_string(),
                            json!({ "verbosity": default_verbosity }),
                        );
                    }
                    Some(_) => {}
                }
            }
        } else if let Some(Value::Object(text)) = body_object.get_mut("text") {
            text.remove("verbosity");
            if text.is_empty() {
                body_object.remove("text");
            }
        }
    }

    if !body_rules_handle_path(body_rules, "service_tier") {
        let service_tier = body_object
            .get("service_tier")
            .and_then(Value::as_str)
            .map(str::to_string);
        if !service_tier.as_deref().is_some_and(|service_tier| {
            service_tier != "default" && capabilities.supports_service_tier(service_tier)
        }) {
            body_object.remove("service_tier");
        }
    }
}

fn ensure_codex_reasoning_defaults(
    body_object: &mut serde_json::Map<String, Value>,
    capabilities: &CodexResponsesModelCapabilities,
    supports_reasoning_mode: bool,
    body_rules: Option<&Value>,
) {
    if body_rules_handle_path(body_rules, "reasoning") && !capabilities.use_responses_lite {
        let has_summary = body_object
            .get_mut("reasoning")
            .and_then(Value::as_object_mut)
            .and_then(|reasoning| {
                if !capabilities.supports_reasoning_summary_parameter
                    || reasoning
                        .get("summary")
                        .is_some_and(codex_reasoning_summary_is_disabled)
                {
                    reasoning.remove("summary");
                }
                reasoning.get("summary")
            })
            .is_some_and(|summary| !summary.is_null());
        if !has_summary {
            remove_codex_reasoning_summary_delivery(body_object);
        }
        return;
    }
    let reasoning = body_object
        .entry("reasoning".to_string())
        .or_insert_with(|| json!({}));
    if reasoning.is_null() {
        *reasoning = json!({});
    }
    let Some(reasoning_object) = reasoning.as_object_mut() else {
        return;
    };
    if reasoning_object.get("effort").is_none_or(Value::is_null) {
        let default_effort = if supports_reasoning_mode
            && reasoning_object
                .get("mode")
                .and_then(Value::as_str)
                .is_some_and(|mode| matches!(mode, "standard" | "pro"))
        {
            Some(CODEX_DEFAULT_REASONING_EFFORT)
        } else {
            capabilities.default_reasoning_effort.as_deref()
        };
        if let Some(default_effort) = default_effort {
            reasoning_object.insert("effort".to_string(), json!(default_effort));
        }
    }
    if !capabilities.supports_reasoning_summary_parameter
        || reasoning_object
            .get("summary")
            .is_some_and(codex_reasoning_summary_is_disabled)
    {
        reasoning_object.remove("summary");
    } else if !reasoning_object.contains_key("summary") {
        if let Some(summary) = capabilities.default_reasoning_summary.as_deref() {
            reasoning_object.insert("summary".to_string(), json!(summary));
        }
    }
    if capabilities.use_responses_lite {
        reasoning_object.insert("context".to_string(), json!("all_turns"));
    }
    let has_summary = reasoning_object
        .get("summary")
        .is_some_and(|summary| !summary.is_null());
    if !has_summary {
        remove_codex_reasoning_summary_delivery(body_object);
    }
}

fn codex_reasoning_summary_is_disabled(value: &Value) -> bool {
    value.is_null()
        || value
            .as_str()
            .is_some_and(|summary| summary.eq_ignore_ascii_case("none"))
}

fn remove_codex_reasoning_summary_delivery(body_object: &mut serde_json::Map<String, Value>) {
    let remove_stream_options = body_object
        .get_mut("stream_options")
        .and_then(Value::as_object_mut)
        .is_some_and(|stream_options| {
            stream_options.remove("reasoning_summary_delivery");
            stream_options.is_empty()
        });
    if remove_stream_options {
        body_object.remove("stream_options");
    }
}

fn normalize_codex_reasoning_effort(body_object: &mut serde_json::Map<String, Value>) {
    let Some(reasoning) = body_object
        .get_mut("reasoning")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let is_ultra = reasoning
        .get("effort")
        .and_then(Value::as_str)
        .is_some_and(|effort| effort == "ultra");
    if is_ultra {
        reasoning.insert("effort".to_string(), json!("max"));
    }
}

pub fn normalize_codex_openai_reasoning_wire_effort(
    provider_request_body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
) {
    if !provider_type.trim().eq_ignore_ascii_case("codex")
        || !(aether_ai_formats::is_openai_responses_family_format(provider_api_format)
            || aether_ai_formats::api_format_alias_matches(provider_api_format, "openai:search"))
    {
        return;
    }
    let Some(body_object) = provider_request_body.as_object_mut() else {
        return;
    };
    normalize_codex_reasoning_effort(body_object);
}

fn is_codex_responses_lite_additional_tools_item(value: &Value) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|item_type| item_type == "additional_tools")
}

fn codex_tool_type_accepts_top_level_name(tool_type: &str) -> bool {
    matches!(tool_type, "function" | "custom" | "namespace")
}

fn is_codex_client_executed_tool(tool: &Value) -> bool {
    match tool.get("type").and_then(Value::as_str) {
        Some("function" | "custom" | "namespace") => true,
        Some("tool_search") => tool.get("execution").and_then(Value::as_str) == Some("client"),
        _ => false,
    }
}

fn retain_codex_client_executed_tools(additional_tools: &mut Value) {
    let Some(tools) = additional_tools
        .get_mut("tools")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    tools.retain(is_codex_client_executed_tool);
}

fn is_codex_responses_lite_instruction_item(value: &Value) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|item_type| item_type == "message")
        && value
            .get("role")
            .and_then(Value::as_str)
            .is_some_and(|role| role == "developer")
        && value
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|content| {
                content.len() == 1
                    && content[0]
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|item_type| item_type == "input_text")
                    && content[0].get("text").is_some_and(Value::is_string)
            })
}

fn strip_codex_responses_lite_image_details(item: &mut Value) {
    let Some(item_object) = item.as_object_mut() else {
        return;
    };
    let item_type = item_object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let content = match item_type {
        "message" => item_object.get_mut("content"),
        "function_call_output" | "custom_tool_call_output" => item_object.get_mut("output"),
        _ => None,
    };
    let Some(content) = content.and_then(Value::as_array_mut) else {
        return;
    };
    for content_item in content {
        let Some(content_object) = content_item.as_object_mut() else {
            continue;
        };
        if content_object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|content_type| content_type == "input_image")
        {
            content_object.remove("detail");
        }
    }
}

fn codex_responses_lite_supports_context_management(context_management: Option<&Value>) -> bool {
    context_management.is_none_or(Value::is_null)
}

fn codex_responses_lite_supports_request_body(provider_request_body: Option<&Value>) -> bool {
    provider_request_body.is_none_or(|body| {
        codex_responses_lite_supports_context_management(body.get("context_management"))
    })
}

fn apply_codex_responses_lite_body_contract(
    body_object: &mut serde_json::Map<String, Value>,
    capabilities: &CodexResponsesModelCapabilities,
) {
    if !capabilities.use_responses_lite {
        return;
    }

    let tools_are_valid = body_object
        .get("tools")
        .is_none_or(|tools| tools.is_null() || tools.is_array());
    let instructions_are_valid = body_object
        .get("instructions")
        .is_none_or(|instructions| instructions.is_null() || instructions.is_string());
    if !tools_are_valid || !instructions_are_valid {
        return;
    }
    if !body_object.get("input").is_some_and(Value::is_array) {
        return;
    }

    let top_level_tools = body_object
        .remove("tools")
        .and_then(|tools| tools.as_array().cloned())
        .map(|tools| {
            tools
                .into_iter()
                .filter(is_codex_client_executed_tool)
                .collect::<Vec<_>>()
        });
    let top_level_instructions = body_object
        .remove("instructions")
        .and_then(|instructions| instructions.as_str().map(ToOwned::to_owned))
        .filter(|instructions| !instructions.is_empty());
    let input = body_object
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .expect("Responses Lite input was validated as an array");

    let existing_additional_tools = input
        .iter()
        .position(is_codex_responses_lite_additional_tools_item)
        .map(|index| input.remove(index));
    let mut additional_tools = existing_additional_tools.unwrap_or_else(|| {
        json!({
            "type": "additional_tools",
            "role": "developer",
            "tools": [],
        })
    });
    if let Some(tools) = top_level_tools {
        if let Some(object) = additional_tools.as_object_mut() {
            object.insert("tools".to_string(), Value::Array(tools));
        }
    }
    retain_codex_client_executed_tools(&mut additional_tools);
    input.insert(0, additional_tools);

    if let Some(instructions) = top_level_instructions {
        if input
            .get(1)
            .is_some_and(is_codex_responses_lite_instruction_item)
        {
            input.remove(1);
        }
        input.insert(
            1,
            json!({
                "type": "message",
                "role": "developer",
                "content": [{
                    "type": "input_text",
                    "text": instructions,
                }],
            }),
        );
    }

    for item in input
        .iter_mut()
        .filter(|item| !is_codex_responses_lite_additional_tools_item(item))
    {
        strip_codex_responses_lite_image_details(item);
    }
    body_object.insert("parallel_tool_calls".to_string(), json!(false));
}

fn remove_empty_codex_instructions(body_object: &mut serde_json::Map<String, Value>) {
    if body_object
        .get("instructions")
        .is_some_and(|value| value.is_null() || value.as_str().is_some_and(str::is_empty))
    {
        body_object.remove("instructions");
    }
}

fn codex_tool_type_rejects_top_level_name(tool_type: &str) -> bool {
    let normalized = tool_type.trim().to_ascii_lowercase();
    !normalized.is_empty() && !codex_tool_type_accepts_top_level_name(normalized.as_str())
}

fn strip_codex_hosted_tool_names_for_backend(body_object: &mut serde_json::Map<String, Value>) {
    let Some(tools) = body_object.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };

    for tool in tools {
        let Some(tool_object) = tool.as_object_mut() else {
            continue;
        };
        if tool_object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(codex_tool_type_rejects_top_level_name)
        {
            tool_object.remove("name");
        }
    }
}

fn strip_codex_hosted_tool_choice_name_for_backend(
    body_object: &mut serde_json::Map<String, Value>,
) {
    let Some(tool_choice_object) = body_object
        .get_mut("tool_choice")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if tool_choice_object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(codex_tool_type_rejects_top_level_name)
    {
        tool_choice_object.remove("name");
    }
}

fn wrap_codex_responses_string_input_for_backend(
    body_object: &mut serde_json::Map<String, Value>,
    provider_api_format: &str,
) {
    if !aether_ai_formats::is_openai_responses_family_format(provider_api_format) {
        return;
    }
    let Some(text) = body_object
        .get("input")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return;
    };

    body_object.insert(
        "input".to_string(),
        json!([{
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": text,
            }],
        }]),
    );
}

fn non_empty_json_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn adapt_codex_prompt_cache_identity_for_backend(body_object: &mut serde_json::Map<String, Value>) {
    // The standard OpenAI contract permits arbitrary prompt_cache_key strings, while the Codex
    // backend's native session identity is UUID-shaped. Adapt only requests that carry a cache key
    // but do not already carry a native Codex session identity.
    let Some(prompt_cache_key) = body_object
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
    else {
        return;
    };

    match body_object.get("client_metadata") {
        Some(Value::Object(metadata)) => {
            match metadata.get("session_id") {
                Some(Value::String(session_id)) if !session_id.trim().is_empty() => return,
                None | Some(Value::Null) => {}
                Some(_) => return,
            }
            match metadata.get("thread_id") {
                None | Some(Value::Null) => {}
                Some(Value::String(thread_id)) if !thread_id.trim().is_empty() => {}
                Some(_) => return,
            }
        }
        Some(Value::Null) | None => {}
        Some(_) => return,
    }

    let cache_identity = uuid::Uuid::parse_str(&prompt_cache_key)
        .unwrap_or_else(|_| {
            uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_URL,
                format!("{CODEX_PROMPT_CACHE_IDENTITY_NAMESPACE}{prompt_cache_key}").as_bytes(),
            )
        })
        .to_string();

    {
        let metadata = body_object
            .entry("client_metadata".to_string())
            .or_insert_with(|| json!({}));
        if metadata.is_null() {
            *metadata = json!({});
        }
        let Some(metadata) = metadata.as_object_mut() else {
            return;
        };
        metadata.insert(
            "session_id".to_string(),
            Value::String(cache_identity.clone()),
        );
        if metadata.get("thread_id").is_none_or(Value::is_null) {
            metadata.insert(
                "thread_id".to_string(),
                Value::String(cache_identity.clone()),
            );
        }
    }

    body_object.insert(
        "prompt_cache_key".to_string(),
        Value::String(cache_identity),
    );
}

fn valid_codex_identity_header(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.parse::<http::HeaderValue>().is_err() {
        return None;
    }
    Some(value.to_string())
}

fn codex_prompt_cache_header_identity(provider_request_body: &Value) -> Option<(String, String)> {
    let prompt_cache_key = non_empty_json_string(provider_request_body.get("prompt_cache_key"))?;

    match provider_request_body.get("client_metadata") {
        Some(Value::Object(metadata)) => match metadata.get("session_id") {
            Some(Value::String(session_id)) if !session_id.trim().is_empty() => {
                let session_id = valid_codex_identity_header(session_id)?;
                let thread_id = match metadata.get("thread_id") {
                    None | Some(Value::Null) => session_id.clone(),
                    Some(Value::String(thread_id)) if !thread_id.trim().is_empty() => {
                        valid_codex_identity_header(thread_id)?
                    }
                    Some(_) => return None,
                };
                return Some((session_id, thread_id));
            }
            None | Some(Value::Null) => {}
            Some(_) => return None,
        },
        Some(Value::Null) | None => {}
        Some(_) => return None,
    }

    let cache_identity = uuid::Uuid::parse_str(prompt_cache_key).ok()?.to_string();
    Some((cache_identity.clone(), cache_identity))
}

fn insert_btree_header_if_missing(
    headers: &mut BTreeMap<String, String>,
    header_name: &str,
    header_value: String,
) {
    if headers.iter().any(|(name, value)| {
        name.trim().eq_ignore_ascii_case(header_name) && !value.trim().is_empty()
    }) {
        return;
    }
    remove_btree_header(headers, header_name);
    headers.insert(header_name.to_string(), header_value);
}

pub fn apply_codex_openai_responses_identity_headers(
    provider_request_headers: &mut BTreeMap<String, String>,
    provider_request_body: &Value,
    provider_type: &str,
    provider_api_format: &str,
) {
    // Codex projects its body session identity into compatibility HTTP headers. Existing explicit
    // headers remain authoritative; only missing values are completed here.
    if !is_codex_openai_responses_request(provider_type, provider_api_format) {
        return;
    }
    let Some((session_id, thread_id)) = codex_prompt_cache_header_identity(provider_request_body)
    else {
        return;
    };
    insert_btree_header_if_missing(provider_request_headers, "session-id", session_id);
    insert_btree_header_if_missing(provider_request_headers, "thread-id", thread_id);
}

pub fn apply_codex_openai_responses_special_body_edits(
    provider_request_body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
    body_rules: Option<&Value>,
    user_api_key_id: Option<&str>,
) {
    let provider_model = provider_request_body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    apply_codex_openai_responses_special_body_edits_with_source_model(
        provider_request_body,
        provider_type,
        provider_api_format,
        provider_model.as_str(),
        provider_model.as_str(),
        body_rules,
        user_api_key_id,
    );
}

pub fn apply_codex_openai_responses_special_body_edits_with_source_model(
    provider_request_body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    body_rules: Option<&Value>,
    _user_api_key_id: Option<&str>,
) {
    apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
        provider_request_body,
        provider_type,
        provider_api_format,
        provider_model,
        source_model,
        None,
        body_rules,
    );
}

pub fn apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
    provider_request_body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    model_capabilities: Option<&CodexResponsesModelCapabilities>,
    body_rules: Option<&Value>,
) {
    if !is_codex_openai_responses_request(provider_type, provider_api_format) {
        return;
    }

    let Some(body_object) = provider_request_body.as_object_mut() else {
        return;
    };

    wrap_codex_responses_string_input_for_backend(body_object, provider_api_format);
    for field in CODEX_OPENAI_RESPONSES_UNSUPPORTED_BODY_FIELDS {
        if !body_rules_handle_path(body_rules, field) {
            body_object.remove(*field);
        }
    }
    adapt_codex_prompt_cache_identity_for_backend(body_object);
    if is_openai_responses_compact_request(provider_api_format) {
        body_object.remove("store");
    } else if !body_rules_handle_path(body_rules, "store") {
        body_object.insert("store".to_string(), json!(false));
    }
    remove_empty_codex_instructions(body_object);
    strip_codex_hosted_tool_names_for_backend(body_object);
    strip_codex_hosted_tool_choice_name_for_backend(body_object);
    if !is_openai_responses_compact_request(provider_api_format) {
        body_object
            .entry("tool_choice".to_string())
            .or_insert_with(|| json!("auto"));
    }
    if !is_openai_responses_compact_request(provider_api_format)
        && codex_openai_responses_tool_choice_references_image_generation(body_object)
    {
        body_object.insert(
            "model".to_string(),
            json!(CODEX_OPENAI_IMAGE_INTERNAL_MODEL),
        );
        body_object.insert("stream".to_string(), json!(true));
        apply_codex_openai_image_tool_overrides(body_object);
        inject_codex_default_variation_prompt(body_object);
    }
    let effective_provider_model = body_object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(provider_model)
        .to_string();
    let bundled_capabilities;
    let capabilities = if effective_provider_model == CODEX_OPENAI_IMAGE_INTERNAL_MODEL {
        bundled_capabilities = resolve_codex_responses_model_capabilities(
            effective_provider_model.as_str(),
            source_model,
            None,
        );
        &bundled_capabilities
    } else if let Some(capabilities) = model_capabilities {
        capabilities
    } else {
        bundled_capabilities = resolve_codex_responses_model_capabilities(
            effective_provider_model.as_str(),
            source_model,
            None,
        );
        &bundled_capabilities
    };
    let standard_contract_capabilities = (capabilities.use_responses_lite
        && !codex_responses_lite_supports_context_management(
            body_object.get("context_management"),
        ))
    .then(|| CodexResponsesModelCapabilities {
        use_responses_lite: false,
        ..capabilities.clone()
    });
    let capabilities = standard_contract_capabilities
        .as_ref()
        .unwrap_or(capabilities);
    let supports_reasoning_mode =
        crate::formats::shared::model_directives::openai_model_resolves_to_gpt_5_6(
            effective_provider_model.as_str(),
            source_model,
        );
    ensure_codex_reasoning_defaults(
        body_object,
        capabilities,
        supports_reasoning_mode,
        body_rules,
    );
    normalize_codex_reasoning_effort(body_object);
    apply_codex_model_request_capabilities(
        body_object,
        provider_api_format,
        capabilities,
        body_rules,
    );
    apply_codex_responses_lite_body_contract(body_object, capabilities);
    strip_codex_cache_control_fields(provider_request_body);
    apply_codex_openai_responses_compact_body_edits(
        provider_request_body,
        provider_type,
        provider_api_format,
    );
}

pub fn apply_codex_openai_responses_chat_body_edits(
    provider_request_body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
    body_rules: Option<&Value>,
    user_api_key_id: Option<&str>,
) {
    let provider_model = provider_request_body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    apply_codex_openai_responses_chat_body_edits_with_source_model(
        provider_request_body,
        provider_type,
        provider_api_format,
        provider_model.as_str(),
        provider_model.as_str(),
        body_rules,
        user_api_key_id,
    );
}

pub fn apply_codex_openai_responses_chat_body_edits_with_source_model(
    provider_request_body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    body_rules: Option<&Value>,
    _user_api_key_id: Option<&str>,
) {
    apply_codex_openai_responses_chat_body_edits_with_source_model_and_capabilities(
        provider_request_body,
        provider_type,
        provider_api_format,
        provider_model,
        source_model,
        None,
        body_rules,
    );
}

pub fn apply_codex_openai_responses_chat_body_edits_with_source_model_and_capabilities(
    provider_request_body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    model_capabilities: Option<&CodexResponsesModelCapabilities>,
    body_rules: Option<&Value>,
) {
    apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
        provider_request_body,
        provider_type,
        provider_api_format,
        provider_model,
        source_model,
        model_capabilities,
        body_rules,
    );

    if !is_codex_openai_responses_request(provider_type, provider_api_format) {
        return;
    }
    let Some(body_object) = provider_request_body.as_object_mut() else {
        return;
    };
    if let Some(prompt_cache_key) = body_object.remove("prompt_cache_key") {
        body_object.insert("prompt_cache_key".to_string(), prompt_cache_key);
    }
}

pub fn apply_codex_openai_responses_lite_header(
    provider_request_headers: &mut BTreeMap<String, String>,
    provider_type: &str,
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
) {
    apply_codex_openai_responses_lite_header_with_capabilities(
        provider_request_headers,
        provider_type,
        provider_api_format,
        provider_model,
        source_model,
        None,
    );
}

pub fn apply_codex_openai_responses_lite_header_with_capabilities(
    provider_request_headers: &mut BTreeMap<String, String>,
    provider_type: &str,
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    model_capabilities: Option<&CodexResponsesModelCapabilities>,
) {
    apply_codex_openai_responses_lite_header_for_request_body_with_capabilities(
        provider_request_headers,
        None,
        provider_type,
        provider_api_format,
        provider_model,
        source_model,
        model_capabilities,
    );
}

pub fn apply_codex_openai_responses_lite_header_for_request_body_with_capabilities(
    provider_request_headers: &mut BTreeMap<String, String>,
    provider_request_body: Option<&Value>,
    provider_type: &str,
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    model_capabilities: Option<&CodexResponsesModelCapabilities>,
) {
    if !is_codex_openai_responses_request(provider_type, provider_api_format) {
        return;
    }
    remove_btree_header(provider_request_headers, CODEX_RESPONSES_LITE_HEADER);
    let bundled_capabilities;
    let capabilities = if let Some(capabilities) = model_capabilities {
        capabilities
    } else {
        bundled_capabilities =
            resolve_codex_responses_model_capabilities(provider_model, source_model, None);
        &bundled_capabilities
    };
    if capabilities.use_responses_lite
        && codex_responses_lite_supports_request_body(provider_request_body)
    {
        provider_request_headers
            .insert(CODEX_RESPONSES_LITE_HEADER.to_string(), "true".to_string());
    }
}

pub fn apply_codex_openai_special_headers(
    provider_request_headers: &mut BTreeMap<String, String>,
    provider_request_body: &Value,
    _original_headers: &http::HeaderMap,
    provider_type: &str,
    provider_api_format: &str,
    _request_id: Option<&str>,
    decrypted_auth_config_raw: Option<&str>,
) {
    let Some(endpoint_kind) = codex_openai_endpoint_kind(provider_type, provider_api_format) else {
        return;
    };

    let auth_identity = parse_codex_auth_identity(decrypted_auth_config_raw);

    remove_btree_header(provider_request_headers, "chatgpt-account-id");
    remove_btree_header(provider_request_headers, "x-openai-fedramp");
    if let Some(account_id) = auth_identity.account_id {
        provider_request_headers.insert("chatgpt-account-id".to_string(), account_id);
    }

    if auth_identity.is_fedramp {
        provider_request_headers.insert("x-openai-fedramp".to_string(), "true".to_string());
    }

    set_codex_client_header(
        provider_request_headers,
        "user-agent",
        CODEX_CLIENT_USER_AGENT,
    );
    set_codex_client_header(
        provider_request_headers,
        "originator",
        CODEX_CLIENT_ORIGINATOR,
    );
    if endpoint_kind == CodexOpenAiEndpointKind::Search {
        remove_btree_header(provider_request_headers, CODEX_RESPONSES_LITE_HEADER);
        remove_btree_header(provider_request_headers, "openai-beta");
        if provider_request_headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("accept")
                && header_value_contains_media_type(value, "text/event-stream")
        }) {
            remove_btree_header(provider_request_headers, "accept");
        }
        return;
    }
    if endpoint_kind == CodexOpenAiEndpointKind::Images {
        return;
    }

    let provider_model = provider_request_body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    apply_codex_openai_responses_lite_header_for_request_body_with_capabilities(
        provider_request_headers,
        Some(provider_request_body),
        provider_type,
        provider_api_format,
        provider_model,
        provider_model,
        None,
    );

    apply_codex_openai_compact_terminal_headers(
        provider_request_headers,
        provider_type,
        provider_api_format,
    );
}

#[cfg(test)]
mod tests {
    use super::{
        apply_codex_openai_responses_chat_body_edits,
        apply_codex_openai_responses_compact_body_edits,
        apply_codex_openai_responses_lite_header_for_request_body_with_capabilities,
        apply_codex_openai_responses_lite_header_with_capabilities,
        apply_codex_openai_responses_special_body_edits,
        apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities,
        apply_codex_openai_special_headers, apply_openai_responses_compact_special_body_edits,
        build_codex_model_catalog_metadata, bundled_codex_model_cards, effective_codex_model_cards,
        resolve_codex_responses_model_capabilities,
        validate_codex_openai_responses_compact_request_contract, CODEX_CLIENT_ORIGINATOR,
        CODEX_CLIENT_USER_AGENT, CODEX_OPENAI_IMAGE_INTERNAL_MODEL,
        CODEX_OPENAI_RESPONSES_UNSUPPORTED_BODY_FIELDS, CODEX_RESPONSES_LITE_HEADER,
    };
    use serde_json::{json, Value};

    #[test]
    fn model_card_drives_unknown_codex_model_body_and_header_contracts() {
        let card = json!({
            "id": "gpt-future-agent",
            "slug": "gpt-future-agent",
            "use_responses_lite": true,
            "default_reasoning_level": "low",
            "default_reasoning_summary": "auto",
            "supports_parallel_tool_calls": true,
            "support_verbosity": true,
            "default_verbosity": "low",
            "service_tiers": [{"id": "priority"}],
            "supported_reasoning_levels": [
                {"effort": "low"},
                {"effort": "max"},
                {"effort": "ultra"}
            ],
            "base_instructions": "large prompt",
            "model_messages": {"instructions_template": "large template"},
            "available_in_plans": ["pro"],
            "future_capability": {"mode": "native"}
        });
        let metadata = build_codex_model_catalog_metadata(&[card]);
        assert_eq!(
            metadata["codex_models"]["cards"]["gpt-future-agent"]["future_capability"]["mode"],
            "native"
        );
        let stored_card = &metadata["codex_models"]["cards"]["gpt-future-agent"];
        assert!(stored_card.get("base_instructions").is_none());
        assert!(stored_card.get("model_messages").is_none());
        assert!(stored_card.get("available_in_plans").is_none());
        let capabilities = resolve_codex_responses_model_capabilities(
            "gpt-future-agent",
            "gpt-future-agent",
            Some(&metadata),
        );
        assert!(capabilities.supports_reasoning_summary_parameter);
        assert_eq!(
            capabilities.default_reasoning_effort.as_deref(),
            Some("low")
        );
        assert_eq!(
            capabilities.default_reasoning_summary.as_deref(),
            Some("auto")
        );

        let mut body = json!({
            "model": "gpt-future-agent",
            "instructions": "Use the tools.",
            "input": [{"id": "msg-1", "type": "message", "role": "user", "content": []}],
            "tools": [{"type": "function", "name": "lookup"}],
            "reasoning": {"effort": "ultra"},
            "parallel_tool_calls": true,
            "service_tier": "priority",
            "text": {"format": {"type": "json_schema"}},
            "include": ["file_search_call.results"]
        });
        apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
            &mut body,
            "codex",
            "openai:responses",
            "gpt-future-agent",
            "gpt-future-agent",
            Some(&capabilities),
            None,
        );
        assert_eq!(body["reasoning"]["effort"], "max");
        assert_eq!(body["reasoning"]["summary"], "auto");
        assert_eq!(body["reasoning"]["context"], "all_turns");
        assert!(body.get("instructions").is_none());
        assert!(body.get("tools").is_none());
        assert_eq!(body["input"][0]["type"], "additional_tools");
        assert!(body["input"][1].get("id").is_none());
        assert_eq!(body["input"][2]["id"], "msg-1");
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["service_tier"], "priority");
        assert_eq!(body["text"]["verbosity"], "low");
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));

        let mut headers = std::collections::BTreeMap::new();
        apply_codex_openai_responses_lite_header_with_capabilities(
            &mut headers,
            "codex",
            "openai:responses",
            "gpt-future-agent",
            "gpt-future-agent",
            Some(&capabilities),
        );
        assert_eq!(
            headers.get("x-openai-internal-codex-responses-lite"),
            Some(&"true".to_string())
        );
    }

    #[test]
    fn codex_responses_lite_header_is_omitted_for_server_side_compaction() {
        let mut headers = std::collections::BTreeMap::from([(
            CODEX_RESPONSES_LITE_HEADER.to_string(),
            "true".to_string(),
        )]);
        let body = json!({
            "model": "gpt-5.6-sol",
            "context_management": [{
                "type": "compaction",
                "compact_threshold": 128000
            }]
        });

        apply_codex_openai_responses_lite_header_for_request_body_with_capabilities(
            &mut headers,
            Some(&body),
            "codex",
            "openai:responses",
            "gpt-5.6-sol",
            "gpt-5.6-sol",
            None,
        );

        assert!(!headers.contains_key(CODEX_RESPONSES_LITE_HEADER));
    }

    #[test]
    fn model_card_without_summary_parameter_support_keeps_reasoning_and_ids() {
        let metadata = build_codex_model_catalog_metadata(&[json!({
            "slug": "gpt-no-summary",
            "supports_reasoning_summary_parameter": false,
            "default_reasoning_level": "high",
            "default_reasoning_summary": "detailed",
            "supported_reasoning_levels": [{"effort": "high"}],
            "supports_parallel_tool_calls": true
        })]);
        let capabilities = resolve_codex_responses_model_capabilities(
            "gpt-no-summary",
            "gpt-no-summary",
            Some(&metadata),
        );
        assert!(!capabilities.supports_reasoning_summary_parameter);

        let mut body = json!({
            "model": "gpt-no-summary",
            "input": [{
                "id": "msg-client",
                "type": "message",
                "role": "user",
                "content": []
            }],
            "reasoning": {"effort": "high", "summary": "detailed"},
            "stream_options": {
                "reasoning_summary_delivery": "sequential_cutoff",
                "future_option": true
            }
        });
        apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
            &mut body,
            "codex",
            "openai:responses",
            "gpt-no-summary",
            "gpt-no-summary",
            Some(&capabilities),
            None,
        );

        assert_eq!(body["reasoning"], json!({"effort": "high"}));
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(body["stream_options"], json!({"future_option": true}));
        assert_eq!(body["input"][0]["id"], "msg-client");
    }

    #[test]
    fn codex_request_normalizes_the_reasoning_envelope_without_model_defaults() {
        let capabilities = resolve_codex_responses_model_capabilities(
            "gpt-future-agent",
            "gpt-future-agent",
            None,
        );
        for initial_reasoning in [None, Some(Value::Null)] {
            let mut body = json!({
                "model": "gpt-future-agent",
                "input": [],
                "stream_options": {
                    "reasoning_summary_delivery": "sequential_cutoff"
                }
            });
            if let Some(initial_reasoning) = initial_reasoning {
                body["reasoning"] = initial_reasoning;
            }

            apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
                &mut body,
                "codex",
                "openai:responses",
                "gpt-future-agent",
                "gpt-future-agent",
                Some(&capabilities),
                None,
            );

            assert_eq!(body["reasoning"], json!({}));
            assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
            assert!(body.get("stream_options").is_none());
        }

        let mut compact = json!({
            "model": "gpt-future-agent",
            "input": [],
            "reasoning": null,
            "include": ["reasoning.encrypted_content"],
            "store": true
        });
        apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
            &mut compact,
            "codex",
            "openai:responses:compact",
            "gpt-future-agent",
            "gpt-future-agent",
            Some(&capabilities),
            None,
        );

        assert_eq!(compact["reasoning"], json!({}));
        assert!(compact.get("include").is_none());
        assert!(compact.get("store").is_none());
    }

    #[test]
    fn empty_codex_model_catalog_clears_the_card_map() {
        assert_eq!(
            build_codex_model_catalog_metadata(&[])["codex_models"]["cards"],
            json!({})
        );
    }

    #[test]
    fn ultra_reasoning_always_uses_max_on_the_provider_wire() {
        for provider_api_format in ["openai:responses", "openai:responses:compact"] {
            let mut body = json!({
                "model": "gpt-5.6-luna",
                "input": [],
                "reasoning": {"effort": "ultra"}
            });
            apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
                &mut body,
                "codex",
                provider_api_format,
                "gpt-5.6-luna",
                "gpt-5.6-luna",
                None,
                None,
            );
            assert_eq!(body["reasoning"]["effort"], "max");
        }

        let mut custom = json!({
            "model": "gpt-5.6-luna",
            "input": [],
            "reasoning": {"effort": " ultra "}
        });
        apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
            &mut custom,
            "codex",
            "openai:responses",
            "gpt-5.6-luna",
            "gpt-5.6-luna",
            None,
            None,
        );
        assert_eq!(custom["reasoning"]["effort"], " ultra ");
    }

    #[test]
    fn populated_remote_catalog_is_authoritative_and_matches_model_variants() {
        let metadata = build_codex_model_catalog_metadata(&[json!({
            "slug": "gpt-future-agent",
            "supports_reasoning_summary_parameter": true,
            "default_reasoning_level": "high",
            "supported_reasoning_levels": [{"effort": "high"}],
            "supports_parallel_tool_calls": true
        })]);

        let variant = resolve_codex_responses_model_capabilities(
            "gpt-future-agent-2026-07-10",
            "gpt-future-agent-2026-07-10",
            Some(&metadata),
        );
        assert_eq!(variant.default_reasoning_effort.as_deref(), Some("high"));
        assert!(variant.supports_parallel_tool_calls);

        for model in [
            "gpt-future-agentpreview",
            "provider_1/gpt-future-agent-2026-07-10",
        ] {
            let capabilities =
                resolve_codex_responses_model_capabilities(model, model, Some(&metadata));
            assert_eq!(
                capabilities.default_reasoning_effort.as_deref(),
                Some("high"),
                "model: {model}"
            );
        }

        for model in [
            "GPT-FUTURE-AGENT",
            "gpt_future_agent",
            "org/team/gpt-future-agent",
            "provider!/gpt-future-agent",
        ] {
            let capabilities =
                resolve_codex_responses_model_capabilities(model, model, Some(&metadata));
            assert_eq!(
                capabilities.default_reasoning_effort, None,
                "model: {model}"
            );
        }

        let missing = resolve_codex_responses_model_capabilities(
            "gpt-5.6-sol",
            "gpt-5.6-sol",
            Some(&metadata),
        );
        assert!(!missing.use_responses_lite);
        assert!(missing.supports_reasoning_summary_parameter);
        assert!(!missing.supports_parallel_tool_calls);
        assert_eq!(missing.default_reasoning_effort, None);

        let mut body = json!({
            "model": "gpt-5.6-sol",
            "input": [],
            "reasoning": {"effort": "high"},
            "parallel_tool_calls": true,
            "service_tier": "priority",
            "text": {
                "verbosity": "high",
                "format": {"type": "json_schema"}
            }
        });
        apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
            &mut body,
            "codex",
            "openai:responses",
            "gpt-5.6-sol",
            "gpt-5.6-sol",
            Some(&missing),
            None,
        );
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(body["parallel_tool_calls"], false);
        assert!(body.get("service_tier").is_none());
        assert!(body["text"].get("verbosity").is_none());
        assert_eq!(body["text"]["format"]["type"], "json_schema");

        let mut compact = json!({
            "model": "gpt-5.6-sol",
            "input": [],
            "reasoning": {"effort": "high"}
        });
        apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
            &mut compact,
            "codex",
            "openai:responses:compact",
            "gpt-5.6-sol",
            "gpt-5.6-sol",
            Some(&missing),
            None,
        );
        assert_eq!(compact["reasoning"]["effort"], "high");
    }

    #[test]
    fn model_catalog_keys_and_hidden_card_overlays_use_exact_slugs() {
        let metadata = build_codex_model_catalog_metadata(&[
            json!({"slug": "gpt-card"}),
            json!({"slug": "GPT-CARD"}),
            json!({"slug": "gpt_card"}),
        ]);
        let cards = metadata["codex_models"]["cards"]
            .as_object()
            .expect("card map");
        assert_eq!(cards.len(), 3);
        assert!(cards.contains_key("gpt-card"));
        assert!(cards.contains_key("GPT-CARD"));
        assert!(cards.contains_key("gpt_card"));

        let effective = effective_codex_model_cards(&[json!({
            "slug": "GPT-5.6-SOL",
            "visibility": "hide"
        })]);
        assert!(effective.iter().any(|card| card["slug"] == "gpt-5.6-sol"));
        assert!(effective.iter().any(|card| card["slug"] == "GPT-5.6-SOL"));

        let effective = effective_codex_model_cards(&[json!({
            "id": "gpt-5.6-sol",
            "default_reasoning_level": "high",
            "future_capability": {"mode": "native"}
        })]);
        let sol = effective
            .iter()
            .find(|card| card["slug"] == "gpt-5.6-sol")
            .expect("merged Sol card");
        assert_eq!(sol["default_reasoning_level"], "high");
        assert_eq!(sol["future_capability"]["mode"], "native");
        assert_eq!(sol["use_responses_lite"], true);
        assert_eq!(sol["default_reasoning_summary"], "none");
        assert!(sol["supported_reasoning_levels"]
            .as_array()
            .is_some_and(|levels| levels.iter().any(|level| level["effort"] == "ultra")));
    }

    #[test]
    fn bare_gpt_5_6_requires_an_explicit_model_card() {
        let conservative = resolve_codex_responses_model_capabilities("gpt-5.6", "gpt-5.6", None);
        assert!(!conservative.use_responses_lite);
        assert!(conservative.supports_reasoning_summary_parameter);
        assert!(!conservative.supports_parallel_tool_calls);
        assert_eq!(conservative.default_reasoning_effort, None);
        assert_eq!(conservative.default_verbosity, None);
        assert!(conservative.supported_service_tiers.is_empty());

        let metadata = build_codex_model_catalog_metadata(&[json!({
            "slug": "gpt-5.6",
            "use_responses_lite": true,
            "supports_reasoning_summary_parameter": true,
            "default_reasoning_level": "medium",
            "supported_reasoning_levels": [{"effort": "medium"}],
            "supports_parallel_tool_calls": true,
            "support_verbosity": true,
            "default_verbosity": "low",
            "service_tiers": [{"id": "priority"}]
        })]);
        let explicit =
            resolve_codex_responses_model_capabilities("gpt-5.6", "gpt-5.6", Some(&metadata));
        assert!(explicit.use_responses_lite);
        assert_eq!(explicit.default_reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(explicit.default_verbosity.as_deref(), Some("low"));
        assert_eq!(explicit.supported_service_tiers, vec!["priority"]);

        let opaque = resolve_codex_responses_model_capabilities(
            "deployment-production",
            "gpt-5.6-sol",
            None,
        );
        assert!(opaque.use_responses_lite);
        assert_eq!(opaque.default_reasoning_effort.as_deref(), Some("low"));

        for provider_model in ["GPT-5.6-SOL", "gpt_5.6_sol", "org/team/gpt-5.6-sol"] {
            let capabilities =
                resolve_codex_responses_model_capabilities(provider_model, "gpt-5.6-sol", None);
            assert!(!capabilities.use_responses_lite, "model: {provider_model}");
        }
    }

    #[test]
    fn model_card_preserves_custom_reasoning_effort_values() {
        let metadata = build_codex_model_catalog_metadata(&[json!({
            "slug": "codex-custom",
            "supports_reasoning_summary_parameter": true,
            "default_reasoning_level": "VendorEffortX",
            "supported_reasoning_levels": [
                {"effort": "VendorEffortX"},
                {"effort": "MAX"}
            ]
        })]);
        let capabilities = resolve_codex_responses_model_capabilities(
            "codex-custom",
            "codex-custom",
            Some(&metadata),
        );
        assert_eq!(
            capabilities.default_reasoning_effort.as_deref(),
            Some("VendorEffortX")
        );
        assert_eq!(
            capabilities.supported_reasoning_efforts,
            vec!["VendorEffortX", "MAX"]
        );
        assert!(capabilities.supports_reasoning_effort("VendorEffortX"));
        assert!(!capabilities.supports_reasoning_effort("vendoreffortx"));
        assert!(!capabilities.supports_reasoning_effort("max"));
    }

    #[test]
    fn bundled_auto_review_card_matches_the_codex_request_profile() {
        let card = bundled_codex_model_cards()
            .iter()
            .find(|card| card["slug"] == "codex-auto-review")
            .expect("Codex auto review card");
        assert_eq!(card["visibility"], "hide");
        assert_eq!(card["supported_in_api"], true);
        assert_eq!(card["priority"], 43);

        let capabilities = resolve_codex_responses_model_capabilities(
            "codex-auto-review",
            "codex-auto-review",
            None,
        );
        assert!(!capabilities.use_responses_lite);
        assert_eq!(
            capabilities.default_reasoning_effort.as_deref(),
            Some("medium")
        );
        assert_eq!(capabilities.default_reasoning_summary, None);
        assert!(capabilities.supports_parallel_tool_calls);
        assert_eq!(capabilities.default_verbosity.as_deref(), Some("low"));
        assert!(capabilities.supported_service_tiers.is_empty());
    }

    #[test]
    fn codex_identity_headers_are_derived_only_from_auth_config() {
        let mut headers = std::collections::BTreeMap::from([
            ("chatgpt-account-id".to_string(), "spoofed".to_string()),
            ("x-openai-fedramp".to_string(), "true".to_string()),
        ]);

        apply_codex_openai_special_headers(
            &mut headers,
            &json!({"model": "gpt-5.6-sol", "input": []}),
            &http::HeaderMap::new(),
            "codex",
            "openai:responses",
            Some("request-1"),
            Some(r#"{"is_fedramp":false}"#),
        );

        assert!(!headers.contains_key("chatgpt-account-id"));
        assert!(!headers.contains_key("x-openai-fedramp"));
        assert_eq!(
            headers
                .get("x-openai-internal-codex-responses-lite")
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn codex_search_uses_identity_headers_without_responses_protocol_headers() {
        let mut headers = std::collections::BTreeMap::from([
            (
                "x-openai-internal-codex-responses-lite".to_string(),
                "true".to_string(),
            ),
            ("openai-beta".to_string(), "responses=v1".to_string()),
            (
                "accept".to_string(),
                "application/json, Text/Event-Stream; q=0.9".to_string(),
            ),
        ]);

        apply_codex_openai_special_headers(
            &mut headers,
            &json!({"id": "session-1", "model": "gpt-5.6-luna"}),
            &http::HeaderMap::new(),
            "codex",
            "openai:search",
            Some("request-search"),
            Some(r#"{"account_id":"account-1","is_fedramp":true}"#),
        );

        assert_eq!(
            headers.get("chatgpt-account-id").map(String::as_str),
            Some("account-1")
        );
        assert_eq!(
            headers.get("x-openai-fedramp").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            headers.get("user-agent").map(String::as_str),
            Some(CODEX_CLIENT_USER_AGENT)
        );
        assert_eq!(
            headers.get("originator").map(String::as_str),
            Some(CODEX_CLIENT_ORIGINATOR)
        );
        assert!(!headers.contains_key(CODEX_RESPONSES_LITE_HEADER));
        assert!(!headers.contains_key("openai-beta"));
        assert!(!headers.contains_key("accept"));
    }

    #[test]
    fn standard_codex_models_do_not_send_the_responses_lite_header() {
        let mut headers = std::collections::BTreeMap::from([(
            "x-openai-internal-codex-responses-lite".to_string(),
            "true".to_string(),
        )]);

        apply_codex_openai_special_headers(
            &mut headers,
            &json!({"model": "gpt-5.4", "input": []}),
            &http::HeaderMap::new(),
            "codex",
            "openai:responses",
            Some("request-1"),
            None,
        );

        assert!(!headers.contains_key("x-openai-internal-codex-responses-lite"));
    }

    #[test]
    fn codex_compact_headers_preserve_session_identity_and_remove_stream_headers() {
        let mut headers = std::collections::BTreeMap::from([
            ("session-id".to_string(), "session-1".to_string()),
            ("thread-id".to_string(), "thread-1".to_string()),
            ("x-client-request-id".to_string(), "thread-1".to_string()),
            ("accept".to_string(), "text/event-stream".to_string()),
        ]);

        apply_codex_openai_special_headers(
            &mut headers,
            &json!({"model": "gpt-5.6", "input": []}),
            &http::HeaderMap::new(),
            "codex",
            "openai:responses:compact",
            Some("request-1"),
            None,
        );

        assert_eq!(
            headers.get("session-id").map(String::as_str),
            Some("session-1")
        );
        assert_eq!(
            headers.get("thread-id").map(String::as_str),
            Some("thread-1")
        );
        assert!(!headers.contains_key("x-client-request-id"));
        assert!(!headers.contains_key("accept"));
    }

    #[test]
    fn codex_responses_body_edits_inject_passthrough_fields_without_reasoning_summary() {
        let mut provider_request_body = json!( {
            "input": [{
                "role": "user",
                "content": "hello"
            }],
            "model": "gpt-5.4",
            "stream": true
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(provider_request_body["reasoning"]["effort"], "medium");
        assert!(provider_request_body["reasoning"].get("summary").is_none());
        assert_eq!(
            provider_request_body["include"],
            json!(["reasoning.encrypted_content"])
        );
        assert_eq!(provider_request_body["parallel_tool_calls"], json!(true));
        assert_eq!(provider_request_body["tool_choice"], json!("auto"));
        assert!(provider_request_body.get("instructions").is_none());
    }

    #[test]
    fn codex_responses_body_edits_omit_disabled_reasoning_summary_and_delivery() {
        let mut provider_request_body = json!({
            "input": [{"role": "user", "content": "hello"}],
            "model": "gpt-5.6-sol",
            "stream": true,
            "reasoning": {"effort": "high", "summary": "none"},
            "stream_options": {
                "reasoning_summary_delivery": "sequential_cutoff",
                "future_option": true
            }
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert!(provider_request_body["reasoning"].get("summary").is_none());
        assert_eq!(
            provider_request_body["stream_options"],
            json!({"future_option": true})
        );
    }

    #[test]
    fn codex_responses_body_edits_project_include_and_preserve_disabled_parallel_calls() {
        let mut provider_request_body = json!( {
            "input": [],
            "model": "gpt-5.4",
            "include": [
                "file_search_call.results",
                "web_search_call.results",
                "web_search_call.action.sources",
                "message.input_image.image_url",
                "computer_call_output.output.image_url",
                "code_interpreter_call.outputs",
                "message.output_text.logprobs"
            ],
            "reasoning": {"effort": "high", "summary": "detailed"},
            "parallel_tool_calls": false
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(provider_request_body["reasoning"]["effort"], json!("high"));
        assert_eq!(
            provider_request_body["reasoning"]["summary"],
            json!("detailed")
        );
        assert_eq!(
            provider_request_body["include"],
            json!(["reasoning.encrypted_content"])
        );
        assert_eq!(provider_request_body["parallel_tool_calls"], json!(false));
    }

    #[test]
    fn codex_responses_body_edits_wrap_string_input_for_backend() {
        let mut provider_request_body = json!({
            "input": "hello",
            "model": "gpt-5.4"
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(
            provider_request_body["input"],
            json!([{
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "hello"
                }]
            }])
        );
    }

    #[test]
    fn codex_responses_body_edits_preserve_function_tools_for_codex_backend() {
        let mut provider_request_body = json!({
            "input": [],
            "model": "gpt-5.4",
            "tools": [{
                "type": "function",
                "name": "lookup_account",
                "description": "Lookup an account by id.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "account_id": {
                            "type": "string"
                        }
                    },
                    "required": ["account_id"],
                    "additionalProperties": false
                },
                "strict": true
            }],
            "tool_choice": {
                "type": "function",
                "name": "lookup_account"
            }
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(
            provider_request_body["tools"][0]["name"],
            json!("lookup_account")
        );
        assert_eq!(
            provider_request_body["tools"][0]["parameters"]["properties"]["account_id"]["type"],
            json!("string")
        );
        assert_eq!(
            provider_request_body["tool_choice"]["name"],
            json!("lookup_account")
        );
        assert!(provider_request_body["tools"][0].get("function").is_none());
    }

    #[test]
    fn codex_responses_body_edits_apply_backend_request_contract() {
        let mut provider_request_body = json!({
            "input": [{
                "id": "msg-1",
                "type": "message",
                "role": "user",
                "content": [{"id": "content-1", "type": "input_text", "text": "hello"}]
            }],
            "model": "gpt-5.4",
            "max_output_tokens": 1024,
            "max_completion_tokens": 1024,
            "temperature": 0.2,
            "top_p": 0.8,
            "frequency_penalty": 0.1,
            "presence_penalty": 0.1,
            "user": "user-123",
            "metadata": {"client": "cursor"},
            "prompt_cache_options": {"mode": "explicit", "ttl": "30m"},
            "prompt_cache_retention": "24h",
            "safety_identifier": "safe-user-123",
            "stream_options": {"reasoning_summary_delivery": "sequential_cutoff"},
            "previous_response_id": "resp_123"
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        for field in CODEX_OPENAI_RESPONSES_UNSUPPORTED_BODY_FIELDS {
            assert!(
                provider_request_body.get(*field).is_none(),
                "{field} must be stripped"
            );
        }
        assert!(provider_request_body.get("stream_options").is_none());
        assert_eq!(provider_request_body["input"][0]["id"], "msg-1");
        assert_eq!(
            provider_request_body["input"][0]["content"][0]["id"],
            json!("content-1")
        );
    }

    #[test]
    fn codex_responses_body_edits_preserve_client_selected_input_item_ids() {
        let body_rules = json!([{"action":"set","path":"store","value":true}]);
        let mut provider_request_body = json!({
            "model": "gpt-5.4",
            "store": true,
            "input": [{
                "id": "msg-1",
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "hello"}]
            }]
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            Some(&body_rules),
            None,
        );

        assert_eq!(provider_request_body["store"], true);
        assert_eq!(provider_request_body["input"][0]["id"], "msg-1");
    }

    #[test]
    fn codex_responses_lite_additional_tools_contain_only_client_executed_specs() {
        let mut provider_request_body = json!({
            "model": "gpt-5.6-sol",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "Use a tool"}]
            }],
            "tools": [
                {"type": "function", "name": "lookup", "parameters": {}},
                {"type": "custom", "name": "apply_patch", "description": "Apply a patch", "format": {}},
                {"type": "namespace", "name": "web", "description": "Web tools", "tools": []},
                {"type": "web_search"},
                {"type": "image_generation"},
                {"type": "tool_search", "execution": "client"},
                {"type": "tool_search", "execution": "server"},
                {"type": "tool_search"},
                {"type": "future_tool"},
                {"name": "missing_type"}
            ]
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert!(provider_request_body.get("tools").is_none());
        assert_eq!(
            provider_request_body["input"][0]["tools"],
            json!([
                {"type": "function", "name": "lookup", "parameters": {}},
                {"type": "custom", "name": "apply_patch", "description": "Apply a patch", "format": {}},
                {"type": "namespace", "name": "web", "description": "Web tools", "tools": []},
                {"type": "tool_search", "execution": "client"}
            ])
        );

        let mut existing_additional_tools = json!({
            "model": "gpt-5.6-sol",
            "input": [{
                "type": "additional_tools",
                "role": "developer",
                "tools": [
                    {"type": "function", "name": "shell", "parameters": {}},
                    {"type": "web_search"},
                    {"type": "image_generation"}
                ]
            }]
        });
        apply_codex_openai_responses_special_body_edits(
            &mut existing_additional_tools,
            "codex",
            "openai:responses:compact",
            None,
            None,
        );
        assert_eq!(
            existing_additional_tools["input"][0]["tools"],
            json!([{"type": "function", "name": "shell", "parameters": {}}])
        );
    }

    #[test]
    fn codex_responses_body_edits_strip_name_from_hosted_web_search_tool() {
        let mut provider_request_body = json!({
            "input": [],
            "model": "gpt-5.4",
            "tools": [{
                "type": "web_search",
                "name": "web_search"
            }],
            "tool_choice": {
                "type": "web_search",
                "name": "web_search"
            }
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert!(provider_request_body["tools"][0].get("name").is_none());
        assert!(provider_request_body["tool_choice"].get("name").is_none());
        assert_eq!(
            provider_request_body["tool_choice"]["type"],
            json!("web_search")
        );
    }

    #[test]
    fn codex_responses_body_edits_do_not_derive_prompt_cache_key_from_metadata() {
        let mut body = json!({
            "input": [{"role": "user", "content": "hello"}],
            "model": "gpt-5.4",
            "metadata": {
                "user_id": "{\"session_id\":\"session-a\",\"device_id\":\"device-a\"}"
            }
        });

        apply_codex_openai_responses_special_body_edits(
            &mut body,
            "codex",
            "openai:responses",
            None,
            Some("key-123"),
        );

        assert!(body.get("prompt_cache_key").is_none());
        assert!(body.get("metadata").is_none());
    }

    #[test]
    fn codex_responses_body_edits_do_not_treat_client_metadata_as_a_cache_key_source() {
        let mut body = json!({
            "model": "gpt-5.6-sol",
            "input": [],
            "client_metadata": {
                "session_id": "session-123",
                "thread_id": "thread-123"
            }
        });

        apply_codex_openai_responses_special_body_edits(
            &mut body,
            "codex",
            "openai:responses",
            None,
            Some("key-123"),
        );

        assert!(body.get("prompt_cache_key").is_none());
        assert_eq!(body["client_metadata"]["thread_id"], "thread-123");
    }

    #[test]
    fn codex_responses_body_edits_strip_cache_control_without_deriving_a_cache_key() {
        let mut body = json!({
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "stable project brief",
                    "cache_control": {"type": "ephemeral"}
                }]
            }, {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "new turn A"}]
            }],
            "model": "gpt-5.4"
        });

        apply_codex_openai_responses_special_body_edits(
            &mut body,
            "codex",
            "openai:responses",
            None,
            Some("key-a"),
        );

        assert!(body.get("prompt_cache_key").is_none());
        assert!(!body.to_string().contains("\"cache_control\""));
    }

    #[test]
    fn codex_responses_body_edits_strip_developer_cache_control_before_upstream() {
        let mut provider_request_body = json!({
            "input": [{
                "type": "message",
                "role": "developer",
                "content": [{
                    "type": "input_text",
                    "text": "stable system brief",
                    "cache_control": {"type": "ephemeral"}
                }]
            }, {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "new turn"}]
            }],
            "model": "gpt-5.4"
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            Some("key-a"),
        );

        assert!(provider_request_body.get("prompt_cache_key").is_none());
        assert!(!provider_request_body
            .to_string()
            .contains("\"cache_control\""));
        assert_eq!(
            provider_request_body["input"][0]["content"][0]["text"],
            json!("stable system brief")
        );
    }

    #[test]
    fn codex_responses_body_edits_preserve_cache_control_named_tool_schema_property() {
        let mut provider_request_body = json!({
            "model": "gpt-5.4",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "inspect the payload",
                    "cache_control": {"type": "ephemeral"}
                }]
            }],
            "tools": [{
                "type": "function",
                "name": "inspect",
                "cache_control": {"type": "ephemeral"},
                "parameters": {
                    "type": "object",
                    "properties": {
                        "cache_control": {"type": "string"}
                    }
                }
            }]
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            Some("key-a"),
        );

        assert!(provider_request_body["input"][0]["content"][0]
            .get("cache_control")
            .is_none());
        assert!(provider_request_body["tools"][0]
            .get("cache_control")
            .is_none());
        assert_eq!(
            provider_request_body["tools"][0]["parameters"]["properties"]["cache_control"]["type"],
            "string"
        );
    }

    #[test]
    fn codex_responses_lite_compact_preserves_cache_control_named_tool_schema_property() {
        let mut provider_request_body = json!({
            "model": "gpt-5.6-sol",
            "instructions": "Use the inspection tool.",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "inspect the payload",
                    "cache_control": {"type": "ephemeral"}
                }]
            }],
            "tools": [{
                "type": "function",
                "name": "inspect",
                "cache_control": {"type": "ephemeral"},
                "parameters": {
                    "type": "object",
                    "properties": {
                        "cache_control": {"type": "string"}
                    }
                }
            }]
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses:compact",
            None,
            Some("key-a"),
        );

        assert!(provider_request_body.get("tools").is_none());
        assert_eq!(
            provider_request_body["input"][0]["type"],
            "additional_tools"
        );
        assert!(provider_request_body["input"][0]["tools"][0]
            .get("cache_control")
            .is_none());
        assert_eq!(
            provider_request_body["input"][0]["tools"][0]["parameters"]["properties"]
                ["cache_control"]["type"],
            "string"
        );
        assert!(provider_request_body["input"][2]["content"][0]
            .get("cache_control")
            .is_none());
    }

    #[test]
    fn codex_responses_body_edits_do_not_synthesize_a_cache_key_from_request_content() {
        let mut body = json!({
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "open workspace"}]
            }, {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "new turn A"}]
            }],
            "model": "gpt-5.4",
            "instructions": "Be concise.",
            "tools": [{
                "type": "function",
                "name": "shell",
                "parameters": {"type": "object", "properties": {}}
            }],
            "reasoning": {"effort": "medium"}
        });
        apply_codex_openai_responses_special_body_edits(
            &mut body,
            "codex",
            "openai:responses",
            None,
            Some("key-a"),
        );

        assert!(body.get("prompt_cache_key").is_none());
    }

    #[test]
    fn compact_body_edits_apply_the_codex_request_projection() {
        let mut provider_request_body = json!({
            "input": [],
            "model": "gpt-5.4",
            "client_metadata": {"origin": "codex"},
            "include": ["reasoning.encrypted_content"],
            "store": true,
            "stream": true,
            "stream_options": {"reasoning_summary_delivery": "sequential_cutoff"},
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "reasoning": {"effort": "high"},
            "text": {"verbosity": "medium"},
            "tools": [{"type": "function", "name": "lookup"}],
            "prompt_cache_key": "session:compact",
        });

        apply_openai_responses_compact_special_body_edits(
            &mut provider_request_body,
            "openai:responses:compact",
        );

        for field in [
            "client_metadata",
            "include",
            "store",
            "stream",
            "stream_options",
            "tool_choice",
        ] {
            assert!(provider_request_body.get(field).is_none());
        }
        assert_eq!(provider_request_body["model"], json!("gpt-5.4"));
        assert_eq!(provider_request_body["input"], json!([]));
        assert_eq!(provider_request_body["parallel_tool_calls"], json!(true));
        assert_eq!(provider_request_body["reasoning"]["effort"], json!("high"));
        assert_eq!(provider_request_body["text"]["verbosity"], json!("medium"));
        assert_eq!(provider_request_body["tools"][0]["name"], json!("lookup"));
        assert_eq!(
            provider_request_body["prompt_cache_key"],
            json!("session:compact")
        );
    }

    #[test]
    fn codex_responses_body_edits_omit_empty_instructions() {
        for instructions in [None, Some(Value::Null), Some(json!(""))] {
            for api_format in ["openai:responses", "openai:responses:compact"] {
                let mut provider_request_body = json!({
                    "model": "gpt-5.6-sol",
                    "input": []
                });
                if let Some(instructions) = instructions.clone() {
                    provider_request_body["instructions"] = instructions;
                }

                apply_codex_openai_responses_special_body_edits(
                    &mut provider_request_body,
                    "codex",
                    api_format,
                    None,
                    None,
                );

                assert!(provider_request_body.get("instructions").is_none());
            }
        }

        let mut whitespace = json!({
            "model": "gpt-5.6-sol",
            "input": [],
            "instructions": " "
        });
        apply_codex_openai_responses_special_body_edits(
            &mut whitespace,
            "codex",
            "openai:responses",
            None,
            None,
        );
        assert!(whitespace.get("instructions").is_none());
        assert_eq!(whitespace["input"][0]["type"], "additional_tools");
        assert_eq!(whitespace["input"][1]["role"], "developer");
        assert_eq!(whitespace["input"][1]["content"][0]["text"], " ");
    }

    #[test]
    fn codex_compact_body_matches_the_typed_client_payload() {
        let mut provider_request_body = json!({
            "model": "gpt-5.6-sol",
            "input": [
                {
                    "id": "msg-1",
                    "type": "message",
                    "role": "user",
                    "content": [{
                        "id": "content-id",
                        "type": "input_text",
                        "text": "hello",
                        "cache_control": {"type": "ephemeral"}
                    }]
                },
                {"id": "call-1", "type": "function_call", "name": "lookup"},
                {"id": "future-1", "type": "future_item", "value": true}
            ],
            "top_logprobs": 5,
            "max_output_tokens": 100,
            "previous_response_id": "resp_123",
            "prompt_cache_options": {"ttl": "30m"},
            "custom_extension": true,
            "service_tier": "priority",
            "text": {"verbosity": "medium"}
        });

        apply_codex_openai_responses_compact_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses:compact",
        );

        assert_eq!(
            provider_request_body,
            json!({
                "model": "gpt-5.6-sol",
                "input": [
                    {
                        "id": "msg-1",
                        "type": "message",
                        "role": "user",
                        "content": [{
                            "id": "content-id",
                            "type": "input_text",
                            "text": "hello"
                        }]
                    },
                    {"id": "call-1", "type": "function_call", "name": "lookup"},
                    {"id": "future-1", "type": "future_item", "value": true}
                ],
                "parallel_tool_calls": true,
                "service_tier": "priority",
                "text": {"verbosity": "medium"}
            })
        );
        validate_codex_openai_responses_compact_request_contract(
            &provider_request_body,
            "codex",
            "openai:responses:compact",
        )
        .expect("projected Compact request should match the typed contract");
    }

    #[test]
    fn codex_compact_ignores_image_generation_tool_choice_before_projection() {
        let base = json!({
            "model": "gpt-5.4",
            "input": [{"role": "user", "content": "generate image"}],
            "tools": [{"type": "image_generation"}]
        });
        let mut without_tool_choice = base.clone();
        let mut with_tool_choice = base;
        with_tool_choice["tool_choice"] = json!({"type": "image_generation"});

        for body in [&mut without_tool_choice, &mut with_tool_choice] {
            apply_codex_openai_responses_special_body_edits(
                body,
                "codex",
                "openai:responses:compact",
                None,
                None,
            );
        }

        assert_eq!(with_tool_choice, without_tool_choice);
        assert_eq!(with_tool_choice["model"], json!("gpt-5.4"));
        assert!(with_tool_choice.get("tool_choice").is_none());
    }

    #[test]
    fn codex_compact_contract_rejects_invalid_routing_mutations() {
        for (field, value) in [
            ("input", json!("hello")),
            ("parallel_tool_calls", json!("true")),
            ("reasoning", json!("high")),
            ("text", json!("medium")),
        ] {
            let mut body = json!({
                "model": "gpt-5.6-sol",
                "input": [],
                "parallel_tool_calls": true
            });
            body[field] = value;
            let violation = validate_codex_openai_responses_compact_request_contract(
                &body,
                "codex",
                "openai:responses:compact",
            )
            .expect_err("invalid Compact field type should be rejected");
            assert_eq!(violation.field, field);
        }
    }

    #[test]
    fn codex_chat_body_edits_apply_model_reasoning_defaults() {
        for (model, effort, summary) in [
            ("gpt-5.6-sol", "low", None),
            ("gpt-5.6-terra", "medium", None),
            ("gpt-5.6-luna", "medium", None),
            ("gpt-5.4", "medium", None),
            ("gpt-5.2", "medium", Some("auto")),
        ] {
            let mut provider_request_body = json!({
                "input": [],
                "model": model
            });

            apply_codex_openai_responses_chat_body_edits(
                &mut provider_request_body,
                "codex",
                "openai:responses",
                None,
                None,
            );

            assert_eq!(provider_request_body["reasoning"]["effort"], effort);
            assert_eq!(
                provider_request_body["reasoning"]
                    .get("summary")
                    .and_then(Value::as_str),
                summary
            );
            assert_eq!(
                provider_request_body["include"],
                json!(["reasoning.encrypted_content"])
            );
            let uses_responses_lite = model.starts_with("gpt-5.6");
            assert_eq!(
                provider_request_body["parallel_tool_calls"],
                json!(!uses_responses_lite)
            );
            assert_eq!(
                provider_request_body["reasoning"]
                    .get("context")
                    .and_then(Value::as_str),
                uses_responses_lite.then_some("all_turns")
            );
        }
    }

    #[test]
    fn codex_chat_body_edits_preserve_existing_reasoning_effort() {
        let mut provider_request_body = json!({
            "input": [],
            "model": "gpt-5.4",
            "reasoning": {"effort": "low"}
        });

        apply_codex_openai_responses_chat_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(provider_request_body["reasoning"]["effort"], json!("low"));
        assert!(provider_request_body["reasoning"].get("summary").is_none());
    }

    #[test]
    fn codex_gpt_5_6_public_reasoning_modes_default_to_medium_effort() {
        for mode in ["standard", "pro"] {
            let mut provider_request_body = json!({
                "input": [],
                "model": "gpt-5.6-sol",
                "reasoning": {"mode": mode}
            });

            apply_codex_openai_responses_chat_body_edits(
                &mut provider_request_body,
                "codex",
                "openai:responses",
                None,
                None,
            );

            assert_eq!(provider_request_body["reasoning"]["effort"], "medium");
            assert_eq!(provider_request_body["reasoning"]["mode"], mode);
        }
    }

    #[test]
    fn codex_responses_image_tool_edits_force_internal_model_and_tool_defaults() {
        let mut provider_request_body = json!({
            "model": "gpt-image-2",
            "input": "generate image",
            "tools": [{
                "type": "image_generation"
            }],
            "tool_choice": {"type": "image_generation"}
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(
            provider_request_body["model"],
            json!(CODEX_OPENAI_IMAGE_INTERNAL_MODEL)
        );
        assert_eq!(provider_request_body["stream"], json!(true));
        assert_eq!(
            provider_request_body["tools"][0]["type"],
            json!("image_generation")
        );
        assert_eq!(
            provider_request_body["tool_choice"]["type"],
            json!("image_generation")
        );
    }

    #[test]
    fn codex_responses_image_tool_edits_triggered_by_string_tool_choice() {
        let mut provider_request_body = json!({
            "model": "gpt-image-2",
            "input": "generate image",
            "tools": [{"type": "image_generation"}],
            "tool_choice": "image_generation"
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(
            provider_request_body["model"],
            json!(CODEX_OPENAI_IMAGE_INTERNAL_MODEL)
        );
        assert_eq!(
            provider_request_body["tool_choice"]["type"],
            json!("image_generation")
        );
    }

    #[test]
    fn codex_responses_image_tool_edits_skipped_when_tool_choice_is_auto() {
        let original_model = "gpt-5.5";
        let mut provider_request_body = json!({
            "model": original_model,
            "input": [{"role": "user", "content": "hi"}],
            "tools": [
                {"type": "function", "name": "shell"},
                {"type": "image_generation"},
                {"type": "web_search"}
            ],
            "tool_choice": "auto"
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(provider_request_body["model"], json!(original_model));
        assert_eq!(provider_request_body["tool_choice"], json!("auto"));
        assert_eq!(
            provider_request_body["tools"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            3,
            "tools array should be preserved when tool_choice is auto"
        );
    }

    #[test]
    fn codex_responses_image_tool_edits_skipped_when_tool_choice_absent() {
        let original_model = "gpt-5.5";
        let mut provider_request_body = json!({
            "model": original_model,
            "input": [{"role": "user", "content": "hi"}],
            "tools": [{"type": "image_generation"}]
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(provider_request_body["model"], json!(original_model));
        assert_eq!(provider_request_body["tool_choice"], json!("auto"));
        assert_eq!(
            provider_request_body["tools"][0]["type"],
            json!("image_generation")
        );
        assert_eq!(
            provider_request_body["tools"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1,
            "tools array should be preserved verbatim when tool_choice is absent"
        );
    }

    #[test]
    fn codex_responses_image_tool_edits_skipped_when_tool_choice_targets_other_tool() {
        let original_model = "gpt-5.5";
        let mut provider_request_body = json!({
            "model": original_model,
            "input": [{"role": "user", "content": "hi"}],
            "tools": [
                {"type": "function", "name": "shell"},
                {"type": "image_generation"}
            ],
            "tool_choice": {"type": "function", "name": "shell"}
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(provider_request_body["model"], json!(original_model));
        assert_eq!(
            provider_request_body["tool_choice"],
            json!({"type": "function", "name": "shell"})
        );
    }
}
