use std::collections::BTreeMap;

use crate::StandardizedUsage;

pub struct UsageMapper;

impl UsageMapper {
    pub fn map(
        raw_usage: &serde_json::Value,
        api_format: &str,
        extra_mapping: Option<&BTreeMap<String, String>>,
    ) -> StandardizedUsage {
        if !raw_usage.is_object() {
            return StandardizedUsage::new();
        }

        let mut usage = StandardizedUsage::new();
        let mut mapping = base_mapping(api_format);
        if let Some(extra_mapping) = extra_mapping {
            mapping.extend(extra_mapping.clone());
        }

        for (source_path, target_field) in mapping {
            if let Some(value) = get_nested_value(raw_usage, &source_path) {
                usage.set(&target_field, value.clone());
            }
        }

        apply_openai_cache_write_tokens(raw_usage, api_format, &mut usage);
        apply_gemini_usage(raw_usage, api_format, &mut usage);
        derive_missing_input_tokens(raw_usage, api_format, &mut usage);
        copy_explicit_total_tokens(raw_usage, api_format, &mut usage);
        usage.normalize_cache_creation_breakdown()
    }

    pub fn map_from_response(response: &serde_json::Value, api_format: &str) -> StandardizedUsage {
        let family = api_family(api_format);
        let mut usage = if let Some(usage_value) = resolve_usage_value(response, family.as_str()) {
            Self::map(usage_value, api_format, None)
        } else {
            StandardizedUsage::new()
        };
        if is_openai_image_api(api_format) {
            apply_openai_image_response_dimensions(response, &mut usage);
        }
        usage
    }
}

fn apply_gemini_usage(
    raw_usage: &serde_json::Value,
    api_format: &str,
    usage: &mut StandardizedUsage,
) {
    if api_family(api_format) != "gemini" {
        return;
    }
    let raw = raw_usage.get("usageMetadata").unwrap_or(raw_usage);
    let interactions = api_kind(api_format) == "interactions";
    let (input, output, thoughts, cached) = if interactions {
        (
            "total_input_tokens",
            "total_output_tokens",
            "total_thought_tokens",
            "total_cached_tokens",
        )
    } else {
        (
            "promptTokenCount",
            "candidatesTokenCount",
            "thoughtsTokenCount",
            "cachedContentTokenCount",
        )
    };
    if let Some(tokens) = numeric_i64(raw.get(input)) {
        usage.input_tokens = tokens.max(0);
    }
    if let Some(tokens) = numeric_i64(raw.get(output)) {
        usage.output_tokens = tokens.max(0);
    }
    if let Some(tokens) = numeric_i64(raw.get(thoughts)) {
        usage.reasoning_tokens = tokens.max(0);
        // Gemini reports visible candidates and internal reasoning separately.
        usage.output_tokens = usage.output_tokens.saturating_add(usage.reasoning_tokens);
    }
    if let Some(tokens) = numeric_i64(raw.get(cached)) {
        usage.cache_read_tokens = tokens.max(0);
    }
    for (source, prefix) in [
        ("promptTokensDetails", "input"),
        ("candidatesTokensDetails", "output"),
        ("cacheTokensDetails", "cache_read"),
        ("input_tokens_by_modality", "input"),
        ("output_tokens_by_modality", "output"),
        ("cached_tokens_by_modality", "cache_read"),
    ] {
        if let Some(details) = raw.get(source).and_then(serde_json::Value::as_array) {
            for detail in details {
                let Some(modality) = detail.get("modality").and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                let modality = modality.to_ascii_lowercase();
                if !matches!(
                    modality.as_str(),
                    "text" | "image" | "audio" | "video" | "document"
                ) {
                    continue;
                }
                if let Some(tokens) =
                    numeric_i64(detail.get("tokenCount").or_else(|| detail.get("tokens")))
                        .filter(|tokens| *tokens >= 0)
                {
                    usage.dimensions.insert(
                        format!("{prefix}_{modality}_tokens"),
                        serde_json::json!(tokens),
                    );
                }
            }
        }
    }
    for field in [
        "promptTokensDetails",
        "candidatesTokensDetails",
        "cacheTokensDetails",
        "toolUsePromptTokensDetails",
        "toolUsePromptTokenCount",
        "input_tokens_by_modality",
        "output_tokens_by_modality",
        "cached_tokens_by_modality",
        "tool_use_tokens_by_modality",
        "total_tool_use_tokens",
        "grounding_tool_count",
    ] {
        if let Some(value) = raw.get(field) {
            usage.dimensions.insert(field.to_string(), value.clone());
        }
    }
}

