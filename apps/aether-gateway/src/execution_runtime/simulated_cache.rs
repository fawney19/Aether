use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::execution_runtime::kiro_cache::{
    simulated_cache_config_from_report_context, SimulatedCacheConfig,
};

static SIMULATED_CACHE_RESPONSE_RANDOM_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) const SIMULATED_CACHE_HIT_PERCENT_CONTEXT_FIELD: &str = "simulated_cache_hit_percent";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SimulatedCacheUsageShape {
    OpenAiChat,
    OpenAiResponses,
}

pub(crate) fn maybe_apply_simulated_cache_to_response_body(
    body: &mut Value,
    report_context: &mut Option<Value>,
    client_api_format: &str,
) -> bool {
    if !client_format_supports_simulated_cache_usage(client_api_format) {
        return false;
    }
    let Some(config) = simulated_cache_config_from_report_context(report_context.as_ref()) else {
        return false;
    };
    let Some(usage) = body.get_mut("usage") else {
        return false;
    };
    let Some(shape) = simulated_cache_usage_shape(usage) else {
        return false;
    };
    apply_simulated_cache_to_usage_value(usage, report_context, config, shape)
}

pub(crate) fn maybe_apply_simulated_cache_to_stream_chunk(
    chunk: Vec<u8>,
    report_context: &mut Option<Value>,
    client_api_format: &str,
) -> Vec<u8> {
    if chunk.is_empty()
        || simulated_cache_config_from_report_context(report_context.as_ref()).is_none()
        || !client_format_supports_simulated_cache_usage(client_api_format)
    {
        return chunk;
    }
    rewrite_sse_usage_chunks(chunk, report_context, client_api_format)
}

fn apply_simulated_cache_to_usage_value(
    usage: &mut Value,
    report_context: &mut Option<Value>,
    config: SimulatedCacheConfig,
    shape: SimulatedCacheUsageShape,
) -> bool {
    let Some(object) = usage.as_object_mut() else {
        return false;
    };
    let total_input_tokens = match shape {
        SimulatedCacheUsageShape::OpenAiChat => number_u64(object.get("prompt_tokens")),
        SimulatedCacheUsageShape::OpenAiResponses => number_u64(object.get("input_tokens")),
    }
    .unwrap_or(0);
    let Some((_, cache_read_tokens)) =
        simulated_cache_hit(report_context, config, total_input_tokens)
    else {
        return false;
    };

    match shape {
        SimulatedCacheUsageShape::OpenAiChat => {
            object.insert("prompt_tokens".to_string(), Value::from(total_input_tokens));
            let output_tokens = number_u64(object.get("completion_tokens")).unwrap_or(0);
            object.insert(
                "total_tokens".to_string(),
                Value::from(total_input_tokens.saturating_add(output_tokens)),
            );
            let details = object
                .entry("prompt_tokens_details")
                .or_insert_with(|| json!({}));
            if !details.is_object() {
                *details = json!({});
            }
            if let Some(details) = details.as_object_mut() {
                details.insert("cached_tokens".to_string(), Value::from(cache_read_tokens));
            }
        }
        SimulatedCacheUsageShape::OpenAiResponses => {
            object.insert("input_tokens".to_string(), Value::from(total_input_tokens));
            let output_tokens = number_u64(object.get("output_tokens")).unwrap_or(0);
            object.insert(
                "total_tokens".to_string(),
                Value::from(total_input_tokens.saturating_add(output_tokens)),
            );
            let details = object
                .entry("input_tokens_details")
                .or_insert_with(|| json!({}));
            if !details.is_object() {
                *details = json!({});
            }
            if let Some(details) = details.as_object_mut() {
                details.insert("cached_tokens".to_string(), Value::from(cache_read_tokens));
            }
        }
    }
    object.insert(
        "cache_read_input_tokens".to_string(),
        Value::from(cache_read_tokens),
    );
    true
}

fn rewrite_sse_usage_chunks(
    mut chunk: Vec<u8>,
    report_context: &mut Option<Value>,
    client_api_format: &str,
) -> Vec<u8> {
    let mut cursor = 0usize;
    let mut output = Vec::with_capacity(chunk.len());
    while let Some((block_end, separator_len)) = find_sse_block_boundary(&chunk[cursor..]) {
        let absolute_end = cursor + block_end;
        let separator_end = absolute_end + separator_len;
        output.extend(rewrite_sse_block(
            &chunk[cursor..absolute_end],
            &chunk[absolute_end..separator_end],
            report_context,
            client_api_format,
        ));
        cursor = separator_end;
    }
    output.extend_from_slice(&chunk[cursor..]);
    chunk.clear();
    output
}

