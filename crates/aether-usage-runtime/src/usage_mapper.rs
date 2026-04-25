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

        derive_missing_input_tokens(raw_usage, api_format, &mut usage);
        usage.normalize_cache_creation_breakdown()
    }

    pub fn map_from_response(response: &serde_json::Value, api_format: &str) -> StandardizedUsage {
        let family = api_family(api_format);
        let Some(usage_value) = resolve_usage_value(response, family.as_str()) else {
            return StandardizedUsage::new();
        };
        Self::map(usage_value, api_format, None)
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
                "input_tokens_details.cached_creation_tokens".to_string(),
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

    for nested_key in ["response", "message", "item"] {
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
            "openai:cli",
        );

        assert_eq!(usage.input_tokens, 14);
        assert_eq!(usage.output_tokens, 6);
        assert_eq!(usage.cache_creation_tokens, 2);
        assert_eq!(usage.cache_read_tokens, 3);
        assert_eq!(usage.reasoning_tokens, 1);
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
            "openai:cli",
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
            "claude:chat",
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
            "openai:cli",
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
            "claude:chat",
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
            "claude:chat",
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
            "claude:chat",
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
            "claude:chat",
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
            "claude:chat",
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
            "claude:chat",
        );

        assert_eq!(usage.input_tokens, 1);
        assert_eq!(usage.output_tokens, 8);
        assert_eq!(usage.cache_creation_tokens, 5191);
        assert_eq!(usage.cache_creation_ephemeral_5m_tokens, 5191);
        assert_eq!(usage.cache_creation_ephemeral_1h_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 97634);
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
            "gemini:chat",
        );

        assert_eq!(usage.input_tokens, 14);
        assert_eq!(usage.output_tokens, 6);
        assert_eq!(usage.cache_read_tokens, 2);
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
            "gemini:chat",
        );

        assert_eq!(usage.input_tokens, 14);
        assert_eq!(usage.output_tokens, 6);
        assert_eq!(usage.cache_read_tokens, 2);
    }
}