fn apply_openai_cache_write_tokens(
    raw_usage: &serde_json::Value,
    api_format: &str,
    usage: &mut StandardizedUsage,
) {
    if api_family(api_format).as_str() != "openai" {
        return;
    }
    for details_key in ["prompt_tokens_details", "input_tokens_details"] {
        if let Some(value) = raw_usage
            .get(details_key)
            .and_then(|details| details.get("cache_write_tokens"))
        {
            usage.set("cache_creation_tokens", value.clone());
            return;
        }
    }
}

pub fn map_usage(raw_usage: &serde_json::Value, api_format: &str) -> StandardizedUsage {
    UsageMapper::map(raw_usage, api_format, None)
}

pub fn map_usage_from_response(
    response: &serde_json::Value,
    api_format: &str,
) -> StandardizedUsage {
    UsageMapper::map_from_response(response, api_format)
}

fn api_family(api_format: &str) -> String {
    api_format
        .split(':')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn api_kind(api_format: &str) -> String {
    api_format
        .split(':')
        .nth(1)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn is_openai_image_api(api_format: &str) -> bool {
    api_family(api_format).as_str() == "openai" && api_kind(api_format).as_str() == "image"
}

fn apply_openai_image_response_dimensions(
    response: &serde_json::Value,
    usage: &mut StandardizedUsage,
) {
    let image_count = openai_image_response_image_count(response);
    if image_count <= 0 {
        return;
    }

    usage.request_count = image_count;
    usage
        .dimensions
        .insert("image_count".to_string(), serde_json::json!(image_count));
}

fn openai_image_response_image_count(response: &serde_json::Value) -> i64 {
    response
        .get("data")
        .and_then(serde_json::Value::as_array)
        .map(|items| items.len() as i64)
        .filter(|value| *value > 0)
        .or_else(|| image_result_count(response.get("result")))
        .unwrap_or(0)
}

fn image_result_count(value: Option<&serde_json::Value>) -> Option<i64> {
    match value? {
        serde_json::Value::Array(items) => Some(items.len() as i64).filter(|count| *count > 0),
        serde_json::Value::Object(object) if !object.is_empty() => Some(1),
        serde_json::Value::String(text) if !text.trim().is_empty() => Some(1),
        _ => None,
    }
}

fn base_mapping(api_format: &str) -> BTreeMap<String, String> {
    let mut mapping = BTreeMap::new();
    match api_family(api_format).as_str() {
        "openai" => {
            mapping.insert("prompt_tokens".to_string(), "input_tokens".to_string());
            mapping.insert("completion_tokens".to_string(), "output_tokens".to_string());
            mapping.insert("input_tokens".to_string(), "input_tokens".to_string());
            mapping.insert("output_tokens".to_string(), "output_tokens".to_string());
            mapping.insert(
                "cache_creation_input_tokens".to_string(),
                "cache_creation_tokens".to_string(),
            );
            mapping.insert(
                "cache_creation.ephemeral_5m_input_tokens".to_string(),
                "cache_creation_ephemeral_5m_tokens".to_string(),
            );
            mapping.insert(
                "cache_creation.ephemeral_1h_input_tokens".to_string(),
                "cache_creation_ephemeral_1h_tokens".to_string(),
            );
            mapping.insert(
                "cache_read_input_tokens".to_string(),
                "cache_read_tokens".to_string(),
            );
            mapping.insert(
                "prompt_tokens_details.cached_tokens".to_string(),
                "cache_read_tokens".to_string(),
            );
            mapping.insert(
                "input_tokens_details.cached_tokens".to_string(),
                "cache_read_tokens".to_string(),
            );
            mapping.insert(
                "prompt_tokens_details.cached_creation_tokens".to_string(),
                "cache_creation_tokens".to_string(),
            );
            mapping.insert(
                "prompt_tokens_details.cache_write_tokens".to_string(),
                "cache_creation_tokens".to_string(),
            );
            mapping.insert(
                "input_tokens_details.cached_creation_tokens".to_string(),
                "cache_creation_tokens".to_string(),
            );
            mapping.insert(
                "input_tokens_details.cache_write_tokens".to_string(),
                "cache_creation_tokens".to_string(),
            );
            mapping.insert(
                "completion_tokens_details.reasoning_tokens".to_string(),
                "reasoning_tokens".to_string(),
            );
            mapping.insert(
                "output_tokens_details.reasoning_tokens".to_string(),
                "reasoning_tokens".to_string(),
            );
        }
        "gemini" => {
            mapping.insert("promptTokenCount".to_string(), "input_tokens".to_string());
            mapping.insert(
                "candidatesTokenCount".to_string(),
                "output_tokens".to_string(),
            );
            mapping.insert(
                "cachedContentTokenCount".to_string(),
                "cache_read_tokens".to_string(),
            );
            mapping.insert(
                "usageMetadata.promptTokenCount".to_string(),
                "input_tokens".to_string(),
            );
            mapping.insert(
                "usageMetadata.candidatesTokenCount".to_string(),
                "output_tokens".to_string(),
            );
            mapping.insert(
                "usageMetadata.cachedContentTokenCount".to_string(),
                "cache_read_tokens".to_string(),
            );
        }
        "claude" | "anthropic" => {
            mapping.insert("input_tokens".to_string(), "input_tokens".to_string());
            mapping.insert("output_tokens".to_string(), "output_tokens".to_string());
            mapping.insert(
                "cache_creation_input_tokens".to_string(),
                "cache_creation_tokens".to_string(),
            );
            mapping.insert(
                "cache_creation.ephemeral_5m_input_tokens".to_string(),
                "cache_creation_ephemeral_5m_tokens".to_string(),
            );
            mapping.insert(
                "cache_creation.ephemeral_1h_input_tokens".to_string(),
                "cache_creation_ephemeral_1h_tokens".to_string(),
            );
            mapping.insert(
                "cache_read_input_tokens".to_string(),
                "cache_read_tokens".to_string(),
            );
        }
        _ => {
            mapping.insert("input_tokens".to_string(), "input_tokens".to_string());
            mapping.insert("output_tokens".to_string(), "output_tokens".to_string());
            mapping.insert(
                "cache_creation_input_tokens".to_string(),
                "cache_creation_tokens".to_string(),
            );
            mapping.insert(
                "cache_read_input_tokens".to_string(),
                "cache_read_tokens".to_string(),
            );
        }
    }
    mapping
}

fn derive_missing_input_tokens(
    raw_usage: &serde_json::Value,
    api_format: &str,
    usage: &mut StandardizedUsage,
) {
    if usage.input_tokens > 0 || api_family(api_format).as_str() != "openai" {
        return;
    }

    let Some(total_tokens) = numeric_i64(raw_usage.get("total_tokens")) else {
        return;
    };
    let output_tokens = usage
        .output_tokens
        .max(numeric_i64(raw_usage.get("completion_tokens")).unwrap_or_default())
        .max(numeric_i64(raw_usage.get("output_tokens")).unwrap_or_default());
    let inferred_input_tokens = total_tokens.saturating_sub(output_tokens);
    if inferred_input_tokens > 0 {
        usage.input_tokens = inferred_input_tokens;
    }
}

fn copy_explicit_total_tokens(
    raw_usage: &serde_json::Value,
    api_format: &str,
    usage: &mut StandardizedUsage,
) {
    let total_tokens = match api_family(api_format).as_str() {
        "gemini" => numeric_i64(
            raw_usage
                .get("totalTokenCount")
                .or_else(|| raw_usage.pointer("/usageMetadata/totalTokenCount"))
                .or_else(|| raw_usage.get("total_tokens")),
        ),
        _ => numeric_i64(raw_usage.get("total_tokens")),
    };
    if let Some(total_tokens) = total_tokens.filter(|value| *value > 0) {
        usage
            .dimensions
            .insert("total_tokens".to_string(), serde_json::json!(total_tokens));
    }
}

fn numeric_i64(value: Option<&serde_json::Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
    })
}