fn rewrite_sse_block(
    block: &[u8],
    separator: &[u8],
    report_context: &mut Option<Value>,
    client_api_format: &str,
) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(block) else {
        let mut output = block.to_vec();
        output.extend_from_slice(separator);
        return output;
    };

    let mut data_lines = Vec::new();
    for line in text.lines() {
        if let Some(data) = line.trim_start().strip_prefix("data:") {
            data_lines.push(data.trim_start());
        }
    }
    if data_lines.is_empty() {
        let mut output = block.to_vec();
        output.extend_from_slice(separator);
        return output;
    }
    let data = data_lines.join("\n");
    if data.trim() == "[DONE]" {
        let mut output = block.to_vec();
        output.extend_from_slice(separator);
        return output;
    }
    let Ok(mut payload) = serde_json::from_str::<Value>(&data) else {
        let mut output = block.to_vec();
        output.extend_from_slice(separator);
        return output;
    };
    if !rewrite_stream_payload_usage(&mut payload, report_context, client_api_format) {
        let mut output = block.to_vec();
        output.extend_from_slice(separator);
        return output;
    }

    let line_ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut output = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("data:") {
            output.extend_from_slice(b"data: ");
            output.extend_from_slice(payload.to_string().as_bytes());
            output.extend_from_slice(line_ending.as_bytes());
        } else {
            output.extend_from_slice(line.as_bytes());
            output.extend_from_slice(line_ending.as_bytes());
        }
    }
    output.extend_from_slice(separator);
    output
}

fn rewrite_stream_payload_usage(
    payload: &mut Value,
    report_context: &mut Option<Value>,
    client_api_format: &str,
) -> bool {
    if payload.get("usage").is_some() {
        return maybe_apply_simulated_cache_to_response_body(
            payload,
            report_context,
            client_api_format,
        );
    }
    let Some(response) = payload.get_mut("response") else {
        return false;
    };
    if response.get("usage").is_none() {
        return false;
    }
    maybe_apply_simulated_cache_to_response_body(response, report_context, client_api_format)
}

fn find_sse_block_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2));
    let crlf = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4));
    match (lf, crlf) {
        (Some(lf), Some(crlf)) => Some(if lf.0 <= crlf.0 { lf } else { crlf }),
        (Some(lf), None) => Some(lf),
        (None, Some(crlf)) => Some(crlf),
        (None, None) => None,
    }
}

fn simulated_cache_usage_shape(usage: &Value) -> Option<SimulatedCacheUsageShape> {
    let object = usage.as_object()?;
    if object.get("prompt_tokens").is_some() {
        return Some(SimulatedCacheUsageShape::OpenAiChat);
    }
    if object.get("input_tokens").is_some() {
        return Some(SimulatedCacheUsageShape::OpenAiResponses);
    }
    None
}

fn client_format_supports_simulated_cache_usage(client_api_format: &str) -> bool {
    matches!(
        aether_ai_formats::normalize_api_format_alias(client_api_format).as_str(),
        "openai:chat" | "openai:responses" | "openai:responses:compact"
    )
}

fn simulated_cache_hit(
    report_context: &mut Option<Value>,
    config: SimulatedCacheConfig,
    total_input_tokens: u64,
) -> Option<(f64, u64)> {
    if total_input_tokens == 0 {
        return None;
    }
    let hit_percent = existing_simulated_cache_hit_percent(report_context.as_ref())
        .unwrap_or_else(|| random_simulated_cache_percent(config.min_percent, config.max_percent));
    store_simulated_cache_hit_percent(report_context, hit_percent);
    let cache_read_tokens = ((total_input_tokens as f64) * hit_percent / 100.0).round() as u64;
    Some((hit_percent, cache_read_tokens.min(total_input_tokens)))
}

fn existing_simulated_cache_hit_percent(report_context: Option<&Value>) -> Option<f64> {
    report_context
        .and_then(Value::as_object)
        .and_then(|context| context.get(SIMULATED_CACHE_HIT_PERCENT_CONTEXT_FIELD))
        .and_then(value_as_f64)
        .filter(|value| value.is_finite())
}

fn store_simulated_cache_hit_percent(report_context: &mut Option<Value>, hit_percent: f64) {
    let value = json!(rounded_percent(hit_percent));
    match report_context {
        Some(Value::Object(context)) => {
            context.insert(SIMULATED_CACHE_HIT_PERCENT_CONTEXT_FIELD.to_string(), value);
        }
        _ => {
            let mut context = Map::new();
            context.insert(SIMULATED_CACHE_HIT_PERCENT_CONTEXT_FIELD.to_string(), value);
            *report_context = Some(Value::Object(context));
        }
    }
}

fn rounded_percent(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn number_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| match value {
        Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_i64().map(|v| v.max(0) as u64)),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    })
}

fn value_as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn random_simulated_cache_percent(min_percent: f64, max_percent: f64) -> f64 {
    if (max_percent - min_percent).abs() <= f64::EPSILON {
        return min_percent;
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let counter = SIMULATED_CACHE_RESPONSE_RANDOM_COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = splitmix64(nanos ^ counter.rotate_left(17));
    let unit = (seed as f64) / (u64::MAX as f64);
    min_percent + (max_percent - min_percent) * unit
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        maybe_apply_simulated_cache_to_response_body, maybe_apply_simulated_cache_to_stream_chunk,
    };

    #[test]
    fn simulated_cache_rewrites_openai_chat_usage_for_client() {
        let mut context = Some(json!({
            "simulated_cache_enabled": true,
            "simulated_cache_min_percent": 90,
            "simulated_cache_max_percent": 90
        }));
        let mut body = json!({
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 7,
                "total_tokens": 107
            }
        });

        assert!(maybe_apply_simulated_cache_to_response_body(
            &mut body,
            &mut context,
            "openai:chat"
        ));

        assert_eq!(body["usage"]["prompt_tokens"], 100);
        assert_eq!(body["usage"]["completion_tokens"], 7);
        assert_eq!(body["usage"]["total_tokens"], 107);
        assert_eq!(body["usage"]["prompt_tokens_details"]["cached_tokens"], 90);
        assert_eq!(body["usage"]["cache_read_input_tokens"], 90);
        assert_eq!(context.unwrap()["simulated_cache_hit_percent"], 90.0);
    }

    #[test]
    fn simulated_cache_rewrites_openai_responses_usage_for_client() {
        let mut context = Some(json!({
            "simulated_cache_enabled": true,
            "simulated_cache_min_percent": 50,
            "simulated_cache_max_percent": 50
        }));
        let mut body = json!({
            "usage": {
                "input_tokens": 80,
                "output_tokens": 3,
                "total_tokens": 83
            }
        });

        assert!(maybe_apply_simulated_cache_to_response_body(
            &mut body,
            &mut context,
            "openai:responses"
        ));

        assert_eq!(body["usage"]["input_tokens"], 80);
        assert_eq!(body["usage"]["output_tokens"], 3);
        assert_eq!(body["usage"]["total_tokens"], 83);
        assert_eq!(body["usage"]["input_tokens_details"]["cached_tokens"], 40);
        assert_eq!(body["usage"]["cache_read_input_tokens"], 40);
    }

    #[test]
    fn simulated_cache_rewrites_terminal_sse_usage_chunk() {
        let mut context = Some(json!({
            "simulated_cache_enabled": true,
            "simulated_cache_min_percent": 25,
            "simulated_cache_max_percent": 25
        }));
        let chunk = br#"data: {"id":"chatcmpl_1","object":"chat.completion.chunk","usage":{"prompt_tokens":20,"completion_tokens":4,"total_tokens":24}}

data: [DONE]

"#
        .to_vec();

        let rewritten =
            maybe_apply_simulated_cache_to_stream_chunk(chunk, &mut context, "openai:chat");
        let text = String::from_utf8(rewritten).expect("sse should remain utf8");

        assert!(text.contains(r#""prompt_tokens_details":{"cached_tokens":5}"#));
        assert!(text.contains(r#""cache_read_input_tokens":5"#));
        assert!(text.contains("data: [DONE]"));
    }

    #[test]
    fn simulated_cache_does_not_rewrite_non_openai_client_usage() {
        let mut context = Some(json!({
            "simulated_cache_enabled": true,
            "simulated_cache_min_percent": 90,
            "simulated_cache_max_percent": 90
        }));
        let mut body = json!({
            "usage": {
                "input_tokens": 100,
                "output_tokens": 7
            }
        });

        assert!(!maybe_apply_simulated_cache_to_response_body(
            &mut body,
            &mut context,
            "claude:messages"
        ));
        assert!(body["usage"].get("input_tokens_details").is_none());
    }
}