fn get_nested_value<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

fn resolve_usage_value<'a>(
    response: &'a serde_json::Value,
    family: &str,
) -> Option<&'a serde_json::Value> {
    match family {
        "gemini" => {
            if let Some(usage) = response.get("usageMetadata") {
                return Some(usage);
            }
            if let Some(usage) = response.get("usage") {
                return Some(usage);
            }
            if let Some(usage) = response
                .get("candidates")
                .and_then(|value| value.get(0))
                .and_then(|value| value.get("usageMetadata"))
            {
                return Some(usage);
            }
        }
        _ => {
            if let Some(usage) = response.get("usage") {
                return Some(usage);
            }
        }
    }

    for nested_key in ["response", "message", "item", "interaction"] {
        if let Some(nested) = response.get(nested_key) {
            if let Some(usage) = resolve_usage_value(nested, family) {
                return Some(usage);
            }
        }
    }

    if let Some(chunks) = response.get("chunks").and_then(serde_json::Value::as_array) {
        for chunk in chunks.iter().rev() {
            if let Some(usage) = resolve_usage_value(chunk, family) {
                return Some(usage);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{map_usage, map_usage_from_response};

    #[test]
    fn maps_openai_usage() {
        let usage = map_usage(
            &serde_json::json!({
                "prompt_tokens": 12,
                "completion_tokens": 8,
                "prompt_tokens_details": {
                    "cached_tokens": 2,
                    "cached_creation_tokens": 1
                },
                "completion_tokens_details": { "reasoning_tokens": 3 }
            }),
            "openai:chat",
        );

        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 8);
        assert_eq!(usage.cache_creation_tokens, 1);
        assert_eq!(usage.cache_read_tokens, 2);
        assert_eq!(usage.reasoning_tokens, 3);
    }

    #[test]
    fn maps_openai_responses_usage_from_response() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "usage": {
                    "input_tokens": 14,
                    "output_tokens": 6,
                    "total_tokens": 20,
                    "input_tokens_details": {
                        "cached_tokens": 3,
                        "cached_creation_tokens": 2
                    },
                    "output_tokens_details": {
                        "reasoning_tokens": 1
                    }
                }
            }),
            "openai:responses",
        );

        assert_eq!(usage.input_tokens, 14);
        assert_eq!(usage.output_tokens, 6);
        assert_eq!(usage.cache_creation_tokens, 2);
        assert_eq!(usage.cache_read_tokens, 3);
        assert_eq!(usage.reasoning_tokens, 1);
    }

    #[test]
    fn maps_openai_responses_cache_write_tokens() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "usage": {
                    "input_tokens": 32_963,
                    "input_tokens_details": {
                        "cache_write_tokens": 512,
                        "cached_creation_tokens": 1,
                        "cached_tokens": 30_336
                    },
                    "output_tokens": 129,
                    "output_tokens_details": {
                        "reasoning_tokens": 8
                    },
                    "total_tokens": 33_092
                }
            }),
            "openai:responses",
        );

        assert_eq!(usage.input_tokens, 32_963);
        assert_eq!(usage.output_tokens, 129);
        assert_eq!(usage.cache_creation_tokens, 512);
        assert_eq!(usage.cache_read_tokens, 30_336);
        assert_eq!(usage.reasoning_tokens, 8);
    }

    #[test]
    fn maps_openai_responses_usage_with_missing_input_from_total() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "usage": {
                    "output_tokens": 899,
                    "total_tokens": 53_499,
                    "input_tokens_details": {
                        "cached_tokens": 52_600
                    }
                }
            }),
            "openai:responses",
        );

        assert_eq!(usage.input_tokens, 52_600);
        assert_eq!(usage.output_tokens, 899);
        assert_eq!(usage.cache_read_tokens, 52_600);
    }

    #[test]
    fn keeps_missing_input_derivation_scoped_to_openai() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "usage": {
                    "output_tokens": 7,
                    "total_tokens": 17,
                    "cache_read_input_tokens": 10
                }
            }),
            "claude:messages",
        );

        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.cache_read_tokens, 10);
    }

    #[test]
    fn maps_openai_responses_with_top_level_cache_fields() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "usage": {
                    "input_tokens": 6,
                    "output_tokens": 20,
                    "cache_creation_input_tokens": 42_262,
                    "cache_read_input_tokens": 0
                }
            }),
            "openai:chat",
        );

        assert_eq!(usage.input_tokens, 6);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.cache_creation_tokens, 42_262);
        assert_eq!(usage.cache_read_tokens, 0);
    }

    #[test]
    fn maps_openai_responses_usage_from_stream_chunks() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "chunks": [
                    {
                        "type": "response.created",
                        "response": {
                            "id": "resp_123",
                            "object": "response"
                        }
                    },
                    {
                        "type": "response.completed",
                        "response": {
                            "id": "resp_123",
                            "object": "response",
                            "usage": {
                                "input_tokens": 9,
                                "output_tokens": 4,
                                "total_tokens": 13,
                                "input_tokens_details": {
                                    "cached_tokens": 5,
                                    "cached_creation_tokens": 2
                                },
                                "output_tokens_details": {
                                    "reasoning_tokens": 1
                                }
                            }
                        }
                    }
                ]
            }),
            "openai:responses",
        );

        assert_eq!(usage.input_tokens, 9);
        assert_eq!(usage.output_tokens, 4);
        assert_eq!(usage.cache_creation_tokens, 2);
        assert_eq!(usage.cache_read_tokens, 5);
        assert_eq!(usage.reasoning_tokens, 1);
    }

    #[test]
    fn maps_claude_usage() {
        let usage = map_usage(
            &serde_json::json!({
                "input_tokens": 10,
                "output_tokens": 5,
                "cache_creation_input_tokens": 4,
                "cache_read_input_tokens": 1
            }),
            "claude:messages",
        );

        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.cache_creation_tokens, 4);
        assert_eq!(usage.cache_read_tokens, 1);
    }

    #[test]
    fn maps_claude_usage_from_stream_chunks() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "chunks": [
                    {
                        "type": "message_start",
                        "message": {
                            "usage": {
                                "input_tokens": 5,
                                "cache_creation_input_tokens": 59_573,
                                "cache_read_input_tokens": 0,
                                "output_tokens": 0
                            }
                        }
                    },
                    {
                        "type": "message_delta",
                        "usage": {
                            "input_tokens": 5,
                            "cache_creation_input_tokens": 59_573,
                            "cache_read_input_tokens": 0,
                            "output_tokens": 162
                        }
                    }
                ]
            }),
            "claude:messages",
        );

        assert_eq!(usage.input_tokens, 5);
        assert_eq!(usage.output_tokens, 162);
        assert_eq!(usage.cache_creation_tokens, 59_573);
        assert_eq!(usage.cache_read_tokens, 0);
    }

    #[test]
    fn maps_claude_usage_from_message_start_chunk() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "chunks": [
                    {
                        "type": "message_start",
                        "message": {
                            "usage": {
                                "input_tokens": 5,
                                "cache_creation_input_tokens": 59_573,
                                "cache_read_input_tokens": 0,
                                "output_tokens": 0
                            }
                        }
                    }
                ]
            }),
            "claude:messages",
        );

        assert_eq!(usage.input_tokens, 5);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_creation_tokens, 59_573);
        assert_eq!(usage.cache_read_tokens, 0);
    }

    #[test]
    fn maps_claude_usage_with_large_cache_read_tokens_without_subtracting_input() {
        let usage = map_usage(
            &serde_json::json!({
                "input_tokens": 4941,
                "cache_creation_input_tokens": 687,
                "cache_read_input_tokens": 52873,
                "output_tokens": 973
            }),
            "claude:messages",
        );

        assert_eq!(usage.input_tokens, 4941);
        assert_eq!(usage.cache_creation_tokens, 687);
        assert_eq!(usage.cache_read_tokens, 52873);
        assert_eq!(usage.output_tokens, 973);
    }

    #[test]
    fn maps_claude_usage_with_cache_creation_total_and_zero_ttl_breakdown() {
        let usage = map_usage(
            &serde_json::json!({
                "cache_creation": {
                    "ephemeral_1h_input_tokens": 0,
                    "ephemeral_5m_input_tokens": 0
                },
                "cache_creation_input_tokens": 2051,
                "cache_read_input_tokens": 2051,
                "inference_geo": "inference_geo",
                "input_tokens": 2095,
                "output_tokens": 503,
                "server_tool_use": {
                    "web_fetch_requests": 2,
                    "web_search_requests": 0
                },
                "service_tier": "standard"
            }),
            "claude:messages",
        );

        assert_eq!(usage.input_tokens, 2095);
        assert_eq!(usage.cache_creation_tokens, 2051);
        assert_eq!(usage.cache_creation_ephemeral_5m_tokens, 0);
        assert_eq!(usage.cache_creation_ephemeral_1h_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 2051);
        assert_eq!(usage.output_tokens, 503);
    }

    #[test]
    fn maps_claude_usage_with_ephemeral_cache_breakdown() {
        let usage = map_usage(
            &serde_json::json!({
                "input_tokens": 1,
                "output_tokens": 8,
                "cache_creation": {
                    "ephemeral_1h_input_tokens": 0,
                    "ephemeral_5m_input_tokens": 5191
                },
                "cache_creation_input_tokens": 5191,
                "cache_read_input_tokens": 97634
            }),
            "claude:messages",
        );

        assert_eq!(usage.input_tokens, 1);
        assert_eq!(usage.output_tokens, 8);
        assert_eq!(usage.cache_creation_tokens, 5191);
        assert_eq!(usage.cache_creation_ephemeral_5m_tokens, 5191);
        assert_eq!(usage.cache_creation_ephemeral_1h_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 97634);
    }

    #[test]
    fn maps_generate_content_modality_tokens_without_losing_raw_details() {
        let usage = map_usage_from_response(
            &serde_json::json!({"usageMetadata": {
                "promptTokenCount": 100, "candidatesTokenCount": 50,
                "thoughtsTokenCount": 20,
                "promptTokensDetails": [{"modality":"AUDIO", "tokenCount":90}],
                "candidatesTokensDetails": [{"modality":"IMAGE", "tokenCount":40}],
                "cacheTokensDetails": [{"modality":"AUDIO", "tokenCount":20}]
            }}),
            "gemini:generate_content",
        );
        assert_eq!(usage.output_tokens, 70);
        assert_eq!(usage.dimensions["input_audio_tokens"], 90);
        assert_eq!(usage.dimensions["output_image_tokens"], 40);
        assert_eq!(usage.dimensions["cache_read_audio_tokens"], 20);
        assert_eq!(usage.dimensions["promptTokensDetails"][0]["tokenCount"], 90);
    }

    #[test]
    fn maps_gemini_interactions_completed_event_and_modality_details() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "event_type": "interaction.completed",
                "interaction": {"usage": {
                    "total_input_tokens": 533, "total_output_tokens": 27,
                    "total_thought_tokens": 230, "total_cached_tokens": 100,
                    "total_tokens": 790,
                    "input_tokens_by_modality": [{"modality":"audio","tokens":533}]
                }}
            }),
            "gemini:interactions",
        );
        assert_eq!(usage.input_tokens, 533);
        assert_eq!(usage.output_tokens, 257);
        assert_eq!(usage.reasoning_tokens, 230);
        assert_eq!(usage.cache_read_tokens, 100);
        assert_eq!(usage.dimensions["total_tokens"], 790);
        assert_eq!(
            usage.dimensions["input_tokens_by_modality"][0]["tokens"],
            533
        );
    }

    #[test]
    fn maps_gemini_usage_from_response() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "usageMetadata": {
                    "promptTokenCount": 14,
                    "candidatesTokenCount": 6,
                    "cachedContentTokenCount": 2
                }
            }),
            "gemini:generate_content",
        );

        assert_eq!(usage.input_tokens, 14);
        assert_eq!(usage.output_tokens, 6);
        assert_eq!(usage.cache_read_tokens, 2);
    }

    #[test]
    fn gemini_metadata_keeps_precedence_over_generic_usage() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "usage": {"prompt_tokens": 99, "completion_tokens": 88},
                "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 20}
            }),
            "gemini:generate_content",
        );
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 20);
    }

    #[test]
    fn maps_gemini_reasoning_as_billable_output() {
        for (visible, reasoning) in [(5, 65), (0, 29), (13, 0), (91, 127)] {
            let usage = map_usage_from_response(
                &serde_json::json!({
                    "usageMetadata": {
                        "promptTokenCount": 35,
                        "candidatesTokenCount": visible,
                        "thoughtsTokenCount": reasoning,
                        "totalTokenCount": 35 + visible + reasoning
                    }
                }),
                "gemini:generate_content",
            );
            assert_eq!(usage.input_tokens, 35);
            assert_eq!(usage.output_tokens, visible + reasoning);
            assert_eq!(usage.reasoning_tokens, reasoning);
            assert_eq!(
                usage.dimensions.get("total_tokens"),
                Some(&serde_json::json!(35 + visible + reasoning))
            );
        }
    }

    #[test]
    fn maps_gemini_reasoning_from_nested_metadata_and_stream_chunks() {
        let metadata = serde_json::json!({
            "promptTokenCount": 18,
            "candidatesTokenCount": 7,
            "thoughtsTokenCount": 23,
            "cachedContentTokenCount": 4
        });
        let nested = serde_json::json!({"usageMetadata": metadata});
        let direct = map_usage(&nested, "gemini:generate_content");
        let streamed = map_usage_from_response(
            &serde_json::json!({"chunks": [{"candidates": []}, nested]}),
            "gemini:generate_content",
        );
        for usage in [direct, streamed] {
            assert_eq!(usage.input_tokens, 18);
            assert_eq!(usage.output_tokens, 30);
            assert_eq!(usage.reasoning_tokens, 23);
            assert_eq!(usage.cache_read_tokens, 4);
        }
    }

    #[test]
    fn maps_gemini_usage_from_stream_chunks() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "chunks": [
                    {
                        "candidates": [
                            {
                                "content": {
                                    "parts": [{ "text": "hello" }]
                                }
                            }
                        ]
                    },
                    {
                        "usageMetadata": {
                            "promptTokenCount": 14,
                            "candidatesTokenCount": 6,
                            "cachedContentTokenCount": 2
                        }
                    }
                ]
            }),
            "gemini:generate_content",
        );

        assert_eq!(usage.input_tokens, 14);
        assert_eq!(usage.output_tokens, 6);
        assert_eq!(usage.cache_read_tokens, 2);
    }

    #[test]
    fn maps_openai_image_response_dimensions_without_usage() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "created": 1_700_000_000,
                "data": [
                    { "b64_json": "abc" },
                    { "url": "https://example.test/image.png" }
                ]
            }),
            "openai:image",
        );

        assert_eq!(usage.request_count, 2);
        assert_eq!(
            usage.dimensions.get("image_count"),
            Some(&serde_json::json!(2))
        );
    }

    #[test]
    fn maps_openai_image_response_dimensions_with_native_usage() {
        let usage = map_usage_from_response(
            &serde_json::json!({
                "usage": {
                    "input_tokens": 11,
                    "output_tokens": 22,
                    "total_tokens": 33
                },
                "data": [{ "b64_json": "abc" }]
            }),
            "openai:image",
        );

        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 22);
        assert_eq!(usage.request_count, 1);
        assert_eq!(
            usage.dimensions.get("image_count"),
            Some(&serde_json::json!(1))
        );
    }
}
